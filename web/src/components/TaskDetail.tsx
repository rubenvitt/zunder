// Task detail side panel: everything inline editable, subtask tree, change history.

import { createEffect, createResource, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { fireOf, formatInstant, fromDatetimeLocalInput, parseDue, toDatetimeLocalInput, toRfc3339Local } from "../sync/dates";
import { hasOpenChildren, isClaimActive, isClosed, isDevLocation, isFlameRunning } from "../sync/derived";
import { hlcWallMs } from "../sync/hlc";
import { PHOENIX_ROLES, TASK_STATUSES, TASK_TYPES, type HistoryEntry, type JsonValue, type Link, type Task, type TaskStatus, type TaskType } from "../sync/types";
import { FIELD_LABELS, locationLabel, PHOENIX_LABELS, sourceLabel, STATUS_LABELS, TYPE_LABELS } from "../labels";
import { createTask, deleteTask, dependentsCount, flame, moveTask, setParent, setStatus, toggleDone, updateTask } from "../state/actions";
import { api, data, now, pendingCount, taskIndex } from "../state/store";
import { copyTaskId, pickLocation, pickTask, selectedId, setDetailId, setSelectedId } from "../state/ui";
import { Row, TextField } from "./Fields";
import { Icon } from "./Icon";

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
    <aside ref={aside} class="detail" aria-label="Aufgabendetail" tabIndex={-1}>
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

function MissingTask() {
  return (
    <div class="detail-body">
      <header class="detail-head">
        <span class="muted">Aufgabe nicht gefunden oder gelöscht.</span>
        <button class="btn btn-ghost btn-icon" aria-label="Schließen" onClick={() => setDetailId(null)}>
          <Icon name="x" />
        </button>
      </header>
    </div>
  );
}

function DetailBody(props: { task: Task; onMount: () => void }) {
  const t = () => props.task;
  // Following a parent/subtask link replaces the body and with it the focused button.
  onMount(() => props.onMount());
  const index = () => taskIndex();
  const location = () => index().location(t());
  const dev = () => isDevLocation(location(), (id) => data.projects[id]);
  const children = () => index().childrenOf(t().id);
  const progress = () => index().progress(t().id);
  const parent = () => (t().parent_id ? data.tasks[t().parent_id!] : undefined);
  const showDev = () => dev() || !!(t().plan || t().branch || t().pr_url || t().claimed_by);

  const set = (fields: Record<string, JsonValue>) => updateTask(t().id, fields);

  return (
    <div class="detail-body">
      <header class="detail-head">
        <button
          class="check"
          classList={{ done: t().status === "done", dropped: t().status === "dropped" }}
          aria-label={t().status === "done" ? "Als offen markieren" : "Erledigt"}
          onClick={() => toggleDone(t().id)}
        >
          <Show when={t().status === "done"}>
            <Icon name="check" size={14} />
          </Show>
        </button>
        <TextField class="detail-title" label="Titel" value={t().title} onCommit={(v) => set({ title: v })} />
        <button class="btn btn-ghost btn-icon" aria-label="Schließen (Esc)" title="Schließen (Esc)" onClick={() => setDetailId(null)}>
          <Icon name="x" />
        </button>
      </header>

      <Show when={t().status === "done" && hasOpenChildren(children())}>
        <p class="notice notice-warning">Hinweis: Diese Aufgabe ist erledigt, hat aber noch offene Unteraufgaben.</p>
      </Show>

      <div class="detail-grid">
        <Show when={showDev()}>
          <Row label="ID">
            <button class="btn btn-ghost btn-small mono task-id" title="ID kopieren (y)" aria-label="ID kopieren" onClick={() => copyTaskId(t().id)}>
              <span class="task-id-text">{t().id}</span>
              <Icon name="copy" size={14} />
            </button>
          </Row>
        </Show>

        <Row label="Status">
          <select
            class="field"
            aria-label="Status"
            value={t().status}
            onChange={(e) => {
              if (!setStatus(t().id, e.currentTarget.value as TaskStatus)) e.currentTarget.value = t().status;
            }}
          >
            <For each={TASK_STATUSES.filter((s) => s !== "review" || dev() || t().status === "review")}>
              {(s) => <option value={s}>{STATUS_LABELS[s]}</option>}
            </For>
          </select>
        </Row>

        <Row label="Typ">
          <select
            class="field"
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
        </Row>

        <Row label="Stelle">
          <Show
            when={parent()}
            fallback={
              <button
                class="btn btn-ghost btn-small"
                onClick={() => pickLocation("Verschieben nach", (loc) => moveTask(t().id, loc))}
                aria-label="Stelle ändern"
              >
                {locationLabel(location())}
              </button>
            }
          >
            <span class="muted">{locationLabel(location())} (über die Elternaufgabe)</span>
          </Show>
        </Row>

        <Row label="Eltern">
          <span class="inline-group">
            <Show when={parent()} fallback={<span class="muted">—</span>}>
              <button
                class="link-button"
                onClick={() => {
                  setSelectedId(parent()!.id);
                  setDetailId(parent()!.id);
                }}
              >
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
        </Row>

        <Row label="Fällig">
          <DueEditor task={t()} />
        </Row>

        <Row label="Erinnerung">
          <span class="inline-group">
            <input
              class="field"
              type="datetime-local"
              aria-label="Erinnerung"
              value={toDatetimeLocalInput(t().remind_at)}
              onChange={(e) => set({ remind_at: fromDatetimeLocalInput(e.currentTarget.value) })}
            />
            <Show when={t().remind_at}>
              <button class="btn btn-ghost btn-small" aria-label="Erinnerung entfernen" onClick={() => set({ remind_at: null })}>
                <Icon name="x" size={14} />
              </button>
            </Show>
          </span>
        </Row>

        <Row label="Phönix">
          <span class="inline-group">
            <select
              class="field"
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
                class="field-short mono"
                label="Feuer (ISO-Woche)"
                value={t().phoenix!.fire}
                placeholder="2026-W45"
                onCommit={(fire) => set({ phoenix: { role: t().phoenix!.role, fire: fire.trim() } })}
              />
              <button class="btn btn-small" classList={{ "btn-primary": !isFlameRunning(t()) }} onClick={() => void flame(t().id, isFlameRunning(t()) ? "stop" : "start")}>
                <Icon name={isFlameRunning(t()) ? "stop" : "play"} size={14} />
                {isFlameRunning(t()) ? "Flamme beenden" : "Flamme starten"}
              </button>
            </Show>
          </span>
        </Row>
      </div>

      <section class="detail-section">
        <h3>Beschreibung</h3>
        <TextField multiline rows={5} label="Beschreibung (Markdown)" placeholder="Markdown …" value={t().description} onCommit={(v) => set({ description: v || null })} />
      </section>

      <section class="detail-section">
        <h3>
          Unteraufgaben
          <Show when={progress().total > 0}>
            <span class="chip">
              {progress().done}/{progress().total}
            </span>
          </Show>
        </h3>
        <ul class="subtree">
          <For each={children()}>{(child) => <SubtaskNode task={child} depth={0} />}</For>
        </ul>
        <AddSubtask parentId={t().id} />
      </section>

      <section class="detail-section">
        <h3>Links</h3>
        <LinksEditor task={t()} />
      </section>

      <section class="detail-section">
        <h3>Vault</h3>
        <VaultLinksEditor task={t()} />
      </section>

      <Show when={showDev()}>
        <section class="detail-section">
          <h3>Dev</h3>
          <div class="detail-grid">
            <Row label="Branch">
              <TextField class="mono" label="Branch" value={t().branch} placeholder="feature/…" onCommit={(v) => set({ branch: v.trim() || null })} />
            </Row>
            <Row label="PR">
              <span class="inline-group">
                <TextField type="url" label="PR-Link" value={t().pr_url} placeholder="https://github.com/…" onCommit={(v) => set({ pr_url: v.trim() || null })} />
                <Show when={t().pr_url}>
                  <a class="btn btn-ghost btn-small" href={t().pr_url!} target="_blank" rel="noopener noreferrer">
                    Öffnen
                  </a>
                </Show>
              </span>
            </Row>
            <Row label="Reserviert">
              <Show when={t().claimed_by} fallback={<span class="muted">—</span>}>
                <span class="inline-group">
                  <span>
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
            </Row>
          </div>
          <TextField multiline rows={6} label="Plan (Markdown)" placeholder="Plan …" value={t().plan} onCommit={(v) => set({ plan: v || null })} />
        </section>
      </Show>

      <section class="detail-section detail-meta">
        <span>Erstellt {hlcLabel(t().created_hlc)}</span>
        <span>Geändert {hlcLabel(t().updated_hlc)}</span>
        <a href={`#/aufgabe/${t().id}`} title="Dauerhafter Link zu dieser Aufgabe (z. B. für den Vault)">
          Link
        </a>
        <Show when={!showDev()}>
          <button class="link-button" title="ID kopieren (y)" onClick={() => copyTaskId(t().id)}>
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

      <History taskId={t().id} version={t().updated_hlc} />
    </div>
  );
}

/** German relative label for the wall time of an HLC. */
function hlcLabel(hlc: string): string {
  const ms = hlcWallMs(hlc);
  return Number.isFinite(ms) ? formatInstant(new Date(ms).toISOString(), new Date(now())) : "";
}

function DueEditor(props: { task: Task }) {
  const info = () => parseDue(props.task.due);
  const commit = (date: string, time: string) => {
    if (!date) {
      updateTask(props.task.id, { due: null });
      return;
    }
    if (!time) {
      updateTask(props.task.id, { due: date });
      return;
    }
    const [y, m, d] = date.split("-").map(Number);
    const [hh, mm] = time.split(":").map(Number);
    updateTask(props.task.id, { due: toRfc3339Local(new Date(y!, m! - 1, d!, hh!, mm!)) });
  };
  let dateInput: HTMLInputElement | undefined;
  let timeInput: HTMLInputElement | undefined;
  return (
    <span class="inline-group">
      <input ref={dateInput} class="field" type="date" aria-label="Fälligkeitsdatum" value={info()?.date ?? ""} onChange={(e) => commit(e.currentTarget.value, timeInput?.value ?? "")} />
      <input
        ref={timeInput}
        class="field field-short"
        type="time"
        aria-label="Fälligkeitsuhrzeit"
        value={info()?.time ?? ""}
        disabled={!info()}
        onChange={(e) => commit(dateInput?.value ?? "", e.currentTarget.value)}
      />
      <Show when={props.task.due}>
        <button class="btn btn-ghost btn-small" aria-label="Fälligkeit entfernen" onClick={() => updateTask(props.task.id, { due: null })}>
          <Icon name="x" size={14} />
        </button>
      </Show>
    </span>
  );
}

function SubtaskNode(props: { task: Task; depth: number }) {
  const children = () => taskIndex().childrenOf(props.task.id);
  const progress = () => taskIndex().progress(props.task.id);
  return (
    <li class="subtask" data-task-id={props.task.id}>
      <div class="subtask-row" classList={{ closed: isClosed(props.task.status) }}>
        <button
          class="check check-small"
          classList={{ done: props.task.status === "done", dropped: props.task.status === "dropped" }}
          aria-label={props.task.status === "done" ? "Als offen markieren" : "Erledigt"}
          onClick={() => toggleDone(props.task.id)}
        >
          <Show when={props.task.status === "done"}>
            <Icon name="check" size={12} />
          </Show>
        </button>
        <button
          class="link-button"
          onClick={() => {
            setSelectedId(props.task.id);
            setDetailId(props.task.id);
          }}
        >
          {props.task.title}
        </button>
        <Show when={progress().total > 0}>
          <span class="chip">
            {progress().done}/{progress().total}
          </span>
        </Show>
      </div>
      <Show when={children().length > 0 && props.depth < 12}>
        <ul class="subtree">
          <For each={children()}>{(c) => <SubtaskNode task={c} depth={props.depth + 1} />}</For>
        </ul>
      </Show>
    </li>
  );
}

function AddSubtask(props: { parentId: string }) {
  const [title, setTitle] = createSignal("");
  return (
    <form
      class="inline-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (title().trim() && createTask({ title: title(), parent_id: props.parentId })) setTitle("");
      }}
    >
      <input class="field" placeholder="Unteraufgabe hinzufügen …" aria-label="Neue Unteraufgabe" value={title()} onInput={(e) => setTitle(e.currentTarget.value)} />
      <button class="btn btn-small" type="submit" disabled={!title().trim()} aria-label="Unteraufgabe anlegen">
        <Icon name="plus" size={14} />
      </button>
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
            <li>
              <Icon name="link" size={14} />
              <Show when={safeHref(link.url)} fallback={<span class="mono" title="Kein http(s)- oder mailto-Link">{link.title || link.url}</span>}>
                {(href) => (
                  <a href={href()} target="_blank" rel="noopener noreferrer">
                    {link.title || link.url}
                  </a>
                )}
              </Show>
              <button class="btn btn-ghost btn-small" aria-label="Link entfernen" onClick={() => save(props.task.links.filter((_, j) => j !== i()))}>
                <Icon name="x" size={12} />
              </button>
            </li>
          )}
        </For>
      </ul>
      <form
        class="inline-form"
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
        <input class="field" type="url" placeholder="https://…" aria-label="Link-URL" value={url()} onInput={(e) => setUrl(e.currentTarget.value)} />
        <input class="field field-short" placeholder="Titel" aria-label="Link-Titel" value={title()} onInput={(e) => setTitle(e.currentTarget.value)} />
        <button class="btn btn-small" type="submit" disabled={!url().trim()} aria-label="Link hinzufügen">
          <Icon name="plus" size={14} />
        </button>
      </form>
    </>
  );
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
            <li>
              <a class="mono" href={href(link)}>
                {link}
              </a>
              <button class="btn btn-ghost btn-small" aria-label="Vault-Link entfernen" onClick={() => save(props.task.vault_links.filter((_, j) => j !== i()))}>
                <Icon name="x" size={12} />
              </button>
            </li>
          )}
        </For>
      </ul>
      <form
        class="inline-form"
        onSubmit={(e) => {
          e.preventDefault();
          const v = value().trim();
          if (v && save([...props.task.vault_links, v])) setValue("");
        }}
      >
        <input class="field" placeholder="Pfad im Vault oder obsidian://…" aria-label="Vault-Link" value={value()} onInput={(e) => setValue(e.currentTarget.value)} />
        <button class="btn btn-small" type="submit" disabled={!value().trim()} aria-label="Vault-Link hinzufügen">
          <Icon name="plus" size={14} />
        </button>
      </form>
    </>
  );
}

function describeValue(field: string, value: JsonValue): string {
  if (value === null) return "entfernt";
  if (field === "status" && typeof value === "string") return STATUS_LABELS[value as TaskStatus] ?? value;
  if (field === "type" && typeof value === "string") return TYPE_LABELS[value as TaskType] ?? value;
  if (field === "location" && value && typeof value === "object" && !Array.isArray(value)) {
    const kind = value.kind;
    if (kind === "inbox" || kind === "loose") return locationLabel({ kind });
    if ((kind === "area" || kind === "project") && typeof value.id === "string") return locationLabel({ kind, id: value.id });
  }
  if (field === "parent_id" && typeof value === "string") return data.tasks[value]?.title ?? "andere Aufgabe";
  if (field === "deleted") return "ja";
  if (field === "phoenix" && value && typeof value === "object" && !Array.isArray(value)) {
    return `${PHOENIX_LABELS[value.role as keyof typeof PHOENIX_LABELS] ?? value.role} ${value.fire}`;
  }
  if (typeof value === "string") return value.length > 80 ? `„${value.slice(0, 79)}…“` : `„${value}“`;
  if (Array.isArray(value)) return `${value.length} Einträge`;
  return JSON.stringify(value);
}

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
    <section class="detail-section history">
      <h3>Änderungsprotokoll</h3>
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
    </section>
  );
}
