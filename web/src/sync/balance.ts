// Wochenbilanz ("Balance sichtbar", contract 5.1): what got done in an ISO week, by weight and
// by whom it was for. Same rule as zunder_core::balance (MCP week_balance), checked by the
// shared vectors (kind "week_balance"):
// - counts live tasks with status done outside dev projects (`excluded` names deleted tasks and
//   dev projects), subtasks included; dropped tasks do not count;
// - a task belongs to the ISO week of the local calendar day of its status change
//   (Monday 00:00 to the next Monday 00:00 in local time, daylight saving included).

import { addDays, dateFromLocal, localDate, shiftFire } from "./dates";
import { statusChangedMs } from "./derived";
import type { ForWhom, Task, Weight } from "./types";

export type WeightKey = Weight | "none";
export type ForWhomKey = ForWhom | "none";

export interface WeekBalance {
  /** ISO week, e.g. "2026-W44". */
  week: string;
  /** Monday of the week (YYYY-MM-DD). */
  from: string;
  /** Sunday of the week (YYYY-MM-DD). */
  to: string;
  total: number;
  by_weight: Record<WeightKey, number>;
  by_for_whom: Record<ForWhomKey, number>;
}

/** Display order of the groups (none last). */
export const WEIGHT_KEYS: readonly WeightKey[] = ["brand", "glut", "rauch", "asche", "none"];
export const FOR_WHOM_KEYS: readonly ForWhomKey[] = ["mich", "uns", "andere", "none"];

const WEEK_RE = /^(\d{4})-W(\d{2})$/;

/** Monday of an ISO week (local midnight), or null for a malformed id. */
export function mondayOf(week: string): Date | null {
  const m = WEEK_RE.exec(week);
  if (!m) return null;
  // Monday of ISO week 1 is the Monday of the week containing January 4.
  const jan4 = new Date(Number(m[1]), 0, 4);
  const monday1 = addDays(jan4, -((jan4.getDay() + 6) % 7));
  const monday = addDays(monday1, (Number(m[2]) - 1) * 7);
  // A week number beyond the year's last week names a week of the next year.
  return shiftFire(week, 0) === week ? monday : null;
}

export function emptyBalance(week: string): WeekBalance {
  const monday = mondayOf(week);
  return {
    week,
    from: monday ? localDate(monday) : "",
    to: monday ? localDate(addDays(monday, 6)) : "",
    total: 0,
    by_weight: { brand: 0, glut: 0, rauch: 0, asche: 0, none: 0 },
    by_for_whom: { mich: 0, uns: 0, andere: 0, none: 0 },
  };
}

/** The balance of `week` over `tasks` in one pass (status first, the costly exclusion last). */
export function weekBalance(tasks: Iterable<Task>, week: string, excluded: (t: Task) => boolean): WeekBalance {
  const balance = emptyBalance(week);
  const start = balance.from ? dateFromLocal(balance.from)!.getTime() : NaN;
  // Local midnight a week later: 167 or 169 hours when the clocks change in between.
  const end = balance.from ? addDays(dateFromLocal(balance.from)!, 7).getTime() : NaN;
  for (const t of tasks) {
    if (t.status !== "done") continue;
    const changed = statusChangedMs(t);
    if (!(changed >= start && changed < end) || excluded(t)) continue;
    balance.total += 1;
    balance.by_weight[t.weight ?? "none"] += 1;
    balance.by_for_whom[t.for_whom ?? "none"] += 1;
  }
  return balance;
}
