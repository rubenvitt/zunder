// Ziele (design 6.2/6.3): the active goals with this week's input, quietly: moves, minutes, the lead
// figure in brackets and a check mark only when it is reached. No colour, no bars. A click opens the
// lead and the week's tasks of the goal. Below: the Feuersturm and what is left of it.
// Timing time (ZUN-66) comes from the server and fills in after the rows; without it nothing shows.

import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { EmptyState } from "../components/EmptyState";
import { Icon } from "../components/Icon";
import { movesLabel, timingDuration, timingLabel, weeksLeftLabel } from "../labels";
import { goalHours } from "../state/goal-hours";
import { data, now, taskIndex } from "../state/store";
import { openTask } from "../state/ui";
import { fireOf } from "../sync/dates";
import { goalInput, goalWeekTasks, stormGoal, weeksLeft, type LogEntry } from "../sync/planning";

function Entries(props: { title: string; entries: LogEntry[] }) {
  return (
    <Show when={props.entries.length > 0}>
      <h3>{props.title}</h3>
      <ul class="goal-tasks">
        <For each={props.entries}>
          {(entry) => (
            <li>
              <button class="goal-task" onClick={() => openTask(entry.task.id)}>
                <span class="goal-task-title">{entry.task.title}</span>
                <Show when={entry.minutes > 0}>
                  <span class="muted">{entry.minutes} min</span>
                </Show>
              </button>
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}

export function GoalsView() {
  const week = createMemo(() => fireOf(new Date(now())));
  const rows = createMemo(() => goalInput(taskIndex(), data.goals, week(), now()).goals);
  // The clock recomputes the rows every 30 s with new objects; rows are keyed by goal id (a string),
  // so they stay in place and keep focus and hover.
  const byId = createMemo(() => new Map(rows().map((r) => [r.goal.id, r])));
  const ids = createMemo(() => rows().map((r) => r.goal.id));
  const [openId, setOpenId] = createSignal<string | null>(null);
  const hours = goalHours();
  createEffect(() => onCleanup(hours.watch(week())));
  const timing = (goalId: string) => {
    const m = hours.minutes(goalId);
    return m !== undefined && m > 0 ? m : null;
  };
  const weekTasks = createMemo(() => {
    const id = openId();
    return id ? goalWeekTasks(taskIndex(), data.goals, id, week(), now()) : null;
  });
  const storm = createMemo(() => {
    const today = new Date(now());
    const weeks = weeksLeft(data.goals, today);
    return weeks === null ? null : { name: stormGoal(data.goals, today)?.storm ?? null, weeks };
  });

  return (
    <div class="view goals-view">
      <header class="view-head">
        <h1>Ziele</h1>
        <span class="muted">Woche {Number(week().slice(-2))}</span>
      </header>
      <Show
        when={rows().length > 0}
        fallback={<EmptyState icon="target" title="Noch keine Ziele. Claude legt sie aus dem Feuersturm an." />}
      >
        <ul class="goal-list" aria-label="Ziele">
          <For each={ids()}>
            {(id) => (
              <Show when={byId().get(id)}>
                {(row) => {
                  const open = () => openId() === id;
                  return (
                    <li class="goal-row" classList={{ open: open() }}>
                      <button class="goal-head" aria-expanded={open()} onClick={() => setOpenId(open() ? null : row().goal.id)}>
                        <span class="goal-emoji" aria-hidden="true">
                          {row().goal.emoji}
                        </span>
                        <span class="goal-name">{row().goal.name}</span>
                        <span class="goal-moves">{movesLabel(row().done)}</span>
                        <span class="goal-minutes">{row().minutes > 0 ? `${row().minutes} min` : ""}</span>
                        <span class="goal-timing">{timing(id) !== null ? timingLabel(timing(id)!) : ""}</span>
                        <span class="goal-lead">{row().lead_per_week !== null ? `(${row().lead_per_week}×)` : ""}</span>
                        <span class="goal-check">
                          <Show when={row().reached}>
                            <span class="goal-reached" role="img" aria-label="Erreicht" title="Erreicht">
                              <Icon name="check" size={14} />
                            </span>
                          </Show>
                        </span>
                      </button>
                      <Show when={open()}>
                        <div class="goal-detail">
                          <Show when={row().goal.lead}>
                            <p class="goal-lead-text">{row().goal.lead}</p>
                          </Show>
                          <Show when={timing(id)}>
                            {(m) => <p class="goal-timing-text muted">Timing: {timingDuration(m())}</p>}
                          </Show>
                          <Entries title="Erledigt diese Woche" entries={weekTasks()?.done ?? []} />
                          <Entries title="Mit Flammen diese Woche" entries={weekTasks()?.worked ?? []} />
                          <Show when={!weekTasks()?.done.length && !weekTasks()?.worked.length}>
                            <p class="muted">Diese Woche noch nichts.</p>
                          </Show>
                        </div>
                      </Show>
                    </li>
                  );
                }}
              </Show>
            )}
          </For>
        </ul>
        <Show when={storm()}>
          {(s) => (
            <p class="goal-storm muted">
              <Show when={s().name}>{s().name} · </Show>
              {weeksLeftLabel(s().weeks)}
            </p>
          )}
        </Show>
      </Show>
    </div>
  );
}
