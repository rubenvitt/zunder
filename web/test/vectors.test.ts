// Shared test vectors (contract 2.6) produced by zunder-core. The Rust and the
// TypeScript merge must agree on every file in crates/zunder-core/tests/vectors.
// The calendar of follow-up instances (next_occurrence) has its only TypeScript
// copy in the mock server (e2e/mock-recurrence.ts).

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { weekBalance, type WeekBalance } from "../src/sync/balance";
import { deferFields } from "../src/sync/defer";
import { doneLog, goalInput, todayList } from "../src/sync/planning";
import { isDevLocation, TaskIndex } from "../src/sync/derived";
import { compareHlc, isValidHlc } from "../src/sync/hlc";
import { toGoal, toProject, toTask } from "../src/sync/model";
import { mergeOps } from "../src/sync/merge";
import { validateOps } from "../src/sync/validate";
import type { Goal, JsonValue, Op, Project, Task } from "../src/sync/types";
import { nextOccurrence, type MockNext, type MockRecurrence } from "../e2e/mock-recurrence";
import { permutations, shuffled } from "./helpers";

const here = dirname(fileURLToPath(import.meta.url));
const VECTOR_DIR = resolve(process.env.ZUNDER_VECTOR_DIR ?? resolve(here, "../../crates/zunder-core/tests/vectors"));

interface MergeVector {
  name: string;
  kind: "merge";
  ops: Op[];
  expected: Record<string, Record<string, JsonValue>>;
}

interface HlcOrderVector {
  name: string;
  kind: "hlc_order";
  sorted: string[];
}

interface ValidateVector {
  name: string;
  kind: "validate";
  now_ms: number;
  state: Op[];
  push: Op[];
  /** entity id → code of the cause; empty = everything accepted. */
  expected_rejected: Record<string, string>;
}

interface WeekBalanceVector {
  name: string;
  kind: "week_balance";
  /** The zone the expected counts hold in; the tests run in it (vite.config.ts, TZ). */
  time_zone: string;
  week: string;
  state: Op[];
  expected: WeekBalance;
}

interface NextOccurrenceVector {
  name: string;
  kind: "next_occurrence";
  /** The zone the cases are computed in; the tests run in it (vite.config.ts, TZ). */
  time_zone: string;
  cases: {
    description: string;
    recurrence: MockRecurrence;
    due: string;
    remind_at?: string;
    /** RFC 3339: when the instance was completed (from done: its local date is the anchor). */
    completed_at: string;
    /** Local date the server handles the push on. */
    today: string;
    /** Next instance; null when none is created (beyond the year 9999). */
    expected: MockNext | null;
  }[];
}

/**
 * Planning vectors (zunder_core::planning, sync/planning): per case its own `state` (merged in
 * every order), the inputs and `expected` in the core's shape, which names tasks and goals by ID.
 */
interface PlanningVector<K extends string, C> {
  name: string;
  kind: K;
  /** The zone the cases hold in; the tests run in it (vite.config.ts, TZ). */
  time_zone: string;
  cases: ({ description: string; state: Op[] } & C)[];
}

type TodayListVector = PlanningVector<
  "today_list",
  { today: string; expected: { items: string[]; focus: string | null; has_plan: boolean; plan_done: boolean } }
>;

type GoalInputVector = PlanningVector<
  "goal_input",
  {
    /** RFC 3339: a running flame counts until then. */
    now: string;
    week: string;
    expected: {
      week: string;
      from: string;
      to: string;
      goals: { goal_id: string; done: number; minutes: number; lead_per_week: number | null; reached: boolean }[];
    };
  }
>;

interface LogEntryIds {
  task_id: string;
  minutes: number;
}

type DoneLogVector = PlanningVector<
  "done_log",
  {
    /** RFC 3339: a running flame counts until then. */
    now: string;
    from: string;
    to: string;
    expected: { date: string; minutes: number; done: LogEntryIds[]; worked: LogEntryIds[] }[];
  }
>;

/** "Heute nicht" (zunder_core::defer::plan, sync/defer deferFields): new values, null = stays. */
interface DeferVector {
  name: string;
  kind: "defer";
  cases: {
    description: string;
    day: string;
    task_id: string;
    state: Op[];
    expected: { due: string | null; remind_at: string | null; planned_on: string | null; deferrals: number } | null;
  }[];
}

type Vector =
  | DeferVector
  | MergeVector
  | HlcOrderVector
  | ValidateVector
  | WeekBalanceVector
  | NextOccurrenceVector
  | TodayListVector
  | GoalInputVector
  | DoneLogVector;

/** Noon of a local date "YYYY-MM-DD" in the process zone, as Unix milliseconds. */
function localNoon(date: string): number {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(y!, m! - 1, d!, 12).getTime();
}

/** Balance as the web UI computes it from a merged state (deleted tasks and dev projects excluded). */
function balanceOf(ops: Op[], week: string): WeekBalance {
  const store = mergeOps(ops);
  const tasks: Task[] = [];
  const projects = new Map<string, Project>();
  for (const e of store.values()) {
    if (e.entity === "task") tasks.push(toTask(e));
    else if (e.entity === "project") projects.set(e.id, toProject(e));
  }
  const index = new TaskIndex(tasks);
  return weekBalance(index.byId.values(), week, (t) => index.isDeleted(t) || isDevLocation(index.location(t), (id) => projects.get(id)));
}

/** What the planning rules read from a merged state. */
function planningInput(ops: Op[]): { index: TaskIndex; goals: Record<string, Goal>; projects: Record<string, Project> } {
  const tasks: Task[] = [];
  const goals: Record<string, Goal> = {};
  const projects: Record<string, Project> = {};
  for (const e of mergeOps(ops).values()) {
    if (e.entity === "task") tasks.push(toTask(e));
    else if (e.entity === "goal") goals[e.id] = toGoal(e);
    else if (e.entity === "project") projects[e.id] = toProject(e);
  }
  return { index: new TaskIndex(tasks), goals, projects };
}

/** Runs the cases of a planning vector in every order of their state. */
function checkPlanning<C extends { description: string; state: Op[]; expected: unknown }>(
  vector: { time_zone: string; cases: C[] },
  run: (c: C, ops: Op[]) => unknown,
): void {
  expect(Intl.DateTimeFormat().resolvedOptions().timeZone).toBe(vector.time_zone);
  expect(vector.cases.length).toBeGreaterThan(0);
  for (const c of vector.cases) {
    for (const order of orderings(c.state)) expect(run(c, order), c.description).toEqual(c.expected);
  }
}

function nowOf(c: { description: string; now: string }): number {
  const now = Date.parse(c.now);
  expect(Number.isNaN(now), c.description).toBe(false);
  return now;
}

function orderings<T>(items: readonly T[]): T[][] {
  const result = [items.slice(), items.slice().reverse()];
  if (items.length <= 6) result.push(...permutations(items, 720));
  else for (let seed = 1; seed <= 25; seed++) result.push(shuffled(items, seed));
  return result;
}

function canonical(value: unknown): string {
  return JSON.stringify(value, (_key, v: unknown) =>
    v && typeof v === "object" && !Array.isArray(v)
      ? Object.fromEntries(Object.entries(v as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
      : v,
  );
}

const files = existsSync(VECTOR_DIR) ? readdirSync(VECTOR_DIR).filter((f) => f.endsWith(".json")).sort() : [];

describe("shared test vectors", () => {
  it(`finds the vectors in ${VECTOR_DIR}`, () => {
    // The vectors are part of the repository (zunder-core); a missing directory is an error, not a skip.
    expect(files.length).toBeGreaterThan(0);
  });

  for (const file of files) {
    const vector = JSON.parse(readFileSync(join(VECTOR_DIR, file), "utf8")) as Vector;
    it(`${file}: ${vector.name}`, () => {
      expect(file).toContain(vector.name);
      if (vector.kind === "hlc_order") {
        for (const hlc of vector.sorted) expect(isValidHlc(hlc), hlc).toBe(true);
        for (const order of orderings(vector.sorted)) expect([...order].sort(compareHlc)).toEqual(vector.sorted);
        return;
      }
      if (vector.kind === "validate") {
        // The client refuses a commit as a whole and names the cause per entity, which must match
        // the core's cause per rejected entity.
        const issues = validateOps(mergeOps(vector.state), vector.push, vector.now_ms);
        const got = Object.fromEntries(issues.map((i) => [i.entity_id, i.code]));
        expect(issues).toHaveLength(Object.keys(got).length);
        expect(got).toEqual(vector.expected_rejected);
        return;
      }
      if (vector.kind === "week_balance") {
        expect(Intl.DateTimeFormat().resolvedOptions().timeZone).toBe(vector.time_zone);
        for (const order of orderings(vector.state)) expect(balanceOf(order, vector.week)).toEqual(vector.expected);
        return;
      }
      if (vector.kind === "next_occurrence") {
        expect(Intl.DateTimeFormat().resolvedOptions().timeZone).toBe(vector.time_zone);
        expect(vector.cases.length).toBeGreaterThan(0);
        for (const c of vector.cases) {
          const completed = Date.parse(c.completed_at);
          expect(Number.isNaN(completed), c.description).toBe(false);
          const got = nextOccurrence(c.recurrence, c.due, c.remind_at ?? null, completed, localNoon(c.today));
          expect(got, c.description).toEqual(c.expected);
        }
        return;
      }
      if (vector.kind === "today_list") {
        checkPlanning(vector, (c, ops) => {
          const got = todayList(planningInput(ops).index, c.today);
          return { items: got.items.map((t) => t.id), focus: got.focus?.id ?? null, has_plan: got.has_plan, plan_done: got.plan_done };
        });
        return;
      }
      if (vector.kind === "defer") {
        expect(vector.cases.length).toBeGreaterThan(0);
        for (const c of vector.cases) {
          for (const order of orderings(c.state)) {
            const task = planningInput(order).index.byId.get(c.task_id);
            expect(task, c.description).toBeDefined();
            const f = deferFields(task!, c.day);
            const got = f && { due: f.due ?? null, remind_at: f.remind_at ?? null, planned_on: f.planned_on ?? null, deferrals: f.deferrals };
            expect(got, c.description).toEqual(c.expected);
          }
        }
        return;
      }
      if (vector.kind === "goal_input") {
        checkPlanning(vector, (c, ops) => {
          const { index, goals } = planningInput(ops);
          const got = goalInput(index, goals, c.week, nowOf(c));
          return { ...got, goals: got.goals.map(({ goal, ...row }) => ({ goal_id: goal.id, ...row })) };
        });
        return;
      }
      if (vector.kind === "done_log") {
        checkPlanning(vector, (c, ops) => {
          const { index, projects } = planningInput(ops);
          const ids = (entries: { task: Task; minutes: number }[]) => entries.map((e) => ({ task_id: e.task.id, minutes: e.minutes }));
          return doneLog(index, projects, c.from, c.to, nowOf(c)).map((d) => ({ ...d, done: ids(d.done), worked: ids(d.worked) }));
        });
        return;
      }
      expect(vector.kind).toBe("merge");
      const expected = canonical(vector.expected);
      for (const order of orderings(vector.ops)) {
        expect(canonical(mergeOps(order).toValues())).toBe(expected);
      }
    });
  }
});
