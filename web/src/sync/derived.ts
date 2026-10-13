// Derived data per contract 2.4 and 5: effective location via the root ancestor,
// subtask progress, status_changed_at, the 3-day hiding of closed tasks, task
// references and the blocking relations in both directions.

import { hlcWallMs } from "./hlc";
import { defaultRank } from "./rank";
import type { Location, Project, Task, TaskStatus } from "./types";

export const HIDE_CLOSED_AFTER_MS = 3 * 24 * 60 * 60 * 1000;
const MAX_DEPTH = 256;

export type TaskLookup = (id: string) => Task | undefined;

export function isClosed(status: TaskStatus): boolean {
  return status === "done" || status === "dropped";
}

/** Root ancestor of a task (itself when it has no resolvable parent). Cycles stop the walk. */
export function rootOf(task: Task, lookup: TaskLookup): Task {
  let current = task;
  const seen = new Set<string>([task.id]);
  for (let depth = 0; depth < MAX_DEPTH && current.parent_id; depth++) {
    const parent = lookup(current.parent_id);
    if (!parent || seen.has(parent.id)) break;
    seen.add(parent.id);
    current = parent;
  }
  return current;
}

/** The location that counts: a subtask's own location is ignored in favor of its root's. */
export function effectiveLocation(task: Task, lookup: TaskLookup): Location {
  return rootOf(task, lookup).location;
}

/** Deleted itself or below a deleted ancestor. */
export function isEffectivelyDeleted(task: Task, lookup: TaskLookup): boolean {
  let current: Task | undefined = task;
  const seen = new Set<string>();
  for (let depth = 0; current && depth < MAX_DEPTH; depth++) {
    if (current.deleted) return true;
    if (!current.parent_id || seen.has(current.id)) return false;
    seen.add(current.id);
    current = lookup(current.parent_id);
  }
  return false;
}

export function createdMs(task: { created_hlc: string }): number {
  return hlcWallMs(task.created_hlc);
}

/** Wall-clock ms of the winning status op (creation time when never set). */
export function statusChangedMs(task: Task): number {
  return hlcWallMs(task.status_hlc);
}

export function isHiddenByAge(task: Task, nowMs: number): boolean {
  if (!isClosed(task.status)) return false;
  const changed = statusChangedMs(task);
  return Number.isFinite(changed) && nowMs - changed > HIDE_CLOSED_AFTER_MS;
}

export interface Progress {
  done: number;
  total: number;
}

/** Progress over direct, non-deleted children; dropped children do not count. */
export function progressOf(children: readonly Task[]): Progress {
  let done = 0;
  let total = 0;
  for (const c of children) {
    if (c.deleted || c.status === "dropped") continue;
    total += 1;
    if (c.status === "done") done += 1;
  }
  return { done, total };
}

export function hasOpenChildren(children: readonly Task[]): boolean {
  return children.some((c) => !c.deleted && !isClosed(c.status));
}

/** Open tasks first, then done and dropped ones; rank order is kept within each group. */
export function closedLast(tasks: readonly Task[]): Task[] {
  return [...tasks.filter((t) => !isClosed(t.status)), ...tasks.filter((t) => isClosed(t.status))];
}

export function sortKey(task: Task): string {
  return task.rank ?? defaultRank(createdMs(task));
}

export function compareTasks(a: Task, b: Task): number {
  const ka = sortKey(a);
  const kb = sortKey(b);
  if (ka !== kb) return ka < kb ? -1 : 1;
  // Same default rank (same millisecond): creation order, IDs are random.
  if (a.created_hlc !== b.created_hlc) return a.created_hlc < b.created_hlc ? -1 : 1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

export function isFlameRunning(task: Task): boolean {
  if (!task.flame_started_at) return false;
  if (!task.flame_ended_at) return true;
  return Date.parse(task.flame_ended_at) < Date.parse(task.flame_started_at);
}

export function isClaimActive(task: Task, nowMs: number): boolean {
  if (!task.claimed_by) return false;
  if (!task.claim_expires_at) return true;
  const expires = Date.parse(task.claim_expires_at);
  return Number.isNaN(expires) || expires > nowMs;
}

export function isDevLocation(location: Location, projectLookup: (id: string) => Project | undefined): boolean {
  if (location.kind !== "project") return false;
  const project = projectLookup(location.id);
  return !!project && !project.deleted && project.kind === "dev";
}

export function sameLocation(a: Location, b: Location): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === "area" || a.kind === "project") return a.id === (b as { id: string }).id;
  return true;
}

/**
 * Index over all tasks: children lists (sorted), effective locations, deletion
 * state, references and blocking relations, computed eagerly once per state change
 * (so a reactive wrapper tracks every field it depends on).
 */
export class TaskIndex {
  readonly byId = new Map<string, Task>();
  private readonly children = new Map<string, Task[]>();
  private readonly effective = new Map<string, Location>();
  private readonly deleted = new Map<string, boolean>();
  private readonly refs = new Map<string, Task>();
  private readonly waiting = new Map<string, Task[]>();

  constructor(tasks: Iterable<Task>) {
    for (const t of tasks) this.byId.set(t.id, t);
    for (const t of this.byId.values()) {
      if (!t.parent_id || !this.byId.has(t.parent_id)) continue;
      let list = this.children.get(t.parent_id);
      if (!list) this.children.set(t.parent_id, (list = []));
      list.push(t);
    }
    for (const list of this.children.values()) list.sort(compareTasks);
    for (const t of this.byId.values()) {
      const loc = effectiveLocation(t, this.lookup);
      this.effective.set(t.id, loc.kind === "area" || loc.kind === "project" ? { kind: loc.kind, id: loc.id } : { kind: loc.kind });
      this.deleted.set(t.id, isEffectivelyDeleted(t, this.lookup));
    }
    for (const t of this.byId.values()) {
      if (this.deleted.get(t.id)) continue;
      if (t.ref) this.refs.set(t.ref, t);
      for (const b of t.blocked_by) {
        if (!this.isLive(b)) continue;
        let list = this.waiting.get(b);
        if (!list) this.waiting.set(b, (list = []));
        list.push(t);
      }
    }
    for (const list of this.waiting.values()) list.sort(compareTasks);
  }

  private isLive(id: string): boolean {
    return this.byId.has(id) && !this.deleted.get(id);
  }

  /** Live task with this reference (e.g. "ZUN-42"; case sensitive, references are upper case). */
  byRef(ref: string): Task | undefined {
    return this.refs.get(ref);
  }

  /** Live tasks this one waits for (`blocked_by` in stored order; deleted ones no longer block). */
  blockersOf(task: Task): Task[] {
    return task.blocked_by.filter((id) => this.isLive(id)).map((id) => this.byId.get(id)!);
  }

  /** Blockers that are neither done nor dropped: the task cannot start yet. */
  openBlockersOf(task: Task): Task[] {
    return this.blockersOf(task).filter((b) => !isClosed(b.status));
  }

  /** Live tasks waiting for this one (the reverse of their `blocked_by`), in rank order. */
  blockedBy(id: string): Task[] {
    return this.waiting.get(id) ?? [];
  }

  readonly lookup: TaskLookup = (id) => this.byId.get(id);

  /** Direct, non-deleted children in rank order. */
  childrenOf(id: string): Task[] {
    return (this.children.get(id) ?? []).filter((t) => !t.deleted);
  }

  /** All descendants (depth first), cycle safe. */
  descendantsOf(id: string): Task[] {
    const out: Task[] = [];
    const seen = new Set<string>([id]);
    const walk = (parent: string) => {
      for (const c of this.children.get(parent) ?? []) {
        if (seen.has(c.id)) continue;
        seen.add(c.id);
        out.push(c);
        walk(c.id);
      }
    };
    walk(id);
    return out;
  }

  location(task: Task): Location {
    return this.effective.get(task.id) ?? effectiveLocation(task, this.lookup);
  }

  isDeleted(task: Task): boolean {
    return this.deleted.get(task.id) ?? isEffectivelyDeleted(task, this.lookup);
  }

  progress(id: string): Progress {
    return progressOf(this.children.get(id) ?? []);
  }

  /** Visible in default views: not deleted, not closed for more than 3 days. */
  isVisible(task: Task, nowMs: number): boolean {
    return !this.isDeleted(task) && !isHiddenByAge(task, nowMs);
  }

  isRoot(task: Task): boolean {
    return !task.parent_id || !this.byId.has(task.parent_id);
  }
}
