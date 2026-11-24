// Erledigt (design 7): the logbook. Days newest first, each with what was done and the flame
// minutes, below it what was only worked on ("Daran gearbeitet"). Computed from local state;
// the rule (no dev tasks, no dropped ones, days without entries left out) lives in doneLog.
// No judgement, no colour: a day with little is a day with little.

import { createMemo, createSignal, For, Show } from "solid-js";
import { EmptyState } from "../components/EmptyState";
import { GoalMark } from "../components/GoalMark";
import { Icon } from "../components/Icon";
import { data, now, taskIndex } from "../state/store";
import { openTask } from "../state/ui";
import { addDays, dateFromLocal, localDate } from "../sync/dates";
import { doneLog, type LogEntry } from "../sync/planning";

const STEP_DAYS = 14;

function dayLabel(date: string): string {
  const d = dateFromLocal(date);
  return d ? d.toLocaleDateString("de-DE", { weekday: "long", day: "numeric", month: "long" }) : date;
}

function Entry(props: { entry: LogEntry; worked?: boolean }) {
  return (
    <li class="log-row" data-task-id={props.entry.task.id}>
      <button class="log-open" onClick={() => openTask(props.entry.task.id)}>
        <span class="log-mark" classList={{ worked: props.worked }} aria-hidden="true">
          {props.worked ? "◐" : <Icon name="check" size={14} />}
        </span>
        <span class="log-title">{props.entry.task.title}</span>
        <GoalMark task={props.entry.task} />
        <Show when={props.entry.minutes > 0}>
          <span class="log-minutes muted">{props.entry.minutes} min</span>
        </Show>
      </button>
    </li>
  );
}

export function DoneLogView() {
  const [days, setDays] = createSignal(STEP_DAYS);
  // One pass over everything; the window only cuts the result.
  const all = createMemo(() => doneLog(taskIndex(), data.projects, "0000-01-01", localDate(new Date(now())), now()));
  const from = createMemo(() => localDate(addDays(new Date(now()), 1 - days())));
  const shown = createMemo(() => all().filter((day) => day.date >= from()));
  const byDate = createMemo(() => new Map(shown().map((day) => [day.date, day])));
  const dates = createMemo(() => shown().map((day) => day.date));
  const hasOlder = createMemo(() => all().some((day) => day.date < from()));

  return (
    <div class="view done-log-view">
      <header class="view-head">
        <h1>Erledigt</h1>
      </header>
      <Show when={shown().length > 0} fallback={<EmptyState icon="check" title="Hier erscheint, was du erledigst." />}>
        <div class="log-days">
          {/* The clock recomputes the log every 30 s with new objects; rows are keyed by date and task id
              (strings), so they stay in place and keep focus and hover. */}
          <For each={dates()}>
            {(date) => {
              const day = () => byDate().get(date);
              const ids = (list: "done" | "worked") => day()?.[list].map((e) => e.task.id) ?? [];
              const entry = (list: "done" | "worked", id: string) => day()?.[list].find((e) => e.task.id === id);
              return (
                <section class="log-day" aria-label={dayLabel(date)}>
                  <h2 class="log-day-head">
                    <span>{dayLabel(date)}</span>
                    <Show when={(day()?.minutes ?? 0) > 0}>
                      <span class="log-day-minutes muted">{day()!.minutes} min</span>
                    </Show>
                  </h2>
                  <Show when={ids("done").length > 0}>
                    <ul class="log-list log-done">
                      <For each={ids("done")}>{(id) => <Show when={entry("done", id)}>{(e) => <Entry entry={e()} />}</Show>}</For>
                    </ul>
                  </Show>
                  <Show when={ids("worked").length > 0}>
                    <div class="log-worked">
                      <h3 class="log-sub">Daran gearbeitet</h3>
                      <ul class="log-list">
                        <For each={ids("worked")}>{(id) => <Show when={entry("worked", id)}>{(e) => <Entry entry={e()} worked />}</Show>}</For>
                      </ul>
                    </div>
                  </Show>
                </section>
              );
            }}
          </For>
        </div>
      </Show>
      <Show when={hasOlder()}>
        <button class="btn log-older" onClick={() => setDays((n) => n + STEP_DAYS)}>
          Ältere zeigen
        </button>
      </Show>
    </div>
  );
}
