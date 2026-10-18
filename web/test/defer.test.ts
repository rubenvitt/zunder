// "Heute nicht" (contract 5.1): the client mirror of zunder_core::defer.

import { describe, expect, it } from "vitest";
import { deferDay, deferFields, needsDecision, offersNextWeek } from "../src/sync/defer";

const base = { due: null, remind_at: null, deferrals: 0, undated_ack: null };

describe("deferDay", () => {
  it("names tomorrow and the Monday of next week", () => {
    expect(deferDay("tomorrow", "2026-10-16")).toBe("2026-10-17");
    expect(deferDay("tomorrow", "2026-12-31")).toBe("2027-01-01");
    // Friday → Monday; Sunday → the next day; Monday → a week later.
    expect(deferDay("next_week", "2026-10-16")).toBe("2026-10-19");
    expect(deferDay("next_week", "2026-10-18")).toBe("2026-10-19");
    expect(deferDay("next_week", "2026-10-19")).toBe("2026-10-26");
  });

  it("offers next week except on Sundays, where it is tomorrow", () => {
    expect(offersNextWeek("2026-10-16")).toBe(true);
    expect(offersNextWeek("2026-10-17")).toBe(true);
    expect(offersNextWeek("2026-10-18")).toBe(false);
    expect(offersNextWeek("2026-10-19")).toBe(true);
  });
});

describe("deferFields", () => {
  it("moves an earlier due date and counts", () => {
    expect(deferFields({ ...base, due: "2026-10-14" }, "2026-10-17")).toEqual({ due: "2026-10-17", deferrals: 1 });
  });

  it("keeps time of day and offset of timestamps", () => {
    const fields = deferFields({ ...base, due: "2026-10-16T18:30:00+02:00", remind_at: "2026-10-16T09:00:00+02:00", deferrals: 1 }, "2026-10-19");
    expect(fields).toEqual({ due: "2026-10-19T18:30:00+02:00", remind_at: "2026-10-19T09:00:00+02:00", deferrals: 2 });
  });

  it("moves a reminder alone without adding a due date", () => {
    expect(deferFields({ ...base, remind_at: "2026-10-16T09:00:00Z" }, "2026-10-17")).toEqual({ remind_at: "2026-10-17T09:00:00Z", deferrals: 1 });
  });

  it("gives an undated task the day and clears the undated mark", () => {
    expect(deferFields(base, "2026-10-17")).toEqual({ due: "2026-10-17", deferrals: 1 });
    expect(deferFields({ ...base, due: "2026-10-01", undated_ack: "2026-09-01T08:00:00Z" }, "2026-10-17")).toEqual({
      due: "2026-10-17",
      undated_ack: null,
      deferrals: 1,
    });
  });

  it("changes nothing when everything already lies later", () => {
    expect(deferFields({ ...base, due: "2026-10-20", remind_at: "2026-10-19T09:00:00Z" }, "2026-10-17")).toBeNull();
    expect(deferFields({ ...base, due: "2026-10-15", remind_at: "2026-10-30T09:00:00Z" }, "2026-10-17")).toEqual({ due: "2026-10-17", deferrals: 1 });
  });
});

describe("needsDecision", () => {
  it("makes the third deferral a decision", () => {
    expect(needsDecision({ deferrals: 0 })).toBe(false);
    expect(needsDecision({ deferrals: 1 })).toBe(false);
    expect(needsDecision({ deferrals: 2 })).toBe(true);
  });
});
