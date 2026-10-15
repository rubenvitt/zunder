// The mock server's calendar of follow-up instances (contract 3.1), checked with the
// examples of the contract and zunder_core::recurrence. Runs in Europe/Berlin (vite.config).

import { describe, expect, it } from "vitest";
import { nextOccurrence, type MockRecurrence } from "../e2e/mock-recurrence";

const at = (y: number, m: number, d: number, hh = 12) => new Date(y, m - 1, d, hh).getTime();
const next = (r: MockRecurrence, due: string, completed: number, today = completed, remind: string | null = null) =>
  nextOccurrence(r, due, remind, completed, today);

describe("from due", () => {
  const last: MockRecurrence = { every: 1, unit: "month", day: "last", from: "due" };

  it("skips missed dates and never creates more than one instance", () => {
    expect(next(last, "2026-07-31", at(2026, 10, 15))?.due).toBe("2026-10-31");
    expect(next({ every: 1, unit: "day", from: "due" }, "2026-10-08", at(2026, 10, 15))?.due).toBe("2026-10-15");
  });

  it("completing early gives the next date after the old due date", () => {
    expect(next(last, "2026-10-31", at(2026, 10, 20))?.due).toBe("2026-11-30");
    expect(next(last, "2027-01-31", at(2026, 10, 20))?.due).toBe("2027-02-28");
  });

  it("keeps a monthly day without drift and writes it into the series", () => {
    const monthly: MockRecurrence = { every: 1, unit: "month", from: "due" };
    const first = next(monthly, "2027-01-31", at(2027, 1, 31))!;
    expect(first).toMatchObject({ due: "2027-02-28", recurrence: { every: 1, unit: "month", day: 31, from: "due" } });
    expect(next(first.recurrence, first.due, at(2027, 2, 28))?.due).toBe("2027-03-31");
  });

  it("snaps weekdays to the weeks of the series", () => {
    const r: MockRecurrence = { every: 2, unit: "week", weekdays: ["mo", "th"], from: "due" };
    // Monday 2026-10-12: Thursday of the same week, then Monday two weeks later.
    expect(next(r, "2026-10-12", at(2026, 10, 12))?.due).toBe("2026-10-15");
    expect(next(r, "2026-10-15", at(2026, 10, 15))?.due).toBe("2026-10-26");
    expect(next({ every: 1, unit: "week", from: "due" }, "2026-10-12", at(2026, 10, 12))?.due).toBe("2026-10-19");
  });

  it("keeps the last day of February in yearly series", () => {
    const yearly: MockRecurrence = { every: 1, unit: "year", from: "due" };
    expect(next(yearly, "2028-02-29", at(2028, 2, 29))?.due).toBe("2029-02-28");
    expect(next(yearly, "2031-02-28", at(2031, 2, 28))?.due).toBe("2032-02-29");
    expect(next(yearly, "2026-10-12", at(2026, 10, 12))?.due).toBe("2027-10-12");
    expect(next(yearly, "2026-10-12", at(2026, 10, 12))?.recurrence).toEqual(yearly);
  });
});

describe("from done", () => {
  it("counts from the completion date", () => {
    expect(next({ every: 3, unit: "month", from: "done" }, "2026-01-01", at(2026, 10, 15))?.due).toBe("2027-01-15");
    expect(next({ every: 1, unit: "month", day: "last", from: "done" }, "2026-01-01", at(2026, 10, 15))?.due).toBe("2026-11-30");
    // Wednesday 2026-10-14 → Monday of the next week.
    expect(next({ every: 1, unit: "week", weekdays: ["mo", "th"], from: "done" }, "2026-01-01", at(2026, 10, 14))?.due).toBe("2026-10-19");
    expect(next({ every: 2, unit: "day", from: "done" }, "2026-01-01", at(2026, 10, 14))?.due).toBe("2026-10-16");
    expect(next({ every: 1, unit: "year", from: "done" }, "2026-01-01", at(2028, 2, 29))?.due).toBe("2029-02-28");
  });
});

describe("time of day", () => {
  it("keeps the local time across the change to winter time", () => {
    expect(next({ every: 1, unit: "day", from: "due" }, "2026-10-24T09:00:00+02:00", at(2026, 10, 24))?.due).toBe("2026-10-25T09:00:00+01:00");
  });

  it("a time in the spring gap is later by the gap", () => {
    expect(next({ every: 1, unit: "day", from: "due" }, "2027-03-27T02:30:00+01:00", at(2027, 3, 27))?.due).toBe("2027-03-28T03:30:00+02:00");
  });

  it("moves the reminder by as many days as the due date", () => {
    const result = next({ every: 1, unit: "month", day: "last", from: "due" }, "2026-10-31", at(2026, 10, 31), at(2026, 10, 31), "2026-10-30T18:00:00+01:00");
    expect(result).toMatchObject({ due: "2026-11-30", remind_at: "2026-11-29T18:00:00+01:00" });
    expect(next({ every: 1, unit: "day", from: "due" }, "2026-10-31", at(2026, 10, 31))?.remind_at).toBeNull();
  });
});
