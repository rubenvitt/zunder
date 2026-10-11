// Status board with HTML5 drag & drop and keyboard moves (h/l, j/k, Shift+H/L/J/K).

import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { compareTasks, isClaimActive } from "../sync/derived";
import type { Task, TaskStatus } from "../sync/types";
import { boardStatuses, STATUS_LABELS } from "../labels";
import { placeTask, warnIfOpenChildren } from "../state/actions";
import { data, now, taskIndex } from "../state/store";
import { registerList, selectedId, setActiveBoard, setDetailId, setSelectedId } from "../state/ui";
import { Icon } from "./Icon";
import { TaskMeta } from "./TaskRow";

export interface BoardProps {
  tasks: Task[];
  dev: boolean;
  /** Show project name, claim, branch and PR on cards (Dev view). */
  devDetails?: boolean;
  label: string;
}

export function Board(props: BoardProps) {
  const statuses = () => boardStatuses(props.dev);
  const columns = createMemo(() => {
    const map = new Map<TaskStatus, Task[]>();
    for (const s of statuses()) map.set(s, []);
    for (const t of props.tasks) map.get(t.status)?.push(t);
    for (const list of map.values()) list.sort(compareTasks);
    return map;
  });
  const column = (status: TaskStatus) => columns().get(status) ?? [];
  const [dragId, setDragId] = createSignal<string | null>(null);
  const [dropTarget, setDropTarget] = createSignal<{ status: TaskStatus; index: number } | null>(null);

  function locate(id: string | null): { col: number; row: number } | null {
    if (!id) return null;
    const list = statuses();
    for (let c = 0; c < list.length; c++) {
      const row = column(list[c]!).findIndex((t) => t.id === id);
      if (row >= 0) return { col: c, row };
    }
    return null;
  }

  /** Moves a task into `status` at `index` (index within the column without the task itself). */
  function move(id: string, status: TaskStatus, index: number) {
    const task = data.tasks[id];
    if (!task) return;
    const target = column(status).filter((t) => t.id !== id);
    const clamped = Math.max(0, Math.min(index, target.length));
    const before = target[clamped - 1] ?? null;
    const after = target[clamped] ?? null;
    const statusChanged = task.status !== status;
    if (placeTask(id, before, after, statusChanged ? { status } : {}) && statusChanged && status === "done") warnIfOpenChildren(id);
  }

  onMount(() => {
    registerList(() => statuses().flatMap((s) => column(s)));
    setActiveBoard({
      moveSelection(dx) {
        const pos = locate(selectedId());
        const list = statuses();
        let col = pos ? pos.col + dx : 0;
        while (col >= 0 && col < list.length && column(list[col]!).length === 0) col += dx;
        if (col < 0 || col >= list.length) return;
        const target = column(list[col]!);
        setSelectedId(target[Math.min(pos?.row ?? 0, target.length - 1)]!.id);
      },
      moveTask(dx) {
        const id = selectedId();
        const pos = locate(id);
        if (!id || !pos) return;
        const status = statuses()[pos.col + dx];
        if (status) move(id, status, pos.row);
      },
      reorder(dy) {
        const id = selectedId();
        const pos = locate(id);
        if (!id || !pos) return;
        const status = statuses()[pos.col]!;
        move(id, status, pos.row + dy);
      },
    });
  });
  onCleanup(() => setActiveBoard(null));

  function dropIndex(e: DragEvent, list: HTMLElement): number {
    const cards = [...list.querySelectorAll<HTMLElement>(".card:not(.dragging)")];
    const idx = cards.findIndex((card) => {
      const rect = card.getBoundingClientRect();
      return e.clientY < rect.top + rect.height / 2;
    });
    return idx < 0 ? cards.length : idx;
  }

  return (
    <div class="board" role="region" aria-label={props.label}>
      <For each={statuses()}>
        {(status) => (
          <section
            class="board-column"
            classList={{ "drop-active": dropTarget()?.status === status }}
            data-status={status}
            onDragOver={(e) => {
              if (!dragId()) return;
              e.preventDefault();
              if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
              const list = e.currentTarget.querySelector<HTMLElement>(".board-cards");
              if (list) setDropTarget({ status, index: dropIndex(e, list) });
            }}
            onDragLeave={(e) => {
              if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropTarget(null);
            }}
            onDrop={(e) => {
              e.preventDefault();
              const id = dragId() ?? e.dataTransfer?.getData("text/plain") ?? null;
              const target = dropTarget();
              setDropTarget(null);
              setDragId(null);
              if (id) move(id, status, target?.status === status ? target.index : column(status).length);
            }}
          >
            <header class="board-column-head">
              <span>{STATUS_LABELS[status]}</span>
              <span class="count">{column(status).length}</span>
            </header>
            <ul class="board-cards">
              <For each={column(status)}>
                {(task, i) => (
                  <>
                    <Show when={dropTarget()?.status === status && dropTarget()?.index === i() && dragId() !== task.id}>
                      <li class="drop-indicator" aria-hidden="true" />
                    </Show>
                    <Card
                      task={task}
                      devDetails={props.devDetails}
                      selected={selectedId() === task.id}
                      dragging={dragId() === task.id}
                      onDragStart={(e) => {
                        setDragId(task.id);
                        e.dataTransfer?.setData("text/plain", task.id);
                        if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
                      }}
                      onDragEnd={() => {
                        setDragId(null);
                        setDropTarget(null);
                      }}
                    />
                  </>
                )}
              </For>
              <Show when={dropTarget()?.status === status && (dropTarget()?.index ?? -1) >= column(status).filter((t) => t.id !== dragId()).length}>
                <li class="drop-indicator" aria-hidden="true" />
              </Show>
            </ul>
          </section>
        )}
      </For>
    </div>
  );
}

function Card(props: {
  task: Task;
  devDetails?: boolean;
  selected: boolean;
  dragging: boolean;
  onDragStart: (e: DragEvent) => void;
  onDragEnd: () => void;
}) {
  let ref: HTMLLIElement | undefined;
  const project = () => {
    const loc = taskIndex().location(props.task);
    return loc.kind === "project" ? data.projects[loc.id] : undefined;
  };
  return (
    <li
      ref={(el) => {
        ref = el;
      }}
      class="card"
      classList={{ selected: props.selected, dragging: props.dragging }}
      data-task-id={props.task.id}
      draggable="true"
      onDragStart={(e) => props.onDragStart(e)}
      onDragEnd={() => props.onDragEnd()}
      onClick={() => {
        setSelectedId(props.task.id);
        setDetailId(props.task.id);
        ref?.focus();
      }}
      tabIndex={-1}
    >
      <Show when={props.devDetails && project()}>
        <span class="card-project">{project()!.name}</span>
      </Show>
      <span class="task-title">{props.task.title}</span>
      <TaskMeta task={props.task} />
      <Show when={props.devDetails && (props.task.branch || props.task.pr_url || isClaimActive(props.task, now()))}>
        <span class="card-dev">
          <Show when={props.task.branch}>
            <span class="mono" title="Branch">
              <Icon name="branch" size={12} /> {props.task.branch}
            </span>
          </Show>
          <Show when={props.task.pr_url}>
            <a href={props.task.pr_url!} target="_blank" rel="noopener noreferrer" onClick={(e) => e.stopPropagation()}>
              PR
            </a>
          </Show>
        </span>
      </Show>
    </li>
  );
}
