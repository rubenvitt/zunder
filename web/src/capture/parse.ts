// Quick capture shorthand (contract section 5), e.g.
//   "Zahnarzt anrufen morgen 9:00 #gesundheit"
// Dates: heute, morgen, übermorgen, weekday names, "nächsten <Wochentag>",
// 12.10., 12.10.2026 (optionally preceded by "am"), "am Monatsende" (once). Times: 9:00, 9 Uhr, 9:30 Uhr
// (optionally preceded by "um"). A time sets due and remind_at; a date alone
// sets a date-only due. #slug resolves to an area or project (project wins on a tie).
// Unresolved tags stay in the title. Rhythms ("jeden Freitag", "alle 2 Wochen",
// "alle zwei Wochen", "jede zweite Woche", "monatlich am 15.", "jeden Monatsletzten",
// "jeden letzten Werktag des Monats" …,
// contract 5.1) set `recurrence` and, without a date, the first due date of the series.
// Unsupported rhythm-like phrases ("jeden ersten Montag", "jeden 1. Werktag", "jeden 1.
// Mai", "alle paar Tage") stay in the title whole, with what follows their unit
// ("jede 2. Woche am ersten Montag"), and set nothing. "!brand", "!glut", "!rauch", "!asche" set the weight, "@mich", "@uns",
// "@andere" whom the task is for (the first of each counts; others stay in the title).
// "wartet:Anna" sets whom the task waits on ("_" stands for a space: "wartet:Team_Einkauf";
// the first counts, the task is then created blocked).

import { describeRecurrence, FOR_WHOM_LABELS, waitingLabel, WEIGHT_LABELS } from "../labels";
import { addDays, isValidCalendarDate, localDate, startOfDay, toRfc3339Local } from "../sync/dates";
import { TITLE_MAX } from "../sync/validate";
import { FOR_WHOMS, WEEKDAY_CODES, WEIGHTS, type ForWhom, type Location, type MonthDay, type Recurrence, type WeekdayCode, type Weight } from "../sync/types";

export interface CaptureTarget {
  id: string;
  name: string;
}

export interface CaptureStructure {
  areas: readonly CaptureTarget[];
  projects: readonly CaptureTarget[];
}

export type CaptureTokenKind = "date" | "time" | "area" | "project" | "recurrence" | "weight" | "for_whom" | "waiting_on";

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
  weight: Weight | null;
  for_whom: ForWhom | null;
  /** Whom the task waits on ("wartet:Anna"); when set, the task is created blocked. */
  waiting_on: string | null;
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

/**
 * Trailing punctuation ("morgen.", "täglich!", "Freitag,") removed. A dot that ends a
 * day ("15.") or day and month ("12.10.") belongs to the word and stays.
 */
function stripPunctuation(word: string): string {
  const trimmed = word.replace(/[,;!?.]+$/, "");
  return word.charAt(trimmed.length) === "." && /^\d{1,3}(\.\d{1,2})?$/.test(trimmed) ? `${trimmed}.` : trimmed;
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

  // "am Monatsende" is a single date, the last day of this month (today at the latest),
  // not a rhythm (contract 5.1). Only after "am", so a "Monatsende" in a title stays.
  if (word === "monatsende" && i > 0 && words[i - 1]!.toLowerCase() === "am") {
    return { date: new Date(today.getFullYear(), today.getMonth() + 1, 0), consumed: 1, label: "Monatsende" };
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
  /** Month (1–12) of a fixed yearly day ("jeden 12.10."). */
  month?: number;
}

/**
 * A rhythm-like phrase the shorthand does not support ("jeden ersten Montag"): it stays
 * in the title as a whole, and none of its words is read as a date or time.
 */
interface UnsupportedHit {
  recurrence: null;
  consumed: number;
}

type RhythmMatch = RecurrenceHit | UnsupportedHit;

const unsupported = (consumed: number): UnsupportedHit => ({ recurrence: null, consumed });

const sameDay = (day: Date) => day;

const UNIT_PLURALS = new Map<string, Recurrence["unit"]>([
  ["tage", "day"],
  ["wochen", "week"],
  ["monate", "month"],
  ["jahre", "year"],
]);

/** "jeden 2. Tag", "jede 2. Woche", "jeden 3. Monat", "jedes 2. Jahr". */
const UNIT_SINGULARS = new Map<string, Recurrence["unit"]>([
  ["tag", "day"],
  ["woche", "week"],
  ["monat", "month"],
  ["jahr", "year"],
]);

const ORDINAL_ENDINGS = ["e", "en", "er", "es"];

/** Ordinals in words with their value, in every inflection ("ersten" → 1, "zweite" → 2 … "zwölftes" → 12). */
const ORDINAL_NUMBERS = new Map<string, number>(
  (
    [
      ["erst", 1],
      ["zweit", 2],
      ["dritt", 3],
      ["viert", 4],
      ["fünft", 5],
      ["fuenft", 5],
      ["sechst", 6],
      ["siebt", 7],
      ["siebent", 7],
      ["acht", 8],
      ["neunt", 9],
      ["zehnt", 10],
      ["elft", 11],
      ["zwölft", 12],
      ["zwoelft", 12],
    ] as const
  ).flatMap(([stem, n]) => ORDINAL_ENDINGS.map((end): [string, number] => [stem + end, n])),
);

/** "letzte", "letzten" …: the last day of the month before "des Monats"/"im Monat". */
const LAST_WORDS = new Set(ORDINAL_ENDINGS.map((end) => `letzt${end}`));

/**
 * Ordinals in words ("ersten", "zweite", "letzte" …). In a list, before a weekday or
 * after "am" they mark a phrase as unsupported; only "jede zweite Woche" (every n units)
 * and "jeden ersten Tag des Monats" (a fixed day) are rhythms.
 */
const ORDINAL_WORDS = new Set([...ORDINAL_NUMBERS.keys(), ...LAST_WORDS, ...ORDINAL_ENDINGS.map((end) => `vorletzt${end}`)]);

/** Units that may follow an ordinal in an unsupported phrase ("jeden letzten Werktag"). */
const ORDINAL_UNITS = new Set(["tag", "werktag", "arbeitstag", "wochentag", "woche", "wochenende", "monat", "quartal", "jahr"]);

const NUMERIC_ORDINAL = /^[1-9]\d{0,2}\.$/;
const DAY_MONTH = /^(\d{1,2})\.(\d{1,2})\.$/;

/** Ordinals ("ersten", "15.") or days of the year ("12.10.") joined by commas or "und", starting at `start`. */
function ordinalList(words: readonly string[], start: number): { count: number; end: number } | null {
  const isItem = (k: number) => {
    const w = wordAt(words, k);
    return ORDINAL_WORDS.has(w) || NUMERIC_ORDINAL.test(w) || DAY_MONTH.test(w);
  };
  if (!isItem(start)) return null;
  let count = 1;
  let end = start + 1;
  for (;;) {
    if (/,$/.test(words[end - 1]!) && isItem(end)) {
      count += 1;
      end += 1;
    } else if (wordAt(words, end) === "und" && isItem(end + 1)) {
      count += 1;
      end += 2;
    } else {
      return { count, end };
    }
  }
}

/** Month names and their abbreviations ("Mai", "Dez."): after "1." they make a day of the year, not of the month. */
const MONTH_NAMES = new Set([
  ...["januar", "jänner", "jaenner", "februar", "märz", "maerz", "april", "mai", "juni", "juli", "august", "september", "oktober", "november", "dezember"],
  ...["jan", "feb", "mär", "maer", "mrz", "apr", "jun", "jul", "aug", "sep", "sept", "okt", "nov", "dez"],
]);

/** A month number after a day ("jeden 1. 5."). */
const MONTH_NUMBER = /^(0?[1-9]|1[0-2])\.$/;

/** Cardinal numbers in words after "alle" ("alle zwei Wochen"). */
const CARDINAL_NUMBERS = new Map<string, number>([
  ["zwei", 2],
  ["drei", 3],
  ["vier", 4],
  ["fünf", 5],
  ["fuenf", 5],
  ["sechs", 6],
  ["sieben", 7],
  ["acht", 8],
  ["neun", 9],
  ["zehn", 10],
  ["elf", 11],
  ["zwölf", 12],
  ["zwoelf", 12],
]);

/** A full date ("12.10.2027"). */
const FULL_DATE = /^\d{1,2}\.\d{1,2}\.\d{4}$/;

/**
 * End of an unsupported phrase whose ordinals end at `end`: a following weekday list,
 * unit, month name or month number belongs to it, after a unit also what a supported
 * rhythm takes there (`unsupportedTailEnd`).
 */
function unsupportedEnd(words: readonly string[], end: number): number {
  const days = weekdayList(words, end, WEEKDAYS);
  if (days) return weekdaysEnd(words, days.end);
  const w = wordAt(words, end);
  if (ORDINAL_UNITS.has(w)) return unsupportedTailEnd(words, end + 1);
  return MONTH_NAMES.has(w) || MONTH_NUMBER.test(w) ? end + 1 : end;
}

/** End of a weekday list ending at `end`, including ordinals continuing it ("montags und ersten Freitag"). */
function weekdaysEnd(words: readonly string[], end: number): number {
  const ordinal = ordinalAfterList(words, end);
  return ordinal === null ? end : unsupportedEnd(words, ordinalList(words, ordinal)!.end);
}

/**
 * End of what follows the unit of an unsupported phrase at word `k`, as after a supported
 * one: weekdays ("montags …", "am Montag …"), a day ("am 15.", "am 12.10.", "am
 * 12.10.2027", "am ersten Montag", "am Monatsletzten", "zum Monatsende") and then
 * "nach/ab Erledigung". Otherwise "jede zweite Woche montags" would silently become weekly
 * and "jede zweite Woche am Montag" a single Monday.
 */
function unsupportedTailEnd(words: readonly string[], k: number): number {
  const w = (j: number) => wordAt(words, j);
  let end = k;
  const adverbs = weekdayList(words, k, WEEKDAY_ADVERBS);
  if (adverbs) {
    end = weekdaysEnd(words, adverbs.end);
  } else if (w(k) === "am" || w(k) === "zum") {
    const named = weekdayList(words, k + 1, WEEKDAYS);
    const list = ordinalList(words, k + 1);
    if (named) end = weekdaysEnd(words, named.end);
    else if (w(k + 1) === "monatsletzten" || w(k + 1) === "monatsende") end = k + 2;
    else if (FULL_DATE.test(w(k + 1))) end = k + 2;
    else if (list) end = unsupportedEnd(words, list.end);
  }
  if ((w(end) === "nach" || w(end) === "ab") && w(end + 1) === "erledigung") end += 2;
  return end;
}

/** End of "des Monats" or "im Monat" at word `k`, else null. */
function ofMonthEnd(words: readonly string[], k: number): number | null {
  const w = (j: number) => wordAt(words, j);
  return (w(k) === "des" && w(k + 1) === "monats") || (w(k) === "im" && w(k + 1) === "monat") ? k + 2 : null;
}

/** End of "des Monats", "im Monat", "Tag des Monats" or "Tag im Monat" after the day at word `k`, else null. */
function ofMonthSuffixEnd(words: readonly string[], k: number): number | null {
  return ofMonthEnd(words, k + 1) ?? (wordAt(words, k + 1) === "tag" ? ofMonthEnd(words, k + 2) : null);
}

/**
 * A single fixed day of the month at word `k` and the end of the phrase: "15.", "15. des
 * Monats", "15. im Monat", "1. Tag des Monats"; an ordinal in words only with "des
 * Monats" or "im Monat" ("ersten Tag des Monats", "letzten im Monat"). "letzten Werktag"
 * is the last workday: with "des Monats" or "im Monat", or without when the rhythm is
 * already monthly (`monthly`: "monatlich am letzten Werktag"). null when a weekday, unit,
 * month name or month number follows the day ("1. Werktag", "1. Mai"), which makes the
 * phrase unsupported.
 */
function monthDay(words: readonly string[], k: number, monthly = false): { day: MonthDay; end: number } | null {
  const w = wordAt(words, k);
  if (LAST_WORDS.has(w) && wordAt(words, k + 1) === "werktag") {
    const end = ofMonthEnd(words, k + 2) ?? (monthly ? k + 2 : null);
    return end === null ? null : { day: "last_workday", end };
  }
  const numeric = NUMERIC_ORDINAL.test(w);
  const day = numeric ? Number(w.slice(0, -1)) : LAST_WORDS.has(w) ? "last" : ORDINAL_NUMBERS.get(w);
  if (day === undefined || (typeof day === "number" && (day < 1 || day > 31))) return null;
  const suffix = ofMonthSuffixEnd(words, k);
  if (suffix !== null) return { day, end: suffix };
  return numeric && unsupportedEnd(words, k + 1) === k + 1 ? { day, end: k + 1 } : null;
}

/**
 * Start of an ordinal ("ersten", "1.") that continues a weekday list ending at `end`
 * with a comma or "und" ("jeden Montag und ersten Freitag"), else null.
 */
function ordinalAfterList(words: readonly string[], end: number): number | null {
  const isOrdinal = (k: number) => {
    const w = wordAt(words, k);
    return ORDINAL_WORDS.has(w) || NUMERIC_ORDINAL.test(w);
  };
  if (/,$/.test(words[end - 1]!) && isOrdinal(end)) return end;
  if (wordAt(words, end) === "und" && isOrdinal(end + 1)) return end + 1;
  return null;
}

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

function monthly(every: number, day: MonthDay | undefined, consumed: number): RecurrenceHit {
  if (day === undefined) return { recurrence: { every, unit: "month", from: "due" }, consumed, first: sameDay };
  return {
    recurrence: { every, unit: "month", day, from: "due" },
    consumed,
    first: (from) => {
      const y = from.getFullYear();
      const m = from.getMonth();
      if (day === "last") return new Date(y, m + 1, 0);
      if (day === "last_workday") {
        const date = lastWorkday(y, m);
        return date < from ? lastWorkday(y, m + 1) : date;
      }
      const date = clampedDate(y, m, day);
      return date < from ? clampedDate(y, m + 1, day) : date;
    },
  };
}

/** Last Monday to Friday of month `m` (0-based) in `y`: a weekend falls back to Friday. */
function lastWorkday(y: number, m: number): Date {
  const last = new Date(y, m + 1, 0);
  const back = last.getDay() === 6 ? 1 : last.getDay() === 0 ? 2 : 0;
  return addDays(last, -back);
}

function simple(every: number, unit: "day" | "year", consumed: number): RecurrenceHit {
  return { recurrence: { every, unit, from: "due" }, consumed, first: sameDay };
}

/** "jeden 12.10.": yearly on that day (29.02. falls on 28.02. in common years and comes back in leap years). */
function yearlyOn(every: number, day: number, month: number, consumed: number): RecurrenceHit {
  return {
    recurrence: { every, unit: "year", day, from: "due" },
    consumed,
    month,
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
function matchRecurrence(words: readonly string[], i: number): RhythmMatch | null {
  const base = matchRhythmBase(words, i);
  if (!base?.recurrence) return base;
  const word = (k: number) => wordAt(words, k);
  const end = i + base.consumed;
  if ((word(end) === "nach" || word(end) === "ab") && word(end + 1) === "erledigung") {
    return { ...base, recurrence: { ...base.recurrence, from: "done" }, consumed: base.consumed + 2 };
  }
  return base;
}

function matchRhythmBase(words: readonly string[], i: number): RhythmMatch | null {
  const word = (k: number) => wordAt(words, k);
  const w = word(i);
  const next = word(i + 1);
  // Relative end positions below are counted from `i`.
  const rel = (end: number) => end - i;
  /** An unsupported phrase from `i` whose ordinals start at `start`. */
  const unsupportedFrom = (start: number): UnsupportedHit | null => {
    const list = ordinalList(words, start);
    return list ? unsupported(rel(unsupportedEnd(words, list.end))) : null;
  };

  /**
   * Weekly on a weekday list; an ordinal continuing the list ("jeden Montag und ersten
   * Freitag") makes the whole phrase unsupported.
   */
  const weeklyOn = (every: number, list: { days: number[]; end: number }): RhythmMatch => {
    const ordinal = ordinalAfterList(words, list.end);
    if (ordinal !== null) return unsupportedFrom(ordinal)!;
    return weekly(every, list.days, rel(list.end));
  };

  /**
   * "wöchentlich"/"jede Woche"/"alle n Wochen" optionally followed by "montags …" or "am
   * Montag …"; an ordinal after "am" ("wöchentlich am ersten Montag") makes the phrase
   * unsupported.
   */
  const weekWithDays = (every: number, end: number): RhythmMatch => {
    const adverbs = weekdayList(words, end, WEEKDAY_ADVERBS);
    if (adverbs) return weeklyOn(every, adverbs);
    const target = word(end + 1);
    if (word(end) === "am" && (ORDINAL_WORDS.has(target) || NUMERIC_ORDINAL.test(target))) return unsupportedFrom(end + 1)!;
    const named = word(end) === "am" ? weekdayList(words, end + 1, WEEKDAYS) : null;
    if (named) return weeklyOn(every, named);
    return weekly(every, null, rel(end));
  };

  /**
   * "monatlich"/"jeden Monat"/"alle n Monate" optionally followed by "am 15.", "am
   * Monatsletzten", "zum Monatsende", "am letzten Werktag", "am monatsletzten Werktag".
   */
  const monthWithDay = (every: number, end: number): RhythmMatch => {
    const prep = word(end);
    if (prep !== "am" && prep !== "zum") return monthly(every, undefined, rel(end));
    const target = word(end + 1);
    if (target === "monatsletzten" && word(end + 2) === "werktag") return monthly(every, "last_workday", rel(end + 3));
    if (target === "monatsletzten" || target === "monatsende") return monthly(every, "last", rel(end + 2));
    // "am 20.10." is a date: due on that day, the rhythm keeps no fixed day.
    if (prep === "zum" || DAY_MONTH.test(target)) return monthly(every, undefined, rel(end));
    const list = ordinalList(words, end + 1);
    if (!list) return monthly(every, undefined, rel(end));
    const fixed = list.count === 1 ? monthDay(words, end + 1, true) : null;
    if (fixed) return monthly(every, fixed.day, rel(fixed.end));
    return unsupported(rel(unsupportedEnd(words, list.end)));
  };

  /** "jährlich"/"jedes Jahr"/"alle n Jahre" optionally followed by "am 15.10.". */
  const yearWithDay = (every: number, end: number): RhythmMatch => {
    if (word(end) !== "am") return simple(every, "year", rel(end));
    const list = ordinalList(words, end + 1);
    if (!list) return simple(every, "year", rel(end));
    const dm = DAY_MONTH.exec(word(end + 1));
    // 2000 is a leap year: 29.02. is a valid day of February.
    if (list.count === 1 && dm && isValidCalendarDate(2000, Number(dm[2]), Number(dm[1]))) {
      return yearlyOn(every, Number(dm[1]), Number(dm[2]), rel(end + 2));
    }
    return unsupported(rel(unsupportedEnd(words, list.end)));
  };

  /** "jede"/"jeden"/"jedes" + "2." or "zweite" … "zwölfte" + unit: every n units. */
  const everyNth = (): RhythmMatch | null => {
    const inWords = ORDINAL_NUMBERS.get(next);
    const n = NUMERIC_ORDINAL.test(next) ? Number(next.slice(0, -1)) : inWords !== undefined && inWords >= 2 ? inWords : null;
    const unit = n === null ? undefined : UNIT_SINGULARS.get(word(i + 2));
    if (!unit || n === null) return null;
    if (unit === "week") return weekWithDays(n, i + 3);
    if (unit === "month") return monthWithDay(n, i + 3);
    if (unit === "year") return yearWithDay(n, i + 3);
    return simple(n, unit, 3);
  };

  switch (w) {
    case "täglich":
    case "taeglich":
      return simple(1, "day", 1);
    case "wöchentlich":
    case "woechentlich":
      return weekWithDays(1, i + 1);
    case "monatlich":
      return monthWithDay(1, i + 1);
    case "jährlich":
    case "jaehrlich":
      return yearWithDay(1, i + 1);
    case "werktags":
      return weekly(1, [1, 2, 3, 4, 5], 1);
    case "jede":
      if (next === "woche") return weekWithDays(1, i + 2);
      return everyNth() ?? unsupportedFrom(i + 1);
    case "jedes":
      if (next === "jahr") return yearWithDay(1, i + 2);
      if (next === "monatsende") return monthly(1, "last", 2);
      return everyNth() ?? unsupportedFrom(i + 1);
    case "jeder":
      return unsupportedFrom(i + 1);
    case "alle": {
      const unit = UNIT_PLURALS.get(word(i + 2));
      if (!unit) return null;
      // "alle paar Tage" has no number: unsupported, with what follows the unit.
      if (next === "paar") return unsupported(rel(unsupportedTailEnd(words, i + 3)));
      const n = /^[1-9]\d{0,2}$/.test(next) ? Number(next) : (CARDINAL_NUMBERS.get(next) ?? null);
      if (n === null) return null;
      if (unit === "week") return weekWithDays(n, i + 3);
      if (unit === "month") return monthWithDay(n, i + 3);
      if (unit === "year") return yearWithDay(n, i + 3);
      return simple(n, unit, 3);
    }
    case "jeden": {
      if (next === "tag") return simple(1, "day", 2);
      if (next === "monat") return monthWithDay(1, i + 2);
      if (next === "monatsletzten") return word(i + 2) === "werktag" ? monthly(1, "last_workday", 3) : monthly(1, "last", 2);
      const named = weekdayList(words, i + 1, WEEKDAYS);
      if (named) return weeklyOn(1, named);
      // "jeden 2. Tag" is every 2 days, "jeden 2. Tag des Monats" day 2 of the month.
      const dayOfMonth = word(i + 2) === "tag" && ofMonthEnd(words, i + 3) !== null;
      const nth = dayOfMonth ? null : everyNth();
      if (nth) return nth;
      // A single "jeden 12.10." (yearly), "jeden 15." or "jeden ersten des Monats"
      // (monthly); lists, other ordinals in words, "jeden 1. Montag", "jeden 1. Werktag"
      // and "jeden 1. Mai" are unsupported.
      const list = ordinalList(words, i + 1);
      if (!list) return null;
      if (list.count === 1) {
        const dm = DAY_MONTH.exec(next);
        if (dm) {
          const day = Number(dm[1]);
          const month = Number(dm[2]);
          // 2000 is a leap year: 29.02. is a valid day of February.
          if (isValidCalendarDate(2000, month, day) && unsupportedEnd(words, i + 2) === i + 2) return yearlyOn(1, day, month, 2);
        } else {
          const fixed = monthDay(words, i + 1);
          if (fixed) return monthly(1, fixed.day, rel(fixed.end));
        }
      }
      return unsupported(rel(unsupportedEnd(words, list.end)));
    }
    default: {
      const adverbs = weekdayList(words, i, WEEKDAY_ADVERBS);
      return adverbs ? weeklyOn(1, adverbs) : null;
    }
  }
}

const WAITING_PREFIX = "wartet:";

/** The name after "wartet:" ("Team_Einkauf." → "Team Einkauf"); null when empty or too long. */
function waitingName(word: string): string | null {
  const name = word
    .slice(WAITING_PREFIX.length)
    .replace(/[,;!?.]+$/, "")
    .replace(/_+/g, " ")
    .trim()
    .replace(/\s+/g, " ");
  return name && name.length <= TITLE_MAX ? name : null;
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
  /** Words and token of the date, to put it back into the title (see below). */
  let dateSpan: { start: number; end: number; token: CaptureToken } | null = null;
  let time: { hours: number; minutes: number } | null = null;
  let project: CaptureTarget | null = null;
  let area: CaptureTarget | null = null;
  let rhythm: RecurrenceHit | null = null;
  let weight: Weight | null = null;
  let forWhom: ForWhom | null = null;
  let waitingOn: string | null = null;

  for (let i = 0; i < words.length; i++) {
    if (!keep[i]) continue;
    const word = words[i]!;
    const lower = word.toLowerCase();

    const code = lower.slice(1);
    if (!weight && lower.startsWith("!") && (WEIGHTS as readonly string[]).includes(code)) {
      weight = code as Weight;
      keep[i] = false;
      tokens.push({ kind: "weight", text: word, label: WEIGHT_LABELS[weight] });
      continue;
    }
    if (!forWhom && lower.startsWith("@") && (FOR_WHOMS as readonly string[]).includes(code)) {
      forWhom = code as ForWhom;
      keep[i] = false;
      tokens.push({ kind: "for_whom", text: word, label: FOR_WHOM_LABELS[forWhom] });
      continue;
    }
    const waiting: string | null = !waitingOn && lower.startsWith(WAITING_PREFIX) ? waitingName(word) : null;
    if (waiting) {
      waitingOn = waiting;
      keep[i] = false;
      tokens.push({ kind: "waiting_on", text: word, label: waitingLabel(waiting) });
      continue;
    }

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
    // not read as a date either. The same holds for unsupported rhythm-like phrases.
    const rec = matchRecurrence(words, i);
    if (rec) {
      if (rec.recurrence && !rhythm && options.recurrence !== false) {
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
        const token: CaptureToken = { kind: "date", text: words.slice(i, at + hit.consumed).join(" "), label: hit.label };
        tokens.push(token);
        dateSpan = { start: i, end: at + hit.consumed, token };
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

  // A fixed yearly day ("jährlich am 29.02.", "jeden 29.02.") applies in the month of the
  // due date (contract 2.4). A date in another month ("jährlich am 29.02. morgen") cannot
  // be the due date without losing the named month, so the fixed day wins: the date stays
  // in the title, visible, and the series starts on its first day.
  if (rhythm?.month !== undefined && date && dateSpan && date.getMonth() + 1 !== rhythm.month) {
    for (let k = dateSpan.start; k < dateSpan.end; k++) keep[k] = true;
    tokens.splice(tokens.indexOf(dateSpan.token), 1);
    date = null;
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

  const recurrence = rhythm?.recurrence ?? null;

  // A yearly day names the month of the due date ("Jährlich am 28.02.").
  if (recurrence) for (const t of tokens) if (t.kind === "recurrence") t.label = describeRecurrence(recurrence, due);

  const location: Location | null = project ? { kind: "project", id: project.id } : area ? { kind: "area", id: area.id } : null;
  const title = words.filter((_, i) => keep[i]).join(" ") || input.trim();
  return { title, due, remind_at: remindAt, location, recurrence, weight, for_whom: forWhom, waiting_on: waitingOn, tokens, unknownTags };
}
