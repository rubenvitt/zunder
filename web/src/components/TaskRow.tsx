import { createEffect, createSignal, For, Show, type JSX } from "solid-js";
import { localDate, formatDue, parseDue } from "../sync/dates";
import { isClosed, isClaimActive, isFlameRunning } from "../sync/derived";
import type { Task } from "../sync/types";
import { locationLabel, PHOENIX_LABELS, STATUS_LABELS, TYPE_LABELS } from "../labels";
import { toggleDone } from "../state/actions";
import { now, taskIndex } from "../state/store";
import { Icon } from "./Icon";

export interface RowAction {
  label: string;
  key?: string;
  /** Also shown on phones, where the other row actions are hidden (swipes cover them). */
  mobile?: boolean;
  run: () => void;
}

export interface SwipeActions {
  left?: RowAction;
  right?: RowAction;
}

export interface TaskRowProps {
  task: Task;
  selected: boolean;
  showLocation?: boolean;
  actions?: RowAction[];
  swipe?: SwipeActions;
  onOpen: () => void;
  onSelect: () => void;
}

const SWIPE_THRESHOLD = 80;

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

export function TaskMeta(props: { task: Task; showLocation?: boolean }) {
  const progress = () => taskIndex().progress(props.task.id);
  return (
    <span class="task-meta">
      <Show when={props.task.type !== "aufgabe"}>
        <span class={`chip chip-type-${props.task.type}`}>{TYPE_LABELS[props.task.type]}</span>
      </Show>
      <Show when={props.task.status !== "open" && props.task.status !== "done"}>
        <span class={`chip chip-status-${props.task.status}`}>{STATUS_LABELS[props.task.status]}</span>
      </Show>
      <DueChip task={props.task} />
      <Show when={progress().total > 0}>
        <span class="chip" title="Unteraufgaben erledigt">
          {progress().done}/{progress().total}
        </span>
      </Show>
      <Show when={props.task.phoenix}>
        {(p) => (
          <span class={`chip chip-phoenix ${isFlameRunning(props.task) ? "chip-burning" : ""}`} title={`${PHOENIX_LABELS[p().role]} ${p().fire}`}>
            <Icon name="flame" size={12} />
            {p().role === "flamme" ? "Flamme" : "Schritt"}
          </span>
        )}
      </Show>
      <Show when={isClaimActive(props.task, now())}>
        <span class="chip" title="Reserviert">
          <Icon name="user" size={12} />
          {props.task.claimed_by}
        </span>
      </Show>
      <Show when={props.showLocation}>
        <span class="chip chip-muted">{locationLabel(taskIndex().location(props.task))}</span>
      </Show>
    </span>
  );
}

export function TaskRow(props: TaskRowProps) {
  const [dx, setDx] = createSignal(0);
  let ref: HTMLLIElement | undefined;
  createEffect(() => {
    if (props.selected) ref?.scrollIntoView({ block: "nearest" });
  });
  let startX = 0;
  let startY = 0;
  let swiping = false;
  let tracking = false;

  const onPointerDown: JSX.EventHandler<HTMLElement, PointerEvent> = (e) => {
    if (!props.swipe || e.pointerType !== "touch") return;
    startX = e.clientX;
    startY = e.clientY;
    tracking = true;
    swiping = false;
  };
  const onPointerMove: JSX.EventHandler<HTMLElement, PointerEvent> = (e) => {
    if (!tracking) return;
    const deltaX = e.clientX - startX;
    const deltaY = e.clientY - startY;
    if (!swiping && Math.abs(deltaX) > 12 && Math.abs(deltaX) > Math.abs(deltaY)) {
      swiping = true;
      e.currentTarget.setPointerCapture(e.pointerId);
    }
    if (swiping) {
      const allowed = (deltaX > 0 && props.swipe?.right) || (deltaX < 0 && props.swipe?.left);
      setDx(allowed ? deltaX : 0);
    }
  };
  const onPointerUp = () => {
    if (!tracking) return;
    tracking = false;
    const delta = dx();
    setDx(0);
    if (delta > SWIPE_THRESHOLD) props.swipe?.right?.run();
    else if (delta < -SWIPE_THRESHOLD) props.swipe?.left?.run();
  };

  return (
    <li
      ref={ref}
      class="task-row"
      classList={{ selected: props.selected, closed: isClosed(props.task.status), swiping: dx() !== 0 }}
      data-task-id={props.task.id}
      data-status={props.task.status}
      aria-selected={props.selected}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
    >
      <Show when={dx() !== 0}>
        <div class="swipe-hint" classList={{ right: dx() > 0 }}>
          {dx() > 0 ? props.swipe?.right?.label : props.swipe?.left?.label}
        </div>
      </Show>
      <div class="task-row-inner" style={dx() ? { transform: `translateX(${dx()}px)` } : undefined}>
        <button
          class="check"
          classList={{ done: props.task.status === "done", dropped: props.task.status === "dropped" }}
          aria-label={props.task.status === "done" ? "Als offen markieren" : "Erledigt"}
          onClick={(e) => {
            e.stopPropagation();
            toggleDone(props.task.id);
          }}
        >
          <Show when={props.task.status === "done"}>
            <Icon name="check" size={14} />
          </Show>
        </button>
        <button
          class="task-main"
          onClick={() => {
            props.onSelect();
            props.onOpen();
          }}
          onFocus={() => props.onSelect()}
        >
          <span class="task-title">{props.task.title}</span>
          <TaskMeta task={props.task} showLocation={props.showLocation} />
        </button>
        <Show when={props.actions?.length}>
          <span class="row-actions">
            <For each={props.actions}>
              {(action) => (
                <button
                  class="btn btn-small btn-ghost"
                  classList={{ "row-action-mobile": !!action.mobile }}
                  title={action.key ? `${action.label} (${action.key})` : action.label}
                  onClick={() => action.run()}
                >
                  {action.label}
                </button>
              )}
            </For>
          </span>
        </Show>
      </div>
    </li>
  );
}
