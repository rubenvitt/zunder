// Ordering Heute by hand (design 5.3): which tasks a move plans for today and which ranks it writes.

import { describe, expect, it } from "vitest";
import { dayOrderChanges } from "../src/sync/day-order";
import type { Task } from "../src/sync/types";

const TODAY = "2026-11-23";

function task(id: string, extra: Partial<Task> = {}): Task {
  return { id, title: id, status: "open", planned_on: null, day_rank: null, ...extra } as Task;
}
const planned = (id: string, rank: string | null) => task(id, { planned_on: TODAY, day_rank: rank });

/** The order after applying `changes`: planned for today by rank first, the rest as before. */
function after(items: Task[], moved: Task, changes: ReturnType<typeof dayOrderChanges>): string[] {
  const all = new Map([...items, moved].map((t) => [t.id, { ...t }]));
  for (const c of changes) Object.assign(all.get(c.id)!, c.fields);
  const list = [...all.values()];
  const plan = list.filter((t) => t.planned_on === TODAY).sort((a, b) => (a.day_rank! < b.day_rank! ? -1 : 1));
  const rest = items.filter((t) => all.get(t.id)!.planned_on !== TODAY);
  return [...plan, ...rest].map((t) => t.id);
}

describe("dayOrderChanges", () => {
  const a = planned("a", "a");
  const b = planned("b", "b");
  const c = planned("c", "c");

  it("moves within the plan with one rank", () => {
    const items = [a, b, c];
    const changes = dayOrderChanges(items, c, a, b, TODAY);
    expect(changes).toEqual([{ id: "c", fields: { day_rank: expect.any(String) } }]);
    expect(after(items, c, changes)).toEqual(["a", "c", "b"]);
  });

  it("to the front and to the end", () => {
    const items = [a, b, c];
    expect(after(items, c, dayOrderChanges(items, c, null, a, TODAY))).toEqual(["c", "a", "b"]);
    expect(after(items, a, dayOrderChanges(items, a, c, null, TODAY))).toEqual(["b", "c", "a"]);
  });

  it("writes nothing when the task stays in place", () => {
    expect(dayOrderChanges([a, b, c], b, a, c, TODAY)).toEqual([]);
  });

  it("plans a task without a plan, and what stands above it", () => {
    const due1 = task("due1");
    const due2 = task("due2");
    const items = [a, due1, due2];
    const changes = dayOrderChanges(items, due2, due1, null, TODAY);
    expect(changes.map((ch) => [ch.id, ch.fields.planned_on])).toEqual([
      ["due1", TODAY],
      ["due2", TODAY],
    ]);
    expect(after(items, due2, changes)).toEqual(["a", "due1", "due2"]);
  });

  it("puts a task between planned ones and plans it for today", () => {
    const due = task("due");
    const items = [a, b, due];
    const changes = dayOrderChanges(items, due, a, b, TODAY);
    expect(changes).toEqual([{ id: "due", fields: { planned_on: TODAY, day_rank: expect.any(String) } }]);
    expect(after(items, due, changes)).toEqual(["a", "due", "b"]);
  });

  it("moves a task planned on an earlier day into today's plan", () => {
    const old = task("old", { planned_on: "2026-11-20", day_rank: "a" });
    const items = [a, b, old];
    expect(after(items, old, dayOrderChanges(items, old, null, a, TODAY))).toEqual(["old", "a", "b"]);
  });

  it("gives ranks to planned tasks without one, keeping the order seen", () => {
    const x = planned("x", null);
    const y = planned("y", null);
    const items = [a, x, y];
    const changes = dayOrderChanges(items, a, y, null, TODAY);
    expect(after(items, a, changes)).toEqual(["x", "y", "a"]);
  });

  it("works with ranks from plan_day and the import", () => {
    const p = planned("p", "00005");
    const q = planned("q", "00015");
    const items = [p, q];
    const n = task("n");
    expect(after(items, n, dayOrderChanges(items, n, null, p, TODAY))).toEqual(["n", "p", "q"]);
    expect(after(items, n, dayOrderChanges(items, n, p, q, TODAY))).toEqual(["p", "n", "q"]);
  });
});
