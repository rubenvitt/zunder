import { createRoot } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { timingDuration, timingLabel } from "../src/labels";
import { createGoalHours, REFRESH_MS } from "../src/state/goal-hours";
import type { GoalHours } from "../src/sync/types";

describe("timingLabel", () => {
  it("shows minutes below an hour and hours with one decimal from then on", () => {
    expect(timingLabel(40)).toBe("40 min Timing");
    expect(timingLabel(60)).toBe("1 h Timing");
    expect(timingLabel(210)).toBe("3,5 h Timing");
    expect(timingLabel(367)).toBe("6,1 h Timing");
    expect(timingDuration(360)).toBe("6 h");
  });
});

describe("createGoalHours", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function setup(answer: (week: string) => Promise<GoalHours>) {
    const calls: string[] = [];
    const fetch = vi.fn((week: string) => {
      calls.push(week);
      return answer(week);
    });
    let dispose = () => {};
    const tracker = createRoot((d) => {
      dispose = d;
      return createGoalHours(() => fetch, () => Date.now());
    });
    return { tracker, calls, done: () => (tracker.dispose(), dispose()) };
  }

  const ok = (week: string, goals: Record<string, number>): GoalHours => ({ week, status: "ok", goals });

  it("asks for the watched week right away and again after the refresh", async () => {
    const { tracker, calls, done } = setup(async (week) => ok(week, { a: 210 }));
    const stop = tracker.watch("2026-W48");
    expect(tracker.minutes("a")).toBeUndefined();
    await vi.advanceTimersByTimeAsync(0);
    expect(tracker.minutes("a")).toBe(210);
    expect(tracker.minutes("b")).toBeUndefined();
    await vi.advanceTimersByTimeAsync(REFRESH_MS);
    expect(calls).toEqual(["2026-W48", "2026-W48"]);
    stop();
    expect(tracker.minutes("a")).toBeUndefined();
    await vi.advanceTimersByTimeAsync(REFRESH_MS);
    expect(calls).toHaveLength(2);
    // Watched again: the known answer shows at once.
    tracker.watch("2026-W48");
    expect(tracker.minutes("a")).toBe(210);
    done();
  });

  it("shows nothing without Timing and keeps the last answer on a network error", async () => {
    let mode: "ok" | "unavailable" | "offline" = "ok";
    const { tracker, done } = setup(async (week) => {
      if (mode === "offline") throw new Error("offline");
      return mode === "ok" ? ok(week, { a: 60 }) : { week, status: "unavailable", goals: {}, message: "weg" };
    });
    tracker.watch("2026-W48");
    await vi.advanceTimersByTimeAsync(0);
    expect(tracker.minutes("a")).toBe(60);
    mode = "offline";
    await vi.advanceTimersByTimeAsync(REFRESH_MS);
    expect(tracker.minutes("a")).toBe(60);
    mode = "unavailable";
    await vi.advanceTimersByTimeAsync(REFRESH_MS);
    expect(tracker.minutes("a")).toBeUndefined();
    done();
  });

  it("forgets the old week when another is watched", async () => {
    const { tracker, done } = setup(async (week) => ok(week, week === "2026-W48" ? { a: 90 } : {}));
    tracker.watch("2026-W48");
    await vi.advanceTimersByTimeAsync(0);
    expect(tracker.minutes("a")).toBe(90);
    tracker.watch("2026-W49");
    expect(tracker.minutes("a")).toBeUndefined();
    await vi.advanceTimersByTimeAsync(0);
    expect(tracker.minutes("a")).toBeUndefined();
    done();
  });
});
