// User intents → sync engine changes. Every mutation goes through engine.change,
// which validates client-side, applies optimistically and queues the push.

import { hasOpenChildren, isClosed, isDevLocation, isRecurring, sortKey } from "../sync/derived";
import type { Change } from "../sync/engine";
import { rankBetween } from "../sync/rank";
import { recurrenceValue } from "../sync/recurrence";
import type { EntityKind, ForWhom, JsonValue, Location, ProjectKind, Recurrence, Task, TaskStatus, Weight } from "../sync/types";
import { deleteCascade as cascadeTargets } from "../sync/validate";
import { newId } from "../sync/ids";
import { STATUS_LABELS } from "../labels";
import { api, data, getEngine, now, taskIndex } from "./store";
import { expectNextInstance } from "./series";
import { setDeferRequest, setDetailId, setDropRequest, setSelectedId, toast } from "./ui";
import { deferDay, deferFields, needsDecision, type DeferTarget } from "../sync/defer";
import { formatDateLabel, formatInstant, localDate, toRfc3339Local } from "../sync/dates";
import { handbackRemindAt, pickAlternative } from "../sync/handback";

function apply(changes: Change[]): boolean {
  const issues = getEngine().change(changes);
  if (issues.length > 0) {
    toast(issues[0]!.message, "error");
    return false;
  }
  return true;
}

function locationValue(location: Location): JsonValue {
  return location.kind === "area" || location.kind === "project" ? { kind: location.kind, id: location.id } : { kind: location.kind };
}

export interface NewTask {
  title: string;
  location?: Location | null;
  parent_id?: string | null;
  due?: string | null;
  remind_at?: string | null;
  status?: TaskStatus;
  rank?: string;
  /** Only for root tasks with a due date (contract 2.5). */
  recurrence?: Recurrence | null;
  weight?: Weight | null;
  for_whom?: ForWhom | null;
}

/** Creates a task (one push group); returns its id or null when refused. */
export function createTask(input: NewTask): string | null {
  const id = newId();
  const fields: Record<string, JsonValue> = { title: input.title.trim() };
  if (input.location && input.location.kind !== "inbox") fields.location = locationValue(input.location);
  if (input.parent_id) fields.parent_id = input.parent_id;
  if (input.due) fields.due = input.due;
  if (input.remind_at) fields.remind_at = input.remind_at;
  if (input.status && input.status !== "open") fields.status = input.status;
  if (input.rank) fields.rank = input.rank;
  if (input.recurrence) fields.recurrence = recurrenceValue(input.recurrence);
  if (input.weight) fields.weight = input.weight;
  if (input.for_whom) fields.for_whom = input.for_whom;
  return apply([{ entity: "task", id, fields }]) ? id : null;
}

export function updateTask(id: string, fields: Record<string, JsonValue>): boolean {
  return apply([{ entity: "task", id, fields }]);
}

/**
 * The one path for status changes (detail, check button, board drag and keys): writes
 * the status (with `extra` fields such as the board's rank in the same push group),
 * warns about open subtasks and waits for the next instance of a completed series.
 */
export function setStatus(id: string, status: TaskStatus, extra: Record<string, JsonValue> = {}): boolean {
  const task = data.tasks[id];
  if (!task) return true;
  if (task.status === status) return Object.keys(extra).length === 0 || updateTask(id, extra);
  const completes = isClosed(status) && !isClosed(task.status);
  const before = { ...task };
  const ok = updateTask(id, { ...extra, status });
  if (ok && status === "done") warnIfOpenChildren(id);
  if (ok && completes) expectNextInstance(before);
  return ok;
}

/**
 * Drops a task. A recurring one asks first whether only this instance (the server
 * creates the next) or the whole series ends; `before` runs right before the status is
 * written (e.g. to move the selection on). Returns false when nothing was written (yet).
 */
export function dropTask(id: string, before?: () => void): boolean {
  const task = data.tasks[id];
  if (!task || task.status === "dropped") return false;
  if (isRecurring(task)) {
    setDropRequest({ taskId: id, before });
    return false;
  }
  before?.();
  if (!setStatus(id, "dropped")) return false;
  toast(`„${shorten(task.title)}“ verworfen.`);
  return true;
}

/** "Nur diese verwerfen": the series goes on with the next instance. */
export function dropInstance(id: string): boolean {
  const task = data.tasks[id];
  if (!task || !setStatus(id, "dropped")) return false;
  toast("Verworfen, die nächste Instanz folgt.");
  return true;
}

/**
 * "Serie beenden": drops the task and removes its rhythm in one action, no next instance.
 * `recurrence: null` comes first in the oplog: the app pushes its pending ops in batches
 * regardless of where an action ends, and a push holding only `status: dropped` would
 * still find the rhythm and create the next instance.
 */
export function endSeries(id: string): boolean {
  const task = data.tasks[id];
  if (!task || !updateTask(id, { recurrence: null, status: "dropped" })) return false;
  toast(`„${shorten(task.title)}“ verworfen, Serie beendet.`);
  return true;
}

/**
 * Removes the due date; a recurring task needs one, so its series ends with it (the
 * caller asks first). `recurrence: null` comes first, so a split push stays valid.
 */
export function clearDue(id: string): boolean {
  const task = data.tasks[id];
  if (!task) return false;
  return updateTask(id, task.recurrence ? { recurrence: null, due: null } : { due: null });
}

/** The "done with open subtasks" hint (contract 2.5: a warning, not a refusal). */
export function warnIfOpenChildren(id: string): void {
  const children = taskIndex().childrenOf(id);
  if (!hasOpenChildren(children)) return;
  const open = children.filter((c) => !isClosed(c.status)).length;
  toast(`Hinweis: ${open === 1 ? "Eine Unteraufgabe ist" : `${open} Unteraufgaben sind`} noch offen.`, "warning");
}

export function toggleDone(id: string): void {
  const task = data.tasks[id];
  if (!task) return;
  const next: TaskStatus = task.status === "done" ? "open" : "done";
  if (setStatus(id, next)) toast(`„${shorten(task.title)}“ → ${STATUS_LABELS[next]}`);
}

/** Moves a task to a location; a subtask is detached from its parent. */
// ---- "Heute nicht" (contract 5.1) --------------------------------------------

/**
 * Puts a task off to tomorrow or next week and counts. After two deferrals the next one is
 * a decision: the dialog asks (drop, cut anew, let rest) instead of moving it once more.
 * `before` runs right before the task leaves the current list. Returns whether it moved now.
 */
export function deferTask(id: string, target: DeferTarget, before?: () => void): boolean {
  const task = data.tasks[id];
  if (!task || isClosed(task.status)) return false;
  if (needsDecision(task)) {
    setDeferRequest({ taskId: id, target, before });
    return false;
  }
  return writeDeferral(id, target, before);
}

/** "morgen", "Mo, 19.10." … for the middle of a sentence. */
function dayPhrase(day: string, today: Date): string {
  const label = formatDateLabel(day, today);
  return /^(Heute|Morgen|Gestern)$/.test(label) ? label.toLowerCase() : label;
}

/** Writes one deferral without asking (also "Trotzdem verschieben" in the decision dialog). */
export function writeDeferral(id: string, target: DeferTarget, before?: () => void): boolean {
  const task = data.tasks[id];
  if (!task) return false;
  const today = new Date(now());
  const day = deferDay(target, localDate(today));
  const fields = deferFields(task, day);
  if (!fields) {
    toast(`„${shorten(task.title)}“ ist schon für ${dayPhrase(day, today)} oder später geplant.`);
    return false;
  }
  before?.();
  if (!updateTask(id, fields)) return false;
  const when = `auf ${dayPhrase(day, today)}`;
  const next = needsDecision({ deferrals: fields.deferrals as number }) ? " Beim nächsten Mal entscheidest du." : "";
  toast(`„${shorten(task.title)}“ ${when} verschoben.${next}`);
  return true;
}

/** Decision "Neu schneiden": the counter starts over, the task opens to sharpen or split it. */
export function cutAnew(id: string): boolean {
  if (!updateTask(id, { deferrals: null })) return false;
  setSelectedId(id);
  setDetailId(id);
  toast("Neu schneiden: Titel schärfen oder in Unteraufgaben zerlegen. Der Zähler beginnt neu.");
  return true;
}

/**
 * Decision "Ruhen lassen": no date, no reminder, marked as deliberately undated, counter
 * reset. Not for recurring tasks (a series needs its due date).
 */
export function letRest(id: string): boolean {
  const task = data.tasks[id];
  if (!task || isRecurring(task)) return false;
  const ok = updateTask(id, { due: null, remind_at: null, undated_ack: toRfc3339Local(new Date(now())), deferrals: null });
  if (ok) toast(`„${shorten(task.title)}“ ruht, ohne Datum. Die Deadline-Prüfung fragt erst später wieder.`);
  return ok;
}

// ---- "Zurückgeben" (contract 5.1) -------------------------------------------

function inDevProject(task: Task): boolean {
  return isDevLocation(taskIndex().location(task), (id) => data.projects[id]);
}

/** Whether a task offers "Zurückgeben": weighed Rauch, not closed, outside dev projects. */
export function canHandBack(task: Task): boolean {
  return task.weight === "rauch" && !isClosed(task.status) && !inDevProject(task);
}

/** The second task the sentence offers (rule in sync/handback). */
export function handBackAlternative(task: Task): Task | null {
  const index = taskIndex();
  return pickAlternative(task, index.byId.values(), (t) => index.isDeleted(t) || inDevProject(t));
}

/**
 * Hands a task back: it waits on `name` and is blocked until the answer, with a reminder in
 * three working days when `remind` is set (one push group). `sentence` goes to the clipboard.
 */
export function handBack(id: string, name: string, remind: boolean, sentence = ""): boolean {
  const task = data.tasks[id];
  const waiting = name.trim();
  if (!task || !waiting) return false;
  const fields: Record<string, JsonValue> = { waiting_on: waiting, status: "blocked" };
  const remindAt = remind ? handbackRemindAt(new Date(now())) : null;
  if (remindAt) fields.remind_at = remindAt;
  if (!updateTask(id, fields)) return false;
  let copied = "";
  if (sentence && navigator.clipboard) {
    copied = "Satz kopiert. ";
    void navigator.clipboard.writeText(sentence).catch(() => toast("Kopieren nicht möglich.", "warning"));
  } else if (sentence) toast("Kopieren nicht möglich.", "warning");
  const reminder = remindAt ? ` Erinnerung: ${formatInstant(remindAt, new Date(now()))}.` : "";
  toast(`${copied}„${shorten(task.title)}“ wartet auf ${waiting}.${reminder}`);
  return true;
}

/** Sets or clears whom a task waits on (trimmed, blank = nobody); the server sets the status. */
export function setWaitingOn(id: string, name: string): boolean {
  const waiting = name.trim();
  return updateTask(id, { waiting_on: waiting || null });
}

export function moveTask(id: string, location: Location): boolean {
  const task = data.tasks[id];
  if (!task) return false;
  const fields: Record<string, JsonValue> = { location: locationValue(location) };
  if (task.parent_id) fields.parent_id = null;
  return updateTask(id, fields);
}

export function setParent(id: string, parentId: string | null): boolean {
  const task = data.tasks[id];
  if (!task) return false;
  if (parentId === null) {
    // Keep the effective location when detaching.
    return updateTask(id, { parent_id: null, location: locationValue(taskIndex().location(task)) });
  }
  return updateTask(id, { parent_id: parentId });
}

/**
 * Tombstones an entity with all live dependents in one push group (core:
 * OpBatch::delete_cascade); the server rejects deleting a still referenced entity.
 */
export function deleteCascade(entity: EntityKind, id: string): boolean {
  const targets = cascadeTargets(getEngine().view, entity, id);
  return apply(targets.map(([kind, tid]) => ({ entity: kind, id: tid, fields: { deleted: true } })));
}

/** Number of live dependents a delete of this entity takes along. */
export function dependentsCount(entity: EntityKind, id: string): number {
  return cascadeTargets(getEngine().view, entity, id).length - 1;
}

/** Deletes a task together with its subtasks. */
export function deleteTask(id: string): boolean {
  return deleteCascade("task", id);
}

/**
 * Places `id` between `before` and `after` (neighbors in the target order), optionally
 * in another status. The status goes the common way (setStatus, dropTask), so completing
 * on the board behaves like the check button.
 */
export function placeTask(id: string, before: Task | null, after: Task | null, status?: TaskStatus): boolean {
  const rank = rankBetween(before ? sortKey(before) : null, after ? sortKey(after) : null);
  const task = data.tasks[id];
  if (!task || status === undefined || status === task.status) return updateTask(id, { rank });
  // Dropping asks first for a recurring task; the rank follows when it goes ahead.
  if (status === "dropped") return dropTask(id, () => updateTask(id, { rank }));
  return setStatus(id, status, { rank });
}

export function shorten(text: string, max = 40): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

// ---- Comments ---------------------------------------------------------

/** Writes a comment on a task as the owner (no author); returns its id or null when refused. */
export function addComment(taskId: string, body: string): string | null {
  const id = newId();
  return apply([{ entity: "comment", id, fields: { task_id: taskId, body } }]) ? id : null;
}

export function updateComment(id: string, body: string): boolean {
  return apply([{ entity: "comment", id, fields: { body } }]);
}

export function deleteComment(id: string): boolean {
  return apply([{ entity: "comment", id, fields: { deleted: true } }]);
}

// ---- Areas and projects -----------------------------------------------

export function createArea(name: string): string | null {
  const id = newId();
  return apply([{ entity: "area", id, fields: { name: name.trim() } }]) ? id : null;
}

export function updateArea(id: string, fields: Record<string, JsonValue>): boolean {
  return apply([{ entity: "area", id, fields }]);
}

export function createProject(name: string, areaId: string, kind: ProjectKind = "standard", repo?: string | null, key?: string | null): string | null {
  const id = newId();
  const fields: Record<string, JsonValue> = { name: name.trim(), area_id: areaId };
  if (kind !== "standard") fields.kind = kind;
  if (kind === "dev" && repo) fields.repo = repo;
  if (key) fields.key = key;
  return apply([{ entity: "project", id, fields }]) ? id : null;
}

export function updateProject(id: string, fields: Record<string, JsonValue>): boolean {
  return apply([{ entity: "project", id, fields }]);
}

// ---- Server-side actions -----------------------------------------------

export async function flame(id: string, action: "start" | "stop"): Promise<void> {
  try {
    const res = await api().flame(id, action);
    const timing = res?.timing;
    if (timing?.configured && !timing.used && !res?.unchanged) {
      // The flame changed, but Timing (the only time tracking) did not follow: say so.
      const error = typeof timing.error === "string" && timing.error ? timing.error : "nicht erreichbar.";
      toast(`${action === "start" ? "Flamme läuft" : "Flamme beendet"}, aber Timing: ${error}`, "warning", 10_000);
    } else {
      toast(action === "start" ? "Flamme gestartet." : "Flamme beendet.");
    }
  } catch (err) {
    toast(err instanceof Error ? err.message : "Flamme konnte nicht geändert werden.", "error");
  } finally {
    void getEngine().pull();
  }
}
