// Task detail side panel: everything inline editable, subtask tree, change history.

import { createEffect, createResource, createSignal, For, on, onCleanup, onMount, Show, type JSX } from "solid-js";
import { fireOf, formatDateLabel, formatInstant, fromDatetimeLocalInput, localDate, parseDue, toDatetimeLocalInput, toRfc3339Local } from "../sync/dates";
import { hasOpenChildren, isClaimActive, isDevLocation, isFlameRunning, isRecurring } from "../sync/derived";
import { hlcWallMs } from "../sync/hlc";
import { PHOENIX_ROLES, TASK_STATUSES, TASK_TYPES, type HistoryEntry, type JsonValue, type Link, type Task, type TaskStatus, type TaskType } from "../sync/types";
import { describeValue, FIELD_LABELS, locationLabel, PHOENIX_LABELS, sourceLabel, STATUS_LABELS, TYPE_LABELS } from "../labels";
import { addComment, createTask, deleteComment, deleteTask, dependentsCount, dropTask, flame, moveTask, setParent, setStatus, updateComment, updateTask } from "../state/actions";
import { detailBack, NARROW_LAYOUT } from "../state/back";
import { appConfig } from "../state/desktop";
import { isTauri } from "../platform/runtime";
import { api, commentsOf, data, now, pendingCount, taskIndex } from "../state/store";
import { copyTaskId, copyText, detailWide, pickLocation, pickTask, selectedId, setDetailId, setDetailWide, setDetailWidth, setSelectedId } from "../state/ui";
import { TextField } from "./Fields";
import { Icon, type IconName } from "./Icon";
import { MarkdownField } from "./MarkdownField";
import { CheckButton, ProgressBar, SubtaskTree } from "./TaskBits";
import { RecurrenceEditor, RecurrenceField, SeriesLinks } from "./RecurrenceField";
import { Dependencies, taskHref } from "./TaskLinks";

/** Only http(s) and mailto links become clickable; anything else (javascript:, data: …) is shown as text. */
export function safeHref(url: string): string | undefined {
  try {
    return ["http:", "https:", "mailto:"].includes(new URL(url).protocol) ? url : undefined;
  } catch {
    return undefined;
  }
}

function focusTaskRow(id: string | null): boolean {
  if (!id) return false;
  const row = document.querySelector<HTMLElement>(`[data-task-id="${CSS.escape(id)}"] .task-main`);
  row?.focus({ preventScroll: true });
  return !!row;
}

export function TaskDetail(props: { id: string }) {
  let aside: HTMLElement | undefined;
  let previous: Element | null = null;
  const focusPanel = () => aside?.focus({ preventScroll: true });
  onMount(() => {
    // Opening moves the focus into the panel (on phones it covers the list).
    previous = document.activeElement;
    if (!aside?.contains(document.activeElement)) focusPanel();
    onCleanup(edgeSwipeBack(aside!));
  });
  onCleanup(() => {
    // Closing returns it to the selected row, unless it already went elsewhere (e.g. the palette).
    const active = document.activeElement;
    if (active && active !== document.body && !aside?.contains(active)) return;
    const id = selectedId();
    // After the update: on phones the list is inert until the panel is gone.
    queueMicrotask(() => {
      if (!focusTaskRow(id) && previous instanceof HTMLElement && previous.isConnected) previous.focus({ preventScroll: true });
    });
  });
  return (
    <aside ref={aside} class="detail" classList={{ wide: detailWide() }} aria-label="Aufgabendetail" tabIndex={-1}>
      <ResizeHandle />
      {/* Keyed: another task gets a fresh body, so unsent drafts never carry over. */}
      <Show when={props.id} keyed>
        {(id) => (
          <Show when={data.tasks[id] && !taskIndex().isDeleted(data.tasks[id]!)} fallback={<MissingTask />}>
            <DetailBody task={data.tasks[id]!} onMount={() => document.activeElement === document.body && focusPanel()} />
          </Show>
        )}
      </Show>
    </aside>
  );
}

const EDGE = 28;
const SETTLE_MS = 180;

/**
 * Phones: swiping right from the left edge goes back, to the parent task or out
 * of the detail (as on iOS; the app's webview has no native back gesture). The
 * panel follows the finger and reveals the list behind it. Touch events rather
 * than pointer events: only they can keep the browser from scrolling.
 */
function edgeSwipeBack(panel: HTMLElement): () => void {
  let start: { x: number; y: number; time: number } | null = null;
  let dragging = false;
  let dx = 0;

  const settle = (transform: string) => {
    panel.style.transition = "";
    panel.style.transform = transform;
  };
  const onStart = (e: TouchEvent) => {
    start = null;
    if (e.touches.length !== 1 || !window.matchMedia(NARROW_LAYOUT).matches) return;
    const touch = e.touches[0]!;
    if (touch.clientX - panel.getBoundingClientRect().left > EDGE) return;
    start = { x: touch.clientX, y: touch.clientY, time: e.timeStamp };
    dragging = false;
  };
  const onMove = (e: TouchEvent) => {
    const touch = e.touches[0];
    if (!start || !touch) return;
    const deltaX = touch.clientX - start.x;
    const deltaY = touch.clientY - start.y;
    if (!dragging) {
      if (Math.abs(deltaX) < 10 && Math.abs(deltaY) < 10) return;
      // Up, down or to the left: scrolling, not back.
      if (deltaX <= Math.abs(deltaY)) {
        start = null;
        return;
      }
      dragging = true;
      panel.classList.add("swiping-back");
      panel.style.transition = "none";
    }
    e.preventDefault();
    dx = Math.max(0, deltaX);
    panel.style.transform = `translateX(${dx}px)`;
  };
  const onEnd = (e: TouchEvent) => {
    const began = start;
    start = null;
    if (!began || !dragging) return;
    dragging = false;
    const speed = dx / Math.max(1, e.timeStamp - began.time);
    const back = e.type === "touchend" && (dx > panel.clientWidth * 0.35 || (dx > 40 && speed > 0.5));
    dx = 0;
    if (!back) {
      settle("");
      setTimeout(() => panel.classList.remove("swiping-back"), SETTLE_MS);
      return;
    }
    settle("translateX(100%)");
    setTimeout(() => {
      // Back to a parent task keeps the panel: put it in place without sliding once the parent is shown.
      const restore = () => {
        window.removeEventListener("popstate", restore);
        clearTimeout(fallback);
        panel.style.transition = "none";
        panel.style.transform = "";
        panel.classList.remove("swiping-back");
        requestAnimationFrame(() => (panel.style.transition = ""));
      };
      window.addEventListener("popstate", restore);
      const fallback = setTimeout(restore, 400);
      detailBack();
    }, SETTLE_MS);
  };

  panel.addEventListener("touchstart", onStart, { passive: true });
  panel.addEventListener("touchmove", onMove, { passive: false });
  panel.addEventListener("touchend", onEnd);
  panel.addEventListener("touchcancel", onEnd);
  return () => {
    panel.removeEventListener("touchstart", onStart);
    panel.removeEventListener("touchmove", onMove);
    panel.removeEventListener("touchend", onEnd);
    panel.removeEventListener("touchcancel", onEnd);
  };
}

/** Drag the left edge to resize the panel (desktop). Double click resets. */
function ResizeHandle() {
  const onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0 || detailWide()) return;
    e.preventDefault();
    const handle = e.currentTarget as HTMLElement;
    handle.setPointerCapture(e.pointerId);
    document.documentElement.classList.add("resizing");
    const move = (ev: PointerEvent) => setDetailWidth(window.innerWidth - ev.clientX);
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
      document.documentElement.classList.remove("resizing");
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  };
  return (
    <div
      class="detail-resize"
      role="separator"
      aria-orientation="vertical"
      aria-label="Breite des Detailbereichs"
      title="Ziehen zum Anpassen, Doppelklick setzt zurück"
      onPointerDown={onPointerDown}
      onDblClick={() => setDetailWidth(560)}
    />
  );
}

function MissingTask() {
  return (
    <div class="detail-body">
      <header class="detail-bar">
        <span class="muted">Aufgabe nicht gefunden oder gelöscht.</span>
        <span class="spacer" />
        <button class="btn btn-ghost btn-icon" aria-label="Schließen (Esc)" title="Schließen (Esc)" onClick={() => setDetailId(null)}>
          <Icon name="x" />
        </button>
      </header>
    </div>
  );
}

function openTask(id: string) {
  setSelectedId(id);
  setDetailId(id);
}

function DetailBody(props: { task: Task; onMount: () => void }) {
  const t = () => props.task;
  // Following a parent/subtask link replaces the body and with it the focused button.
  onMount(() => props.onMount());
  const index = () => taskIndex();
  const location = () => index().location(t());
  const dev = () => isDevLocation(location(), (id) => data.projects[id]);
  const progress = () => index().progress(t().id);
  const parent = () => (t().parent_id ? data.tasks[t().parent_id!] : undefined);
  const ancestors = () => {
    const out: Task[] = [];
    let p = parent();
    while (p && out.length < 12) {
      out.unshift(p);
      p = p.parent_id ? data.tasks[p.parent_id] : undefined;
    }
    return out;
  };
  const showDev = () => dev() || !!(t().plan || t().branch || t().pr_url || t().claimed_by);

  const set = (fields: Record<string, JsonValue>) => updateTask(t().id, fields);
  const [recurrenceOpen, setRecurrenceOpen] = createSignal(false);

  return (
    <div class="detail-body">
      <header class="detail-bar">
        <nav class="crumbs" aria-label="Ort der Aufgabe">
          <Show
            when={!parent()}
            fallback={
              <span class="crumb muted" title="Unteraufgaben liegen immer bei ihrer Wurzelaufgabe">
                <Icon name="folder" size={14} />
                {locationLabel(location())}
              </span>
            }
          >
            <button
              class="crumb crumb-button"
              onClick={() => pickLocation("Verschieben nach", (loc) => moveTask(t().id, loc))}
              aria-label="Stelle ändern"
              title="Verschieben"
            >
              <Icon name={location().kind === "inbox" ? "inbox" : "folder"} size={14} />
              {locationLabel(location())}
            </button>
          </Show>
          <For each={ancestors()}>
            {(a) => (
              <>
                <Icon name="right" size={12} />
                <button class="crumb crumb-button crumb-task" title={a.title} onClick={() => openTask(a.id)}>
                  {a.title}
                </button>
              </>
            )}
          </For>
        </nav>
        <span class="spacer" />
        <RefCopy task={t()} />
        <button
          class="btn btn-ghost btn-icon hide-mobile"
          aria-label={detailWide() ? "Schmale Ansicht" : "Breite Ansicht"}
          title={detailWide() ? "Schmale Ansicht" : "Breite Ansicht"}
          aria-pressed={detailWide()}
          onClick={() => setDetailWide(!detailWide())}
        >
          <Icon name={detailWide() ? "collapse" : "expand"} />
        </button>
        <button class="btn btn-ghost btn-icon" aria-label="Schließen (Esc)" title="Schließen (Esc)" onClick={() => setDetailId(null)}>
          <Icon name="x" />
        </button>
      </header>

      <div class="detail-scroll">
        <div class="detail-content">
          <div class="detail-head">
            <CheckButton task={t()} />
            <TextField wrap class="detail-title" label="Titel" value={t().title} onCommit={(v) => set({ title: v })} />
          </div>

          <Show when={t().status === "done" && hasOpenChildren(index().childrenOf(t().id))}>
            <p class="notice notice-warning">Hinweis: Diese Aufgabe ist erledigt, hat aber noch offene Unteraufgaben.</p>
          </Show>

          <div class="props">
            <Prop icon="circle" label="Status">
              <select
                class="prop-select status-select"
                data-status={t().status}
                aria-label="Status"
                value={t().status}
                onChange={(e) => {
                  const status = e.currentTarget.value as TaskStatus;
                  if (status === "dropped" && isRecurring(t())) {
                    // Asks first: only this instance or the whole series (the dialog writes the status).
                    e.currentTarget.value = t().status;
                    dropTask(t().id);
                  } else if (!setStatus(t().id, status)) e.currentTarget.value = t().status;
                }}
              >
                <For each={TASK_STATUSES.filter((s) => s !== "review" || dev() || t().status === "review")}>
                  {(s) => <option value={s}>{STATUS_LABELS[s]}</option>}
                </For>
              </select>
            </Prop>

            <Prop icon={t().type === "bug" ? "bug" : t().type === "feature" ? "star" : "tag"} label="Typ">
              <select
                class="prop-select"
                aria-label="Typ"
                value={t().type}
                disabled={!dev() && t().type === "aufgabe"}
                title={dev() ? undefined : "Bugs und Features gibt es nur in Dev-Projekten."}
                onChange={(e) => {
                  if (!set({ type: e.currentTarget.value as TaskType })) e.currentTarget.value = t().type;
                }}
              >
                <For each={TASK_TYPES.filter((ty) => ty === "aufgabe" || dev() || t().type === ty)}>{(ty) => <option value={ty}>{TYPE_LABELS[ty]}</option>}</For>
              </select>
            </Prop>

            <Prop icon="calendar" label="Fällig">
              <span class="prop-inline">
                <DateSlot isSet={!!t().due} label="Fälligkeit setzen">
                  {(autoOpen) => <DueEditor task={t()} autoOpen={autoOpen} />}
                </DateSlot>
                <UndatedAck task={t()} />
              </span>
            </Prop>

            {/* Only root tasks can recur (contract 2.5). */}
            <Show when={!t().parent_id}>
              <Prop icon="repeat" label="Wiederholen">
                <RecurrenceField task={t()} editorOpen={recurrenceOpen()} onToggleEditor={setRecurrenceOpen} />
                <Show when={recurrenceOpen() && !!t().due && t().recurrence}>{(r) => <RecurrenceEditor task={t()} recurrence={r()} />}</Show>
              </Prop>
            </Show>

            <Show when={index().previousInstance(t()) || index().nextInstance(t().id)}>
              <Prop icon="history" label="Serie">
                <SeriesLinks task={t()} />
              </Prop>
            </Show>

            <Prop icon="bell" label="Erinnerung">
              <DateSlot isSet={!!t().remind_at} label="Erinnerung setzen">
                {(autoOpen) => (
                  <span class="prop-inline nowrap">
                    <input
                      ref={(el) => autoOpen(el)}
                      class="prop-input"
                      type="datetime-local"
                      aria-label="Erinnerung"
                      value={toDatetimeLocalInput(t().remind_at)}
                      onChange={(e) => set({ remind_at: fromDatetimeLocalInput(e.currentTarget.value) })}
                    />
                    <Show when={t().remind_at}>
                      <button class="btn btn-ghost btn-icon btn-xs" aria-label="Erinnerung entfernen" onClick={() => set({ remind_at: null })}>
                        <Icon name="x" size={14} />
                      </button>
                    </Show>
                  </span>
                )}
              </DateSlot>
            </Prop>

            <Prop icon="arrowUp" label="Eltern">
              <span class="prop-inline">
                <Show when={parent()}>
                  <button class="prop-value link-button prop-link" title={parent()!.title} onClick={() => openTask(parent()!.id)}>
                    {parent()!.title}
                  </button>
                </Show>
                <button
                  class="btn btn-ghost btn-small"
                  onClick={() => {
                    const exclude = new Set([t().id, ...index().descendantsOf(t().id).map((d) => d.id)]);
                    pickTask("Unter Aufgabe hängen", exclude, (pid) => setParent(t().id, pid));
                  }}
                >
                  {parent() ? "Ändern" : "Unterordnen"}
                </button>
                <Show when={parent()}>
                  <button class="btn btn-ghost btn-small" onClick={() => setParent(t().id, null)}>
                    Lösen
                  </button>
                </Show>
              </span>
            </Prop>

            <Prop icon="flame" label="Phönix">
              <span class="prop-inline">
                <select
                  class="prop-select"
                  aria-label="Phönix-Rolle"
                  value={t().phoenix?.role ?? ""}
                  onChange={(e) => {
                    const role = e.currentTarget.value;
                    set({ phoenix: role ? { role, fire: t().phoenix?.fire ?? fireOf(new Date(now())) } : null });
                  }}
                >
                  <option value="">—</option>
                  <For each={PHOENIX_ROLES}>{(r) => <option value={r}>{PHOENIX_LABELS[r]}</option>}</For>
                </select>
                <Show when={t().phoenix}>
                  <TextField
                    class="prop-input fire-input mono"
                    label="Feuer (ISO-Woche)"
                    value={t().phoenix!.fire}
                    placeholder="2026-W45"
                    onCommit={(fire) => set({ phoenix: { role: t().phoenix!.role, fire: fire.trim() } })}
                  />
                </Show>
              </span>
            </Prop>
          </div>

          <Show when={t().phoenix}>
            <div class="flame-strip" classList={{ burning: isFlameRunning(t()) }}>
              <Icon name="flame" size={18} />
              <span>{isFlameRunning(t()) ? "Flamme brennt" : "Bereit für eine Flamme"}</span>
              <span class="spacer" />
              <button class="btn btn-small" classList={{ "btn-primary": !isFlameRunning(t()) }} onClick={() => void flame(t().id, isFlameRunning(t()) ? "stop" : "start")}>
                <Icon name={isFlameRunning(t()) ? "stop" : "play"} size={14} />
                {isFlameRunning(t()) ? "Flamme beenden" : "Flamme starten"}
              </button>
            </div>
          </Show>

          <section class="detail-section">
            <h3>
              <Icon name="document" size={16} />
              Beschreibung
            </h3>
            <MarkdownField label="Beschreibung (Markdown)" placeholder="Beschreibung schreiben … Markdown geht auch: **fett**, - Liste, [ ] Checkliste" value={t().description} onCommit={(v) => set({ description: v || null })} />
          </section>

          <section class="detail-section">
            <h3>
              <Icon name="tree" size={16} />
              Unteraufgaben
              <Show when={progress().total > 0}>
                <span class="chip chip-count">
                  {progress().done}/{progress().total}
                </span>
                <ProgressBar done={progress().done} total={progress().total} label="Unteraufgaben erledigt" />
              </Show>
            </h3>
            <SubtaskTree parentId={t().id} />
            <AddSubtask parentId={t().id} />
          </section>

          <Dependencies task={t()} />

          <Show when={showDev()}>
            <section class="detail-section dev-section">
              <h3>
                <Icon name="code" size={16} />
                Dev
              </h3>
              <div class="props">
                <Prop icon="branch" label="Branch">
                  <span class="prop-inline">
                    <TextField class="prop-input mono" label="Branch" value={t().branch} placeholder="feature/…" onCommit={(v) => set({ branch: v.trim() || null })} />
                    <Show when={t().branch}>
                      <button class="btn btn-ghost btn-icon btn-xs" aria-label="Branch kopieren" title="Branch kopieren" onClick={() => copyText(t().branch!, `Branch kopiert: ${t().branch}`)}>
                        <Icon name="copy" size={14} />
                      </button>
                    </Show>
                  </span>
                </Prop>
                <Prop icon="pullRequest" label="PR">
                  <span class="prop-inline">
                    <TextField class="prop-input" type="url" label="PR-Link" value={t().pr_url} placeholder="https://github.com/…" onCommit={(v) => set({ pr_url: v.trim() || null })} />
                    <Show when={t().pr_url && safeHref(t().pr_url!)}>
                      {(href) => (
                        <a class="btn btn-ghost btn-icon btn-xs" href={href()} target="_blank" rel="noopener noreferrer" aria-label="PR öffnen" title="PR öffnen">
                          <Icon name="external" size={14} />
                        </a>
                      )}
                    </Show>
                  </span>
                </Prop>
                <Prop icon="user" label="Reserviert">
                  <Show when={t().claimed_by} fallback={<span class="prop-value muted">—</span>}>
                    <span class="prop-inline">
                      <span class="prop-value">
                        {t().claimed_by}
                        <Show when={t().claim_expires_at}>
                          <span class="muted">
                            {" "}
                            bis {formatInstant(t().claim_expires_at, new Date(now()))}
                            {isClaimActive(t(), now()) ? "" : " (abgelaufen)"}
                          </span>
                        </Show>
                      </span>
                      <button class="btn btn-ghost btn-small" onClick={() => set({ claimed_by: null, claim_expires_at: null })}>
                        Aufheben
                      </button>
                    </span>
                  </Show>
                </Prop>
              </div>
              <h4>Plan</h4>
              <MarkdownField label="Plan (Markdown)" placeholder="Plan …" value={t().plan} onCommit={(v) => set({ plan: v || null })} />
            </section>
          </Show>

          <section class="detail-section">
            <h3>
              <Icon name="link" size={16} />
              Links
            </h3>
            <LinksEditor task={t()} />
          </section>

          <section class="detail-section">
            <h3>
              <Icon name="archive" size={16} />
              Vault
            </h3>
            <VaultLinksEditor task={t()} />
          </section>

          <Comments taskId={t().id} />

          <History taskId={t().id} version={t().updated_hlc} />

          <section class="detail-section detail-meta">
            <span>Erstellt {hlcLabel(t().created_hlc)}</span>
            <span>Geändert {hlcLabel(t().updated_hlc)}</span>
            <button class="link-button" title="Dauerhaften Link zu dieser Aufgabe kopieren (z. B. für den Vault)" onClick={() => copyText(taskUrl(t()), "Link kopiert")}>
              Link kopieren
            </button>
            <Show when={t().ref}>
              <button class="link-button" title="ID der Aufgabe kopieren" onClick={() => copyTaskId({ id: t().id, ref: null })}>
                ID kopieren
              </button>
            </Show>
            <Show when={t().clickup_id}>
              <span class="mono">ClickUp {t().clickup_id}</span>
            </Show>
            <Show when={t().timing_ref}>
              <span class="mono">Timing {t().timing_ref}</span>
            </Show>
            <button
              class="btn btn-danger btn-small"
              onClick={() => {
                const count = dependentsCount("task", t().id);
                const msg = count > 0 ? `„${t().title}“ und ${count} Unteraufgabe(n) löschen?` : `„${t().title}“ löschen?`;
                if (confirm(msg) && deleteTask(t().id)) setDetailId(null);
              }}
            >
              <Icon name="trash" size={14} /> Löschen
            </button>
          </section>
        </div>
      </div>
    </div>
  );
}

/** The task's reference (or ID) at the top of the panel: one click copies it. */
function RefCopy(props: { task: Task }) {
  const kind = () => (props.task.ref ? "Kennung" : "ID");
  return (
    <button
      class="btn btn-ghost btn-small mono task-id"
      title={`${kind()} kopieren (y)`}
      aria-label={props.task.ref ? "Kennung kopieren" : "ID kopieren"}
      onClick={() => copyTaskId(props.task)}
    >
      <Icon name="copy" size={14} />
      <span class="task-id-text">{props.task.ref ?? "ID"}</span>
    </button>
  );
}

function Prop(props: { icon: IconName; label: string; children: JSX.Element }) {
  return (
    <div class="prop">
      <span class="prop-label">
        <Icon name={props.icon} size={15} />
        {props.label}
      </span>
      <div class="prop-control">{props.children}</div>
    </div>
  );
}

/**
 * Full, permanent address of a task (by reference if it has one). In the app the
 * webview's own origin is useless outside it, so the link points to the server.
 */
function taskUrl(task: Task): string {
  const server = isTauri() ? appConfig()?.server_url?.replace(/\/+$/, "") : null;
  return `${server ? `${server}/` : `${location.origin}${location.pathname}`}${taskHref(task)}`;
}

/** German relative label for the wall time of an HLC. */
function hlcLabel(hlc: string): string {
  const ms = hlcWallMs(hlc);
  return Number.isFinite(ms) ? formatInstant(new Date(ms).toISOString(), new Date(now())) : "";
}

/**
 * An unset date shows a quiet "set" button instead of an empty native field
 * (tt.mm.jjjj, --:--); clicking it reveals the field and opens its picker.
 */
function DateSlot(props: { isSet: boolean; label: string; children: (autoOpen: (el: HTMLInputElement) => void) => JSX.Element }) {
  const [editing, setEditing] = createSignal(false);
  const autoOpen = (el: HTMLInputElement) => {
    if (!editing() || props.isSet) return;
    queueMicrotask(() => {
      el.focus();
      try {
        el.showPicker?.();
      } catch {
        // Some browsers allow the picker only on a direct click.
      }
    });
    el.addEventListener("blur", () => setTimeout(() => !props.isSet && setEditing(false), 150), { once: true });
  };
  return (
    <Show
      when={props.isSet || editing()}
      fallback={
        <button class="prop-input unset prop-empty" onClick={() => setEditing(true)}>
          {props.label}
        </button>
      }
    >
      {props.children(autoOpen)}
    </Show>
  );
}

/**
 * "Bewusst ohne Fälligkeit": records the decision to leave a task undated, so the
 * weekly deadline review (filter_tasks with has_due/undated_ack) skips it.
 */
function UndatedAck(props: { task: Task }) {
  const since = () => {
    const at = new Date(props.task.undated_ack!);
    return Number.isNaN(at.getTime()) ? props.task.undated_ack! : formatDateLabel(localDate(at), new Date(now()));
  };
  return (
    <Show when={!props.task.due}>
      <Show
        when={props.task.undated_ack}
        fallback={
          <button
            class="btn btn-ghost btn-small"
            title="Entscheidung festhalten: Die Deadline-Prüfung überspringt diese Aufgabe."
            onClick={() => updateTask(props.task.id, { undated_ack: toRfc3339Local(new Date()) })}
          >
            Bewusst ohne
          </button>
        }
      >
        <span class="chip undated-ack" title={`Bewusst ohne Fälligkeit, entschieden ${formatInstant(props.task.undated_ack, new Date(now()))}`}>
          Bewusst ohne · {since()}
          <button
            class="btn btn-ghost btn-icon btn-xs"
            aria-label="Markierung „bewusst ohne Fälligkeit“ entfernen"
            onClick={() => updateTask(props.task.id, { undated_ack: null })}
          >
            <Icon name="x" size={12} />
          </button>
        </span>
      </Show>
    </Show>
  );
}

function DueEditor(props: { task: Task; autoOpen?: (el: HTMLInputElement) => void }) {
  const info = () => parseDue(props.task.due);
  // A due date supersedes the decision to leave the task undated.
  const setDue = (due: string) => updateTask(props.task.id, props.task.undated_ack ? { due, undated_ack: null } : { due });
  // A recurring task needs its due date (contract 2.5): removing it ends the series, after asking.
  const removeDue = () => {
    if (!props.task.recurrence) return updateTask(props.task.id, { due: null });
    if (!confirm("Ohne Fälligkeit endet die Serie.")) return false;
    return updateTask(props.task.id, { due: null, recurrence: null });
  };
  const commit = (date: string, time: string) => {
    if (!date) {
      if (!removeDue() && dateInput) dateInput.value = info()?.date ?? "";
      return;
    }
    if (!time) {
      setDue(date);
      return;
    }
    const [y, m, d] = date.split("-").map(Number);
    const [hh, mm] = time.split(":").map(Number);
    setDue(toRfc3339Local(new Date(y!, m! - 1, d!, hh!, mm!)));
  };
  let dateInput: HTMLInputElement | undefined;
  let timeInput: HTMLInputElement | undefined;
  return (
    <span class="prop-inline nowrap">
      <input
        ref={(el) => {
          dateInput = el;
          props.autoOpen?.(el);
        }}
        class="prop-input date-input"
        classList={{ unset: !info() }}
        type="date" aria-label="Fälligkeitsdatum" value={info()?.date ?? ""} onChange={(e) => commit(e.currentTarget.value, timeInput?.value ?? "")} />
      <input
        ref={timeInput}
        class="prop-input time-input"
        classList={{ unset: !info()?.time }}
        type="time"
        aria-label="Fälligkeitsuhrzeit"
        value={info()?.time ?? ""}
        disabled={!info()}
        onChange={(e) => commit(dateInput?.value ?? "", e.currentTarget.value)}
      />
      <Show when={props.task.due}>
        <button class="btn btn-ghost btn-icon btn-xs" aria-label="Fälligkeit entfernen" onClick={() => removeDue()}>
          <Icon name="x" size={14} />
        </button>
      </Show>
    </span>
  );
}

function AddSubtask(props: { parentId: string }) {
  const [title, setTitle] = createSignal("");
  return (
    <form
      class="add-row"
      onSubmit={(e) => {
        e.preventDefault();
        // The field keeps the focus: type, Enter, type, Enter … for a whole checklist.
        if (title().trim() && createTask({ title: title(), parent_id: props.parentId })) setTitle("");
      }}
    >
      <Icon name="plus" size={16} />
      <input
        class="add-input"
        placeholder="Unteraufgabe hinzufügen … (Enter)"
        aria-label="Neue Unteraufgabe"
        value={title()}
        onInput={(e) => setTitle(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && title()) {
            e.stopPropagation();
            setTitle("");
          }
        }}
      />
      <Show when={title().trim()}>
        <button class="btn btn-primary btn-small" type="submit" aria-label="Unteraufgabe anlegen">
          Anlegen
        </button>
      </Show>
    </form>
  );
}

function LinksEditor(props: { task: Task }) {
  const [url, setUrl] = createSignal("");
  const [title, setTitle] = createSignal("");
  const save = (links: Link[]) =>
    updateTask(props.task.id, { links: links.map((l): JsonValue => (l.title ? { url: l.url, title: l.title } : { url: l.url })) });
  return (
    <>
      <ul class="link-list">
        <For each={props.task.links}>
          {(link, i) => (
            <li class="link-item">
              <Icon name="link" size={14} />
              <Show when={safeHref(link.url)} fallback={<span class="mono link-text" title="Kein http(s)- oder mailto-Link">{link.title || link.url}</span>}>
                {(href) => (
                  <a class="link-text" href={href()} target="_blank" rel="noopener noreferrer" title={link.url}>
                    {link.title || link.url}
                  </a>
                )}
              </Show>
              <Show when={link.title}>
                <span class="link-host muted">{hostOf(link.url)}</span>
              </Show>
              <button class="btn btn-ghost btn-icon btn-xs" aria-label="Link entfernen" onClick={() => save(props.task.links.filter((_, j) => j !== i()))}>
                <Icon name="x" size={14} />
              </button>
            </li>
          )}
        </For>
      </ul>
      <form
        class="add-row"
        onSubmit={(e) => {
          e.preventDefault();
          const u = url().trim();
          if (!u) return;
          if (save([...props.task.links, { url: u, title: title().trim() || undefined }])) {
            setUrl("");
            setTitle("");
          }
        }}
      >
        <Icon name="plus" size={16} />
        <input class="add-input" type="url" placeholder="Link hinzufügen: https://…" aria-label="Link-URL" value={url()} onInput={(e) => setUrl(e.currentTarget.value)} />
        <input class="add-input add-input-short" placeholder="Titel (optional)" aria-label="Link-Titel" value={title()} onInput={(e) => setTitle(e.currentTarget.value)} />
        <button class="btn btn-small" type="submit" disabled={!url().trim()} aria-label="Link hinzufügen">
          Hinzufügen
        </button>
      </form>
    </>
  );
}

function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return "";
  }
}

function VaultLinksEditor(props: { task: Task }) {
  const [value, setValue] = createSignal("");
  const save = (links: string[]) => updateTask(props.task.id, { vault_links: links });
  const href = (link: string) => (link.startsWith("obsidian://") ? link : `obsidian://open?file=${encodeURIComponent(link)}`);
  return (
    <>
      <ul class="link-list">
        <For each={props.task.vault_links}>
          {(link, i) => (
            <li class="link-item">
              <Icon name="document" size={14} />
              <a class="mono link-text" href={href(link)}>
                {link}
              </a>
              <button class="btn btn-ghost btn-icon btn-xs" aria-label="Vault-Link entfernen" onClick={() => save(props.task.vault_links.filter((_, j) => j !== i()))}>
                <Icon name="x" size={14} />
              </button>
            </li>
          )}
        </For>
      </ul>
      <form
        class="add-row"
        onSubmit={(e) => {
          e.preventDefault();
          const v = value().trim();
          if (v && save([...props.task.vault_links, v])) setValue("");
        }}
      >
        <Icon name="plus" size={16} />
        <input class="add-input" placeholder="Pfad im Vault oder obsidian://…" aria-label="Vault-Link" value={value()} onInput={(e) => setValue(e.currentTarget.value)} />
        <button class="btn btn-small" type="submit" disabled={!value().trim()} aria-label="Vault-Link hinzufügen">
          Hinzufügen
        </button>
      </form>
    </>
  );
}

/** Comments of a task (notes, status updates from agents), oldest first. */
function Comments(props: { taskId: string }) {
  const [draft, setDraft] = createSignal("");
  const comments = () => commentsOf(props.taskId);
  const send = () => {
    if (draft().trim() && addComment(props.taskId, draft())) setDraft("");
  };
  return (
    <section class="detail-section comments">
      <h3>
        <Icon name="comment" size={16} />
        Kommentare
        <Show when={comments().length > 0}>
          <span class="chip chip-count">{comments().length}</span>
        </Show>
      </h3>
      <ul class="comment-list">
        <For each={comments()}>
          {(c) => (
            <li class="comment" data-comment-id={c.id}>
              <div class="comment-head">
                <span class="comment-author" classList={{ agent: !!c.author }}>
                  {c.author ?? "Du"}
                </span>
                <span class="muted" title={new Date(hlcWallMs(c.created_hlc)).toLocaleString("de-DE")}>
                  {hlcLabel(c.created_hlc)}
                </span>
                <span class="spacer" />
                <button
                  class="btn btn-ghost btn-icon btn-xs"
                  aria-label="Kommentar löschen"
                  title="Kommentar löschen"
                  onClick={() => confirm("Kommentar löschen?") && deleteComment(c.id)}
                >
                  <Icon name="trash" size={14} />
                </button>
              </div>
              <MarkdownField
                class="comment-body"
                label="Kommentar (Markdown)"
                value={c.body}
                onCommit={(v) => {
                  // An emptied comment keeps its text; deleting is explicit.
                  if (v.trim()) updateComment(c.id, v);
                }}
              />
            </li>
          )}
        </For>
      </ul>
      <form
        class="comment-form"
        onSubmit={(e) => {
          e.preventDefault();
          send();
        }}
      >
        <textarea
          class="field comment-input"
          rows={2}
          placeholder="Kommentar schreiben … (Markdown, ⌘/Strg+Enter sendet)"
          aria-label="Neuer Kommentar"
          value={draft()}
          onInput={(e) => setDraft(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              send();
            } else if (e.key === "Escape" && draft()) {
              e.stopPropagation();
              setDraft("");
            }
          }}
        />
        <Show when={draft().trim()}>
          <button class="btn btn-primary btn-small" type="submit">
            Kommentieren
          </button>
        </Show>
      </form>
    </section>
  );
}

const [historyOpen, setHistoryOpen] = createSignal(false);

function History(props: { taskId: string; version: string }) {
  // Refetch shortly after changes settle (pushed and confirmed).
  const [key, setKey] = createSignal({ id: props.taskId, n: 0 });
  let timer: ReturnType<typeof setTimeout> | undefined;
  createEffect(
    on(
      () => [props.taskId, props.version, pendingCount()] as const,
      ([id, , pending], prev) => {
        clearTimeout(timer);
        if (!prev || prev[0] !== id) {
          setKey({ id, n: 0 });
          return;
        }
        if (pending === 0) timer = setTimeout(() => setKey((k) => ({ id, n: k.n + 1 })), 600);
      },
    ),
  );
  onCleanup(() => clearTimeout(timer));
  const [entries] = createResource(key, async ({ id }): Promise<HistoryEntry[] | null> => {
    try {
      return await api().history(id);
    } catch {
      return null;
    }
  });
  const sorted = () => [...(entries() ?? [])].sort((a, b) => b.seq - a.seq);
  return (
    <details class="detail-section history" open={historyOpen()} onToggle={(e) => setHistoryOpen(e.currentTarget.open)}>
      <summary>
        <h3>
          <Icon name="history" size={16} />
          Änderungsprotokoll
          <Show when={(entries()?.length ?? 0) > 0}>
            <span class="chip chip-count">{entries()!.length}</span>
          </Show>
          <Icon name="down" size={14} />
        </h3>
      </summary>
      <Show when={entries() !== null} fallback={<p class="muted">Protokoll nicht verfügbar.</p>}>
        <ul class="history-list">
          <For each={sorted()} fallback={<li class="muted">{entries.loading ? "Lädt …" : "Noch keine Einträge."}</li>}>
            {(entry) => (
              <li>
                <span class="history-time">{hlcLabel(entry.op.hlc)}</span>
                <span>
                  <strong>{FIELD_LABELS[entry.op.field] ?? entry.op.field}</strong> → {describeValue(entry.op.field, entry.op.value)}
                </span>
                <span class="history-source" title={entry.source}>
                  {sourceLabel(entry.source)}
                </span>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </details>
  );
}
