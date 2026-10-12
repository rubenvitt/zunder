// Links between tasks: the hover card shown for any task link (mentions like
// "ZUN-42" in Markdown, blocker lists), and the "Abhängigkeiten" section of the
// detail with "Blockiert durch" and its reverse "Blockiert".
//
// A task link is an element with `data-task-id` (or `data-task-ref` for mentions)
// inside an `a.task-link`/`a.task-mention`; one listener on the document serves them all.

import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { formatDue } from "../sync/dates";
import { isClosed } from "../sync/derived";
import type { Task } from "../sync/types";
import { locationLabel, STATUS_LABELS, TYPE_LABELS } from "../labels";
import { updateTask } from "../state/actions";
import { now, taskIndex } from "../state/store";
import { openTask, pickTask } from "../state/ui";
import { Icon } from "./Icon";

const LINK_SELECTOR = "a[data-task-id], a[data-task-ref]";
const SHOW_DELAY_MS = 350;
const HIDE_DELAY_MS = 150;

/** The live task an element links to. */
function taskOf(el: Element): Task | undefined {
  const index = taskIndex();
  const id = el.getAttribute("data-task-id");
  const task = id ? index.byId.get(id) : index.byRef(el.getAttribute("data-task-ref") ?? "");
  return task && !index.isDeleted(task) ? task : undefined;
}

/** Link target of a task: its reference if it has one (stable and readable), else its ID. */
export function taskHref(task: Task): string {
  return `#/aufgabe/${encodeURIComponent(task.ref ?? task.id)}`;
}

/** A plain click opens the task in the panel; modifier and middle clicks keep the browser's own behaviour. */
function opensInPlace(e: MouseEvent): boolean {
  return e.button === 0 && !e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey;
}

interface Hover {
  task: Task;
  anchor: DOMRect;
}

/**
 * Hover card for task links (mounted once). Shows reference, title, status, place,
 * due date, progress and blockers after a short delay; moving the pointer onto the
 * card keeps it open, a click on it opens the task. Keyboard focus on a link shows
 * it too. Touch has no hover: there a tap simply opens the task.
 */
export function TaskHoverCard() {
  const [hover, setHover] = createSignal<Hover | null>(null);
  let showTimer: ReturnType<typeof setTimeout> | undefined;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  let current: Element | null = null;
  let pointer: { x: number; y: number } | null = null;
  let card: HTMLDivElement | undefined;

  const show = (el: Element) => {
    clearTimeout(hideTimer);
    if (current === el) return;
    current = el;
    clearTimeout(showTimer);
    showTimer = setTimeout(() => {
      // The editor may have redrawn the mention meanwhile: take the link now under the pointer.
      if (!el.isConnected && pointer) el = document.elementFromPoint(pointer.x, pointer.y)?.closest(LINK_SELECTOR) ?? el;
      current = el;
      const task = taskOf(el);
      setHover(task && el.isConnected ? { task, anchor: el.getBoundingClientRect() } : null);
    }, SHOW_DELAY_MS);
  };
  const hide = (delay = HIDE_DELAY_MS) => {
    clearTimeout(showTimer);
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      current = null;
      setHover(null);
    }, delay);
  };

  onMount(() => {
    const over = (e: PointerEvent) => {
      if (e.pointerType === "touch") return;
      pointer = { x: e.clientX, y: e.clientY };
      const target = e.target as Element | null;
      if (card?.contains(target)) {
        clearTimeout(hideTimer);
        return;
      }
      const link = target?.closest?.(LINK_SELECTOR);
      if (link) show(link);
      else if (current) hide();
    };
    const focusIn = (e: FocusEvent) => {
      pointer = null;
      const link = (e.target as Element | null)?.closest?.(LINK_SELECTOR);
      if (link) show(link);
    };
    // Only focus leaving the link itself ends a card it opened (not focus moving elsewhere meanwhile).
    const focusOut = (e: FocusEvent) => {
      if (current && e.target instanceof Node && current.contains(e.target)) hide();
    };
    const click = (e: MouseEvent) => {
      // Mentions inside the editor are handled there (a click while writing places the caret).
      const target = e.target as Element | null;
      const link = target?.closest?.(LINK_SELECTOR);
      if (!link || !opensInPlace(e) || target?.closest(".md-content")) return;
      const task = taskOf(link);
      if (!task) return;
      e.preventDefault();
      hide(0);
      openTask(task.id);
    };
    const away = () => hide(0);
    // Fixed position: only a scroll that moves the link leaves the card behind.
    const scrolled = (e: Event) => {
      if (current && e.target instanceof Node && e.target.contains(current)) hide(0);
    };
    document.addEventListener("pointerover", over);
    document.addEventListener("focusin", focusIn);
    document.addEventListener("focusout", focusOut);
    document.addEventListener("click", click);
    window.addEventListener("scroll", scrolled, true);
    window.addEventListener("resize", away);
    window.addEventListener("keydown", away);
    onCleanup(() => {
      clearTimeout(showTimer);
      clearTimeout(hideTimer);
      document.removeEventListener("pointerover", over);
      document.removeEventListener("focusin", focusIn);
      document.removeEventListener("focusout", focusOut);
      document.removeEventListener("click", click);
      window.removeEventListener("scroll", scrolled, true);
      window.removeEventListener("resize", away);
      window.removeEventListener("keydown", away);
    });
  });

  return (
    <Show when={hover()}>
      {(h) => {
        // Below the link, or above it when there is no room; never past the window edges.
        const width = Math.min(340, window.innerWidth - 16);
        const left = Math.max(8, Math.min(h().anchor.left, window.innerWidth - width - 8));
        const below = h().anchor.bottom + 6;
        const style = () =>
          below + 220 > window.innerHeight && h().anchor.top > 240
            ? { left: `${left}px`, bottom: `${window.innerHeight - h().anchor.top + 6}px`, width: `${width}px` }
            : { left: `${left}px`, top: `${below}px`, width: `${width}px` };
        // The task may change while the card is open: always read the current one.
        const task = () => taskIndex().byId.get(h().task.id) ?? h().task;
        return (
          <div ref={card} class="task-card" role="tooltip" style={style()} onPointerLeave={() => hide()}>
            <TaskCardBody task={task()} onOpen={() => (hide(0), openTask(task().id))} />
          </div>
        );
      }}
    </Show>
  );
}

function TaskCardBody(props: { task: Task; onOpen: () => void }) {
  const index = () => taskIndex();
  const progress = () => index().progress(props.task.id);
  const openBlockers = () => index().openBlockersOf(props.task);
  const waiting = () => index().blockedBy(props.task.id).filter((t) => !isClosed(t.status));
  return (
    <button type="button" class="task-card-body" onClick={() => props.onOpen()}>
      <span class="task-card-head">
        <Show when={props.task.ref}>
          <span class="mono task-card-ref">{props.task.ref}</span>
        </Show>
        <Show when={props.task.type !== "aufgabe"}>
          <span class={`chip chip-type-${props.task.type}`}>{TYPE_LABELS[props.task.type]}</span>
        </Show>
        <span class={`chip chip-status-${props.task.status}`} data-status={props.task.status}>
          {STATUS_LABELS[props.task.status]}
        </span>
      </span>
      <span class="task-card-title" classList={{ closed: isClosed(props.task.status) }}>
        {props.task.title}
      </span>
      <span class="task-card-meta">
        <span>
          <Icon name="folder" size={12} />
          {locationLabel(index().location(props.task))}
        </span>
        <Show when={props.task.due}>
          <span>
            <Icon name="calendar" size={12} />
            {formatDue(props.task.due, new Date(now()))}
          </span>
        </Show>
        <Show when={progress().total > 0}>
          <span>
            <Icon name="tree" size={12} />
            {progress().done}/{progress().total}
          </span>
        </Show>
      </span>
      <Show when={openBlockers().length > 0}>
        <span class="task-card-blocked">
          <Icon name="lock" size={12} />
          Wartet auf {openBlockers().map((b) => b.ref ?? `„${b.title}“`).join(", ")}
        </span>
      </Show>
      <Show when={waiting().length > 0}>
        <span class="task-card-meta">Blockiert {waiting().length === 1 ? "1 Aufgabe" : `${waiting().length} Aufgaben`}</span>
      </Show>
    </button>
  );
}

/** One task in a relation list: status, reference and title as a link, optional remove button. */
function RelatedTask(props: { task: Task; removeLabel: string; onRemove: () => void }) {
  return (
    <li class="related-task" classList={{ closed: isClosed(props.task.status) }}>
      <span class="status-dot" data-status={props.task.status} title={STATUS_LABELS[props.task.status]} />
      <a class="task-link" href={taskHref(props.task)} data-task-id={props.task.id}>
        <Show when={props.task.ref}>
          <span class="mono task-link-ref">{props.task.ref}</span>
        </Show>
        <span class="task-link-title">{props.task.title}</span>
      </a>
      <Show when={props.task.status !== "open"}>
        <span class={`chip chip-status-${props.task.status}`}>{STATUS_LABELS[props.task.status]}</span>
      </Show>
      <button class="btn btn-ghost btn-icon btn-xs" aria-label={props.removeLabel} title={props.removeLabel} onClick={() => props.onRemove()}>
        <Icon name="x" size={14} />
      </button>
    </li>
  );
}

/**
 * "Blockiert durch" (stored in this task's `blocked_by`) and "Blockiert" (tasks whose
 * `blocked_by` names this one). Both sides can be edited here; the reverse side writes
 * the other task. Entries for deleted tasks are dropped when the list is rewritten.
 */
export function Dependencies(props: { task: Task }) {
  const index = () => taskIndex();
  const blockers = () => index().blockersOf(props.task);
  const waiting = () => index().blockedBy(props.task.id);
  const openBlockers = () => index().openBlockersOf(props.task);
  const live = (ids: readonly string[]) => ids.filter((id) => index().byId.has(id) && !index().isDeleted(index().byId.get(id)!));

  const setBlockers = (task: Task, ids: string[]) => updateTask(task.id, { blocked_by: ids.length > 0 ? ids : null });
  // Tasks that cannot become a blocker of `task`: itself, what it already waits for, and
  // everything waiting for it (directly or not), which would be a cycle.
  const waitingClosure = (id: string) => {
    const out = new Set([id]);
    const stack = [id];
    while (stack.length > 0) {
      for (const t of index().blockedBy(stack.pop()!)) {
        if (!out.has(t.id)) {
          out.add(t.id);
          stack.push(t.id);
        }
      }
    }
    return out;
  };
  const addBlocker = () => {
    const exclude = waitingClosure(props.task.id);
    for (const b of blockers()) exclude.add(b.id);
    pickTask("Blockiert durch …", exclude, (id) => setBlockers(props.task, [...live(props.task.blocked_by), id]));
  };
  const addWaiting = () => {
    // The picked task will wait for this one: it must not be something this one waits for.
    const exclude = new Set([props.task.id, ...waiting().map((t) => t.id)]);
    const stack = [props.task.id];
    while (stack.length > 0) {
      const t = index().byId.get(stack.pop()!);
      for (const b of t ? index().blockersOf(t) : []) {
        if (!exclude.has(b.id)) stack.push(b.id);
        exclude.add(b.id);
      }
    }
    pickTask("Blockiert …", exclude, (id) => {
      const other = index().byId.get(id);
      if (other) setBlockers(other, [...live(other.blocked_by), props.task.id]);
    });
  };

  return (
    <section class="detail-section dependencies">
      <h3>
        <Icon name="lock" size={16} />
        Abhängigkeiten
        <Show when={openBlockers().length > 0}>
          <span class="chip chip-status-blocked" title="Noch nicht erledigte Aufgaben, auf die diese wartet">
            wartet auf {openBlockers().length}
          </span>
        </Show>
      </h3>
      <div class="relation">
        <h4>Blockiert durch</h4>
        <ul class="related-list" aria-label="Blockiert durch">
          <For each={blockers()}>
            {(b) => (
              <RelatedTask
                task={b}
                removeLabel={`„${b.title}“ blockiert nicht mehr`}
                onRemove={() => setBlockers(props.task, live(props.task.blocked_by).filter((id) => id !== b.id))}
              />
            )}
          </For>
        </ul>
        <button class="btn btn-ghost btn-small add-relation" onClick={addBlocker}>
          <Icon name="plus" size={14} /> Blockierende Aufgabe
        </button>
      </div>
      <div class="relation">
        <h4>Blockiert</h4>
        <ul class="related-list" aria-label="Blockiert">
          <For each={waiting()}>
            {(t) => (
              <RelatedTask
                task={t}
                removeLabel={`„${t.title}“ wartet nicht mehr auf diese Aufgabe`}
                onRemove={() => setBlockers(t, live(t.blocked_by).filter((id) => id !== props.task.id))}
              />
            )}
          </For>
        </ul>
        <button class="btn btn-ghost btn-small add-relation" onClick={addWaiting}>
          <Icon name="plus" size={14} /> Wartende Aufgabe
        </button>
      </div>
    </section>
  );
}
