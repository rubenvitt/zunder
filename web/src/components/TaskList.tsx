import { For, Show, type JSX } from "solid-js";
import type { Task } from "../sync/types";
import { selectedId, setDetailId, setSelectedId } from "../state/ui";
import { TaskRow, type RowAction, type SwipeActions } from "./TaskRow";

export interface TaskListProps {
  tasks: Task[];
  /** Shown instead of the list when it has no tasks, usually an `EmptyState`. */
  empty?: JSX.Element;
  showLocation?: boolean;
  actions?: (task: Task) => RowAction[];
  swipe?: (task: Task) => SwipeActions;
  label?: string;
}

/** A plain task list; keyboard navigation is registered by the surrounding view. */
export function TaskList(props: TaskListProps) {
  return (
    <Show when={props.tasks.length > 0} fallback={props.empty}>
      <ul class="task-list" aria-label={props.label}>
        <For each={props.tasks}>
          {(task) => (
            <TaskRow
              task={task}
              selected={selectedId() === task.id}
              showLocation={props.showLocation}
              actions={props.actions?.(task)}
              swipe={props.swipe?.(task)}
              onSelect={() => setSelectedId(task.id)}
              onOpen={() => setDetailId(task.id)}
            />
          )}
        </For>
      </ul>
    </Show>
  );
}
