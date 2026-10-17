// Zusagen-Bremse (contract 5.1): which tasks count as open commitments, the label in Heute's
// head and the fixed threshold for the hint.

import { describe, expect, it } from "vitest";
import { commitmentsLabel, COMMITMENT_HINT } from "../src/labels";
import { COMMITMENT_LIMIT, isOpenCommitment, openCommitments, tooManyCommitments } from "../src/sync/commitments";
import type { Task, TaskStatus } from "../src/sync/types";

function task(id: string, extra: Partial<Task> = {}): Task {
  return {
    id,
    title: id,
    status: "open",
    for_whom: "andere",
    waiting_on: null,
    due: null,
    rank: null,
    created_hlc: "1760000000000:00000:test",
    ...extra,
  } as Task;
}

describe("isOpenCommitment", () => {
  it("counts tasks for others that are not done or dropped", () => {
    const counted: TaskStatus[] = ["open", "in_progress", "blocked", "review"];
    for (const status of counted) expect(isOpenCommitment(task("t", { status }))).toBe(true);
    expect(isOpenCommitment(task("t", { status: "done" }))).toBe(false);
    expect(isOpenCommitment(task("t", { status: "dropped" }))).toBe(false);
  });

  it("only for others: mich, uns and unset are no commitments", () => {
    expect(isOpenCommitment(task("t", { for_whom: "mich" }))).toBe(false);
    expect(isOpenCommitment(task("t", { for_whom: "uns" }))).toBe(false);
    expect(isOpenCommitment(task("t", { for_whom: null }))).toBe(false);
  });

  it("handed back (waiting on someone) leaves the count, blocked by a task does not", () => {
    expect(isOpenCommitment(task("t", { status: "blocked", waiting_on: "Anna" }))).toBe(false);
    expect(isOpenCommitment(task("t", { status: "in_progress", waiting_on: "Anna" }))).toBe(false);
    expect(isOpenCommitment(task("t", { status: "blocked", waiting_on: null }))).toBe(true);
  });
});

describe("openCommitments", () => {
  const none = () => false;

  it("leaves out what the caller excludes (dev projects, deleted)", () => {
    const tasks = [
      task("a"),
      task("dev"),
      task("deleted"),
      task("mine", { for_whom: "mich" }),
      task("done", { status: "done" }),
      task("waiting", { status: "blocked", waiting_on: "Anna" }),
    ];
    const excluded = (t: Task) => t.id === "dev" || t.id === "deleted";
    expect(openCommitments(tasks, excluded).map((t) => t.id)).toEqual(["a"]);
  });

  it("sorts due first, then by rank", () => {
    const tasks = [
      task("undated-b", { rank: "b" }),
      task("later", { due: "2026-10-30" }),
      task("undated-a", { rank: "a" }),
      task("sooner", { due: "2026-10-20T08:00:00+02:00" }),
    ];
    expect(openCommitments(tasks, none).map((t) => t.id)).toEqual(["sooner", "later", "undated-a", "undated-b"]);
  });
});

describe("threshold and label", () => {
  it("hints from 8 open commitments on", () => {
    expect(COMMITMENT_LIMIT).toBe(8);
    expect(tooManyCommitments(0)).toBe(false);
    expect(tooManyCommitments(7)).toBe(false);
    expect(tooManyCommitments(8)).toBe(true);
    expect(tooManyCommitments(12)).toBe(true);
    expect(COMMITMENT_HINT).toBe("Viele Zusagen offen. Eine zurückgeben?");
  });

  it("names one commitment in the singular, more in the plural", () => {
    expect(commitmentsLabel(1)).toBe("1 offene Zusage");
    expect(commitmentsLabel(2)).toBe("2 offene Zusagen");
    expect(commitmentsLabel(8)).toBe("8 offene Zusagen");
  });
});
