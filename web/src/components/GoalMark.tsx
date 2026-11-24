// The sign of a task's effective goal, small and quiet; nothing without one. Every task row shows it.

import { Show, type JSX } from "solid-js";
import { effectiveGoal } from "../sync/planning";
import type { Task } from "../sync/types";
import { data, taskIndex } from "../state/store";

export function GoalMark(props: { task: Task }): JSX.Element {
  const goal = () => effectiveGoal(taskIndex(), data.goals, props.task);
  return (
    <Show when={goal()?.emoji ? goal() : null}>
      {(g) => (
        <span class="goal-mark" role="img" title={`Ziel: ${g().name}`} aria-label={`Ziel: ${g().name}`}>
          {g().emoji}
        </span>
      )}
    </Show>
  );
}
