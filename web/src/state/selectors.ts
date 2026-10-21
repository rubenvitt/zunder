// Task selections for the views. All functions are reactive (read the store).

import { localDate, parseDue } from "../sync/dates";
import { weekBalance, type WeekBalance } from "../sync/balance";
import { openCommitments } from "../sync/commitments";
import { pickFocus, type Focus } from "../sync/focus";
import { awaitsAnswer } from "../sync/handback";
import { compareTasks, isClosed, isDevLocation, isFlameRunning, isHiddenByAge, statusChangedMs, weightRank } from "../sync/derived";
import type { Location, Task } from "../sync/types";
import { data, now, taskIndex } from "./store";

/** Open tasks in rank order, then recently closed ones (newest first). */
export function sortForList(tasks: Task[]): Task[] {
  const open = tasks.filter((t) => !isClosed(t.status)).sort(compareTasks);
  const closed = tasks.filter((t) => isClosed(t.status)).sort((a, b) => statusChangedMs(b) - statusChangedMs(a));
  return [...open, ...closed];
}

function visibleRoots(filter: (t: Task) => boolean): Task[] {
  const index = taskIndex();
  const nowMs = now();
  const out: Task[] = [];
  for (const t of index.byId.values()) {
    if (!index.isRoot(t) || !index.isVisible(t, nowMs)) continue;
    if (filter(t)) out.push(t);
  }
  return out;
}

/** Board archive: done roots past the 3-day window, most recently closed first. */
function archivedDone(filter: (t: Task) => boolean): Task[] {
  const index = taskIndex();
  const nowMs = now();
  const out: Task[] = [];
  for (const t of index.byId.values()) {
    if (t.status !== "done" || !index.isRoot(t) || index.isDeleted(t) || !isHiddenByAge(t, nowMs)) continue;
    if (filter(t)) out.push(t);
  }
  return out.sort((a, b) => statusChangedMs(b) - statusChangedMs(a));
}

function atLocation(location: Location) {
  return (t: Task) => {
    const loc = taskIndex().location(t);
    if (loc.kind !== location.kind) return false;
    return loc.kind === "inbox" || loc.kind === "loose" || (loc as { id: string }).id === (location as { id: string }).id;
  };
}

export function tasksAt(location: Location): Task[] {
  return sortForList(visibleRoots(atLocation(location)));
}

export function archivedAt(location: Location): Task[] {
  return archivedDone(atLocation(location));
}

/** Inbox wants to reach zero: closed tasks leave it immediately. */
export function inboxTasks(): Task[] {
  return visibleRoots((t) => t.location.kind === "inbox" && !isClosed(t.status)).sort(compareTasks);
}

export function devProjectIds(): Set<string> {
  const ids = new Set<string>();
  for (const p of Object.values(data.projects)) {
    if (!p.deleted && p.kind === "dev" && p.status !== "completed" && data.areas[p.area_id]?.deleted !== true) ids.add(p.id);
  }
  return ids;
}

function inDevProject(ids: Set<string>) {
  return (t: Task) => {
    const loc = taskIndex().location(t);
    return loc.kind === "project" && ids.has(loc.id);
  };
}

export function devTasks(): Task[] {
  return visibleRoots(inDevProject(devProjectIds()));
}

/** Archive of the Dev board, limited to `projectIds` when given. */
export function devArchive(projectIds?: Set<string>): Task[] {
  return archivedDone(inDevProject(projectIds ?? devProjectIds()));
}

export interface TodayGroups {
  overdue: Task[];
  dueToday: Task[];
  reminders: Task[];
  /** Today's reminders of tasks that still wait on someone (Wiedervorlage), outside dev projects. */
  followUps: Task[];
  flames: Task[];
}

/** Heute: overdue and due today, today's reminders and follow-ups, running flames (subtasks included). */
export function todayGroups(): TodayGroups {
  const index = taskIndex();
  const nowMs = now();
  const today = localDate(new Date(nowMs));
  const groups: TodayGroups = { overdue: [], dueToday: [], reminders: [], followUps: [], flames: [] };
  for (const t of index.byId.values()) {
    if (index.isDeleted(t)) continue;
    if (isFlameRunning(t)) groups.flames.push(t);
    if (isClosed(t.status)) continue;
    const due = parseDue(t.due);
    if (due) {
      if (due.date < today) {
        groups.overdue.push(t);
        continue;
      }
      if (due.date === today) {
        groups.dueToday.push(t);
        continue;
      }
    }
    if (t.remind_at) {
      const at = new Date(t.remind_at);
      if (!Number.isNaN(at.getTime()) && localDate(at) === today) {
        const dev = isDevLocation(index.location(t), (id) => data.projects[id]);
        (awaitsAnswer(t, dev) ? groups.followUps : groups.reminders).push(t);
      }
    }
  }
  // What matters first (Brand, Glut, unweighed, Rauch, Asche), then by due time.
  const byDue = (a: Task, b: Task) =>
    weightRank(a.weight) - weightRank(b.weight) || (parseDue(a.due)?.at.getTime() ?? 0) - (parseDue(b.due)?.at.getTime() ?? 0) || compareTasks(a, b);
  groups.overdue.sort(byDue);
  groups.dueToday.sort(byDue);
  const byReminder = (a: Task, b: Task) => Date.parse(a.remind_at!) - Date.parse(b.remind_at!);
  groups.reminders.sort(byReminder);
  groups.followUps.sort(byReminder);
  return groups;
}

export interface WaitingGroup {
  name: string;
  tasks: Task[];
}

/**
 * Wartet auf andere: open tasks (subtasks included) whose `waiting_on` names someone, grouped
 * by that name regardless of case and sorted by it; within a group due first, then rank.
 */
export function waitingGroups(): WaitingGroup[] {
  const index = taskIndex();
  const groups = new Map<string, WaitingGroup>();
  for (const t of index.byId.values()) {
    const name = t.waiting_on?.trim();
    if (!name || index.isDeleted(t) || isClosed(t.status)) continue;
    const key = name.toLocaleLowerCase("de");
    let group = groups.get(key);
    if (!group) groups.set(key, (group = { name, tasks: [] }));
    group.tasks.push(t);
  }
  const due = (t: Task) => parseDue(t.due)?.at.getTime() ?? Infinity;
  const out = [...groups.values()].sort((a, b) => a.name.localeCompare(b.name, "de"));
  for (const g of out) g.tasks.sort((a, b) => due(a) - due(b) || compareTasks(a, b));
  return out;
}

/**
 * Offene Zusagen: live tasks for others, not done or dropped and waiting on nobody (subtasks
 * included), outside dev projects; due first, then rank (rule in sync/commitments).
 */
export function commitmentTasks(): Task[] {
  const index = taskIndex();
  return openCommitments(index.byId.values(), (t) => index.isDeleted(t) || isDevLocation(index.location(t), (id) => data.projects[id]));
}

/** Deleted tasks and tasks in dev projects: they count neither in the balance nor in "Erst für dich". */
function notPersonal(t: Task): boolean {
  const index = taskIndex();
  return index.isDeleted(t) || isDevLocation(index.location(t), (id) => data.projects[id]);
}

/** "Erst für dich": the task at the top of Heute for `today`, or Glut tasks to pick (rule in sync/focus). */
export function focusOf(today: string, chosenId: string | null): Focus {
  return pickFocus(taskIndex().byId.values(), today, notPersonal, chosenId);
}

/** Wochenspiegel: the balance of `week` (rule in sync/balance, one pass over the tasks). */
export function weekBalanceOf(week: string): WeekBalance {
  return weekBalance(taskIndex().byId.values(), week, notPersonal);
}

export function phoenixTasks(fire: string): { steps: Task[]; flames: Task[] } {
  const index = taskIndex();
  const nowMs = now();
  const steps: Task[] = [];
  const flames: Task[] = [];
  for (const t of index.byId.values()) {
    if (!t.phoenix || t.phoenix.fire !== fire || !index.isVisible(t, nowMs)) continue;
    (t.phoenix.role === "flamme" ? flames : steps).push(t);
  }
  return { steps: sortForList(steps), flames: sortForList(flames) };
}

export function inboxCount(): number {
  return inboxTasks().length;
}
