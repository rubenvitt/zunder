import { describe, expect, it } from "vitest";
import { parseCapture, slugify } from "../src/capture/parse";
import type { Recurrence } from "../src/sync/types";

// Sunday, 11 Oct 2026, 14:30 in Europe/Berlin (CEST, +02:00); TZ is set in vite.config.ts.
const NOW = new Date(2026, 9, 11, 14, 30);
const structure = {
  areas: [
    { id: "area-gesundheit", name: "Gesundheit" },
    { id: "area-buero", name: "Büro" },
    { id: "area-zunder", name: "Zunder" },
    { id: "area-arbeit", name: "Arbeit" },
  ],
  projects: [
    { id: "proj-zunder", name: "Zunder" },
    { id: "proj-umzug", name: "Umzug 2027" },
  ],
};
const parse = (input: string, now = NOW) => parseCapture(input, now, structure);
const parseNoRhythm = (input: string, now = NOW) => parseCapture(input, now, structure, { recurrence: false });

describe("quick capture parser", () => {
  it("parses the contract example", () => {
    const r = parse("Zahnarzt anrufen morgen 9:00 #gesundheit");
    expect(r.title).toBe("Zahnarzt anrufen");
    expect(r.due).toBe("2026-10-12T09:00:00+02:00");
    expect(r.remind_at).toBe("2026-10-12T09:00:00+02:00");
    expect(r.location).toEqual({ kind: "area", id: "area-gesundheit" });
    expect(r.tokens.map((t) => t.kind)).toEqual(["date", "time", "area"]);
  });

  it("keeps a plain title untouched and lands in the inbox", () => {
    const r = parse("  Milch kaufen  ");
    expect(r).toMatchObject({ title: "Milch kaufen", due: null, remind_at: null, location: null, tokens: [] });
  });

  it.each([
    ["heute", "2026-10-11"],
    ["morgen", "2026-10-12"],
    ["übermorgen", "2026-10-13"],
    ["uebermorgen", "2026-10-13"],
    ["Montag", "2026-10-12"],
    ["mittwoch", "2026-10-14"],
    ["Sonntag", "2026-10-18"],
    ["12.10.", "2026-10-12"],
    ["1.11.", "2026-11-01"],
    ["12.10.2027", "2027-10-12"],
    ["10.10.", "2027-10-10"],
    ["am Freitag", "2026-10-16"],
  ])("date-only %s → due %s without reminder", (token, due) => {
    const r = parse(`Bericht schreiben ${token}`);
    expect(r.title).toBe("Bericht schreiben");
    expect(r.due).toBe(due);
    expect(r.remind_at).toBeNull();
  });

  it("nächsten <Wochentag> means that weekday in the following calendar week", () => {
    // Wednesday 14 Oct 2026: "Freitag" is this week, "nächsten Freitag" next week.
    const wed = new Date(2026, 9, 14, 8, 0);
    expect(parse("Abgabe Freitag", wed).due).toBe("2026-10-16");
    expect(parse("Abgabe nächsten Freitag", wed).due).toBe("2026-10-23");
    expect(parse("Abgabe nächsten Freitag", wed).title).toBe("Abgabe");
    expect(parse("Abgabe naechsten montag", wed).due).toBe("2026-10-19");
  });

  it("parses times in several notations", () => {
    expect(parse("Call 9 Uhr morgen").due).toBe("2026-10-12T09:00:00+02:00");
    expect(parse("Call morgen um 9:30 Uhr").due).toBe("2026-10-12T09:30:00+02:00");
    expect(parse("Call morgen um 9:30 Uhr").title).toBe("Call");
    expect(parse("Call 17uhr heute").due).toBe("2026-10-11T17:00:00+02:00");
  });

  it("a bare time later today stays today, an earlier one moves to tomorrow", () => {
    expect(parse("Tee 16:00").due).toBe("2026-10-11T16:00:00+02:00");
    expect(parse("Tee 9:00").due).toBe("2026-10-12T09:00:00+02:00");
  });

  it("uses the winter offset after the DST change", () => {
    expect(parse("Termin 2.11. 10:15").due).toBe("2026-11-02T10:15:00+01:00");
  });

  it("ignores invalid dates and times", () => {
    const r = parse("Version 31.02. 25:00 10 Kilo");
    expect(r.title).toBe("Version 31.02. 25:00 10 Kilo");
    expect(r.due).toBeNull();
  });

  it("resolves #slug to areas and projects, project wins on a tie", () => {
    expect(parse("Release #zunder").location).toEqual({ kind: "project", id: "proj-zunder" });
    expect(parse("Kisten #umzug-2027").location).toEqual({ kind: "project", id: "proj-umzug" });
    expect(parse("Drucker #büro").location).toEqual({ kind: "area", id: "area-buero" });
    expect(parse("Drucker #BUERO").location).toEqual({ kind: "area", id: "area-buero" });
  });

  it("prefers a project over an area when both tags are given", () => {
    expect(parse("Etwas #gesundheit #umzug-2027").location).toEqual({ kind: "project", id: "proj-umzug" });
  });

  it("keeps unknown tags in the title and reports them", () => {
    const r = parse("Etwas #unbekannt morgen");
    expect(r.title).toBe("Etwas #unbekannt");
    expect(r.unknownTags).toEqual(["#unbekannt"]);
    expect(r.location).toBeNull();
  });

  it("only takes the first date and the first time", () => {
    const r = parse("heute morgen 9:00 10:00");
    expect(r.due).toBe("2026-10-11T09:00:00+02:00");
    expect(r.title).toBe("morgen 10:00");
  });

  it("does not swallow prepositions that do not precede a token", () => {
    expect(parse("Treffen am Bahnhof").title).toBe("Treffen am Bahnhof");
    expect(parse("Treffen um Mitternacht").title).toBe("Treffen um Mitternacht");
  });

  it("falls back to the raw input when only tokens were typed", () => {
    const r = parse("morgen");
    expect(r.title).toBe("morgen");
    expect(r.due).toBe("2026-10-12");
  });

  it("slugify transliterates umlauts and punctuation", () => {
    expect(slugify("Größe & Übermut")).toBe("groesse-uebermut");
    expect(slugify("Café Noir")).toBe("cafe-noir");
  });

  it.each(["Refactor constructor logic", "Check __proto__ handling", "nächsten constructor prüfen", "nächsten __proto__ prüfen"])(
    "does not take inherited object keys for weekdays: %s",
    (input) => {
      const r = parse(input);
      expect(r.title).toBe(input);
      expect(r.due).toBeNull();
      expect(r.tokens).toEqual([]);
    },
  );
});

// Contract 5.1, "Kurzschreibweise für Rhythmen". NOW is Sunday, 11 Oct 2026, 14:30.
describe("quick capture recurrence shorthand", () => {
  const week = (every: number, weekdays?: Recurrence["weekdays"]): Recurrence =>
    weekdays ? { every, unit: "week", weekdays, from: "due" } : { every, unit: "week", from: "due" };

  it("parses the contract example", () => {
    const r = parse("Odoo absenden und Stunden bestätigen jeden Monatsletzten #arbeit");
    expect(r.title).toBe("Odoo absenden und Stunden bestätigen");
    expect(r.due).toBe("2026-10-31");
    expect(r.remind_at).toBeNull();
    expect(r.recurrence).toEqual({ every: 1, unit: "month", day: "last", from: "due" });
    expect(r.location).toEqual({ kind: "area", id: "area-arbeit" });
    expect(r.tokens).toContainEqual({ kind: "recurrence", text: "jeden Monatsletzten", label: "Monatlich am Monatsletzten" });
    expect(parse("Frist jeden 28.02.").tokens).toContainEqual({ kind: "recurrence", text: "jeden 28.02.", label: "Jährlich am 28.02." });
  });

  it.each<[string, Recurrence, string]>([
    ["täglich", { every: 1, unit: "day", from: "due" }, "2026-10-11"],
    ["taeglich", { every: 1, unit: "day", from: "due" }, "2026-10-11"],
    ["jeden Tag", { every: 1, unit: "day", from: "due" }, "2026-10-11"],
    ["alle 3 Tage", { every: 3, unit: "day", from: "due" }, "2026-10-11"],
    ["wöchentlich", week(1), "2026-10-11"],
    ["jede Woche", week(1), "2026-10-11"],
    ["alle 2 Wochen", week(2), "2026-10-11"],
    ["werktags", week(1, ["mo", "tu", "we", "th", "fr"]), "2026-10-12"],
    ["jeden Freitag", week(1, ["fr"]), "2026-10-16"],
    // Today is Sunday: a series on Sundays starts today.
    ["jeden Sonntag", week(1, ["su"]), "2026-10-11"],
    ["jeden Sonnabend", week(1, ["sa"]), "2026-10-17"],
    ["jeden Montag und Donnerstag", week(1, ["mo", "th"]), "2026-10-12"],
    ["jeden Montag, Mittwoch und Freitag", week(1, ["mo", "we", "fr"]), "2026-10-12"],
    ["jeden Donnerstag und Montag", week(1, ["mo", "th"]), "2026-10-12"],
    ["freitags", week(1, ["fr"]), "2026-10-16"],
    ["montags, mittwochs und freitags", week(1, ["mo", "we", "fr"]), "2026-10-12"],
    ["alle 2 Wochen montags und donnerstags", week(2, ["mo", "th"]), "2026-10-12"],
    ["wöchentlich am Montag und Donnerstag", week(1, ["mo", "th"]), "2026-10-12"],
    ["jede Woche am Freitag", week(1, ["fr"]), "2026-10-16"],
    ["alle 3 Wochen dienstags", week(3, ["tu"]), "2026-10-13"],
    ["monatlich", { every: 1, unit: "month", from: "due" }, "2026-10-11"],
    ["jeden Monat", { every: 1, unit: "month", from: "due" }, "2026-10-11"],
    ["alle 3 Monate", { every: 3, unit: "month", from: "due" }, "2026-10-11"],
    ["jeden 15.", { every: 1, unit: "month", day: 15, from: "due" }, "2026-10-15"],
    ["jeden 11.", { every: 1, unit: "month", day: 11, from: "due" }, "2026-10-11"],
    ["jeden 5.", { every: 1, unit: "month", day: 5, from: "due" }, "2026-11-05"],
    ["jeden 12.10.", { every: 1, unit: "year", day: 12, from: "due" }, "2026-10-12"],
    ["jeden 1.3.", { every: 1, unit: "year", day: 1, from: "due" }, "2027-03-01"],
    ["jeden 28.02.", { every: 1, unit: "year", day: 28, from: "due" }, "2027-02-28"],
    ["jeden 29.02.", { every: 1, unit: "year", day: 29, from: "due" }, "2027-02-28"],
    ["jeden Monatsletzten", { every: 1, unit: "month", day: "last", from: "due" }, "2026-10-31"],
    ["jeden letzten des Monats", { every: 1, unit: "month", day: "last", from: "due" }, "2026-10-31"],
    ["jeden letzten Tag des Monats", { every: 1, unit: "month", day: "last", from: "due" }, "2026-10-31"],
    ["am Monatsende", { every: 1, unit: "month", day: "last", from: "due" }, "2026-10-31"],
    ["jedes Monatsende", { every: 1, unit: "month", day: "last", from: "due" }, "2026-10-31"],
    ["jährlich", { every: 1, unit: "year", from: "due" }, "2026-10-11"],
    ["jedes Jahr", { every: 1, unit: "year", from: "due" }, "2026-10-11"],
    ["alle 2 Jahre", { every: 2, unit: "year", from: "due" }, "2026-10-11"],
    ["alle 999 Tage", { every: 999, unit: "day", from: "due" }, "2026-10-11"],
    ["monatlich nach Erledigung", { every: 1, unit: "month", from: "done" }, "2026-10-11"],
    ["alle 3 Tage ab Erledigung", { every: 3, unit: "day", from: "done" }, "2026-10-11"],
    ["jeden Monatsletzten nach Erledigung", { every: 1, unit: "month", day: "last", from: "done" }, "2026-10-31"],
    ["JEDEN freitag", week(1, ["fr"]), "2026-10-16"],
    ["Täglich", { every: 1, unit: "day", from: "due" }, "2026-10-11"],
  ])("%s → rhythm and first due date", (shorthand, recurrence, due) => {
    const r = parse(`Gießen ${shorthand}`);
    expect(r.title).toBe("Gießen");
    expect(r.recurrence).toEqual(recurrence);
    expect(r.due).toBe(due);
    expect(r.remind_at).toBeNull();
    expect(r.tokens).toEqual([{ kind: "recurrence", text: shorthand, label: expect.any(String) }]);
  });

  it("recognises the shorthand anywhere in the input", () => {
    const r = parse("jeden Freitag Wochenbericht schreiben");
    expect(r.title).toBe("Wochenbericht schreiben");
    expect(r.recurrence).toEqual(week(1, ["fr"]));
  });

  it("starts weekday series at the earliest named weekday from today", () => {
    // Wednesday, 14 Oct 2026 (contract example: Monday and Thursday → Thursday).
    const wed = new Date(2026, 9, 14, 8, 0);
    expect(parse("Sport montags und donnerstags", wed).due).toBe("2026-10-15");
    expect(parse("Sport jeden Mittwoch", wed).due).toBe("2026-10-14");
    expect(parse("Sport alle 2 Wochen am Montag", wed).due).toBe("2026-10-19");
  });

  it("clamps a fixed day to the end of shorter months", () => {
    expect(parse("Miete jeden 31.", new Date(2026, 10, 10, 12, 0)).due).toBe("2026-11-30");
    expect(parse("Miete jeden 30.", new Date(2027, 1, 3, 12, 0)).due).toBe("2027-02-28");
    // October has a 31st.
    expect(parse("Miete jeden 31.").due).toBe("2026-10-31");
    // A passed day moves to the next month (clamped there).
    expect(parse("Miete jeden 31.", new Date(2027, 0, 31, 23, 0)).due).toBe("2027-01-31");
    expect(parse("Bericht jeden 30.", new Date(2027, 0, 31, 9, 0)).due).toBe("2027-02-28");
    expect(parse("Feier jeden 29.02.", new Date(2027, 11, 1)).due).toBe("2028-02-29");
    expect(parse("Feier jeden 29.02.", new Date(2027, 0, 1)).due).toBe("2027-02-28");
  });

  it("takes the last day of the month for the month's end", () => {
    expect(parse("Odoo jeden Monatsletzten", new Date(2027, 1, 3)).due).toBe("2027-02-28");
    expect(parse("Odoo jeden Monatsletzten", new Date(2028, 1, 3)).due).toBe("2028-02-29");
    expect(parse("Odoo jeden Monatsletzten", new Date(2026, 9, 31, 8)).due).toBe("2026-10-31");
  });

  it("combines with a time: due and reminder at the first instant not yet passed", () => {
    let r = parse("Wochenbericht jeden Freitag 9:00");
    expect(r.title).toBe("Wochenbericht");
    expect(r.due).toBe("2026-10-16T09:00:00+02:00");
    expect(r.remind_at).toBe(r.due);
    expect(r.recurrence).toEqual(week(1, ["fr"]));
    // Today is Sunday: 16:00 is still ahead, 9:00 has passed → next Sunday.
    expect(parse("Planen jeden Sonntag 16:00").due).toBe("2026-10-11T16:00:00+02:00");
    expect(parse("Planen jeden Sonntag um 9 Uhr").due).toBe("2026-10-18T09:00:00+02:00");
    // Without weekdays or day: today or tomorrow as before.
    expect(parse("Tabletten täglich 16:00").due).toBe("2026-10-11T16:00:00+02:00");
    expect(parse("Tabletten täglich 9:00").due).toBe("2026-10-12T09:00:00+02:00");
    // Today is the 11th, 9:00 passed → 11 Nov (winter time).
    expect(parse("Abrechnung jeden 11. 9:00").due).toBe("2026-11-11T09:00:00+01:00");
    expect(parse("Odoo jeden Monatsletzten 17:00").due).toBe("2026-10-31T17:00:00+01:00");
    r = parse("Odoo 17:00 jeden Monatsletzten #arbeit");
    expect(r).toMatchObject({ title: "Odoo", due: "2026-10-31T17:00:00+01:00", location: { kind: "area", id: "area-arbeit" } });
  });

  it("an explicit date becomes the due date", () => {
    let r = parse("Zahnarzt wöchentlich morgen 9:00 #gesundheit");
    expect(r.title).toBe("Zahnarzt");
    expect(r.due).toBe("2026-10-12T09:00:00+02:00");
    expect(r.remind_at).toBe(r.due);
    expect(r.recurrence).toEqual(week(1));
    expect(r.location).toEqual({ kind: "area", id: "area-gesundheit" });
    expect(r.tokens.map((t) => t.kind)).toEqual(["recurrence", "date", "time", "area"]);

    r = parse("Steuer monatlich am 20.10.");
    expect(r).toMatchObject({ title: "Steuer", due: "2026-10-20", recurrence: { every: 1, unit: "month", from: "due" } });
    r = parse("Team am Dienstag alle 2 Wochen");
    expect(r).toMatchObject({ title: "Team", due: "2026-10-13", recurrence: week(2) });
  });

  it("takes at most one rhythm; a second one stays in the title and is not read as a date", () => {
    const r = parse("Gießen täglich jeden Montag");
    expect(r.title).toBe("Gießen jeden Montag");
    expect(r.recurrence).toEqual({ every: 1, unit: "day", from: "due" });
    expect(r.due).toBe("2026-10-11");
  });

  it("ends a weekday list at the first word that does not continue it", () => {
    let r = parse("Sport jeden Montag und Mittag essen");
    expect(r.title).toBe("Sport und Mittag essen");
    expect(r.recurrence).toEqual(week(1, ["mo"]));
    r = parse("Sport montags und dienstags, mittwochs");
    expect(r.recurrence).toEqual(week(1, ["mo", "tu", "we"]));
    r = parse("Sport jeden Montag und montags");
    expect(r.recurrence).toEqual(week(1, ["mo"]));
    expect(r.title).toBe("Sport und montags");
  });

  it.each([
    "Alle Tage gleich",
    "Ferien alle 0 Tage",
    "Ferien alle 1000 Tage",
    "Alle 2 Kinder abholen",
    "Und jeden Mittag",
    "Wöchentliche Besprechung vorbereiten",
    "Plan für jeden",
    "Miete jeden 32.",
    "Miete jeden 0.",
    "Feier jeden 30.02.",
    "Treffen am Monatsanfang",
    "Liste jedes Mal prüfen",
    "Jeden Montags-Termin absagen",
  ])("leaves text without a rhythm untouched: %s", (input) => {
    const r = parse(input);
    expect(r.title).toBe(input);
    expect(r.recurrence).toBeNull();
    expect(r.due).toBeNull();
    expect(r.tokens).toEqual([]);
  });

  it("a weekday on its own stays a single date", () => {
    expect(parse("Abgabe Freitag")).toMatchObject({ due: "2026-10-16", recurrence: null });
    expect(parse("Abgabe nächsten Montag")).toMatchObject({ due: "2026-10-12", recurrence: null });
  });

  it("falls back to the raw input when only the rhythm was typed", () => {
    const r = parse("täglich");
    expect(r.title).toBe("täglich");
    expect(r.recurrence).toEqual({ every: 1, unit: "day", from: "due" });
  });

  it("with the rhythm dismissed, its words stay in the title and set no date", () => {
    let r = parseNoRhythm("Odoo jeden Monatsletzten #arbeit");
    expect(r).toMatchObject({ title: "Odoo jeden Monatsletzten", due: null, recurrence: null, location: { kind: "area", id: "area-arbeit" } });
    r = parseNoRhythm("Team jeden Montag 9:00");
    expect(r.title).toBe("Team jeden Montag");
    expect(r.recurrence).toBeNull();
    expect(r.due).toBe("2026-10-12T09:00:00+02:00");
    expect(r.tokens.map((t) => t.kind)).toEqual(["time"]);
  });
});

// ZUN-24: phrases that used to give silently wrong results. NOW is Sunday, 11 Oct 2026, 14:30.
describe("quick capture recurrence shorthand: fixed days, ordinals, punctuation", () => {
  const week = (every: number, weekdays?: Recurrence["weekdays"]): Recurrence =>
    weekdays ? { every, unit: "week", weekdays, from: "due" } : { every, unit: "week", from: "due" };
  const month = (every: number, day?: number | "last"): Recurrence =>
    day === undefined ? { every, unit: "month", from: "due" } : { every, unit: "month", day, from: "due" };

  it.each<[string, Recurrence, string, string]>([
    // Monthly with a fixed day.
    ["monatlich am 15.", month(1, 15), "2026-10-15", "Monatlich am 15."],
    ["jeden Monat am 15.", month(1, 15), "2026-10-15", "Monatlich am 15."],
    ["monatlich am 5.", month(1, 5), "2026-11-05", "Monatlich am 5."],
    ["alle 2 Monate am 20.", month(2, 20), "2026-10-20", "Alle 2 Monate am 20."],
    ["monatlich am Monatsletzten", month(1, "last"), "2026-10-31", "Monatlich am Monatsletzten"],
    ["monatlich zum Monatsende", month(1, "last"), "2026-10-31", "Monatlich am Monatsletzten"],
    ["monatlich am Monatsende", month(1, "last"), "2026-10-31", "Monatlich am Monatsletzten"],
    ["jeden Monat zum Monatsletzten", month(1, "last"), "2026-10-31", "Monatlich am Monatsletzten"],
    // Yearly with a fixed date.
    ["jährlich am 15.10.", { every: 1, unit: "year", day: 15, from: "due" }, "2026-10-15", "Jährlich am 15.10."],
    ["jährlich am 11.10.", { every: 1, unit: "year", day: 11, from: "due" }, "2026-10-11", "Jährlich am 11.10."],
    ["jährlich am 10.10.", { every: 1, unit: "year", day: 10, from: "due" }, "2027-10-10", "Jährlich am 10.10."],
    ["jährlich am 29.02.", { every: 1, unit: "year", day: 29, from: "due" }, "2027-02-28", "Jährlich am 29.02."],
    ["jedes Jahr am 1.3.", { every: 1, unit: "year", day: 1, from: "due" }, "2027-03-01", "Jährlich am 01.03."],
    ["alle 2 Jahre am 12.10.", { every: 2, unit: "year", day: 12, from: "due" }, "2026-10-12", "Alle 2 Jahre am 12.10."],
    // "jeden <n>." followed by a unit is every n units, not day n of the month.
    ["jeden 2. Tag", { every: 2, unit: "day", from: "due" }, "2026-10-11", "Alle 2 Tage"],
    ["jede 2. Woche", week(2), "2026-10-11", "Alle 2 Wochen"],
    ["jeden 3. Monat", month(3), "2026-10-11", "Alle 3 Monate"],
    ["jedes 2. Jahr", { every: 2, unit: "year", from: "due" }, "2026-10-11", "Alle 2 Jahre"],
    ["jeden 2. Woche", week(2), "2026-10-11", "Alle 2 Wochen"],
    ["jede 2. Woche montags", week(2, ["mo"]), "2026-10-12", "Alle 2 Wochen (Mo)"],
    ["jeden 3. Monat am 15.", month(3, 15), "2026-10-15", "Alle 3 Monate am 15."],
    ["jedes 2. Jahr am 1.3.", { every: 2, unit: "year", day: 1, from: "due" }, "2027-03-01", "Alle 2 Jahre am 01.03."],
    ["monatlich am 15. nach Erledigung", { every: 1, unit: "month", day: 15, from: "done" }, "2026-10-15", "Monatlich am 15. nach Erledigung"],
    // "des Monats" after a fixed day belongs to the rhythm; "Tag des Monats" is a day, not every n days.
    ["jeden 1. des Monats", month(1, 1), "2026-11-01", "Monatlich am 1."],
    ["jeden 1. Tag des Monats", month(1, 1), "2026-11-01", "Monatlich am 1."],
    ["jeden 15. Tag des Monats.", month(1, 15), "2026-10-15", "Monatlich am 15."],
    ["monatlich am 15. des Monats", month(1, 15), "2026-10-15", "Monatlich am 15."],
    ["alle 2 Monate am 20. Tag des Monats", month(2, 20), "2026-10-20", "Alle 2 Monate am 20."],
  ])("%s → rhythm, first due date and label", (shorthand, recurrence, due, label) => {
    const r = parse(`Gießen ${shorthand}`);
    expect(r.title).toBe("Gießen");
    expect(r.recurrence).toEqual(recurrence);
    expect(r.due).toBe(due);
    expect(r.remind_at).toBeNull();
    expect(r.tokens).toEqual([{ kind: "recurrence", text: shorthand, label }]);
  });

  it("starts a yearly 29.02. in the next leap year when that comes first", () => {
    expect(parse("Feier jährlich am 29.02.", new Date(2027, 11, 1)).due).toBe("2028-02-29");
    expect(parse("Feier jährlich am 29.02.", new Date(2028, 1, 29, 8)).due).toBe("2028-02-29");
  });

  it("combines a fixed monthly day with a time", () => {
    const r = parse("Abrechnung monatlich am 11. 9:00");
    expect(r.title).toBe("Abrechnung");
    expect(r.due).toBe("2026-11-11T09:00:00+01:00");
    expect(r.remind_at).toBe(r.due);
    expect(r.recurrence).toEqual(month(1, 11));
  });

  it("a full date after monatlich stays the due date", () => {
    expect(parse("Steuer monatlich am 20.10.")).toMatchObject({ title: "Steuer", due: "2026-10-20", recurrence: month(1) });
    expect(parse("Fest jährlich am 15.10.2027")).toMatchObject({ title: "Fest", due: "2027-10-15", recurrence: { every: 1, unit: "year", from: "due" } });
  });

  it.each<[string, Recurrence, string]>([
    ["täglich.", { every: 1, unit: "day", from: "due" }, "2026-10-11"],
    ["jeden Freitag.", week(1, ["fr"]), "2026-10-16"],
    ["jeden Montag und Donnerstag.", week(1, ["mo", "th"]), "2026-10-12"],
    ["alle 2 Wochen!", week(2), "2026-10-11"],
    ["werktags,", week(1, ["mo", "tu", "we", "th", "fr"]), "2026-10-12"],
    ["jeden Monatsletzten.", month(1, "last"), "2026-10-31"],
    ["monatlich am 15..", month(1, 15), "2026-10-15"],
    ["jeden 15.!", month(1, 15), "2026-10-15"],
    ["jeden 12.10..", { every: 1, unit: "year", day: 12, from: "due" }, "2026-10-12"],
    ["monatlich nach Erledigung.", { every: 1, unit: "month", from: "done" }, "2026-10-11"],
  ])("trailing punctuation does not hide the rhythm: %s", (shorthand, recurrence, due) => {
    const r = parse(`Gießen ${shorthand}`);
    expect(r.title).toBe("Gießen");
    expect(r.recurrence).toEqual(recurrence);
    expect(r.due).toBe(due);
  });

  it("trailing punctuation does not hide dates and times either", () => {
    expect(parse("Abgabe morgen.")).toMatchObject({ title: "Abgabe", due: "2026-10-12" });
    expect(parse("Abgabe am 20.10.2026.")).toMatchObject({ title: "Abgabe", due: "2026-10-20" });
    expect(parse("Zahnarzt Freitag um 9 Uhr.")).toMatchObject({ title: "Zahnarzt", due: "2026-10-16T09:00:00+02:00" });
    expect(parse("Zahnarzt morgen 9:00.")).toMatchObject({ title: "Zahnarzt", due: "2026-10-12T09:00:00+02:00" });
    expect(parse("Satz endet hier.")).toMatchObject({ title: "Satz endet hier.", due: null });
  });

  it.each([
    "Meeting jeden letzten Freitag im Monat",
    "Stammtisch jeden ersten Montag",
    "Stammtisch jeden ersten und dritten Montag",
    "Stammtisch jeden letzten Freitag.",
    "Treffen jeden 1. Montag",
    "Treffen jeden 2. und 4. Dienstag",
    "Gießen jeden zweiten Tag",
    "Kehrwoche jede zweite Woche",
    "Termin jeden letzten Werktag",
    "Abrechnung monatlich am ersten Montag",
    "Abrechnung jeden Monat am letzten Freitag",
    "Abrechnung monatlich am 1. Montag",
    "Abrechnung monatlich am 1. und 15.",
    "Miete monatlich am 32.",
    "Fest jährlich am 30.02.",
    "Fest jährlich am 15.",
    "Fest jährlich am ersten Sonntag",
    "Fest jeden 12.10. und 15.04.",
    "Jeden letzten Cent sparen",
    // A unit, month name or month number after "<d>." is no fixed day of the month.
    "Lohn jeden 1. Werktag",
    "Lohn jeden 1. Werktag des Monats",
    "Lohn monatlich am 2. Werktag",
    "Lohn jeden Monat am 1. Werktag",
    "Bericht jeden 2. Arbeitstag",
    "Putzen jeden 1. Wochenende",
    "Steuer jeden 1. Quartal",
    "Miete monatlich am 1. Tag",
    "Miete jeden 32. Tag des Monats",
    "Fest jeden 1. Mai",
    "Fest jeden 24. Dezember",
    "Fest jeden 24. Dez.",
    "Fest jeden 1. 5.",
    "Fest monatlich am 1. Mai",
    "Fest jährlich am 1. Mai",
    "Fest jeden 12.10. Montag",
    // An ordinal continuing a weekday list.
    "Treffen jeden Montag und ersten Freitag",
    "Treffen jeden Montag und letzten Freitag",
    "Treffen jeden Montag, 1. Freitag",
    "Treffen wöchentlich am Montag und ersten Freitag",
    "Treffen montags und ersten Freitag",
  ])("leaves unsupported rhythm-like phrases whole in the title, without a due date: %s", (input) => {
    for (const r of [parse(input), parseNoRhythm(input)]) {
      expect(r.title).toBe(input);
      expect(r.recurrence).toBeNull();
      expect(r.due).toBeNull();
      expect(r.tokens).toEqual([]);
    }
  });

  it("other tokens next to an unsupported phrase are still read", () => {
    let r = parse("Meeting jeden letzten Freitag im Monat #arbeit");
    expect(r).toMatchObject({ title: "Meeting jeden letzten Freitag im Monat", due: null, recurrence: null, location: { kind: "area", id: "area-arbeit" } });
    // The time is not part of the phrase: due today at 19:00, no rhythm.
    r = parse("Stammtisch jeden ersten Montag 19:00");
    expect(r).toMatchObject({ title: "Stammtisch jeden ersten Montag", due: "2026-10-11T19:00:00+02:00", recurrence: null });
    // A rhythm before it still counts; the phrase stays in the title.
    r = parse("Gießen täglich jeden ersten Montag");
    expect(r).toMatchObject({ title: "Gießen jeden ersten Montag", due: "2026-10-11", recurrence: { every: 1, unit: "day", from: "due" } });
  });
});
