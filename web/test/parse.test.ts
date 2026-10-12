import { describe, expect, it } from "vitest";
import { parseCapture, slugify } from "../src/capture/parse";

// Sunday, 11 Oct 2026, 14:30 in Europe/Berlin (CEST, +02:00); TZ is set in vite.config.ts.
const NOW = new Date(2026, 9, 11, 14, 30);
const structure = {
  areas: [
    { id: "area-gesundheit", name: "Gesundheit" },
    { id: "area-buero", name: "Büro" },
    { id: "area-zunder", name: "Zunder" },
  ],
  projects: [
    { id: "proj-zunder", name: "Zunder" },
    { id: "proj-umzug", name: "Umzug 2027" },
  ],
};
const parse = (input: string, now = NOW) => parseCapture(input, now, structure);

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
