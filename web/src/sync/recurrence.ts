// Shape of a task's `recurrence` (contract 2.4), mirroring zunder_core::model::Recurrence.
// The messages match the core so the UI shows the same reason as the server.

import { RECURRENCE_FROMS, RECURRENCE_UNITS, WEEKDAY_CODES, type JsonValue, type Recurrence, type WeekdayCode } from "./types";

/** Highest `every` of a recurrence (core: model::MAX_RECURRENCE_EVERY). */
export const MAX_RECURRENCE_EVERY = 999;

const KEYS = ["every", "unit", "weekdays", "day", "from"];

type Result = { ok: Recurrence } | { error: string };

function check(v: JsonValue | undefined): Result {
  if (!v || typeof v !== "object" || Array.isArray(v)) {
    return {
      error:
        '„recurrence“ muss null oder ein Rhythmus {"every": 1–999, "unit": "day"|"week"|"month"|"year", "weekdays"?: […], "day"?: 1–31|"last", "from": "due"|"done"} sein.',
    };
  }
  const unknown = Object.keys(v).find((k) => !KEYS.includes(k));
  if (unknown !== undefined) return { error: `Unbekannter Schlüssel „${unknown}“ im Rhythmus. Erlaubt: every, unit, weekdays, day, from.` };
  const { every, unit, from } = v;
  if (typeof every !== "number" || !Number.isInteger(every) || every < 1 || every > MAX_RECURRENCE_EVERY) {
    return { error: `„every“ im Rhythmus muss eine ganze Zahl von 1 bis ${MAX_RECURRENCE_EVERY} sein.` };
  }
  if (typeof unit !== "string" || !(RECURRENCE_UNITS as readonly string[]).includes(unit)) {
    return { error: "„unit“ im Rhythmus muss day, week, month oder year sein." };
  }
  if (typeof from !== "string" || !(RECURRENCE_FROMS as readonly string[]).includes(from)) {
    return { error: "„from“ im Rhythmus muss due (ab Fälligkeit) oder done (nach Erledigung) sein." };
  }
  const out: Recurrence = { every, unit: unit as Recurrence["unit"], from: from as Recurrence["from"] };
  if ("weekdays" in v) {
    const w = v.weekdays;
    const ok =
      Array.isArray(w) &&
      w.length > 0 &&
      w.every((x) => typeof x === "string" && (WEEKDAY_CODES as readonly string[]).includes(x)) &&
      new Set(w).size === w.length;
    if (!ok) {
      return { error: "„weekdays“ im Rhythmus muss eine nicht leere Liste verschiedener Wochentage aus mo, tu, we, th, fr, sa, su sein." };
    }
    out.weekdays = w as WeekdayCode[];
  }
  if ("day" in v) {
    const d = v.day;
    if (!(d === "last" || (typeof d === "number" && Number.isInteger(d) && d >= 1 && d <= 31))) {
      return { error: '„day“ im Rhythmus muss eine ganze Zahl von 1 bis 31 oder "last" sein.' };
    }
    out.day = d;
  }
  if (out.weekdays && out.unit !== "week") return { error: "Wochentage („weekdays“) gehen nur bei wöchentlichen Rhythmen." };
  if (out.day !== undefined && out.unit !== "month") {
    return { error: "Ein fester Tag („day“) geht nur bei monatlichen Rhythmen (jährliche folgen dem Tag der Fälligkeit)." };
  }
  return { ok: out };
}

/** German reason why `v` is not a valid recurrence, or null when it is one. */
export function recurrenceProblem(v: JsonValue | undefined): string | null {
  const r = check(v);
  return "error" in r ? r.error : null;
}

/** Strictly parses a stored recurrence; null for none or a malformed value. */
export function parseRecurrence(v: JsonValue | undefined): Recurrence | null {
  const r = check(v);
  return "ok" in r ? r.ok : null;
}

// ---- Editing (detail panel) ------------------------------------------------------
// Each helper returns a fresh recurrence with only the keys that apply: an optional
// key is present with a value or absent (contract 2.4: `null` or `undefined` would be invalid).

/** The recurrence as the JSON value written in one operation. */
export function recurrenceValue(r: Recurrence): JsonValue {
  const out: { [key: string]: JsonValue } = { every: r.every, unit: r.unit };
  if (r.weekdays && r.weekdays.length > 0) out.weekdays = [...r.weekdays];
  if (r.day !== undefined) out.day = r.day;
  out.from = r.from;
  return out;
}

function build(every: number, unit: Recurrence["unit"], from: Recurrence["from"], weekdays?: WeekdayCode[], day?: number | "last"): Recurrence {
  const out: Recurrence = { every, unit, from };
  if (unit === "week" && weekdays && weekdays.length > 0) out.weekdays = weekdays;
  if (unit === "month" && day !== undefined) out.day = day;
  return out;
}

/** Another unit; weekdays (weeks only) and day (months only) that no longer fit are dropped. */
export function withUnit(r: Recurrence, unit: Recurrence["unit"]): Recurrence {
  return build(r.every, unit, r.from, r.weekdays, r.day);
}

export function withEvery(r: Recurrence, every: number): Recurrence {
  return build(every, r.unit, r.from, r.weekdays, r.day);
}

export function withFrom(r: Recurrence, from: Recurrence["from"]): Recurrence {
  return build(r.every, r.unit, from, r.weekdays, r.day);
}

/** Fixed day of the month (`undefined`: the day of the due date or completion). */
export function withDay(r: Recurrence, day: number | "last" | undefined): Recurrence {
  return build(r.every, r.unit, r.from, r.weekdays, day);
}

/** Switches a weekday on or off, keeping Monday-to-Sunday order; none left = no `weekdays`. */
export function toggleWeekday(r: Recurrence, day: WeekdayCode): Recurrence {
  const current = new Set(r.weekdays ?? []);
  if (current.has(day)) current.delete(day);
  else current.add(day);
  return build(r.every, r.unit, r.from, WEEKDAY_CODES.filter((d) => current.has(d)));
}

/** Same rhythm (weekdays compared as a set). */
export function sameRecurrence(a: Recurrence, b: Recurrence): boolean {
  const days = (r: Recurrence) => WEEKDAY_CODES.filter((d) => r.weekdays?.includes(d)).join(",");
  return a.every === b.every && a.unit === b.unit && a.from === b.from && a.day === b.day && days(a) === days(b);
}

/** Weekday of a local date "YYYY-MM-DD" (the date part of a due value is enough). */
export function weekdayOf(date: string): WeekdayCode {
  const [y, m, d] = date.slice(0, 10).split("-").map(Number);
  // getDay: 0 = Sunday.
  return WEEKDAY_CODES[(new Date(y!, m! - 1, d!).getDay() + 6) % 7]!;
}
