// "Heute nicht": putting a task off to a later day, counted by the deferral counter of the
// Phönix-Progression. Mirrors zunder_core::defer; the MCP tool defer_task follows the same rules.

import { addDays, dateFromLocal, isDateOnly, isRfc3339, localDate } from "./dates";
import type { JsonValue, Task } from "./types";

/** Deferrals that need no decision; the next one (the third) is a decision (core: FREE_DEFERRALS). */
export const FREE_DEFERRALS = 2;

export type DeferTarget = "tomorrow" | "next_week";

/** The day ("YYYY-MM-DD") a target names, seen from `today`. Next week = its Monday. */
export function deferDay(target: DeferTarget, today: string): string {
  const d = dateFromLocal(today)!;
  if (target === "tomorrow") return localDate(addDays(d, 1));
  const untilMonday = ((8 - d.getDay()) % 7) || 7;
  return localDate(addDays(d, untilMonday));
}

/** Whether "Nächste Woche" names another day than "Morgen": not on a Sunday, where both are Monday. */
export function offersNextWeek(today: string): boolean {
  return deferDay("next_week", today) !== deferDay("tomorrow", today);
}

/** Whether putting the task off once more is a decision rather than a click. */
export function needsDecision(task: Pick<Task, "deferrals">): boolean {
  return task.deferrals >= FREE_DEFERRALS;
}

/**
 * The calendar date of a due value or reminder in its own offset, as the core reads it
 * (a timestamp's leading "YYYY-MM-DD").
 */
function ownDate(value: string): string | null {
  if (isDateOnly(value)) return value;
  if (isRfc3339(value)) return value.slice(0, 10);
  return null;
}

/**
 * The fields one deferral to `day` writes, or null when the due date and the reminder
 * already lie on or after it. An earlier due date moves to `day` (a timestamp keeps its time
 * of day and offset), an earlier reminder as well; a task without both gets `day` as due date.
 */
export function deferFields(task: Pick<Task, "due" | "remind_at" | "deferrals" | "undated_ack">, day: string): Record<string, JsonValue> | null {
  const fields: Record<string, JsonValue> = {};
  const due = task.due ? ownDate(task.due) : null;
  if (task.due && due && due < day) fields.due = isDateOnly(task.due) ? day : day + task.due.slice(10);
  const remind = task.remind_at ? ownDate(task.remind_at) : null;
  if (task.remind_at && remind && remind < day) fields.remind_at = day + task.remind_at.slice(10);
  if (!task.due && !task.remind_at) fields.due = day;
  if (Object.keys(fields).length === 0) return null;
  if (fields.due !== undefined && task.undated_ack) fields.undated_ack = null;
  fields.deferrals = task.deferrals + 1;
  return fields;
}
