// Calendar of follow-up instances for the mock server (contract 3.1, "Termin der
// Folgeinstanz"), a port of zunder_core::recurrence::next_occurrence. Local dates and
// times are those of the mock process (TZ), like the real server's zone. JavaScript
// resolves local times like the contract: a time in the spring gap is later by the gap,
// one that exists twice is the earlier instant.

export interface MockRecurrence {
  every: number;
  unit: "day" | "week" | "month" | "year";
  weekdays?: string[];
  day?: number | "last";
  from: "due" | "done";
}

export interface MockNext {
  due: string;
  remind_at: string | null;
  recurrence: MockRecurrence;
}

const WEEKDAYS = ["mo", "tu", "we", "th", "fr", "sa", "su"];
const DAY_MS = 86_400_000;
const pad = (n: number, w = 2) => String(n).padStart(w, "0");

/** A calendar date as days since 1970-01-01 (proleptic Gregorian, no time zone). */
type Days = number;

const days = (y: number, m: number, d: number): Days => Date.UTC(y, m - 1, d) / DAY_MS;
function civil(n: Days): [number, number, number] {
  const dt = new Date(n * DAY_MS);
  return [dt.getUTCFullYear(), dt.getUTCMonth() + 1, dt.getUTCDate()];
}
/** 0 = Monday. */
const weekday = (n: Days) => (new Date(n * DAY_MS).getUTCDay() + 6) % 7;
const monday = (n: Days) => n - weekday(n);
const monthIndex = (n: Days) => {
  const [y, m] = civil(n);
  return y * 12 + m - 1;
};
const ceilDiv = (a: number, b: number) => Math.ceil(a / b);
const isoDate = (n: Days) => {
  const [y, m, d] = civil(n);
  return `${pad(y, 4)}-${pad(m)}-${pad(d)}`;
};

/** Day `day` in the month with index `year · 12 + month − 1`, clamped to its length; null beyond 9999. */
function dayInMonth(index: number, day: number | "last"): Days | null {
  const y = Math.floor(index / 12);
  const m = (index % 12) + 1;
  if (y < 0 || y > 9999) return null;
  const length = new Date(Date.UTC(y, m, 0)).getUTCDate();
  return days(y, m, day === "last" ? length : Math.min(day, length));
}

function localDays(at: Date): Days {
  return days(at.getFullYear(), at.getMonth() + 1, at.getDate());
}

/**
 * Fractional seconds of an RFC 3339 timestamp as the core writes them (time's Rfc3339): at
 * most nine digits, no trailing zeros, "" for none. Taken from the text because a Date keeps
 * only milliseconds; offsets are whole minutes, so the local time has the same fraction.
 */
function fraction(rfc3339: string): string {
  const digits = (/\.(\d+)/.exec(rfc3339)?.[1] ?? "").slice(0, 9).replace(/0+$/, "");
  return digits ? `.${digits}` : "";
}

/** RFC 3339 with the local offset of the mock process, `Z` for UTC like the core. */
function rfc3339Local(d: Date, frac: string): string {
  const offset = -d.getTimezoneOffset();
  const abs = Math.abs(offset);
  return (
    `${pad(d.getFullYear(), 4)}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}${frac}` +
    (offset === 0 ? "Z" : `${offset > 0 ? "+" : "-"}${pad(Math.floor(abs / 60))}:${pad(abs % 60)}`)
  );
}

/** The same local time on another date (gap: later by the gap; overlap: earlier). */
function atLocal(date: Days, time: Date): Date {
  const [y, m, d] = civil(date);
  return new Date(y, m - 1, d, time.getHours(), time.getMinutes(), time.getSeconds());
}

function fromDue(r: MockRecurrence, d0: Days, today: Days): Days | null {
  const lower = Math.max(d0 + 1, today);
  const step = (unitDays: number) => d0 + Math.max(1, ceilDiv(lower - d0, unitDays)) * unitDays;
  switch (r.unit) {
    case "day":
      return step(r.every);
    case "week": {
      if (!r.weekdays) return step(7 * r.every);
      const offsets = r.weekdays.map((w) => WEEKDAYS.indexOf(w)).sort((a, b) => a - b);
      const week0 = monday(d0);
      for (let k = Math.max(0, ceilDiv((monday(lower) - week0) / 7, r.every)); ; k++) {
        const week = week0 + k * r.every * 7;
        for (const o of offsets) if (week + o >= lower) return week + o;
      }
    }
    case "month":
    case "year": {
      // Years use the month of D0 and move on by at least a year: a 28.02. stays a 28.02.,
      // a 29.02. comes back in leap years, a moved instance takes the series along.
      const months = r.unit === "year" ? 12 * r.every : r.every;
      const day = r.day ?? civil(d0)[2];
      const minK = r.unit === "year" ? 1 : 0;
      for (let k = Math.max(minK, ceilDiv(monthIndex(lower) - monthIndex(d0), months)); ; k++) {
        const candidate = dayInMonth(monthIndex(d0) + k * months, day);
        if (candidate === null || candidate >= lower) return candidate;
      }
    }
  }
}

function fromDone(r: MockRecurrence, done: Days): Days | null {
  switch (r.unit) {
    case "day":
      return done + r.every;
    case "week": {
      const target = done + 7 * r.every;
      if (!r.weekdays) return target;
      return monday(target) + Math.min(...r.weekdays.map((w) => WEEKDAYS.indexOf(w)));
    }
    case "month":
      return dayInMonth(monthIndex(done) + r.every, r.day ?? civil(done)[2]);
    case "year":
      return dayInMonth(monthIndex(done) + 12 * r.every, r.day ?? civil(done)[2]);
  }
}

/**
 * Due date, reminder and recurrence of the next instance of a series whose current
 * instance (`due`, `remindAt`) was completed at `completedMs`, with `nowMs` the time the
 * push is handled. Null when a date would lie beyond the year 9999.
 */
export function nextOccurrence(r: MockRecurrence, due: string, remindAt: string | null, completedMs: number, nowMs: number): MockNext | null {
  const dateOnly = /^(\d{4})-(\d{2})-(\d{2})$/.exec(due);
  const dueAt = dateOnly ? null : new Date(due);
  if (dueAt && Number.isNaN(dueAt.getTime())) return null;
  const d0 = dateOnly ? days(Number(dateOnly[1]), Number(dateOnly[2]), Number(dateOnly[3])) : localDays(dueAt!);
  const date = r.from === "due" ? fromDue(r, d0, localDays(new Date(nowMs))) : fromDone(r, localDays(new Date(completedMs)));
  if (date === null || civil(date)[0] > 9999) return null;
  let remind: string | null = null;
  if (remindAt) {
    const local = new Date(remindAt);
    if (!Number.isNaN(local.getTime())) {
      const shifted = atLocal(localDays(local) + (date - d0), local);
      if (shifted.getFullYear() > 9999) return null;
      remind = rfc3339Local(shifted, fraction(remindAt));
    }
  }
  const recurrence: MockRecurrence = { ...r };
  if (r.from === "due" && (r.unit === "month" || r.unit === "year") && r.day === undefined) recurrence.day = civil(d0)[2];
  return { due: dueAt ? rfc3339Local(atLocal(date, dueAt), fraction(due)) : isoDate(date), remind_at: remind, recurrence };
}
