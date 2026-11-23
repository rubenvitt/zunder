// Ordering Heute by hand (design 5.3): dragging or Shift+J/K puts a task between two others of
// the order (`todayList(...).items`). Heute sorts what is planned for today first, by day_rank,
// so a place below tasks without a plan for today only holds when those are planned as well:
// everything from the top down to the moved task ends up planned for today, in the order seen.
// Ranks that already fit stay, so a move within the plan usually writes one field.

import { rankBetween } from "./rank";
import type { Task } from "./types";

export interface DayOrderChange {
  id: string;
  fields: { planned_on?: string; day_rank: string };
}

/**
 * The writes that put `moved` between `before` and `after` (neighbours in `items`, Heute's
 * order without `moved`; null = open end) on the local date `today`. `before` wins when both are
 * given and do not sit side by side. Empty when the task stays where it is.
 */
export function dayOrderChanges(items: Task[], moved: Task, before: Task | null, after: Task | null, today: string): DayOrderChange[] {
  const order = items.filter((t) => t.id !== moved.id);
  let at = before ? order.findIndex((t) => t.id === before.id) + 1 : -1;
  if (at <= 0) at = after ? order.findIndex((t) => t.id === after.id) : -1;
  if (at < 0) at = before ? order.length : 0;
  const planned = (t: Task) => t.planned_on === today;
  // Nothing moves: the same place in a plan that already holds it.
  const was = items.findIndex((t) => t.id === moved.id);
  if (was === at && planned(moved) && moved.day_rank !== null) return [];

  const placed = [...order.slice(0, at), moved, ...order.slice(at).filter(planned)];
  const changes: DayOrderChange[] = [];
  let prev: string | null = null;
  const fits = (t: Task) => t.id !== moved.id && planned(t) && t.day_rank !== null && (prev === null || t.day_rank > prev);
  for (let i = 0; i < placed.length; i++) {
    const t = placed[i]!;
    if (fits(t)) {
      prev = t.day_rank;
      continue;
    }
    // Below the next task that keeps its rank, so it keeps it.
    const upper = placed.slice(i + 1).find(fits)?.day_rank ?? null;
    const rank = rankBetween(prev, upper);
    changes.push({ id: t.id, fields: planned(t) ? { day_rank: rank } : { planned_on: today, day_rank: rank } });
    prev = rank;
  }
  return changes;
}
