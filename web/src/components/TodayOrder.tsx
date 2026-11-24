// Heute's order by hand (design 5.3): HTML5 drag & drop like the board, onto the rows under
// "Danach" or onto the Fokus box. A drop writes day_rank and plans the task (and whatever stands
// above it) for today; the rule is in sync/day-order, the writes in state/actions.

import { createSignal, For, Show } from "solid-js";
import type { Task } from "../sync/types";
import { placeInToday } from "../state/actions";
import { selectedId, setDetailId, setSelectedId } from "../state/ui";
import { TaskRow, type RowAction, type RowDrag, type SwipeActions } from "./TaskRow";

/**
 * The neighbours `id` gets in `items` when put in front of `anchor`; without an anchor right
 * behind `last`, else at the very end.
 */
function neighbours(items: Task[], id: string, anchor: Task | null, last: Task | null = null): [Task | null, Task | null] {
  const order = items.filter((t) => t.id !== id);
  const at = (t: Task | null) => (t ? order.findIndex((x) => x.id === t.id) : -1);
  let i = at(anchor);
  if (i < 0 && at(last) >= 0) i = at(last) + 1;
  if (i < 0) i = order.length;
  return [order[i - 1] ?? null, order[i] ?? null];
}

/** The drag state shared by the Fokus box and the list under "Danach". */
export function createTodayDrag(items: () => Task[]) {
  const [dragId, setDragId] = createSignal<string | null>(null);
  const [dropIndex, setDropIndex] = createSignal<number | null>(null);
  const [overFocus, setOverFocus] = createSignal(false);
  const end = () => {
    setDragId(null);
    setDropIndex(null);
    setOverFocus(false);
  };
  /** Drag handlers for the row of `task` (an accessor: the box's task changes in place). */
  const row = (task: () => Task): RowDrag => ({
    get dragging() {
      return dragId() === task().id;
    },
    onDragStart(e) {
      setDragId(task().id);
      e.dataTransfer?.setData("text/plain", task().id);
      if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
    },
    onDragEnd: end,
  });
  const accept = (e: DragEvent) => {
    if (!dragId()) return false;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    return true;
  };
  /** Drop handlers for the Fokus box: the task goes in front of the box's task. */
  const focusTarget = (focus: () => Task | null) => ({
    onDragOver(e: DragEvent) {
      if (accept(e)) setOverFocus(true);
    },
    onDragLeave(e: DragEvent & { currentTarget: HTMLElement }) {
      if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOverFocus(false);
    },
    onDrop(e: DragEvent) {
      e.preventDefault();
      const id = dragId();
      end();
      const target = focus();
      if (!id || id === target?.id) return;
      const list = items();
      const anchor = target && list.some((t) => t.id === target.id) ? target : (list.find((t) => t.id !== id) ?? null);
      placeInToday(id, ...neighbours(list, id, anchor));
    },
  });
  return { dragId, dropIndex, setDropIndex, overFocus, row, accept, end, focusTarget };
}

export type TodayDrag = ReturnType<typeof createTodayDrag>;

export interface TodayOrderProps {
  /** The rows shown (Heute's order without the task in the box). */
  tasks: Task[];
  /** The whole order (sync/planning todayList items): where a drop lands. */
  items: Task[];
  drag: TodayDrag;
  label: string;
  actions?: (task: Task) => RowAction[];
  swipe?: (task: Task) => SwipeActions;
}

/** The rows under "Danach", each with its goal's sign, to drag into another place. */
export function TodayOrder(props: TodayOrderProps) {
  const d = props.drag;
  const shown = () => props.tasks.filter((t) => t.id !== d.dragId());
  function indexAt(e: DragEvent, list: HTMLElement): number {
    const rows = [...list.querySelectorAll<HTMLElement>(":scope > .task-row:not(.dragging)")];
    const i = rows.findIndex((r) => {
      const rect = r.getBoundingClientRect();
      return e.clientY < rect.top + rect.height / 2;
    });
    return i < 0 ? rows.length : i;
  }
  return (
    <ul
      class="task-list today-order"
      aria-label={props.label}
      onDragOver={(e) => {
        if (d.accept(e)) d.setDropIndex(indexAt(e, e.currentTarget));
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) d.setDropIndex(null);
      }}
      onDrop={(e) => {
        e.preventDefault();
        const id = d.dragId();
        const index = d.dropIndex() ?? indexAt(e, e.currentTarget);
        const rest = shown();
        d.end();
        if (!id) return;
        const anchor = rest[index] ?? null;
        placeInToday(id, ...neighbours(props.items, id, anchor, rest[rest.length - 1] ?? null));
      }}
    >
      <For each={props.tasks}>
        {(task) => (
          <>
            <Show when={d.dropIndex() !== null && d.dragId() !== task.id && shown()[d.dropIndex()!]?.id === task.id}>
              <li class="drop-indicator" aria-hidden="true" />
            </Show>
            <TaskRow
              task={task}
              selected={selectedId() === task.id}
              showLocation
              quietDue
              drag={d.row(() => task)}
              actions={props.actions?.(task)}
              swipe={props.swipe?.(task)}
              onSelect={() => setSelectedId(task.id)}
              onOpen={() => setDetailId(task.id)}
            />
          </>
        )}
      </For>
      <Show when={d.dropIndex() !== null && d.dropIndex()! >= shown().length}>
        <li class="drop-indicator" aria-hidden="true" />
      </Show>
    </ul>
  );
}
