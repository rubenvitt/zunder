// The mock server's calendar of follow-up instances (contract 3.1), checked with the
// examples of the contract and zunder_core::recurrence, and its shape check of
// `recurrence` (contract 2.5). Runs in Europe/Berlin (vite.config).

import { describe, expect, it } from "vitest";
import { nextOccurrence, type MockRecurrence } from "../e2e/mock-recurrence";
import { recurrenceProblem as mockProblem } from "../e2e/mock-recurrence-shape";
import { recurrenceProblem as clientProblem } from "../src/sync/recurrence";
import type { JsonValue } from "../src/sync/types";

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

  /** Follows a series n times, each instance completed on its due date. */
  const chain = (r: MockRecurrence, due: string, n: number) => {
    const out: string[] = [];
    for (let i = 0; i < n; i++) {
      const [y, m, d] = due.split("-").map(Number);
      const result = next(r, due, at(y!, m!, d!))!;
      out.push(result.due);
      ({ due, recurrence: r } = result);
    }
    return out;
  };

  it("keeps the day of yearly series without drift and writes it into the series", () => {
    const yearly: MockRecurrence = { every: 1, unit: "year", from: "due" };
    expect(chain(yearly, "2027-02-28", 8)).toEqual(["2028", "2029", "2030", "2031", "2032", "2033", "2034", "2035"].map((y) => `${y}-02-28`));
    expect(chain(yearly, "2028-02-29", 4)).toEqual(["2029-02-28", "2030-02-28", "2031-02-28", "2032-02-29"]);
    expect(next(yearly, "2026-10-12", at(2026, 10, 12))).toMatchObject({ due: "2027-10-12", recurrence: { ...yearly, day: 12 } });
    expect(chain({ every: 1, unit: "year", day: "last", from: "due" }, "2027-02-28", 2)).toEqual(["2028-02-29", "2029-02-28"]);
    // Unlike months, at least a year: a moved instance waits a year and takes the series along.
    expect(next({ every: 1, unit: "year", day: 20, from: "due" }, "2026-03-10", at(2026, 3, 10))?.due).toBe("2027-03-20");
    expect(next({ every: 1, unit: "year", day: 31, from: "due" }, "2027-06-10", at(2027, 6, 10))?.due).toBe("2028-06-30");
    expect(next({ every: 1, unit: "year", day: 28, from: "due" }, "2027-02-20", at(2027, 2, 20))?.due).toBe("2028-02-28");
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
    expect(next({ every: 1, unit: "year", day: "last", from: "done" }, "2026-01-01", at(2027, 2, 10))?.due).toBe("2028-02-29");
    expect(next({ every: 1, unit: "year", from: "done" }, "2026-01-01", at(2027, 2, 10))?.recurrence).toEqual({ every: 1, unit: "year", from: "done" });
  });
});

describe("time of day", () => {
  it("keeps the local time across the change to winter time", () => {
    expect(next({ every: 1, unit: "day", from: "due" }, "2026-10-24T09:00:00+02:00", at(2026, 10, 24))?.due).toBe("2026-10-25T09:00:00+01:00");
  });

  it("a time in the spring gap is later by the gap", () => {
    expect(next({ every: 1, unit: "day", from: "due" }, "2027-03-27T02:30:00+01:00", at(2027, 3, 27))?.due).toBe("2027-03-28T03:30:00+02:00");
  });

  it("keeps fractional seconds like the core (no trailing zeros)", () => {
    const daily: MockRecurrence = { every: 1, unit: "day", from: "due" };
    expect(next(daily, "2026-10-24T09:00:00.500+02:00", at(2026, 10, 24))?.due).toBe("2026-10-25T09:00:00.5+01:00");
    expect(next(daily, "2026-10-24T09:00:00.000+02:00", at(2026, 10, 24))?.due).toBe("2026-10-25T09:00:00+01:00");
    expect(next(daily, "2026-10-24", at(2026, 10, 24), at(2026, 10, 24), "2026-10-23T18:00:00.123456789+02:00")?.remind_at).toBe(
      "2026-10-24T18:00:00.123456789+02:00",
    );
  });

  it("moves the reminder by as many days as the due date", () => {
    const result = next({ every: 1, unit: "month", day: "last", from: "due" }, "2026-10-31", at(2026, 10, 31), at(2026, 10, 31), "2026-10-30T18:00:00+01:00");
    expect(result).toMatchObject({ due: "2026-11-30", remind_at: "2026-11-29T18:00:00+01:00" });
    expect(next({ every: 1, unit: "day", from: "due" }, "2026-10-31", at(2026, 10, 31))?.remind_at).toBeNull();
  });
});

describe("shape of recurrence", () => {
  // The samples of zunder_core::model's test `recurrences` (null is handled before the shape check).
  const valid: JsonValue[] = [
    { every: 1, unit: "day", from: "due" },
    { from: "done", unit: "year", every: 999 },
    { every: 2, unit: "week", weekdays: ["th", "mo"], from: "due" },
    { every: 1, unit: "week", weekdays: ["mo", "tu", "we", "th", "fr", "sa", "su"], from: "done" },
    { every: 1, unit: "month", day: "last", from: "due" },
    { every: 3, unit: "month", day: 31, from: "done" },
    { every: 1, unit: "month", day: 1, from: "due" },
    { every: 1, unit: "year", day: 28, from: "due" },
    { every: 2, unit: "year", day: "last", from: "done" },
  ];
  const invalid: JsonValue[] = [
    "monatlich",
    [],
    {},
    { every: 0, unit: "day", from: "due" },
    { every: 1000, unit: "day", from: "due" },
    { every: -1, unit: "day", from: "due" },
    { every: 1.5, unit: "day", from: "due" },
    { every: "1", unit: "day", from: "due" },
    { unit: "day", from: "due" },
    { every: 1, unit: "hour", from: "due" },
    { every: 1, from: "due" },
    { every: 1, unit: "day" },
    { every: 1, unit: "day", from: "start" },
    { every: 1, unit: "day", from: "due", until: "2027-01-01" },
    { every: 1, unit: "week", weekdays: [], from: "due" },
    { every: 1, unit: "week", weekdays: ["mo", "mo"], from: "due" },
    { every: 1, unit: "week", weekdays: ["montag"], from: "due" },
    { every: 1, unit: "week", weekdays: ["MO"], from: "due" },
    { every: 1, unit: "week", weekdays: "mo", from: "due" },
    { every: 1, unit: "week", weekdays: null, from: "due" },
    { every: 1, unit: "day", weekdays: ["mo"], from: "due" },
    { every: 1, unit: "month", weekdays: ["mo"], from: "due" },
    { every: 1, unit: "month", day: 0, from: "due" },
    { every: 1, unit: "month", day: 32, from: "due" },
    { every: 1, unit: "month", day: "first", from: "due" },
    { every: 1, unit: "month", day: "15", from: "due" },
    { every: 1, unit: "month", day: null, from: "due" },
    { every: 1, unit: "month", day: 15, weekdays: ["mo"], from: "due" },
    { every: 1, unit: "day", day: 29, from: "due" },
    { every: 1, unit: "week", day: "last", from: "due" },
    { every: 1, unit: "year", day: 0, from: "due" },
    { every: 1, unit: "year", weekdays: ["mo"], from: "due" },
  ];

  it("accepts and rejects like the core", () => {
    for (const v of valid) expect(mockProblem(v), JSON.stringify(v)).toBeNull();
    for (const v of invalid) expect(mockProblem(v), JSON.stringify(v)).not.toBeNull();
  });

  it("gives the same reason as the client (and so the core)", () => {
    for (const v of [...valid, ...invalid]) expect(mockProblem(v), JSON.stringify(v)).toBe(clientProblem(v));
  });
});
