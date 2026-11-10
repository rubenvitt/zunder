import { createRoot } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPrTracker, isGithubPr, REFRESH_MS } from "../src/state/prs";
import type { PrStatuses } from "../src/sync/types";

const A = "https://github.com/o/r/pull/1";
const B = "https://github.com/o/r/pull/2";

const open = { number: 1, title: "Eins", state: "open", review: "approved", checks: "failure" } as const;

describe("isGithubPr", () => {
  it("matches PR links only", () => {
    expect(isGithubPr(A)).toBe(true);
    expect(isGithubPr("https://www.github.com/o/r/pull/7/files")).toBe(true);
    expect(isGithubPr("https://github.com/o/r/issues/1")).toBe(false);
    expect(isGithubPr("https://gitlab.com/o/r/pull/1")).toBe(false);
    expect(isGithubPr("https://github.com/o/r/pull/0")).toBe(false);
    expect(isGithubPr("https://github.com/o/r/pull/12abc")).toBe(false);
  });
});

describe("createPrTracker", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function setup(answer: (urls: string[]) => Promise<PrStatuses>) {
    const calls: string[][] = [];
    const fetch = vi.fn((urls: string[]) => {
      calls.push(urls);
      return answer(urls);
    });
    let dispose = () => {};
    const tracker = createRoot((d) => {
      dispose = d;
      return createPrTracker(() => fetch, () => Date.now());
    });
    return { tracker, calls, done: () => (tracker.dispose(), dispose()) };
  }

  it("batches the links of one render and stores the answers", async () => {
    const { tracker, calls, done } = setup(async () => ({ configured: true, statuses: { [A]: open, [B]: null } }));
    expect(tracker.status(A)).toBeUndefined();
    expect(tracker.status(B)).toBeUndefined();
    expect(tracker.status("https://example.org")).toBeUndefined();
    await vi.advanceTimersByTimeAsync(50);
    expect(calls).toEqual([[A, B]]);
    expect(tracker.status(A)).toEqual(open);
    expect(tracker.status(B)).toBeNull();
    // Shown again: no new request until the refresh.
    await vi.advanceTimersByTimeAsync(50);
    expect(calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(REFRESH_MS);
    expect(calls).toHaveLength(2);
    done();
  });

  it("stays quiet for a while without a token", async () => {
    let configured = false;
    const { tracker, calls, done } = setup(async () => ({ configured, statuses: {} }));
    tracker.status(A);
    await vi.advanceTimersByTimeAsync(50);
    expect(calls).toHaveLength(1);
    configured = true;
    // Refreshes within the back-off ask nothing.
    await vi.advanceTimersByTimeAsync(REFRESH_MS);
    expect(calls).toHaveLength(1);
    expect(tracker.status(A)).toBeUndefined();
    done();
  });

  it("keeps the shown status when a refresh fails", async () => {
    let fail = false;
    const { tracker, calls, done } = setup(async () => {
      if (fail) throw new Error("offline");
      return { configured: true, statuses: { [A]: open } };
    });
    tracker.status(A);
    await vi.advanceTimersByTimeAsync(50);
    fail = true;
    await vi.advanceTimersByTimeAsync(REFRESH_MS + 50);
    expect(calls).toHaveLength(2);
    expect(tracker.status(A)).toEqual(open);
    // The failed link is asked again on the next refresh.
    fail = false;
    await vi.advanceTimersByTimeAsync(REFRESH_MS + 50);
    expect(calls).toHaveLength(3);
    done();
  });
});
