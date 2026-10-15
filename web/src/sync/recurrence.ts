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
