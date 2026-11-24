// Timing hours per goal (ZUN-66), read through `GET /api/goal-hours` (browser) or the app command
// `goal_hours`. The Ziele view watches the shown week: it asks right away, every few minutes while it
// is open and when the window comes back into view.
//
// Quiet by design: without a Timing token (`not_configured`), when Timing does not answer
// (`unavailable`) or for an old server (`unsupported`), `minutes` stays `undefined` and the view shows
// nothing. On a network error the last answer for the same week stays (it is at most a few minutes old).

import { createSignal, untrack } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import type { GoalHours } from "../sync/types";
import { api } from "./store";

type Fetch = (week: string) => Promise<GoalHours>;

/** How often the watched week is asked again (the server keeps Timing's answer as long). */
export const REFRESH_MS = 5 * 60 * 1000;

export interface GoalHoursTracker {
  /** Minutes of Timing time entries for the goal in the watched week; `undefined`: none or not known. */
  minutes(goalId: string): number | undefined;
  /** Asks for `week` now and keeps it fresh until the returned function is called. */
  watch(week: string): () => void;
  dispose(): void;
}

export function createGoalHours(fetch: () => Fetch, now: () => number = Date.now): GoalHoursTracker {
  const [state, setState] = createStore<{ week: string | null; goals: Record<string, number> }>({ week: null, goals: {} });
  let watched: string | null = null;
  // The same as a signal: a view that comes back shows what is known right away.
  const [watchedWeek, setWatchedWeek] = createSignal<string | null>(null);
  let fetchedAt = 0;
  let interval: ReturnType<typeof setInterval> | null = null;
  let seq = 0;

  async function ask(week: string): Promise<void> {
    const mine = ++seq;
    try {
      const answer = await fetch()(week);
      if (mine !== seq || watched !== week) return;
      fetchedAt = now();
      setState("goals", reconcile(answer.status === "ok" ? { ...answer.goals } : {}));
      setState("week", week);
    } catch {
      // Offline, an old server or Timing unreachable: keep what is shown.
    }
  }

  const onVisible = () => {
    if (watched === null || typeof document === "undefined" || document.visibilityState !== "visible") return;
    if (now() - fetchedAt >= REFRESH_MS / 2) void ask(watched);
  };
  if (typeof document !== "undefined") document.addEventListener("visibilitychange", onVisible);

  function clear(): void {
    setState("week", null);
    setState("goals", reconcile({}));
  }

  function stop(): void {
    watched = null;
    setWatchedWeek(null);
    if (interval !== null) clearInterval(interval);
    interval = null;
  }

  return {
    minutes(goalId) {
      const week = watchedWeek();
      if (week === null || state.week !== week) return undefined;
      return state.goals[goalId];
    },
    watch(week) {
      stop();
      watched = week;
      setWatchedWeek(week);
      // Called from effects: reading the store here must not subscribe them.
      if (untrack(() => state.week) !== week) clear();
      void ask(week);
      interval = setInterval(() => {
        if (watched !== null) void ask(watched);
      }, REFRESH_MS);
      const mine = interval;
      return () => {
        if (interval === mine) stop();
      };
    },
    dispose() {
      stop();
      if (typeof document !== "undefined") document.removeEventListener("visibilitychange", onVisible);
      clear();
    },
  };
}

let shared: GoalHoursTracker | null = null;

/** The tracker the UI shares (one Ziele view at a time). */
export function goalHours(): GoalHoursTracker {
  shared ??= createGoalHours(() => (week) => api().goalHours(week));
  return shared;
}
