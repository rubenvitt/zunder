// Status board with HTML5 drag & drop and keyboard moves (h/l, j/k, Shift+H/L/J/K).

import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { compareTasks, hasOpenChildren, isClaimActive, isClosed, statusChangedMs } from "../sync/derived";
import type { Task, TaskStatus } from "../sync/types";
import { boardStatuses, STATUS_LABELS } from "../labels";
import { placeTask, setStatus, shorten } from "../state/actions";
import { data, now, taskIndex } from "../state/store";
import { detailId, registerList, selectedId, setActiveBoard, setDetailId, setSelectedId, toast } from "../state/ui";
import { EmptyState } from "./EmptyState";
import { Icon } from "./Icon";
import { Modal } from "./Modal";
import { RefButton, SubtaskToggle, SubtaskTree } from "./TaskBits";
import { TaskList } from "./TaskList";
import { TaskMeta } from "./TaskRow";

/** Done cards shown before the column folds; the rest stays one click away. */
export const DONE_PREVIEW = 5;

export interface BoardProps {
  tasks: Task[];
  dev: boolean;
  /** Show project name, claim, branch and PR on cards (Dev view). */
  devDetails?: boolean;
  label: string;
  /** Done tasks past the 3-day window in the board's scope, newest first ("Ältere erledigte"). */
  archive?: () => Task[];
}

export function Board(props: BoardProps) {
  const statuses = () => boardStatuses(props.dev);
  const columns = createMemo(() => {
    const map = new Map<TaskStatus, Task[]>();
    for (const s of statuses()) map.set(s, []);
    for (const t of props.tasks) map.get(t.status)?.push(t);
    for (const [status, list] of map) {
      // Done reads as a log: most recently closed first, so the fold keeps what just happened.
      if (status === "done") list.sort((a, b) => statusChangedMs(b) - statusChangedMs(a) || compareTasks(a, b));
      else list.sort(compareTasks);
    }
    return map;
  });
  const column = (status: TaskStatus) => columns().get(status) ?? [];
  const [doneExpanded, setDoneExpanded] = createSignal(false);
  const [archiveOpen, setArchiveOpen] = createSignal(false);
  /** What the column shows: done folds to the newest few unless expanded. */
  const shown = (status: TaskStatus) => {
    const list = column(status);
    return status === "done" && !doneExpanded() ? list.slice(0, DONE_PREVIEW) : list;
  };
  const hiddenDone = () => Math.max(0, column("done").length - DONE_PREVIEW);
  // The detail panel sits beside the board; the archive dialog would cover it.
  createEffect(on(detailId, (id) => id && setArchiveOpen(false), { defer: true }));
  const [dragId, setDragId] = createSignal<string | null>(null);
  const [dropTarget, setDropTarget] = createSignal<{ status: TaskStatus; index: number } | null>(null);

  function locate(id: string | null): { col: number; row: number } | null {
    if (!id) return null;
    const list = statuses();
    for (let c = 0; c < list.length; c++) {
      const row = shown(list[c]!).findIndex((t) => t.id === id);
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
    placeTask(id, before, after, status);
  }

  onMount(() => {
    registerList(() => statuses().flatMap((s) => shown(s)));
    setActiveBoard({
      moveSelection(dx) {
        const pos = locate(selectedId());
        const list = statuses();
        let col = pos ? pos.col + dx : 0;
        while (col >= 0 && col < list.length && shown(list[col]!).length === 0) col += dx;
        if (col < 0 || col >= list.length) return;
        const target = shown(list[col]!);
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
              <For each={shown(status)}>
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
              <Show when={dropTarget()?.status === status && (dropTarget()?.index ?? -1) >= shown(status).filter((t) => t.id !== dragId()).length}>
                <li class="drop-indicator" aria-hidden="true" />
              </Show>
            </ul>
            <Show when={status === "done" && (hiddenDone() > 0 || props.archive)}>
              <div class="board-column-foot">
                <Show when={hiddenDone() > 0}>
                  <button class="btn btn-ghost btn-small" aria-expanded={doneExpanded()} onClick={() => setDoneExpanded(!doneExpanded())}>
                    {doneExpanded() ? "Weniger zeigen" : `${hiddenDone()} weitere zeigen`}
                  </button>
                </Show>
                <Show when={props.archive}>
                  <button class="btn btn-ghost btn-small" onClick={() => setArchiveOpen(true)} title="Erledigt vor mehr als 3 Tagen">
                    <Icon name="archive" size={14} /> Ältere erledigte
                  </button>
                </Show>
              </div>
            </Show>
          </section>
        )}
      </For>
      <Show when={archiveOpen() && props.archive}>
        {(archive) => <ArchiveDialog tasks={archive()()} showLocation={props.devDetails} onClose={() => setArchiveOpen(false)} />}
      </Show>
    </div>
  );
}

function ArchiveDialog(props: { tasks: Task[]; showLocation?: boolean; onClose: () => void }) {
  return (
    <Modal label="Ältere erledigte" onClose={props.onClose} class="board-archive">
      <h2>Ältere erledigte</h2>
      <p class="muted">Erledigt vor mehr als 3 Tagen, zuletzt erledigte zuerst.</p>
      <TaskList
        tasks={props.tasks}
        showLocation={props.showLocation}
        label="Ältere erledigte Aufgaben"
        empty={<EmptyState compact icon="check" title="Noch nichts im Archiv." />}
      />
      <div class="modal-actions">
        <button class="btn btn-small" onClick={props.onClose}>
          Schließen
        </button>
      </div>
    </Modal>
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
  const [subtasksOpen, setSubtasksOpen] = createSignal(false);
  const project = () => {
    const loc = taskIndex().location(props.task);
    return loc.kind === "project" ? data.projects[loc.id] : undefined;
  };
  // Every subtask done or dropped while the task itself is still open: closing is one click away.
  const readyToClose = () => {
    if (isClosed(props.task.status)) return false;
    const children = taskIndex().childrenOf(props.task.id);
    return children.length > 0 && !hasOpenChildren(children);
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
      <span class="card-top">
        <Show when={props.devDetails && project()} fallback={<span class="spacer" />}>
          <span class="card-project">{project()!.name}</span>
        </Show>
        <RefButton task={props.task} idFallback={props.devDetails} />
      </span>
      <span class="task-title">{props.task.title}</span>
      <TaskMeta task={props.task} />
      <Show when={taskIndex().progress(props.task.id).total > 0 || readyToClose()}>
        <span class="card-subtasks-bar">
          <Show when={taskIndex().progress(props.task.id).total > 0}>
            <SubtaskToggle task={props.task} open={subtasksOpen()} onToggle={() => setSubtasksOpen((o) => !o)} />
          </Show>
          <Show when={readyToClose()}>
            <button
              type="button"
              class="btn btn-small card-close"
              title="Alle Unteraufgaben sind erledigt"
              onClick={(e) => {
                e.stopPropagation();
                if (setStatus(props.task.id, "done")) toast(`„${shorten(props.task.title)}“ → ${STATUS_LABELS.done}`);
              }}
              onPointerDown={(e) => e.stopPropagation()}
              draggable={false}
            >
              <Icon name="check" size={12} />
              Abschließen
            </button>
          </Show>
        </span>
      </Show>
      <Show when={subtasksOpen()}>
        <div class="card-subtasks" onClick={(e) => e.stopPropagation()}>
          <SubtaskTree parentId={props.task.id} compact />
        </div>
      </Show>
      <Show when={props.devDetails && (props.task.branch || props.task.pr_url || isClaimActive(props.task, now()))}>
        <span class="card-dev">
          <Show when={props.task.branch}>
            <span class="mono card-branch" title="Branch">
              <Icon name="branch" size={12} /> {props.task.branch}
            </span>
          </Show>
          <Show when={props.task.pr_url}>
            <a class="card-pr" href={props.task.pr_url!} target="_blank" rel="noopener noreferrer" onClick={(e) => e.stopPropagation()}>
              <Icon name="pullRequest" size={12} />
              PR
            </a>
          </Show>
        </span>
      </Show>
    </li>
  );
}
