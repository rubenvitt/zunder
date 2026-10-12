// Small task building blocks shared by rows, board cards and the detail panel:
// the done check, the copyable reference, the progress bar and the subtask tree.

import { createSignal, For, Show } from "solid-js";
import { formatDue, localDate, parseDue } from "../sync/dates";
import { isClosed, isDevLocation } from "../sync/derived";
import type { Task } from "../sync/types";
import { STATUS_LABELS } from "../labels";
import { toggleDone } from "../state/actions";
import { data, now, taskIndex } from "../state/store";
import { copyTaskId, setDetailId, setSelectedId } from "../state/ui";
import { Icon } from "./Icon";

export function CheckButton(props: { task: Task; small?: boolean }) {
  return (
    <button
      type="button"
      class="check"
      classList={{ "check-small": !!props.small, done: props.task.status === "done", dropped: props.task.status === "dropped" }}
      data-status={props.task.status}
      aria-label={props.task.status === "done" ? "Als offen markieren" : "Erledigt"}
      onClick={(e) => {
        e.stopPropagation();
        toggleDone(props.task.id);
      }}
    >
      <Icon name="check" size={props.small ? 12 : 14} />
    </button>
  );
}

/** The reference (ZUN-42) or ID as a button that copies it: one click away for agents. */
export function RefButton(props: { task: Task; idFallback?: boolean; class?: string }) {
  const kind = () => (props.task.ref ? "Kennung" : "ID");
  return (
    <Show when={props.task.ref || props.idFallback}>
      <button
        type="button"
        class={`chip chip-ref ref-button mono ${props.class ?? ""}`}
        classList={{ "ref-id": !props.task.ref }}
        title={`${kind()} ${props.task.ref ?? props.task.id} kopieren (y)`}
        aria-label={`${kind()} ${props.task.ref ?? ""} kopieren`.replace("  ", " ")}
        onClick={(e) => {
          e.stopPropagation();
          copyTaskId(props.task);
        }}
        onPointerDown={(e) => e.stopPropagation()}
        draggable={false}
      >
        <Show when={props.task.ref} fallback={<><Icon name="copy" size={12} />ID</>}>
          {props.task.ref}
        </Show>
      </button>
    </Show>
  );
}

export function ProgressBar(props: { done: number; total: number; label?: string }) {
  const pct = () => (props.total > 0 ? Math.round((props.done / props.total) * 100) : 0);
  return (
    <span class="progress" role="progressbar" aria-label={props.label ?? "Fortschritt"} aria-valuemin={0} aria-valuemax={props.total} aria-valuenow={props.done}>
      <span class="progress-fill" style={{ width: `${pct()}%` }} classList={{ complete: props.total > 0 && props.done === props.total }} />
    </span>
  );
}

function openTask(id: string) {
  setSelectedId(id);
  setDetailId(id);
}

/**
 * Subtasks of a task, nested and collapsible: check off, open, copy the reference.
 * `compact` is the variant inside list rows and board cards.
 */
export function SubtaskTree(props: { parentId: string; compact?: boolean; depth?: number }) {
  const children = () => taskIndex().childrenOf(props.parentId);
  return (
    <ul class="subtree" classList={{ compact: !!props.compact }} role={props.depth ? "group" : "tree"} aria-label={props.depth ? undefined : "Unteraufgaben"}>
      <For each={children()}>{(child) => <SubtaskNode task={child} compact={props.compact} depth={props.depth ?? 0} />}</For>
    </ul>
  );
}

function SubtaskNode(props: { task: Task; compact?: boolean; depth: number }) {
  const children = () => taskIndex().childrenOf(props.task.id);
  const progress = () => taskIndex().progress(props.task.id);
  const [open, setOpen] = createSignal(!props.compact);
  return (
    <li class="subtask" data-task-id={props.task.id} role="treeitem" aria-expanded={children().length > 0 ? open() : undefined}>
      <div class="subtask-row" classList={{ closed: isClosed(props.task.status) }}>
        <Show when={children().length > 0} fallback={<span class="subtask-toggle-space" />}>
          <button
            type="button"
            class="subtask-toggle"
            aria-label={open() ? "Unteraufgaben zuklappen" : "Unteraufgaben aufklappen"}
            onClick={(e) => {
              e.stopPropagation();
              setOpen((o) => !o);
            }}
          >
            <Icon name={open() ? "down" : "right"} size={12} />
          </button>
        </Show>
        <CheckButton task={props.task} small />
        <button
          type="button"
          class="link-button subtask-title"
          onClick={(e) => {
            e.stopPropagation();
            openTask(props.task.id);
          }}
        >
          {props.task.title}
        </button>
        <Show when={props.task.status !== "open" && props.task.status !== "done"}>
          <span class={`chip chip-status-${props.task.status}`}>{STATUS_LABELS[props.task.status]}</span>
        </Show>
        <Show when={!props.compact}>
          <DueChip task={props.task} />
        </Show>
        <Show when={progress().total > 0}>
          <span class="chip chip-count" title="Unteraufgaben erledigt">
            {progress().done}/{progress().total}
          </span>
        </Show>
        {/* Agents address tasks by reference or ID: in dev projects every subtask offers one. */}
        <RefButton task={props.task} idFallback={isDevLocation(taskIndex().location(props.task), (id) => data.projects[id])} />
      </div>
      <Show when={children().length > 0 && open() && props.depth < 12}>
        <SubtaskTree parentId={props.task.id} compact={props.compact} depth={props.depth + 1} />
      </Show>
    </li>
  );
}

/** "3/5" toggle that unfolds the subtasks inside a row or card. */
export function SubtaskToggle(props: { task: Task; open: boolean; onToggle: () => void }) {
  const progress = () => taskIndex().progress(props.task.id);
  return (
    <Show when={progress().total > 0}>
      <button
        type="button"
        class="subtask-chip"
        classList={{ open: props.open }}
        aria-expanded={props.open}
        title={props.open ? "Unteraufgaben zuklappen" : "Unteraufgaben zeigen"}
        aria-label={`Unteraufgaben ${progress().done} von ${progress().total} erledigt, ${props.open ? "zuklappen" : "zeigen"}`}
        onClick={(e) => {
          e.stopPropagation();
          props.onToggle();
        }}
        onPointerDown={(e) => e.stopPropagation()}
      >
        <Icon name="tree" size={13} />
        <span>
          {progress().done}/{progress().total}
        </span>
        <ProgressBar done={progress().done} total={progress().total} />
      </button>
    </Show>
  );
}

export function DueChip(props: { task: Task }) {
  const info = () => parseDue(props.task.due);
  const tone = () => {
    const due = info();
    if (!due || isClosed(props.task.status)) return "";
    const today = localDate(new Date(now()));
    return due.date < today ? "chip-danger" : due.date === today ? "chip-accent" : "";
  };
  return (
    <Show when={props.task.due}>
      <span class={`chip ${tone()}`} title="Fällig">
        {formatDue(props.task.due, new Date(now()))}
      </span>
    </Show>
  );
}

