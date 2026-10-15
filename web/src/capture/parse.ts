// Quick capture shorthand (contract section 5), e.g.
//   "Zahnarzt anrufen morgen 9:00 #gesundheit"
// Dates: heute, morgen, übermorgen, weekday names, "nächsten <Wochentag>",
// 12.10., 12.10.2026 (optionally preceded by "am"). Times: 9:00, 9 Uhr, 9:30 Uhr
// (optionally preceded by "um"). A time sets due and remind_at; a date alone
// sets a date-only due. #slug resolves to an area or project (project wins on a tie).
// Unresolved tags stay in the title. Rhythms ("jeden Freitag", "alle 2 Wochen",
// "jeden Monatsletzten" …, contract 5.1) set `recurrence` and, without a date, the
// first due date of the series.

import { describeRecurrence } from "../labels";
import { addDays, isValidCalendarDate, localDate, startOfDay, toRfc3339Local } from "../sync/dates";
import { WEEKDAY_CODES, type Location, type Recurrence, type WeekdayCode } from "../sync/types";

export interface CaptureTarget {
  id: string;
  name: string;
}

export interface CaptureStructure {
  areas: readonly CaptureTarget[];
  projects: readonly CaptureTarget[];
}

export type CaptureTokenKind = "date" | "time" | "area" | "project" | "recurrence";

export interface CaptureToken {
  kind: CaptureTokenKind;
  text: string;
  /** German label for previews ("Morgen", "09:00", area/project name, "Monatlich am Monatsletzten"). */
  label: string;
}

export interface CaptureResult {
  title: string;
  /** "YYYY-MM-DD" or RFC 3339 with offset. */
  due: string | null;
  remind_at: string | null;
  /** null = inbox. */
  location: Location | null;
  /** Rhythm of the new task; when set, `due` is set too. */
  recurrence: Recurrence | null;
  tokens: CaptureToken[];
  unknownTags: string[];
}

export interface CaptureOptions {
  /**
   * false: rhythms are not taken (the preview's × dismissed it); their words stay in
   * the title and are not read as a date either. Default true.
   */
  recurrence?: boolean;
}

// A Map, not an object literal: `in` would also match inherited keys such as "constructor".
const WEEKDAYS = new Map<string, number>([
  ["sonntag", 0],
  ["montag", 1],
  ["dienstag", 2],
  ["mittwoch", 3],
  ["donnerstag", 4],
  ["freitag", 5],
  ["samstag", 6],
  ["sonnabend", 6],
]);

const NEXT_WORDS = new Set(["nächsten", "nächster", "nächste", "naechsten", "naechster", "naechste", "kommenden", "kommender", "kommende"]);
const WEEKDAY_LABELS = ["Sonntag", "Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag"];

/** Lowercase ASCII slug: umlauts transliterated, other characters become "-". */
export function slugify(value: string): string {
  return value
    .toLowerCase()
    .replace(/ä/g, "ae")
    .replace(/ö/g, "oe")
    .replace(/ü/g, "ue")
    .replace(/ß/g, "ss")
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function stripPunctuation(word: string): string {
  return word.replace(/[,;!?]+$/, "");
}

/** Word `k` in lower case without trailing punctuation; "" past the end. */
function wordAt(words: readonly string[], k: number): string {
  return k < words.length ? stripPunctuation(words[k]!.toLowerCase()) : "";
}

interface DateHit {
  date: Date;
  consumed: number;
  label: string;
}

function matchDate(words: readonly string[], i: number, today: Date): DateHit | null {
  const word = stripPunctuation(words[i]!.toLowerCase());
  if (word === "heute") return { date: today, consumed: 1, label: "Heute" };
  if (word === "morgen") return { date: addDays(today, 1), consumed: 1, label: "Morgen" };
  if (word === "übermorgen" || word === "uebermorgen") return { date: addDays(today, 2), consumed: 1, label: "Übermorgen" };

  const weekday = WEEKDAYS.get(word);
  if (weekday !== undefined) {
    const target = weekday;
    const delta = ((target - today.getDay() + 7) % 7) || 7;
    return { date: addDays(today, delta), consumed: 1, label: WEEKDAY_LABELS[target]! };
  }

  if (NEXT_WORDS.has(word) && i + 1 < words.length) {
    const next = stripPunctuation(words[i + 1]!.toLowerCase());
    const target = WEEKDAYS.get(next);
    if (target !== undefined) {
      // The weekday in the following (Monday-based) calendar week.
      const mondayThisWeek = addDays(today, -((today.getDay() + 6) % 7));
      const offset = (target + 6) % 7;
      return { date: addDays(mondayThisWeek, 7 + offset), consumed: 2, label: `Nächsten ${WEEKDAY_LABELS[target]}` };
    }
  }

  const m = /^(\d{1,2})\.(\d{1,2})\.(\d{4})?$/.exec(stripPunctuation(words[i]!));
  if (m) {
    const day = Number(m[1]);
    const month = Number(m[2]);
    let year = m[3] ? Number(m[3]) : today.getFullYear();
    if (!isValidCalendarDate(year, month, day)) return null;
    let date = new Date(year, month - 1, day);
    if (!m[3] && date < today) {
      year += 1;
      if (!isValidCalendarDate(year, month, day)) return null;
      date = new Date(year, month - 1, day);
    }
    return { date, consumed: 1, label: `${String(day).padStart(2, "0")}.${String(month).padStart(2, "0")}.${m[3] ? year : ""}` };
  }
  return null;
}

interface TimeHit {
  hours: number;
  minutes: number;
  consumed: number;
}

function matchTime(words: readonly string[], i: number): TimeHit | null {
  const word = stripPunctuation(words[i]!.toLowerCase());
  const nextIsUhr = i + 1 < words.length && stripPunctuation(words[i + 1]!.toLowerCase()) === "uhr";

  let m = /^(\d{1,2}):(\d{2})(uhr)?$/.exec(word);
  if (m) {
    const hours = Number(m[1]);
    const minutes = Number(m[2]);
    if (hours > 23 || minutes > 59) return null;
    return { hours, minutes, consumed: !m[3] && nextIsUhr ? 2 : 1 };
  }
  m = /^(\d{1,2})uhr$/.exec(word);
  if (m) {
    const hours = Number(m[1]);
    return hours > 23 ? null : { hours, minutes: 0, consumed: 1 };
  }
  m = /^(\d{1,2})$/.exec(word);
  if (m && nextIsUhr) {
    const hours = Number(m[1]);
    return hours > 23 ? null : { hours, minutes: 0, consumed: 2 };
  }
  return null;
}

// ---- Rhythms (contract 5.1, "Kurzschreibweise für Rhythmen") ----------------------

/** Weekday adverbs ("montags"), getDay numbering like WEEKDAYS. */
const WEEKDAY_ADVERBS = new Map<string, number>([...WEEKDAYS].map(([name, day]) => [`${name}s`, day]));

/** getDay (0 = Sunday) → wire code. */
function weekdayCode(day: number): WeekdayCode {
  return WEEKDAY_CODES[(day + 6) % 7]!;
}

function daysInMonth(year: number, monthIndex: number): number {
  return new Date(year, monthIndex + 1, 0).getDate();
}

/** Day `day` of the month (shorter months: their last day). */
function clampedDate(year: number, monthIndex: number, day: number): Date {
  return new Date(year, monthIndex, Math.min(day, daysInMonth(year, monthIndex)));
}

interface RecurrenceHit {
  recurrence: Recurrence;
  consumed: number;
  /** First day of the series on or after `day` (start of day). */
  first: (day: Date) => Date;
}

const sameDay = (day: Date) => day;

const UNIT_PLURALS = new Map<string, Recurrence["unit"]>([
  ["tage", "day"],
  ["wochen", "week"],
  ["monate", "month"],
  ["jahre", "year"],
]);

/** A list of weekdays joined by commas or "und", starting at `start`. */
function weekdayList(words: readonly string[], start: number, names: ReadonlyMap<string, number>): { days: number[]; end: number } | null {
  const word = (k: number) => wordAt(words, k);
  const first = names.get(word(start));
  if (first === undefined) return null;
  const days = [first];
  let end = start + 1;
  for (;;) {
    const afterComma = /,$/.test(words[end - 1]!) ? names.get(word(end)) : undefined;
    if (afterComma !== undefined) {
      days.push(afterComma);
      end += 1;
      continue;
    }
    const afterAnd = word(end) === "und" ? names.get(word(end + 1)) : undefined;
    if (afterAnd !== undefined) {
      days.push(afterAnd);
      end += 2;
      continue;
    }
    return { days, end };
  }
}

function weekly(every: number, days: readonly number[] | null, consumed: number): RecurrenceHit {
  if (!days) return { recurrence: { every, unit: "week", from: "due" }, consumed, first: sameDay };
  const set = new Set(days);
  return {
    // Monday to Sunday, each once (contract 2.4: distinct weekdays).
    recurrence: { every, unit: "week", weekdays: WEEKDAY_CODES.filter((c) => days.some((d) => weekdayCode(d) === c)), from: "due" },
    consumed,
    first: (day) => {
      let offset = 0;
      while (!set.has((day.getDay() + offset) % 7)) offset++;
      return addDays(day, offset);
    },
  };
}

function monthly(every: number, day: number | "last" | undefined, consumed: number): RecurrenceHit {
  if (day === undefined) return { recurrence: { every, unit: "month", from: "due" }, consumed, first: sameDay };
  return {
    recurrence: { every, unit: "month", day, from: "due" },
    consumed,
    first: (from) => {
      const y = from.getFullYear();
      const m = from.getMonth();
      if (day === "last") return new Date(y, m + 1, 0);
      const date = clampedDate(y, m, day);
      return date < from ? clampedDate(y, m + 1, day) : date;
    },
  };
}

function simple(every: number, unit: "day" | "year", consumed: number): RecurrenceHit {
  return { recurrence: { every, unit, from: "due" }, consumed, first: sameDay };
}

/** "jeden 12.10.": yearly on that day (29.02. falls on 28.02. in common years and comes back in leap years). */
function yearlyOn(day: number, month: number, consumed: number): RecurrenceHit {
  return {
    recurrence: { every: 1, unit: "year", day, from: "due" },
    consumed,
    first: (from) => {
      const date = clampedDate(from.getFullYear(), month - 1, day);
      return date < from ? clampedDate(from.getFullYear() + 1, month - 1, day) : date;
    },
  };
}

/**
 * A rhythm starting at word `i` (the whole phrase, including a weekday list and
 * "nach/ab Erledigung"); `consumed` counts from `i`.
 */
function matchRecurrence(words: readonly string[], i: number): RecurrenceHit | null {
  const base = matchRhythmBase(words, i);
  if (!base) return null;
  const word = (k: number) => wordAt(words, k);
  const end = i + base.consumed;
  if ((word(end) === "nach" || word(end) === "ab") && word(end + 1) === "erledigung") {
    return { ...base, recurrence: { ...base.recurrence, from: "done" }, consumed: base.consumed + 2 };
  }
  return base;
}

function matchRhythmBase(words: readonly string[], i: number): RecurrenceHit | null {
  const word = (k: number) => wordAt(words, k);
  const w = word(i);
  const next = word(i + 1);
  // Relative end positions below are counted from `i`.
  const rel = (end: number) => end - i;

  /** "wöchentlich"/"jede Woche"/"alle n Wochen" optionally followed by "montags …" or "am Montag …". */
  const weekWithDays = (every: number, end: number): RecurrenceHit => {
    const adverbs = weekdayList(words, end, WEEKDAY_ADVERBS);
    if (adverbs) return weekly(every, adverbs.days, rel(adverbs.end));
    const named = word(end) === "am" ? weekdayList(words, end + 1, WEEKDAYS) : null;
    if (named) return weekly(every, named.days, rel(named.end));
    return weekly(every, null, rel(end));
  };

  switch (w) {
    case "täglich":
    case "taeglich":
      return simple(1, "day", 1);
    case "wöchentlich":
    case "woechentlich":
      return weekWithDays(1, i + 1);
    case "monatlich":
      return monthly(1, undefined, 1);
    case "jährlich":
    case "jaehrlich":
      return simple(1, "year", 1);
    case "werktags":
      return weekly(1, [1, 2, 3, 4, 5], 1);
    case "jede":
      return next === "woche" ? weekWithDays(1, i + 2) : null;
    case "jedes":
      if (next === "jahr") return simple(1, "year", 2);
      if (next === "monatsende") return monthly(1, "last", 2);
      return null;
    case "am":
      return next === "monatsende" ? monthly(1, "last", 2) : null;
    case "alle": {
      const n = /^[1-9]\d{0,2}$/.test(next) ? Number(next) : null;
      const unit = UNIT_PLURALS.get(word(i + 2));
      if (n === null || !unit) return null;
      if (unit === "week") return weekWithDays(n, i + 3);
      if (unit === "month") return monthly(n, undefined, 3);
      return simple(n, unit, 3);
    }
    case "jeden": {
      if (next === "tag") return simple(1, "day", 2);
      if (next === "monat") return monthly(1, undefined, 2);
      if (next === "monatsletzten") return monthly(1, "last", 2);
      if (next === "letzten") {
        if (word(i + 2) === "des" && word(i + 3) === "monats") return monthly(1, "last", 4);
        if (word(i + 2) === "tag" && word(i + 3) === "des" && word(i + 4) === "monats") return monthly(1, "last", 5);
        return null;
      }
      const named = weekdayList(words, i + 1, WEEKDAYS);
      if (named) return weekly(1, named.days, rel(named.end));
      // "jeden 12.10." (yearly) is checked before "jeden 15." (monthly).
      const dm = /^(\d{1,2})\.(\d{1,2})\.$/.exec(next);
      if (dm) {
        const day = Number(dm[1]);
        const month = Number(dm[2]);
        // 2000 is a leap year: 29.02. is a valid day of February.
        return isValidCalendarDate(2000, month, day) ? yearlyOn(day, month, 2) : null;
      }
      const d = /^(\d{1,2})\.$/.exec(next);
      if (d) {
        const day = Number(d[1]);
        return day >= 1 && day <= 31 ? monthly(1, day, 2) : null;
      }
      return null;
    }
    default: {
      const adverbs = weekdayList(words, i, WEEKDAY_ADVERBS);
      return adverbs ? weekly(1, adverbs.days, rel(adverbs.end)) : null;
    }
  }
}

function resolveTag(slug: string, structure: CaptureStructure): { kind: "area" | "project"; target: CaptureTarget } | null {
  const project = structure.projects.find((p) => slugify(p.name) === slug);
  if (project) return { kind: "project", target: project };
  const area = structure.areas.find((a) => slugify(a.name) === slug);
  if (area) return { kind: "area", target: area };
  return null;
}

export function parseCapture(input: string, now: Date, structure: CaptureStructure, options: CaptureOptions = {}): CaptureResult {
  const words = input.trim().split(/\s+/).filter(Boolean);
  const keep: boolean[] = words.map(() => true);
  const tokens: CaptureToken[] = [];
  const unknownTags: string[] = [];
  const today = startOfDay(now);

  let date: Date | null = null;
  let time: { hours: number; minutes: number } | null = null;
  let project: CaptureTarget | null = null;
  let area: CaptureTarget | null = null;
  let rhythm: RecurrenceHit | null = null;

  for (let i = 0; i < words.length; i++) {
    if (!keep[i]) continue;
    const word = words[i]!;
    const lower = word.toLowerCase();

    if (word.startsWith("#") && word.length > 1) {
      const slug = slugify(word.slice(1));
      const hit = slug ? resolveTag(slug, structure) : null;
      if (hit) {
        keep[i] = false;
        tokens.push({ kind: hit.kind, text: word, label: hit.target.name });
        if (hit.kind === "project") project ??= hit.target;
        else area ??= hit.target;
      } else {
        unknownTags.push(word);
      }
      continue;
    }

    // Rhythms come before dates so that "jeden Montag" is not a single Monday. Only the
    // first one counts; a second (or dismissed) one stays in the title, its words are
    // not read as a date either.
    const rec = matchRecurrence(words, i);
    if (rec) {
      if (!rhythm && options.recurrence !== false) {
        rhythm = rec;
        for (let k = i; k < i + rec.consumed; k++) keep[k] = false;
        tokens.push({ kind: "recurrence", text: words.slice(i, i + rec.consumed).join(" "), label: describeRecurrence(rec.recurrence) });
      }
      i += rec.consumed - 1;
      continue;
    }

    // "am <date>" and "um <time>" swallow their preposition.
    const prep = lower === "am" || lower === "um" ? lower : null;
    const at = prep && i + 1 < words.length ? i + 1 : i;

    if (!date && prep !== "um") {
      const hit = matchDate(words, at, today);
      if (hit) {
        date = hit.date;
        for (let k = i; k < at + hit.consumed; k++) keep[k] = false;
        tokens.push({ kind: "date", text: words.slice(i, at + hit.consumed).join(" "), label: hit.label });
        i = at + hit.consumed - 1;
        continue;
      }
    }
    if (!time && prep !== "am") {
      const hit = matchTime(words, at);
      if (hit) {
        time = { hours: hit.hours, minutes: hit.minutes };
        for (let k = i; k < at + hit.consumed; k++) keep[k] = false;
        const label = `${String(hit.hours).padStart(2, "0")}:${String(hit.minutes).padStart(2, "0")}`;
        tokens.push({ kind: "time", text: words.slice(i, at + hit.consumed).join(" "), label });
        i = at + hit.consumed - 1;
        continue;
      }
    }
  }

  // Without a date, a rhythm's series starts at its first day from today on.
  const first = rhythm?.first ?? sameDay;
  let due: string | null = null;
  let remindAt: string | null = null;
  if (time) {
    let day = date ?? first(today);
    let at = new Date(day.getFullYear(), day.getMonth(), day.getDate(), time.hours, time.minutes);
    if (!date && at.getTime() <= now.getTime()) {
      // A bare time that already passed today means the next day (of the series).
      day = first(addDays(today, 1));
      at = new Date(day.getFullYear(), day.getMonth(), day.getDate(), time.hours, time.minutes);
    }
    due = toRfc3339Local(at);
    remindAt = due;
  } else if (date) {
    due = localDate(date);
  } else if (rhythm) {
    due = localDate(first(today));
  }

  // A yearly day names the month of the due date ("Jährlich am 28.02.").
  if (rhythm) for (const t of tokens) if (t.kind === "recurrence") t.label = describeRecurrence(rhythm.recurrence, due);

  const location: Location | null = project ? { kind: "project", id: project.id } : area ? { kind: "area", id: area.id } : null;
  const title = words.filter((_, i) => keep[i]).join(" ") || input.trim();
  return { title, due, remind_at: remindAt, location, recurrence: rhythm?.recurrence ?? null, tokens, unknownTags };
}
