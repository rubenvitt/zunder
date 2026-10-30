// Shape check of a task's `recurrence` for the mock server (contract 2.5, "Rhythmen"), a port
// of zunder_core::model::Recurrence::parse with the core's messages. The mock cannot import
// web/src (extensionless imports), so web/test/mock-recurrence.test.ts keeps it in step with
// the client's recurrenceProblem and the core's examples.

const KEYS = ["every", "unit", "weekdays", "day", "from"];
const WEEKDAYS = ["mo", "tu", "we", "th", "fr", "sa", "su"];
const MAX_EVERY = 999;

/** Why `v` is not a valid `recurrence`, or null when it is one (null itself is handled before). */
export function recurrenceProblem(v: unknown): string | null {
  if (!v || typeof v !== "object" || Array.isArray(v)) {
    return '„recurrence“ muss null oder ein Rhythmus {"every": 1–999, "unit": "day"|"week"|"month"|"year", "weekdays"?: […], "day"?: 1–31|"last"|"last_workday", "from": "due"|"done"} sein.';
  }
  const o = v as Record<string, unknown>;
  const unknown = Object.keys(o).find((k) => !KEYS.includes(k));
  if (unknown !== undefined) return `Unbekannter Schlüssel „${unknown}“ im Rhythmus. Erlaubt: every, unit, weekdays, day, from.`;
  if (typeof o.every !== "number" || !Number.isInteger(o.every) || o.every < 1 || o.every > MAX_EVERY) {
    return `„every“ im Rhythmus muss eine ganze Zahl von 1 bis ${MAX_EVERY} sein.`;
  }
  if (typeof o.unit !== "string" || !["day", "week", "month", "year"].includes(o.unit)) {
    return "„unit“ im Rhythmus muss day, week, month oder year sein.";
  }
  if (typeof o.from !== "string" || !["due", "done"].includes(o.from)) {
    return "„from“ im Rhythmus muss due (ab Fälligkeit) oder done (nach Erledigung) sein.";
  }
  const w = o.weekdays;
  if ("weekdays" in o) {
    const ok = Array.isArray(w) && w.length > 0 && w.every((x) => typeof x === "string" && WEEKDAYS.includes(x)) && new Set(w).size === w.length;
    if (!ok) return "„weekdays“ im Rhythmus muss eine nicht leere Liste verschiedener Wochentage aus mo, tu, we, th, fr, sa, su sein.";
  }
  const d = o.day;
  if ("day" in o && !(d === "last" || d === "last_workday" || (typeof d === "number" && Number.isInteger(d) && d >= 1 && d <= 31))) {
    return '„day“ im Rhythmus muss eine ganze Zahl von 1 bis 31, "last" oder "last_workday" sein.';
  }
  if ("weekdays" in o && o.unit !== "week") return "Wochentage („weekdays“) gehen nur bei wöchentlichen Rhythmen.";
  if ("day" in o && o.unit !== "month" && o.unit !== "year") {
    return "Ein fester Tag („day“) geht nur bei monatlichen und jährlichen Rhythmen (bei jährlichen im Monat der Fälligkeit).";
  }
  return null;
}
