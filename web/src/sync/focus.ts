// "Erst für dich" (contract 5.1): pay yourself first, also with time. Heute starts with one
// Glut task for oneself; without one it says so quietly and offers a few Glut tasks to pick for
// the day. Clicking it away holds for the day. Rules here, state and server in state/focus.

import { parseDue } from "./dates";
import type { FocusSettings, FocusSettingsUpdate, Task } from "./types";

/** How many Glut tasks the empty state offers to pick. */
export const FOCUS_CHOICES = 3;

/**
 * Whether a task can stand at the top of Heute (whom it is for aside): weighed Glut, open or in
 * progress (blocked by another task, waiting on someone, in review or closed it cannot be done
 * today), and not put off past today with "Heute nicht" (`deferrals` and a later due day).
 */
export function isFocusCandidate(task: Task, today: string): boolean {
  if (task.weight !== "glut" || (task.status !== "open" && task.status !== "in_progress") || task.waiting_on) return false;
  const due = parseDue(task.due);
  return !(task.deferrals > 0 && due && due.date > today);
}

/** Earliest due first (none last), then the oldest (creation, then ID): deterministic. */
export function compareFocus(a: Task, b: Task): number {
  const da = parseDue(a.due)?.at.getTime() ?? Infinity;
  const db = parseDue(b.due)?.at.getTime() ?? Infinity;
  if (da !== db) return da < db ? -1 : 1;
  if (a.created_hlc !== b.created_hlc) return a.created_hlc < b.created_hlc ? -1 : 1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

export interface Focus {
  /** The task at the top, or null. */
  task: Task | null;
  /** Whether it was picked for today (rather than found by the rule). */
  chosen: boolean;
  /** Without a task: up to FOCUS_CHOICES Glut tasks to pick from (not for others), best first. */
  choices: Task[];
}

/**
 * The task for "Erst für dich" today. One pass over `tasks` (`excluded`: deleted tasks, dev
 * projects): a task picked for today (`chosenId`) wins while it is still a candidate and not for
 * others; otherwise the best Glut task for oneself (`for_whom: mich`). Without either, the best
 * Glut tasks that are not for others (unset or "uns") are offered to pick.
 */
export function pickFocus(tasks: Iterable<Task>, today: string, excluded: (t: Task) => boolean, chosenId: string | null = null): Focus {
  let chosen: Task | null = null;
  let best: Task | null = null;
  const choices: Task[] = [];
  for (const t of tasks) {
    if (t.for_whom === "andere" || !isFocusCandidate(t, today) || excluded(t)) continue;
    if (t.id === chosenId) chosen = t;
    if (t.for_whom === "mich") {
      if (!best || compareFocus(t, best) < 0) best = t;
    } else if (choices.length < FOCUS_CHOICES || compareFocus(t, choices[choices.length - 1]!) < 0) {
      // Keep the best few, sorted, without sorting everything.
      choices.push(t);
      choices.sort(compareFocus);
      if (choices.length > FOCUS_CHOICES) choices.pop();
    }
  }
  if (chosen) return { task: chosen, chosen: true, choices: [] };
  if (best) return { task: best, chosen: false, choices: [] };
  return { task: null, chosen: false, choices };
}

// ---- Day state: server value, local changes not yet sent ---------------------------------

export const NO_FOCUS_SETTINGS: FocusSettings = { dismissed_on: null, chosen: null };

/**
 * What a device knows: the last value it has (from the server, with its own changes on top) and
 * the changes the server has not confirmed yet (offline, or sent and not answered).
 */
export interface FocusLocal {
  known: FocusSettings;
  pending: FocusSettingsUpdate | null;
}

export function applyUpdate(settings: FocusSettings, update: FocusSettingsUpdate): FocusSettings {
  return {
    dismissed_on: update.dismissed_on !== undefined ? update.dismissed_on : settings.dismissed_on,
    chosen: update.chosen !== undefined ? update.chosen : settings.chosen,
  };
}

/** A local change: visible at once, sent with the next chance. */
export function changeLocally(local: FocusLocal, update: FocusSettingsUpdate): FocusLocal {
  return { known: applyUpdate(local.known, update), pending: { ...local.pending, ...update } };
}

/** A value from the server; changes it has not confirmed stay on top. */
export function fromServer(local: FocusLocal, server: FocusSettings): FocusLocal {
  return { known: local.pending ? applyUpdate(server, local.pending) : server, pending: local.pending };
}

/**
 * The server confirmed `sent` with `server`. Changes made while it was on its way stay pending
 * (and on top); the rest is done.
 */
export function confirmed(local: FocusLocal, sent: FocusSettingsUpdate, server: FocusSettings): FocusLocal {
  const rest: FocusSettingsUpdate = {};
  for (const key of ["dismissed_on", "chosen"] as const) {
    const now = local.pending?.[key];
    if (now !== undefined && JSON.stringify(now) !== JSON.stringify(sent[key])) (rest as Record<string, unknown>)[key] = now;
  }
  const pending = Object.keys(rest).length > 0 ? rest : null;
  return { known: pending ? applyUpdate(server, pending) : server, pending };
}

/** Whether "Erst für dich" is clicked away for `today` (a value of another day no longer holds). */
export function isDismissed(settings: FocusSettings, today: string): boolean {
  return settings.dismissed_on === today;
}

/** The task picked for `today`, if any. */
export function chosenFor(settings: FocusSettings, today: string): string | null {
  return settings.chosen && settings.chosen.on === today ? settings.chosen.task_id : null;
}

/** Reads a stored FocusLocal defensively (anything unexpected counts as nothing known). */
export function parseFocusLocal(raw: string | null): FocusLocal {
  try {
    const value = raw ? (JSON.parse(raw) as Partial<FocusLocal>) : null;
    const settings = (v: unknown): FocusSettings => {
      const s = (v ?? {}) as Partial<FocusSettings>;
      const chosen = s.chosen && typeof s.chosen.on === "string" && typeof s.chosen.task_id === "string" ? { on: s.chosen.on, task_id: s.chosen.task_id } : null;
      return { dismissed_on: typeof s.dismissed_on === "string" ? s.dismissed_on : null, chosen };
    };
    if (!value || typeof value !== "object") return { known: NO_FOCUS_SETTINGS, pending: null };
    const pending = value.pending && typeof value.pending === "object" ? (value.pending as FocusSettingsUpdate) : null;
    return { known: settings(value.known), pending };
  } catch {
    return { known: NO_FOCUS_SETTINGS, pending: null };
  }
}
