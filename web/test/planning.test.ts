// Planung (design 5.1/5.2, 6.1, 7): the edges the shared vectors leave out. The rules themselves
// are checked against the core by the vectors (kinds today_list, goal_input, done_log).

import { describe, expect, it } from "vitest";
import { TaskIndex } from "../src/sync/derived";
import { doneLog, effectiveGoal, goalInput, goalWeekTasks, todayList, weeksLeft } from "../src/sync/planning";
import type { Goal, Task } from "../src/sync/types";

function task(id: string, extra: Partial<Task> = {}): Task {
  return {
    id,
    title: id,
    status: "open",
    deleted: false,
    parent_id: null,
    location: { kind: "loose" },
    goal_id: null,
    planned_on: null,
    day_rank: null,
    due: null,
    weight: null,
    waiting_on: null,
    flame_log: [],
    flame_started_at: null,
    flame_ended_at: null,
    blocked_by: [],
    created_hlc: "1790000000000:00000:test",
    status_hlc: "1790000000000:00000:test",
    ...extra,
  } as Task;
}

function goal(id: string, extra: Partial<Goal> = {}): Goal {
  return { id, name: id, deleted: false, state: "active", lead_per_week: null, rank: null, ...extra } as Goal;
}

describe("effectiveGoal", () => {
  const goals = { sport: goal("sport"), weg: goal("weg", { deleted: true }) };
  const root = task("root", { goal_id: "sport" });
  const child = task("child", { parent_id: "root" });
  const grandchild = task("grandchild", { parent_id: "child" });
  const own = task("own", { parent_id: "root", goal_id: "weg" });
  const index = new TaskIndex([root, child, grandchild, own]);

  it("is the own goal, otherwise the root's", () => {
    expect(effectiveGoal(index, goals, root)?.id).toBe("sport");
    expect(effectiveGoal(index, goals, child)?.id).toBe("sport");
    expect(effectiveGoal(index, goals, grandchild)?.id).toBe("sport");
  });

  it("counts a deleted own goal as none, so the root's goal applies", () => {
    expect(effectiveGoal(index, goals, own)?.id).toBe("sport");
  });

  it("is none for a deleted or unknown goal without a live one at the root", () => {
    expect(effectiveGoal(index, goals, task("x", { goal_id: "unbekannt" }))).toBeNull();
    const gone = task("gone", { goal_id: "weg" });
    const below = task("below", { parent_id: "gone" });
    const other = new TaskIndex([gone, below]);
    expect(effectiveGoal(other, goals, gone)).toBeNull();
    expect(effectiveGoal(other, goals, below)).toBeNull();
  });
});

describe("flame minutes", () => {
  const t = task("t", {
    flame_log: [
      { start: "2026-11-23T08:00:00+01:00", end: "2026-11-23T08:00:59+01:00" },
      { start: "2026-11-23T09:00:00+01:00", end: "2026-11-23T09:10:30+01:00" },
    ],
    flame_started_at: "2026-11-23T10:00:00+01:00",
  });
  const index = new TaskIndex([t]);

  it("round down per flame and count the running one until now", () => {
    const now = Date.parse("2026-11-23T10:05:59+01:00");
    expect(doneLog(index, {}, "2026-11-23", "2026-11-23", now)).toEqual([
      { date: "2026-11-23", minutes: 15, done: [], worked: [{ task: t, minutes: 15 }] },
    ]);
  });

  it("never count negative minutes when the clock is behind the start", () => {
    const now = Date.parse("2026-11-23T09:59:00+01:00");
    expect(doneLog(index, {}, "2026-11-23", "2026-11-23", now)[0]?.minutes).toBe(10);
  });
});

describe("empty input", () => {
  it("has no plan, no rows for a malformed week and no log for a reversed range", () => {
    const index = new TaskIndex([]);
    expect(todayList(index, "2026-11-23")).toEqual({ items: [], focus: null, has_plan: false, plan_done: false });
    expect(goalInput(index, { g: goal("g", { lead_per_week: 1 }) }, "2026-W99", 0)).toMatchObject({ from: "", to: "", goals: [{ done: 0, minutes: 0, reached: false }] });
    expect(doneLog(new TaskIndex([task("t", { status: "done" })]), {}, "2026-11-30", "2026-11-01", 0)).toEqual([]);
  });
});

describe("focus", () => {
  it("skips waiting_on and takes the next open task", () => {
    const waiting = task("a", { planned_on: "2026-11-23", day_rank: "a", waiting_on: "Anna" });
    const free = task("b", { planned_on: "2026-11-23", day_rank: "b" });
    const out = todayList(new TaskIndex([waiting, free]), "2026-11-23");
    expect(out.items.map((t) => t.id)).toEqual(["a", "b"]);
    expect(out.focus?.id).toBe("b");
    expect(todayList(new TaskIndex([waiting]), "2026-11-23").focus).toBeNull();
  });
});

describe("closed goal", () => {
  it("keeps its emoji in the log (effectiveGoal) but is left out of the goal input", () => {
    const closed = goal("zu", { state: "closed", emoji: "🏃" });
    const goals = { zu: closed };
    const done = task("t", { goal_id: "zu", status: "done", status_hlc: "1790000000000:00000:test" });
    const index = new TaskIndex([done]);
    const log = doneLog(index, {}, "2000-01-01", "2100-01-01", 0);
    const entry = log.flatMap((d) => d.done)[0]!;
    expect(effectiveGoal(index, goals, entry.task)?.emoji).toBe("🏃");
    expect(goalInput(index, goals, "2026-W47", 0).goals).toEqual([]);
  });
});

describe("goalWeekTasks", () => {
  const goals = { sport: goal("sport"), lesen: goal("lesen") };
  const monday = Date.parse("2026-11-23T10:00:00+01:00");
  const hlcAt = (ms: number) => `${ms}:00000:test`;
  const index = new TaskIndex([
    task("a", { goal_id: "sport", status: "done", status_hlc: hlcAt(monday) }),
    task("b", { goal_id: "sport", status: "done", status_hlc: hlcAt(monday - 7 * 86_400_000) }),
    task("c", { goal_id: "sport", flame_log: [{ start: "2026-11-24T08:00:00+01:00", end: "2026-11-24T08:40:00+01:00" }] }),
    task("c2", { parent_id: "c", flame_log: [{ start: "2026-11-25T08:00:00+01:00", end: "2026-11-25T08:10:00+01:00" }] }),
    task("d", { goal_id: "lesen", status: "done", status_hlc: hlcAt(monday) }),
  ]);

  it("lists the week's done tasks and tasks with flames of that goal, with minutes", () => {
    const out = goalWeekTasks(index, goals, "sport", "2026-W48", 0);
    expect(out.done.map((e) => e.task.id)).toEqual(["a"]);
    expect(out.worked.map((e) => [e.task.id, e.minutes])).toEqual([["c", 40], ["c2", 10]]);
  });

  it("is empty for a malformed week", () => {
    expect(goalWeekTasks(index, goals, "sport", "x", 0)).toEqual({ done: [], worked: [] });
  });
});

describe("weeksLeft", () => {
  const today = new Date(2026, 10, 23);
  it("rounds weeks up to the latest end of the active goals", () => {
    const goals = { a: goal("a", { ends_on: "2026-12-07" }), b: goal("b", { ends_on: "2026-12-10" }), c: goal("c", { ends_on: "2027-06-01", state: "closed" }) };
    expect(weeksLeft(goals, today)).toBe(3);
  });
  it("is null when no end lies ahead", () => {
    expect(weeksLeft({ a: goal("a", { ends_on: "2026-11-20" }), b: goal("b") }, today)).toBeNull();
  });
  it("counts today's end as the last week", () => {
    expect(weeksLeft({ a: goal("a", { ends_on: "2026-11-23" }) }, today)).toBe(1);
  });
});
