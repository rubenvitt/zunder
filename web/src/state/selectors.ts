// Task selections for the views. All functions are reactive (read the store).

import { localDate, parseDue } from "../sync/dates";
import { compareTasks, isClosed, isFlameRunning, statusChangedMs, weightRank } from "../sync/derived";
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

export function devTasks(): Task[] {
  const ids = devProjectIds();
  return visibleRoots((t) => {
    const loc = taskIndex().location(t);
    return loc.kind === "project" && ids.has(loc.id);
  });
}

export interface TodayGroups {
  overdue: Task[];
  dueToday: Task[];
  reminders: Task[];
  flames: Task[];
}

/** Heute: overdue and due today, today's reminders, running flames (subtasks included). */
export function todayGroups(): TodayGroups {
  const index = taskIndex();
  const nowMs = now();
  const today = localDate(new Date(nowMs));
  const groups: TodayGroups = { overdue: [], dueToday: [], reminders: [], flames: [] };
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
      if (!Number.isNaN(at.getTime()) && localDate(at) === today) groups.reminders.push(t);
    }
  }
  // What matters first (Brand, Glut, unweighed, Rauch, Asche), then by due time.
  const byDue = (a: Task, b: Task) =>
    weightRank(a.weight) - weightRank(b.weight) || (parseDue(a.due)?.at.getTime() ?? 0) - (parseDue(b.due)?.at.getTime() ?? 0) || compareTasks(a, b);
  groups.overdue.sort(byDue);
  groups.dueToday.sort(byDue);
  groups.reminders.sort((a, b) => Date.parse(a.remind_at!) - Date.parse(b.remind_at!));
  return groups;
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
