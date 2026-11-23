// The Fokus box at the top of Heute (design 5.1/5.2): the one task to do now. The running flame,
// else the first open task of the order. On a day without a plan, "Erst für dich" (contract 5.1,
// rule in sync/focus) comes first: a Glut task for oneself stands in the box without a click;
// without one a quiet line offers a few to plan for today, and the box shows the list's first
// task below it. Hidden for the day, only that offer goes. With the plan done the box says so.

import { For, Show } from "solid-js";
import { isFlameRunning } from "../sync/derived";
import type { Task } from "../sync/types";
import { flame, planForToday } from "../state/actions";
import { dismissFocus, undismissFocus } from "../state/focus";
import { focusOf, type TodaySections } from "../state/selectors";
import { askToast, selectedId, setDetailId, setSelectedId } from "../state/ui";
import { FOCUS_EMPTY, FOR_YOU_FIRST, PLAN_DONE, TODAY_FOCUS } from "../labels";
import { Icon } from "./Icon";
import { TaskRow, type RowAction, type SwipeActions } from "./TaskRow";
import { GoalMark, type TodayDrag } from "./TodayOrder";

export interface TodayFocusState {
  /** The task in the box. */
  task: Task | null;
  /** It stands there by "Erst für dich" (no plan for today). */
  forYou: boolean;
  /** The quiet offer of Glut tasks to plan (no plan, none for oneself); null = not shown. */
  choices: Task[] | null;
  /** Everything planned for today is done (and no flame runs). */
  planDone: boolean;
}

/** What the box shows today; `dismissed`: "Erst für dich" is hidden for the day. */
export function todayFocus(s: TodaySections, today: string, dismissed: boolean): TodayFocusState {
  const burning = !!s.focus && isFlameRunning(s.focus);
  if (burning || s.has_plan || dismissed) return { task: s.focus, forYou: false, choices: null, planDone: !burning && s.plan_done };
  const rule = focusOf(today);
  if (rule.task) return { task: rule.task, forYou: true, choices: null, planDone: false };
  return { task: s.focus, forYou: false, choices: rule.choices, planDone: false };
}

export interface TodayFocusProps {
  state: TodayFocusState;
  drag: TodayDrag;
  actions?: (task: Task) => RowAction[];
  swipe?: (task: Task) => SwipeActions;
}

export function TodayFocus(props: TodayFocusProps) {
  const task = () => props.state.task;
  const burning = () => !!task() && isFlameRunning(task()!);
  const target = props.drag.focusTarget(task);
  const dismiss = () => {
    dismissFocus();
    askToast("„Erst für dich“ ist für heute ausgeblendet.", [{ label: "Rückgängig", run: undismissFocus }], 6000);
  };
  // `t` is an accessor: the box keeps its row while the task in it changes.
  const row = (t: () => Task) => (
    <ul class="task-list focus-list" aria-label={TODAY_FOCUS}>
      <TaskRow
        task={t()}
        selected={selectedId() === t().id}
        showLocation
        mark={<GoalMark task={t()} />}
        drag={props.drag.row(t)}
        actions={props.actions?.(t())}
        swipe={props.swipe?.(t())}
        onSelect={() => setSelectedId(t().id)}
        onOpen={() => setDetailId(t().id)}
      />
    </ul>
  );
  return (
    <section
      class="focus"
      classList={{ "focus-quiet": !task(), "focus-burning": burning(), "drop-active": props.drag.overFocus() }}
      aria-labelledby="focus-heading"
      onDragOver={target.onDragOver}
      onDragLeave={target.onDragLeave}
      onDrop={target.onDrop}
    >
      <div class="focus-head">
        <h2 id="focus-heading">{TODAY_FOCUS}</h2>
      </div>
      <Show when={props.state.forYou || props.state.choices}>
        <div class="focus-for-you" role="group" aria-label={FOR_YOU_FIRST}>
          <div class="focus-for-you-head">
            <span class="focus-note">{FOR_YOU_FIRST}</span>
            <button type="button" class="btn btn-ghost btn-icon btn-xs focus-dismiss" aria-label="Für heute ausblenden" title="Für heute ausblenden" onClick={dismiss}>
              <Icon name="x" size={12} />
            </button>
          </div>
          <Show when={props.state.forYou && task()}>{(t) => row(t)}</Show>
          <Show when={props.state.choices}>
            {(choices) => (
              <div class="focus-empty">
                <p class="muted">{FOCUS_EMPTY}</p>
                <Show when={choices().length > 0}>
                  <div class="focus-choices" role="group" aria-label="Glut für heute wählen">
                    <span class="muted">Für heute einplanen:</span>
                    <For each={choices()}>
                      {(choice) => (
                        <button type="button" class="chip focus-choice" title="Für heute einplanen, vorn" onClick={() => planForToday(choice.id, "front")}>
                          {choice.title}
                        </button>
                      )}
                    </For>
                  </div>
                </Show>
              </div>
            )}
          </Show>
        </div>
      </Show>
      <Show when={!props.state.forYou && task()}>{(t) => row(t)}</Show>
      <Show when={props.state.planDone}>
        <p class="muted focus-done">{PLAN_DONE}</p>
      </Show>
      <Show when={task()}>
        {(t) => (
          <div class="focus-actions">
            <button
              type="button"
              class="btn btn-small flame-button"
              classList={{ "btn-soft": !burning(), burning: burning() }}
              data-flame-for={t().id}
              onClick={() => void flame(t().id, burning() ? "stop" : "start")}
            >
              <Icon name={burning() ? "stop" : "flame"} size={14} />
              {burning() ? "Flamme beenden" : "Flamme starten"}
            </button>
          </div>
        )}
      </Show>
    </section>
  );
}
