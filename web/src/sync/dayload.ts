// Tagesvorschau (contract 5.1): how many tasks fall due on each of the next days, so a pile-up
// of small things shows before the day arrives. It counts, it does not move anything: whether
// to spread the tasks out stays a decision in the detail.

import { addDays, dateFromLocal, localDate, parseDue } from "./dates";
import { isClosed } from "./derived";
import type { Task } from "./types";

/** From this many tasks due on one day on, the day counts as crowded. Fixed, not a setting. */
export const CROWDED_DAY = 5;

/** Days the preview in Heute looks ahead: tomorrow up to and including a week from today. */
export const PREVIEW_DAYS = 7;

export interface DayLoad {
  /** Local calendar day "YYYY-MM-DD". */
  date: string;
  /** Tasks due that day, in rank order of the caller's choice (here: as met). */
  tasks: Task[];
}

/** Whether `count` tasks on one day are a pile-up. */
export function isCrowded(count: number): boolean {
  return count >= CROWDED_DAY;
}

/**
 * A task weighs on its due day while it lives, is neither done nor dropped and is not excluded
 * (deleted, dev projects); subtasks count like any task, blocked ones and those that wait on
 * someone too (the day still holds them).
 */
function counts(task: Task, excluded: (t: Task) => boolean): boolean {
  return !isClosed(task.status) && !!task.due && !excluded(task);
}

/**
 * The next `days` local calendar days after `today` ("YYYY-MM-DD"), each with the tasks due on
 * it. One pass over `tasks`; days without tasks are in the list with an empty one.
 */
export function dayLoads(tasks: Iterable<Task>, today: string, days: number, excluded: (t: Task) => boolean): DayLoad[] {
  const start = dateFromLocal(today);
  if (!start) return [];
  const out: DayLoad[] = [];
  const byDate = new Map<string, DayLoad>();
  for (let i = 1; i <= days; i++) {
    const load = { date: localDate(addDays(start, i)), tasks: [] };
    out.push(load);
    byDate.set(load.date, load);
  }
  for (const t of tasks) {
    if (!counts(t, excluded)) continue;
    const due = parseDue(t.due);
    if (due) byDate.get(due.date)?.tasks.push(t);
  }
  return out;
}

/** How many tasks besides `exceptId` are due on `date` (for the hint when picking a day). */
export function dueOnDay(tasks: Iterable<Task>, date: string, excluded: (t: Task) => boolean, exceptId?: string): number {
  let n = 0;
  for (const t of tasks) {
    if (t.id === exceptId || !counts(t, excluded)) continue;
    if (parseDue(t.due)?.date === date) n++;
  }
  return n;
}
