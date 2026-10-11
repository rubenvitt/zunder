// User intents → sync engine changes. Every mutation goes through engine.change,
// which validates client-side, applies optimistically and queues the push.

import { hasOpenChildren, isClosed, sortKey } from "../sync/derived";
import type { Change } from "../sync/engine";
import { rankBetween } from "../sync/rank";
import type { JsonValue, Location, ProjectKind, Task, TaskStatus } from "../sync/types";
import { uuidv7 } from "../sync/uuid";
import { STATUS_LABELS } from "../labels";
import { api, data, getEngine, taskIndex } from "./store";
import { toast } from "./ui";

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
}

/** Creates a task (one push group); returns its id or null when refused. */
export function createTask(input: NewTask): string | null {
  const id = uuidv7();
  const fields: Record<string, JsonValue> = { title: input.title.trim() };
  if (input.location && input.location.kind !== "inbox") fields.location = locationValue(input.location);
  if (input.parent_id) fields.parent_id = input.parent_id;
  if (input.due) fields.due = input.due;
  if (input.remind_at) fields.remind_at = input.remind_at;
  if (input.status && input.status !== "open") fields.status = input.status;
  if (input.rank) fields.rank = input.rank;
  return apply([{ entity: "task", id, fields }]) ? id : null;
}

export function updateTask(id: string, fields: Record<string, JsonValue>): boolean {
  return apply([{ entity: "task", id, fields }]);
}

export function setStatus(id: string, status: TaskStatus): boolean {
  const task = data.tasks[id];
  if (!task || task.status === status) return true;
  const ok = updateTask(id, { status });
  if (ok && status === "done") warnIfOpenChildren(id);
  return ok;
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

/** Tombstones a task with all its descendants in one push group. */
export function deleteTask(id: string): boolean {
  const ids = [id, ...taskIndex().descendantsOf(id).map((t) => t.id)];
  return apply(ids.map((tid) => ({ entity: "task", id: tid, fields: { deleted: true } })));
}

/** Places `id` between `before` and `after` (neighbors in the target order) and optionally sets fields. */
export function placeTask(id: string, before: Task | null, after: Task | null, fields: Record<string, JsonValue> = {}): boolean {
  const rank = rankBetween(before ? sortKey(before) : null, after ? sortKey(after) : null);
  return updateTask(id, { ...fields, rank });
}

export function shorten(text: string, max = 40): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

// ---- Areas and projects -----------------------------------------------

export function createArea(name: string): string | null {
  const id = uuidv7();
  return apply([{ entity: "area", id, fields: { name: name.trim() } }]) ? id : null;
}

export function updateArea(id: string, fields: Record<string, JsonValue>): boolean {
  return apply([{ entity: "area", id, fields }]);
}

export function createProject(name: string, areaId: string, kind: ProjectKind = "standard", repo?: string | null): string | null {
  const id = uuidv7();
  const fields: Record<string, JsonValue> = { name: name.trim(), area_id: areaId };
  if (kind !== "standard") fields.kind = kind;
  if (kind === "dev" && repo) fields.repo = repo;
  return apply([{ entity: "project", id, fields }]) ? id : null;
}

export function updateProject(id: string, fields: Record<string, JsonValue>): boolean {
  return apply([{ entity: "project", id, fields }]);
}

// ---- Server-side actions -----------------------------------------------

export async function flame(id: string, action: "start" | "stop"): Promise<void> {
  try {
    await api().flame(id, action);
    toast(action === "start" ? "Flamme gestartet." : "Flamme beendet.");
  } catch (err) {
    toast(err instanceof Error ? err.message : "Flamme konnte nicht geändert werden.", "error");
  } finally {
    void getEngine().pull();
  }
}
