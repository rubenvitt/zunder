// Date helpers: local calendar dates, RFC 3339 with offset, ISO weeks, German formatting.

const DATE_ONLY_RE = /^(\d{4})-(\d{2})-(\d{2})$/;
const RFC3339_RE = /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(\.\d+)?([Zz]|[+-]\d{2}:\d{2})$/;
const FIRE_RE = /^(\d{4})-W(\d{2})$/;

const pad = (n: number, width = 2) => String(n).padStart(width, "0");

export function isValidCalendarDate(year: number, month: number, day: number): boolean {
  if (month < 1 || month > 12 || day < 1) return false;
  const d = new Date(year, month - 1, day);
  return d.getFullYear() === year && d.getMonth() === month - 1 && d.getDate() === day;
}

export function isDateOnly(value: string): boolean {
  const m = DATE_ONLY_RE.exec(value);
  return !!m && isValidCalendarDate(Number(m[1]), Number(m[2]), Number(m[3]));
}

export function isRfc3339(value: string): boolean {
  const m = RFC3339_RE.exec(value);
  if (!m) return false;
  if (!isValidCalendarDate(Number(m[1]), Number(m[2]), Number(m[3]))) return false;
  return Number(m[4]) < 24 && Number(m[5]) < 60 && Number(m[6]) < 61 && !Number.isNaN(Date.parse(value));
}

export function isValidDue(value: string): boolean {
  return isDateOnly(value) || isRfc3339(value);
}

export function isValidFire(value: string): boolean {
  const m = FIRE_RE.exec(value);
  if (!m) return false;
  const week = Number(m[2]);
  return week >= 1 && week <= weeksInIsoYear(Number(m[1]));
}

/** "YYYY-MM-DD" of a Date in local time. */
export function localDate(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** "HH:MM" of a Date in local time. */
export function localTime(d: Date): string {
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** RFC 3339 in local time with numeric offset, seconds precision. */
export function toRfc3339Local(d: Date): string {
  const offsetMin = -d.getTimezoneOffset();
  const sign = offsetMin >= 0 ? "+" : "-";
  const abs = Math.abs(offsetMin);
  return `${localDate(d)}T${localTime(d)}:${pad(d.getSeconds())}${sign}${pad(Math.floor(abs / 60))}:${pad(abs % 60)}`;
}

export function startOfDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

export function addDays(d: Date, days: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + days, d.getHours(), d.getMinutes(), d.getSeconds());
}

/** Local date from "YYYY-MM-DD" (midnight local time). */
export function dateFromLocal(value: string): Date | null {
  const m = DATE_ONLY_RE.exec(value);
  if (!m) return null;
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
}

export interface DueInfo {
  /** Local calendar date "YYYY-MM-DD". */
  date: string;
  /** Local "HH:MM" when the due value carries a time. */
  time: string | null;
  /** The instant (date-only: local midnight). */
  at: Date;
}

export function parseDue(due: string | null): DueInfo | null {
  if (!due) return null;
  if (isDateOnly(due)) return { date: due, time: null, at: dateFromLocal(due)! };
  if (isRfc3339(due)) {
    const at = new Date(due);
    return { date: localDate(at), time: localTime(at), at };
  }
  return null;
}

export function weeksInIsoYear(year: number): number {
  // A year has 53 ISO weeks if Dec 28 falls into week 53.
  return isoWeekOf(new Date(year, 11, 28)).week;
}

export function isoWeekOf(d: Date): { year: number; week: number } {
  const date = new Date(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()));
  const day = date.getUTCDay() || 7;
  date.setUTCDate(date.getUTCDate() + 4 - day);
  const yearStart = Date.UTC(date.getUTCFullYear(), 0, 1);
  const week = Math.ceil(((date.getTime() - yearStart) / 86_400_000 + 1) / 7);
  return { year: date.getUTCFullYear(), week };
}

/** Phönix fire id of the ISO week, e.g. "2026-W41". */
export function fireOf(d: Date): string {
  const { year, week } = isoWeekOf(d);
  return `${year}-W${pad(week)}`;
}

/** Shift a fire id by whole weeks. */
export function shiftFire(fire: string, weeks: number): string {
  const m = FIRE_RE.exec(fire);
  if (!m) return fire;
  // Monday of ISO week 1 is the Monday of the week containing Jan 4.
  const jan4 = new Date(Number(m[1]), 0, 4);
  const mondayWeek1 = addDays(jan4, -((jan4.getDay() + 6) % 7));
  return fireOf(addDays(mondayWeek1, (Number(m[2]) - 1 + weeks) * 7));
}

const WEEKDAYS_SHORT = ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"];

/** Short German label relative to `now`: "Heute", "Morgen", "Gestern" or "Mo, 12.10." (+ year if different). */
export function formatDateLabel(date: string, now: Date): string {
  const d = dateFromLocal(date);
  if (!d) return date;
  const today = startOfDay(now);
  const diff = Math.round((d.getTime() - today.getTime()) / 86_400_000);
  if (diff === 0) return "Heute";
  if (diff === 1) return "Morgen";
  if (diff === -1) return "Gestern";
  const base = `${WEEKDAYS_SHORT[d.getDay()]}, ${pad(d.getDate())}.${pad(d.getMonth() + 1)}.`;
  return d.getFullYear() === now.getFullYear() ? base : `${base}${d.getFullYear()}`;
}

export function formatDue(due: string | null, now: Date): string {
  const info = parseDue(due);
  if (!info) return due ?? "";
  const label = formatDateLabel(info.date, now);
  return info.time ? `${label} ${info.time}` : label;
}

export function formatInstant(value: string | null, now: Date): string {
  if (!value) return "";
  const at = new Date(value);
  if (Number.isNaN(at.getTime())) return value;
  return `${formatDateLabel(localDate(at), now)} ${localTime(at)}`;
}

/** Value for <input type="datetime-local"> from RFC 3339. */
export function toDatetimeLocalInput(value: string | null): string {
  if (!value) return "";
  const at = new Date(value);
  if (Number.isNaN(at.getTime())) return "";
  return `${localDate(at)}T${localTime(at)}`;
}

/** RFC 3339 (local offset) from an <input type="datetime-local"> value. */
export function fromDatetimeLocalInput(value: string): string | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(value);
  if (!m) return null;
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]), Number(m[4]), Number(m[5]));
  return toRfc3339Local(d);
}
