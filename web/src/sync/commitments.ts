// Zusagen-Bremse (contract 5.1): tasks for others ("für andere") are commitments someone waits
// on. Heute shows how many are open, and from a fixed number on a quiet hint to hand one back.

import { parseDue } from "./dates";
import { compareTasks, isClosed } from "./derived";
import type { Task } from "./types";

/** From this many open commitments on, Heute adds the hint. Fixed, not a setting. */
export const COMMITMENT_LIMIT = 8;

/**
 * A commitment is open while the task is for others and neither done nor dropped. Waiting on
 * someone (`waiting_on`, blocked) does not close it: the promise still stands.
 */
export function isOpenCommitment(task: Task): boolean {
  return task.for_whom === "andere" && !isClosed(task.status);
}

/**
 * Open commitments among `tasks` (subtasks included), without what `excluded` names (deleted
 * tasks, dev projects); due first, then rank.
 */
export function openCommitments(tasks: Iterable<Task>, excluded: (t: Task) => boolean): Task[] {
  const out: Task[] = [];
  for (const t of tasks) if (isOpenCommitment(t) && !excluded(t)) out.push(t);
  const due = (t: Task) => parseDue(t.due)?.at.getTime() ?? Infinity;
  return out.sort((a, b) => due(a) - due(b) || compareTasks(a, b));
}

/** Whether Heute adds "Viele Zusagen offen. Eine zurückgeben?". */
export function tooManyCommitments(count: number): boolean {
  return count >= COMMITMENT_LIMIT;
}
