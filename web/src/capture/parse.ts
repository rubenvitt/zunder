// Quick capture shorthand (contract section 5), e.g.
//   "Zahnarzt anrufen morgen 9:00 #gesundheit"
// Dates: heute, morgen, übermorgen, weekday names, "nächsten <Wochentag>",
// 12.10., 12.10.2026 (optionally preceded by "am"). Times: 9:00, 9 Uhr, 9:30 Uhr
// (optionally preceded by "um"). A time sets due and remind_at; a date alone
// sets a date-only due. #slug resolves to an area or project (project wins on a tie).
// Unresolved tags stay in the title.

import { addDays, isValidCalendarDate, localDate, startOfDay, toRfc3339Local } from "../sync/dates";
import type { Location } from "../sync/types";

export interface CaptureTarget {
  id: string;
  name: string;
}

export interface CaptureStructure {
  areas: readonly CaptureTarget[];
  projects: readonly CaptureTarget[];
}

export type CaptureTokenKind = "date" | "time" | "area" | "project";

export interface CaptureToken {
  kind: CaptureTokenKind;
  text: string;
  /** German label for previews ("Morgen", "09:00", area/project name). */
  label: string;
}

export interface CaptureResult {
  title: string;
  /** "YYYY-MM-DD" or RFC 3339 with offset. */
  due: string | null;
  remind_at: string | null;
  /** null = inbox. */
  location: Location | null;
  tokens: CaptureToken[];
  unknownTags: string[];
}

const WEEKDAYS: Record<string, number> = {
  sonntag: 0,
  montag: 1,
  dienstag: 2,
  mittwoch: 3,
  donnerstag: 4,
  freitag: 5,
  samstag: 6,
  sonnabend: 6,
};

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

  if (word in WEEKDAYS) {
    const target = WEEKDAYS[word]!;
    const delta = ((target - today.getDay() + 7) % 7) || 7;
    return { date: addDays(today, delta), consumed: 1, label: WEEKDAY_LABELS[target]! };
  }

  if (NEXT_WORDS.has(word) && i + 1 < words.length) {
    const next = stripPunctuation(words[i + 1]!.toLowerCase());
    if (next in WEEKDAYS) {
      const target = WEEKDAYS[next]!;
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

function resolveTag(slug: string, structure: CaptureStructure): { kind: "area" | "project"; target: CaptureTarget } | null {
  const project = structure.projects.find((p) => slugify(p.name) === slug);
  if (project) return { kind: "project", target: project };
  const area = structure.areas.find((a) => slugify(a.name) === slug);
  if (area) return { kind: "area", target: area };
  return null;
}

export function parseCapture(input: string, now: Date, structure: CaptureStructure): CaptureResult {
  const words = input.trim().split(/\s+/).filter(Boolean);
  const keep: boolean[] = words.map(() => true);
  const tokens: CaptureToken[] = [];
  const unknownTags: string[] = [];
  const today = startOfDay(now);

  let date: Date | null = null;
  let time: { hours: number; minutes: number } | null = null;
  let project: CaptureTarget | null = null;
  let area: CaptureTarget | null = null;

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

  let due: string | null = null;
  let remindAt: string | null = null;
  if (time) {
    let day = date ?? today;
    let at = new Date(day.getFullYear(), day.getMonth(), day.getDate(), time.hours, time.minutes);
    if (!date && at.getTime() <= now.getTime()) {
      // A bare time that already passed today means tomorrow.
      day = addDays(today, 1);
      at = new Date(day.getFullYear(), day.getMonth(), day.getDate(), time.hours, time.minutes);
    }
    due = toRfc3339Local(at);
    remindAt = due;
  } else if (date) {
    due = localDate(date);
  }

  const location: Location | null = project ? { kind: "project", id: project.id } : area ? { kind: "area", id: area.id } : null;
  const title = words.filter((_, i) => keep[i]).join(" ") || input.trim();
  return { title, due, remind_at: remindAt, location, tokens, unknownTags };
}
