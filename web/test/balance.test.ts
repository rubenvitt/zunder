// Wochenbilanz (contract 5.1): week bounds in local time. The counting rule itself is checked
// against the core by the shared vectors (kind "week_balance", vectors.test.ts).

import { describe, expect, it } from "vitest";
import { emptyBalance, mondayOf, weekBalance } from "../src/sync/balance";
import type { Task } from "../src/sync/types";

function done(id: string, at: string, extra: Partial<Task> = {}): Task {
  const ms = new Date(at).getTime();
  return { id, title: id, status: "done", weight: null, for_whom: null, status_hlc: `${String(ms).padStart(13, "0")}:00000:test`, ...extra } as Task;
}

describe("week bounds", () => {
  it("runs Monday to Sunday, also across the turn of the year", () => {
    expect(mondayOf("2026-W44")).toEqual(new Date(2026, 9, 26));
    expect(emptyBalance("2026-W44")).toMatchObject({ from: "2026-10-26", to: "2026-11-01", total: 0 });
    expect(emptyBalance("2026-W53")).toMatchObject({ from: "2026-12-28", to: "2027-01-03" });
    expect(emptyBalance("2027-W01")).toMatchObject({ from: "2027-01-04", to: "2027-01-10" });
  });

  it("rejects weeks a year does not have", () => {
    expect(mondayOf("2025-W53")).toBeNull();
    expect(mondayOf("2026-W00")).toBeNull();
    expect(mondayOf("2026-W5")).toBeNull();
    expect(weekBalance([done("a", "2026-01-01T12:00:00Z")], "2025-W53", () => false).total).toBe(0);
  });
});

describe("weekBalance", () => {
  it("ends the week at local midnight after a change of the clocks", () => {
    // Week 13 of 2026 ends Monday 0:00 summer time (22:00 UTC), one hour earlier than in winter.
    const tasks = [done("in", "2026-03-29T21:59:59.999Z"), done("out", "2026-03-29T22:00:00Z"), done("first", "2026-03-22T23:00:00Z")];
    expect(weekBalance(tasks, "2026-W13", () => false).total).toBe(2);
    expect(weekBalance(tasks, "2026-W14", () => false).total).toBe(1);
  });

  it("asks the costly exclusion only for done tasks of the week", () => {
    const asked: string[] = [];
    const tasks = [done("week", "2026-10-28T12:00:00Z"), done("before", "2026-10-20T12:00:00Z"), { ...done("open", "2026-10-28T12:00:00Z"), status: "open" } as Task];
    weekBalance(tasks, "2026-W44", (t) => (asked.push(t.id), false));
    expect(asked).toEqual(["week"]);
  });
});
