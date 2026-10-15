import { describe, expect, it } from "vitest";
import { describeRecurrence, describeValue, matchPreset, nextInstanceMessage, recurrencePresets } from "../src/labels";
import { TaskIndex } from "../src/sync/derived";
import { mergeOps } from "../src/sync/merge";
import { toTask } from "../src/sync/model";
import { recurrenceProblem, recurrenceValue, toggleWeekday, weekdayOf, withDay, withEvery, withFrom, withUnit } from "../src/sync/recurrence";
import type { Op, Recurrence } from "../src/sync/types";
import { op } from "./helpers";

const r = (x: Recurrence) => x;

describe("describeRecurrence (contract 5.1)", () => {
  it("names the unit, every n, weekdays, the day and the anchor", () => {
    expect(describeRecurrence(r({ every: 1, unit: "day", from: "due" }))).toBe("Täglich");
    expect(describeRecurrence(r({ every: 1, unit: "week", from: "due" }))).toBe("Wöchentlich");
    expect(describeRecurrence(r({ every: 1, unit: "month", from: "due" }))).toBe("Monatlich");
    expect(describeRecurrence(r({ every: 1, unit: "year", from: "due" }))).toBe("Jährlich");
    expect(describeRecurrence(r({ every: 3, unit: "day", from: "due" }))).toBe("Alle 3 Tage");
    expect(describeRecurrence(r({ every: 2, unit: "week", from: "due" }))).toBe("Alle 2 Wochen");
    expect(describeRecurrence(r({ every: 3, unit: "month", from: "done" }))).toBe("Alle 3 Monate nach Erledigung");
    expect(describeRecurrence(r({ every: 2, unit: "year", from: "due" }))).toBe("Alle 2 Jahre");
  });

  it("lists weekdays from Monday and calls Monday to Friday Werktags", () => {
    expect(describeRecurrence(r({ every: 1, unit: "week", weekdays: ["th", "mo"], from: "due" }))).toBe("Wöchentlich (Mo, Do)");
    expect(describeRecurrence(r({ every: 1, unit: "week", weekdays: ["fr", "th", "we", "tu", "mo"], from: "due" }))).toBe("Werktags");
    expect(describeRecurrence(r({ every: 2, unit: "week", weekdays: ["mo", "tu", "we", "th", "fr"], from: "due" }))).toBe(
      "Alle 2 Wochen (Mo, Di, Mi, Do, Fr)",
    );
    expect(describeRecurrence(r({ every: 1, unit: "week", weekdays: ["su"], from: "done" }))).toBe("Wöchentlich (So) nach Erledigung");
  });

  it("names a fixed day of the month", () => {
    expect(describeRecurrence(r({ every: 1, unit: "month", day: "last", from: "due" }))).toBe("Monatlich am Monatsletzten");
    expect(describeRecurrence(r({ every: 1, unit: "month", day: "last", from: "done" }))).toBe("Monatlich am Monatsletzten nach Erledigung");
    expect(describeRecurrence(r({ every: 2, unit: "month", day: 15, from: "due" }))).toBe("Alle 2 Monate am 15.");
  });

  it("names the day of a yearly rhythm with the month of the due date", () => {
    expect(describeRecurrence(r({ every: 1, unit: "year", day: 28, from: "due" }), "2027-02-28")).toBe("Jährlich am 28.02.");
    expect(describeRecurrence(r({ every: 1, unit: "year", day: 29, from: "due" }), "2027-02-28T09:00:00+01:00")).toBe("Jährlich am 29.02.");
    // From done the month of the completion counts: no month.
    expect(describeRecurrence(r({ every: 2, unit: "year", day: 5, from: "done" }), "2026-10-05")).toBe("Alle 2 Jahre am 5. nach Erledigung");
    // A day beyond the month's length is its last (February: 29, back in leap years).
    expect(describeRecurrence(r({ every: 1, unit: "year", day: 31, from: "due" }), "2027-04-10")).toBe("Jährlich am 30.04.");
    expect(describeRecurrence(r({ every: 1, unit: "year", day: 31, from: "due" }), "2027-02-10")).toBe("Jährlich am 29.02.");
    expect(describeRecurrence(r({ every: 1, unit: "year", day: 31, from: "due" }), "2027-12-10")).toBe("Jährlich am 31.12.");
    expect(describeRecurrence(r({ every: 1, unit: "year", day: "last", from: "due" }), "2027-02-28")).toBe("Jährlich am Monatsletzten");
    // Without a due date only the day.
    expect(describeRecurrence(r({ every: 1, unit: "year", day: 28, from: "due" }))).toBe("Jährlich am 28.");
    // The due date does not change other units.
    expect(describeRecurrence(r({ every: 1, unit: "month", day: 15, from: "due" }), "2027-02-15")).toBe("Monatlich am 15.");
  });

  it("describes a recurrence in the change history", () => {
    expect(describeValue("recurrence", { every: 1, unit: "month", day: "last", from: "due" })).toBe("Monatlich am Monatsletzten");
    expect(describeValue("recurrence", null)).toBe("entfernt");
  });
});

describe("editing a recurrence", () => {
  const weekly: Recurrence = { every: 2, unit: "week", weekdays: ["mo", "th"], from: "due" };
  const monthly: Recurrence = { every: 1, unit: "month", day: "last", from: "done" };

  it("drops weekdays and day that do not fit the new unit", () => {
    expect(withUnit(weekly, "month")).toEqual({ every: 2, unit: "month", from: "due" });
    expect(withUnit(monthly, "week")).toEqual({ every: 1, unit: "week", from: "done" });
    expect(withUnit(monthly, "year")).toEqual({ every: 1, unit: "year", day: "last", from: "done" });
    expect(withUnit(withUnit(monthly, "year"), "month")).toEqual(monthly);
    expect(withUnit(weekly, "week")).toEqual(weekly);
    for (const v of [withUnit(weekly, "day"), withUnit(monthly, "day")]) expect(recurrenceProblem(recurrenceValue(v))).toBeNull();
  });

  it("toggles weekdays in calendar order and leaves out an empty list", () => {
    expect(toggleWeekday(weekly, "we").weekdays).toEqual(["mo", "we", "th"]);
    const none = toggleWeekday(toggleWeekday(weekly, "mo"), "th");
    expect(none).toEqual({ every: 2, unit: "week", from: "due" });
    expect("weekdays" in none).toBe(false);
  });

  it("sets or clears the day, the interval and the anchor", () => {
    expect(withDay(monthly, 15)).toEqual({ every: 1, unit: "month", day: 15, from: "done" });
    expect(withDay({ every: 1, unit: "year", from: "due" }, 28)).toEqual({ every: 1, unit: "year", day: 28, from: "due" });
    expect(withDay({ every: 1, unit: "week", from: "due" }, 28)).toEqual({ every: 1, unit: "week", from: "due" });
    const cleared = withDay(monthly, undefined);
    expect(cleared).toEqual({ every: 1, unit: "month", from: "done" });
    expect("day" in cleared).toBe(false);
    expect(withEvery(monthly, 3).every).toBe(3);
    expect(withFrom(weekly, "done")).toEqual({ ...weekly, from: "done" });
  });

  it("writes only present keys (null or undefined would be invalid)", () => {
    expect(recurrenceValue({ every: 1, unit: "day", from: "due" })).toEqual({ every: 1, unit: "day", from: "due" });
    expect(recurrenceValue(weekly)).toEqual({ every: 2, unit: "week", weekdays: ["mo", "th"], from: "due" });
  });

  it("knows the weekday of a date", () => {
    expect(weekdayOf("2026-10-30")).toBe("fr");
    expect(weekdayOf("2026-10-26")).toBe("mo");
    expect(weekdayOf("2026-11-01")).toBe("su");
  });
});

describe("presets", () => {
  it("offers the common rhythms, weekly on the weekday of the due date", () => {
    const presets = recurrencePresets("2026-10-30");
    expect(presets.map((p) => p.label)).toEqual([
      "Täglich",
      "Wöchentlich am Freitag",
      "Alle 2 Wochen",
      "Monatlich",
      "Monatlich am Monatsletzten",
      "Jährlich",
    ]);
    expect(presets[1]!.value).toEqual({ every: 1, unit: "week", weekdays: ["fr"], from: "due" });
    expect(presets[4]!.value).toEqual({ every: 1, unit: "month", day: "last", from: "due" });
    for (const p of presets) expect(recurrenceProblem(recurrenceValue(p.value))).toBeNull();
  });

  it("matches a stored recurrence to its preset, otherwise none", () => {
    const presets = recurrencePresets("2026-10-30T09:00:00+01:00");
    expect(matchPreset(presets, { every: 1, unit: "month", day: "last", from: "due" })?.label).toBe("Monatlich am Monatsletzten");
    expect(matchPreset(presets, { every: 1, unit: "week", weekdays: ["fr"], from: "due" })?.label).toBe("Wöchentlich am Freitag");
    expect(matchPreset(presets, { every: 1, unit: "week", weekdays: ["mo"], from: "due" })).toBeUndefined();
    expect(matchPreset(presets, { every: 1, unit: "month", day: "last", from: "done" })).toBeUndefined();
    expect(matchPreset(presets, null)).toBeUndefined();
  });

  it("without a due date there is no weekly preset on a weekday", () => {
    expect(recurrencePresets(null).map((p) => p.label)).not.toContain("Wöchentlich am Freitag");
  });
});

describe("next instance message", () => {
  const now = new Date(2026, 9, 15, 12, 0);
  it("names the date of the next instance", () => {
    expect(nextInstanceMessage("2026-10-31", now)).toBe("Nächste Instanz am Sa, 31.10.");
    expect(nextInstanceMessage("2027-01-31", now)).toBe("Nächste Instanz am So, 31.01.2027");
    expect(nextInstanceMessage("2026-10-16", now)).toBe("Nächste Instanz morgen");
    expect(nextInstanceMessage("2026-10-15", now)).toBe("Nächste Instanz heute");
    expect(nextInstanceMessage("2026-11-02T09:00:00+01:00", now)).toBe("Nächste Instanz am Mo, 02.11. um 09:00");
  });
});

describe("series in the task index", () => {
  function index(ops: Op[]): TaskIndex {
    return new TaskIndex([...mergeOps(ops).values()].filter((e) => e.entity === "task").map(toTask));
  }
  const series = [
    op("task", "a", "title", "Odoo", 1000),
    op("task", "b", "title", "Odoo", 2000),
    op("task", "b", "recurrence_prev", "a", 2000, { counter: 1 }),
    op("task", "c", "title", "Odoo (wiederhergestellt)", 3000),
    op("task", "c", "recurrence_prev", "a", 3000, { counter: 1 }),
  ];

  it("links the previous and the next instance (the first created when there are several)", () => {
    const idx = index(series);
    expect(idx.nextInstance("a")?.id).toBe("b");
    expect(idx.previousInstance(idx.byId.get("b")!)?.id).toBe("a");
    expect(idx.nextInstance("b")).toBeUndefined();
    expect(idx.previousInstance(idx.byId.get("a")!)).toBeUndefined();
  });

  it("ignores deleted instances", () => {
    const idx = index([...series, op("task", "b", "deleted", true, 4000), op("task", "a", "deleted", true, 4000)]);
    expect(idx.nextInstance("a")?.id).toBe("c");
    expect(idx.previousInstance(idx.byId.get("c")!)).toBeUndefined();
  });
});
