// Planung: Heute as one list, input per goal and week and the Logbuch "Erledigt" (design 5.1/5.2,
// 6.1 and 7). Same rules as zunder_core::planning, checked by the shared vectors (kinds
// "today_list", "goal_input", "done_log"):
// - live = not deleted (itself or an ancestor); open statuses are open, in_progress, blocked
//   and review;
// - a flame runs while flame_started_at is set and flame_ended_at is empty or older
//   (isFlameRunning) and counts until `nowMs`;
// - a flame counts whole minutes (rounded down per flame) for the local calendar day of its
//   start, even when it ends after midnight;
// - the effective goal is the task's own goal_id, otherwise its root's; a deleted goal counts
//   as none (a deleted own goal falls back to the root's).

import { addDays, dateFromLocal, localDate, parseDue } from "./dates";
import { mondayOf } from "./balance";
import { compareTasks, isClosed, isDevLocation, isFlameRunning, rootOf, statusChangedMs, weightRank, type TaskIndex } from "./derived";
import type { Goal, Project, Task } from "./types";

const MINUTE_MS = 60_000;

/**
 * The effective goal of a task: its own goal, otherwise its root's. A deleted (or unknown) goal
 * counts as none, so a subtask whose own goal is deleted counts for its root's goal.
 */
export function effectiveGoal(index: TaskIndex, goals: Record<string, Goal>, task: Task): Goal | null {
  const live = (id: string | null) => {
    const goal = id ? goals[id] : undefined;
    return goal && !goal.deleted ? goal : null;
  };
  return live(task.goal_id) ?? live(rootOf(task, index.lookup).goal_id);
}

/** Whole minutes from `startMs` to `endMs` (rounded down, never negative; 0 when unreadable). */
function wholeMinutes(startMs: number, endMs: number): number {
  const minutes = Math.floor((endMs - startMs) / MINUTE_MS);
  return Number.isFinite(minutes) && minutes > 0 ? minutes : 0;
}

/** The flames of a task as local start day and whole minutes: flame_log, then the running one. */
function flamesOf(task: Task, nowMs: number): { day: string; minutes: number }[] {
  const out: { day: string; minutes: number }[] = [];
  const add = (start: number, end: number) => {
    if (!Number.isNaN(start)) out.push({ day: localDate(new Date(start)), minutes: wholeMinutes(start, end) });
  };
  for (const f of task.flame_log) add(Date.parse(f.start), Date.parse(f.end));
  if (isFlameRunning(task)) add(Date.parse(task.flame_started_at!), nowMs);
  return out;
}

function compareStrings(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Present values first (in string order), missing ones last. */
function presentFirst(a: string | null, b: string | null): number {
  if (a === b) return 0;
  if (a === null) return 1;
  if (b === null) return -1;
  return compareStrings(a, b);
}

/** Creation order: smallest HLC, then ID. */
function byCreation(a: Task, b: Task): number {
  return compareStrings(a.created_hlc, b.created_hlc) || compareStrings(a.id, b.id);
}

function byDayRank(a: Task, b: Task): number {
  return presentFirst(a.day_rank, b.day_rank) || byCreation(a, b);
}

// ---- Heute ---------------------------------------------------------------------------------

export interface TodayList {
  /** Planned for today, then left over from earlier days, then due by today without a plan. */
  items: Task[];
  /** The running flame, otherwise (unless the plan is done) the first item that is open or in progress and waits on nobody. */
  focus: Task | null;
  /** A live task of any status is planned for today. */
  has_plan: boolean;
  /** There is a plan and none of its tasks is open any more. */
  plan_done: boolean;
}

/**
 * Heute for the local date `today` (design 5.1/5.2). Candidates are live tasks with an open
 * status (subtasks and dev projects included):
 * 1. planned_on = today, by day_rank (missing last), creation, ID;
 * 2. planned_on < today, by planned_on, day_rank, creation, ID;
 * 3. no planned_on and due by today, by weight (Brand, Glut, none, Rauch, Asche), due instant
 *    (a date without time is due at local midnight), then compareTasks (rank, else the default
 *    rank of the creation time, creation, ID): as byDue in state/selectors.
 */
export function todayList(index: TaskIndex, today: string): TodayList {
  const planned: Task[] = [];
  const leftOver: Task[] = [];
  const due: { task: Task; at: number }[] = [];
  let hasPlan = false;
  let planOpen = false;
  // The running flame; of several, the latest started, then the smallest ID.
  let flame: { task: Task; started: number } | null = null;
  for (const t of index.byId.values()) {
    if (index.isDeleted(t)) continue;
    if (isFlameRunning(t)) {
      const started = Date.parse(t.flame_started_at!);
      if (!flame || started > flame.started || (started === flame.started && t.id < flame.task.id)) flame = { task: t, started };
    }
    const open = !isClosed(t.status);
    if (t.planned_on === today) {
      hasPlan = true;
      if (open) planOpen = true;
    }
    if (!open) continue;
    if (t.planned_on === today) planned.push(t);
    else if (t.planned_on !== null) {
      if (t.planned_on < today) leftOver.push(t);
    } else {
      const info = parseDue(t.due);
      if (info && info.date <= today) due.push({ task: t, at: info.at.getTime() });
    }
  }
  planned.sort(byDayRank);
  leftOver.sort((a, b) => compareStrings(a.planned_on!, b.planned_on!) || byDayRank(a, b));
  due.sort((a, b) => weightRank(a.task.weight) - weightRank(b.task.weight) || a.at - b.at || compareTasks(a.task, b.task));
  const items = [...planned, ...leftOver, ...due.map((d) => d.task)];
  const planDone = hasPlan && !planOpen;
  const focus =
    flame?.task ?? (planDone ? null : (items.find((t) => (t.status === "open" || t.status === "in_progress") && !t.waiting_on) ?? null));
  return { items, focus, has_plan: hasPlan, plan_done: planDone };
}

// ---- Ziele ---------------------------------------------------------------------------------

export interface GoalInput {
  goal: Goal;
  /** Done tasks with this effective goal ("Züge"). */
  done: number;
  /** Flame minutes on tasks with this effective goal. */
  minutes: number;
  lead_per_week: number | null;
  /** lead_per_week is set and done reaches it. */
  reached: boolean;
}

export interface WeekGoalInput {
  /** ISO week, e.g. "2026-W48". */
  week: string;
  /** Monday of the week (YYYY-MM-DD). */
  from: string;
  /** Sunday of the week (YYYY-MM-DD). */
  to: string;
  /** Live active goals by rank (missing last), then name, then ID. */
  goals: GoalInput[];
}

/**
 * Input of every live active goal in `week` (design 6.1): `done` counts live done tasks whose
 * status changed in the week and whose effective goal it is (subtasks one by one, dev projects
 * included); `minutes` sums the flames of live tasks with this effective goal (any status) that
 * started in the week, the running one until `nowMs`.
 */
/** Goal order everywhere: rank (missing last), then name, then ID. */
export function compareGoals(a: Goal, b: Goal): number {
  return presentFirst(a.rank, b.rank) || compareStrings(a.name, b.name) || compareStrings(a.id, b.id);
}

export function goalInput(index: TaskIndex, goals: Record<string, Goal>, week: string, nowMs: number): WeekGoalInput {
  const monday = mondayOf(week);
  const from = monday ? localDate(monday) : "";
  const to = monday ? localDate(addDays(monday, 6)) : "";
  const active = Object.values(goals)
    .filter((g) => !g.deleted && g.state === "active")
    .sort(compareGoals);
  const rows = active.map((goal): GoalInput => ({ goal, done: 0, minutes: 0, lead_per_week: goal.lead_per_week, reached: false }));
  const rowOf = new Map(rows.map((r) => [r.goal.id, r]));
  if (monday) {
    const start = dateFromLocal(from)!.getTime();
    // Local midnight a week later: 167 or 169 hours when the clocks change in between.
    const end = addDays(dateFromLocal(from)!, 7).getTime();
    for (const t of index.byId.values()) {
      if (index.isDeleted(t)) continue;
      const goal = effectiveGoal(index, goals, t);
      const row = goal ? rowOf.get(goal.id) : undefined;
      if (!row) continue;
      if (t.status === "done") {
        const changed = statusChangedMs(t);
        if (changed >= start && changed < end) row.done += 1;
      }
      for (const f of flamesOf(t, nowMs)) if (f.day >= from && f.day <= to) row.minutes += f.minutes;
    }
  }
  for (const r of rows) r.reached = r.lead_per_week !== null && r.done >= r.lead_per_week;
  return { week, from, to, goals: rows };
}

// ---- Logbuch -------------------------------------------------------------------------------

export interface LogEntry {
  task: Task;
  /** Flame minutes of the task on that day. */
  minutes: number;
}

export interface LogDay {
  /** Local date YYYY-MM-DD. */
  date: string;
  /** Flame minutes of both lists. */
  minutes: number;
  /** Done that day, the latest first. */
  done: LogEntry[];
  /** Flames that day but not done that day, most minutes first, then ID. */
  worked: LogEntry[];
}

/**
 * The Logbuch from `to` back to `from` (both YYYY-MM-DD, included; design 7); days without
 * entries are left out, tasks in dev projects (`projects` by ID) never appear. `done`: live done
 * tasks whose status changed that day, with their flame minutes of that day; `worked`: live
 * tasks (not dropped) with flame minutes that day that are not in that day's `done`.
 */
export function doneLog(index: TaskIndex, projects: Record<string, Project>, from: string, to: string, nowMs: number): LogDay[] {
  const days = new Map<string, { done: LogEntry[]; worked: LogEntry[] }>();
  const dayOf = (date: string) => {
    let lists = days.get(date);
    if (!lists) days.set(date, (lists = { done: [], worked: [] }));
    return lists;
  };
  const inRange = (day: string) => day >= from && day <= to;
  for (const t of index.byId.values()) {
    if (t.status === "dropped" || index.isDeleted(t) || isDevLocation(index.location(t), (id) => projects[id])) continue;
    const perDay = new Map<string, number>();
    for (const f of flamesOf(t, nowMs)) if (inRange(f.day)) perDay.set(f.day, (perDay.get(f.day) ?? 0) + f.minutes);
    let doneDay: string | null = null;
    if (t.status === "done") {
      const changed = statusChangedMs(t);
      const day = Number.isNaN(changed) ? null : localDate(new Date(changed));
      if (day && inRange(day)) {
        doneDay = day;
        dayOf(day).done.push({ task: t, minutes: perDay.get(day) ?? 0 });
      }
    }
    for (const [day, minutes] of perDay) if (minutes > 0 && day !== doneDay) dayOf(day).worked.push({ task: t, minutes });
  }
  return [...days.entries()]
    .sort(([a], [b]) => compareStrings(b, a))
    .map(([date, lists]) => {
      lists.done.sort((a, b) => compareStrings(b.task.status_hlc, a.task.status_hlc) || compareStrings(a.task.id, b.task.id));
      lists.worked.sort((a, b) => b.minutes - a.minutes || compareStrings(a.task.id, b.task.id));
      const minutes = [...lists.done, ...lists.worked].reduce((sum, e) => sum + e.minutes, 0);
      return { date, minutes, done: lists.done, worked: lists.worked };
    });
}

export interface GoalWeekTasks {
  /** Done in the week, the latest first. */
  done: LogEntry[];
  /** Flames in the week but not done in it, most minutes first. */
  worked: LogEntry[];
}

/**
 * The tasks of one goal in a week (what `goalInput` counts for it): live tasks whose effective
 * goal is `goalId` and that were done in the week (with their minutes of the week), and those
 * with flame minutes in the week that were not.
 */
export function goalWeekTasks(index: TaskIndex, goals: Record<string, Goal>, goalId: string, week: string, nowMs: number): GoalWeekTasks {
  const monday = mondayOf(week);
  if (!monday) return { done: [], worked: [] };
  const from = localDate(monday);
  const to = localDate(addDays(monday, 6));
  const start = dateFromLocal(from)!.getTime();
  const end = addDays(dateFromLocal(from)!, 7).getTime();
  const done: LogEntry[] = [];
  const worked: LogEntry[] = [];
  for (const t of index.byId.values()) {
    if (index.isDeleted(t) || effectiveGoal(index, goals, t)?.id !== goalId) continue;
    let minutes = 0;
    for (const f of flamesOf(t, nowMs)) if (f.day >= from && f.day <= to) minutes += f.minutes;
    const changed = t.status === "done" ? statusChangedMs(t) : NaN;
    if (changed >= start && changed < end) done.push({ task: t, minutes });
    else if (minutes > 0) worked.push({ task: t, minutes });
  }
  done.sort((a, b) => compareStrings(b.task.status_hlc, a.task.status_hlc) || compareStrings(a.task.id, b.task.id));
  worked.sort((a, b) => b.minutes - a.minutes || compareStrings(a.task.id, b.task.id));
  return { done, worked };
}

/** The active goal whose `ends_on` is the latest and not past `today` (its `storm` names the Feuersturm). */
export function stormGoal(goals: Record<string, Goal>, today: Date): Goal | null {
  const todayDate = localDate(today);
  let latest: Goal | null = null;
  for (const g of Object.values(goals)) {
    if (g.deleted || g.state !== "active" || !g.ends_on || g.ends_on < todayDate) continue;
    if (!latest || g.ends_on > latest.ends_on! || (g.ends_on === latest.ends_on && compareGoals(g, latest) < 0)) latest = g;
  }
  return latest;
}

/**
 * Whole weeks (rounded up, at least 1) from `today` to the latest `ends_on` among the active
 * goals; null when none has an end that is still ahead (today counts).
 */
export function weeksLeft(goals: Record<string, Goal>, today: Date): number | null {
  const endDay = dateFromLocal(stormGoal(goals, today)?.ends_on ?? "");
  if (!endDay) return null;
  const days = Math.round((endDay.getTime() - dateFromLocal(localDate(today))!.getTime()) / 86_400_000);
  return Math.max(1, Math.ceil(days / 7));
}
