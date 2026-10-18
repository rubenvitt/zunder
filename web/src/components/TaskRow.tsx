import { createEffect, createSignal, For, Show, type JSX } from "solid-js";
import { isClosed, isClaimActive, isDevLocation, isFlameRunning } from "../sync/derived";
import { needsDecision } from "../sync/defer";
import type { Task } from "../sync/types";
import { deferralsLabel, describeRecurrence, FOR_WHOM_HINTS, FOR_WHOM_LABELS, locationLabel, PHOENIX_LABELS, STATUS_LABELS, TYPE_LABELS, waitingLabel, WEIGHT_HINTS, WEIGHT_LABELS } from "../labels";
import { data, now, taskIndex } from "../state/store";
import { Icon } from "./Icon";
import { CheckButton, DueChip, RefButton, SubtaskToggle, SubtaskTree } from "./TaskBits";

export { DueChip };

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
  /** The chip reads "wartet noch auf <Name>" (Wiedervorlage in Heute). */
  waitingStill?: boolean;
  onOpen: () => void;
  onSelect: () => void;
}

const SWIPE_THRESHOLD = 80;

/**
 * Weight, for whom and deferrals: the personal reading of a task. Dev projects leave them out
 * (agents work there, not the owner).
 */
function PersonalChips(props: { task: Task }) {
  const personal = () => !isDevLocation(taskIndex().location(props.task), (id) => data.projects[id]);
  return (
    <Show when={personal()}>
      <Show when={props.task.weight}>
        {(w) => (
          <span class={`chip chip-weight chip-weight-${w()}`} title={`${WEIGHT_LABELS[w()]}: ${WEIGHT_HINTS[w()]}`}>
            <span class="weight-dot" aria-hidden="true" />
            {WEIGHT_LABELS[w()]}
          </span>
        )}
      </Show>
      <Show when={props.task.for_whom}>
        {(f) => (
          <span class={`chip chip-for chip-for-${f()}`} title={FOR_WHOM_HINTS[f()]}>
            {FOR_WHOM_LABELS[f()]}
          </span>
        )}
      </Show>
      <Show when={props.task.deferrals > 0 && !isClosed(props.task.status)}>
        <span
          class="chip chip-deferred"
          classList={{ "chip-decision": needsDecision(props.task) }}
          title={needsDecision(props.task) ? `${deferralsLabel(props.task.deferrals)}: beim nächsten Mal entscheiden` : deferralsLabel(props.task.deferrals)}
          aria-label={deferralsLabel(props.task.deferrals)}
        >
          <Icon name="history" size={12} />
          {props.task.deferrals}
        </span>
      </Show>
    </Show>
  );
}

export function TaskMeta(props: { task: Task; showLocation?: boolean; showRef?: boolean; showProgress?: boolean; waitingStill?: boolean }) {
  const progress = () => taskIndex().progress(props.task.id);
  return (
    <span class="task-meta">
      <Show when={props.showRef && props.task.ref}>
        <span class="chip chip-ref mono" title="Kennung">
          {props.task.ref}
        </span>
      </Show>
      <Show when={props.task.type !== "aufgabe"}>
        <span class={`chip chip-type-${props.task.type}`}>{TYPE_LABELS[props.task.type]}</span>
      </Show>
      <Show when={props.task.status !== "open" && props.task.status !== "done"}>
        <span class={`chip chip-status-${props.task.status}`}>{STATUS_LABELS[props.task.status]}</span>
      </Show>
      <Show when={!isClosed(props.task.status) && taskIndex().openBlockersOf(props.task)}>
        {(blockers) => (
          <Show when={blockers().length > 0}>
            <span class="chip chip-status-blocked chip-waiting" title={`Wartet auf: ${blockers().map((b) => (b.ref ? `${b.ref} ${b.title}` : b.title)).join(", ")}`}>
              <Icon name="lock" size={12} />
              {blockers().length}
            </span>
          </Show>
        )}
      </Show>
      <Show when={!isClosed(props.task.status) && props.task.waiting_on}>
        {(name) => (
          <span class="chip chip-waiting-on" title={`Die Entscheidung liegt bei ${name()}`}>
            <Icon name="user" size={12} />
            {waitingLabel(name(), props.waitingStill)}
          </span>
        )}
      </Show>
      <DueChip task={props.task} />
      <PersonalChips task={props.task} />
      <Show when={props.task.recurrence}>
        {(r) => (
          <span class="chip chip-recurring" title={`Wiederkehrend: ${describeRecurrence(r(), props.task.due)}`} aria-label={`Wiederkehrend: ${describeRecurrence(r(), props.task.due)}`} role="img">
            <Icon name="repeat" size={12} />
          </span>
        )}
      </Show>
      <Show when={props.showProgress && progress().total > 0}>
        <span class="chip chip-count" title="Unteraufgaben erledigt">
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
        <span class="chip chip-muted">
          <Icon name="folder" size={12} />
          {locationLabel(taskIndex().location(props.task))}
        </span>
      </Show>
    </span>
  );
}

export function TaskRow(props: TaskRowProps) {
  const [dx, setDx] = createSignal(0);
  const [subtasksOpen, setSubtasksOpen] = createSignal(false);
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
        <CheckButton task={props.task} />
        <button
          class="task-main"
          onClick={() => {
            props.onSelect();
            props.onOpen();
          }}
          onFocus={() => props.onSelect()}
        >
          <span class="task-title">{props.task.title}</span>
          <TaskMeta task={props.task} showLocation={props.showLocation} waitingStill={props.waitingStill} />
        </button>
        <span class="row-side">
          <SubtaskToggle task={props.task} open={subtasksOpen()} onToggle={() => setSubtasksOpen((o) => !o)} />
          <RefButton task={props.task} />
        </span>
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
      <Show when={subtasksOpen()}>
        <div class="row-subtasks">
          <SubtaskTree parentId={props.task.id} compact />
        </div>
      </Show>
    </li>
  );
}
