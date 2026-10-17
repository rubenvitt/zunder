// "Zurückgeben" (contract 5.1): a Rauch task goes back to whoever asked for it. The sentence
// to copy (sent by the owner, never by Zunder), the second task it offers instead, and the
// reminder to ask again.

import { addDays, parseDue, startOfDay, toRfc3339Local } from "./dates";
import type { Task } from "./types";

/** Working days (Monday to Friday) until the reminder. */
export const HANDBACK_WORKDAYS = 3;
/** Local hour of the reminder. */
export const HANDBACK_HOUR = 9;

/** `workdays` working days (Mon–Fri) after the day of `from`, at HANDBACK_HOUR local time. */
export function workdaysLater(from: Date, workdays: number): Date {
  let day = startOfDay(from);
  let left = workdays;
  while (left > 0) {
    day = addDays(day, 1);
    if (day.getDay() !== 0 && day.getDay() !== 6) left -= 1;
  }
  return new Date(day.getFullYear(), day.getMonth(), day.getDate(), HANDBACK_HOUR);
}

/** `remind_at` of a handed-back task: three working days later, 9:00 local time. */
export function handbackRemindAt(now: Date): string {
  return toRfc3339Local(workdaysLater(now, HANDBACK_WORKDAYS));
}

/** The sentence for the person who asked: a choice when there is a second task, else a plain question. */
export function handbackSentence(title: string, alternative: string | null): string {
  return alternative
    ? `Ich kann „${title}“ oder „${alternative}“ diese Woche machen. Was davon ist dir wichtiger?`
    : `Ich schaffe „${title}“ diese Woche nicht. Wie wichtig ist es dir?`;
}

/**
 * The second task the sentence offers: open or in progress, weighed Rauch or Brand, waiting
 * on nobody, not the task itself and not `excluded` (deleted ones, dev projects). The one due
 * first wins; without due dates the oldest. Ties go by creation, then ID.
 */
export function pickAlternative(task: Task, candidates: Iterable<Task>, excluded: (t: Task) => boolean): Task | null {
  let best: Task | null = null;
  let bestDue = Infinity;
  for (const t of candidates) {
    if (t.id === task.id || (t.status !== "open" && t.status !== "in_progress")) continue;
    if ((t.weight !== "rauch" && t.weight !== "brand") || t.waiting_on || excluded(t)) continue;
    const due = parseDue(t.due)?.at.getTime() ?? Infinity;
    if (!best || due < bestDue || (due === bestDue && byAge(t, best) < 0)) {
      best = t;
      bestDue = due;
    }
  }
  return best;
}

/** Creation order: smallest HLC, then ID. */
function byAge(a: Task, b: Task): number {
  return a.created_hlc < b.created_hlc ? -1 : a.created_hlc > b.created_hlc ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

/** Names used so far in `waiting_on` (live tasks), once each regardless of case, sorted. */
export function waitingNames(tasks: Iterable<Task>): string[] {
  const names = new Map<string, string>();
  for (const t of tasks) {
    const name = t.waiting_on?.trim();
    if (name && !names.has(name.toLocaleLowerCase("de"))) names.set(name.toLocaleLowerCase("de"), name);
  }
  return [...names.values()].sort((a, b) => a.localeCompare(b, "de"));
}
