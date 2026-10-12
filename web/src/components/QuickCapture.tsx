import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import { parseCapture, slugify } from "../capture/parse";
import { formatDue } from "../sync/dates";
import { locationLabel } from "../labels";
import { createTask } from "../state/actions";
import { areaList, now, projectList } from "../state/store";
import { setCaptureOpen, toast } from "../state/ui";
import { Icon } from "./Icon";
import { Modal } from "./Modal";

export function QuickCapture() {
  const [text, setText] = createSignal("");
  let input: HTMLInputElement | undefined;

  const structure = createMemo(() => ({
    areas: areaList().filter((a) => a.state === "active"),
    projects: projectList().filter((p) => p.status !== "completed"),
  }));
  const parsed = createMemo(() => parseCapture(text(), new Date(now()), structure()));

  const suggestions = createMemo(() => {
    const m = /(?:^|\s)#([^\s#]*)$/.exec(text());
    if (!m) return [];
    const prefix = slugify(m[1] ?? "");
    const items = [
      ...structure().projects.map((p) => ({ slug: slugify(p.name), label: locationLabel({ kind: "project", id: p.id }), kind: "Projekt" })),
      ...structure().areas.map((a) => ({ slug: slugify(a.name), label: a.name, kind: "Bereich" })),
    ];
    return items.filter((i) => i.slug.startsWith(prefix) && i.slug !== prefix).slice(0, 5);
  });

  onMount(() => input?.focus());

  function complete(slug: string) {
    setText((t) => t.replace(/#([^\s#]*)$/, `#${slug} `));
    input?.focus();
  }

  function submit() {
    const raw = text().trim();
    if (!raw) return;
    const result = parsed();
    const id = createTask({ title: result.title, location: result.location, due: result.due, remind_at: result.remind_at });
    if (!id) return;
    toast(`Erfasst in ${locationLabel(result.location ?? { kind: "inbox" })}.`);
    setText("");
    setCaptureOpen(false);
  }

  return (
    <Modal label="Schnellerfassung" onClose={() => setCaptureOpen(false)} class="capture">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <div class="capture-row">
        <Icon name="plus" size={20} />
        <input
          ref={input}
          class="capture-input"
          name="capture"
          placeholder="Neue Aufgabe … z. B. „Zahnarzt anrufen morgen 9:00 #gesundheit“"
          autocomplete="off"
          value={text()}
          onInput={(e) => setText(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Tab" && suggestions().length > 0) {
              e.preventDefault();
              complete(suggestions()[0]!.slug);
            }
          }}
          aria-label="Titel der neuen Aufgabe"
        />
        </div>
        <div class="capture-preview" aria-live="polite">
          <Show when={text().trim()} fallback={<span class="muted">Landet in der Inbox. <kbd>#</kbd> ordnet zu, „morgen 9:00“ setzt Fälligkeit. <kbd>Enter</kbd> speichert.</span>}>
            <span class="capture-title">{parsed().title}</span>
            <span class="chip">
              <Icon name={parsed().location ? "folder" : "inbox"} size={12} />
              {locationLabel(parsed().location ?? { kind: "inbox" })}
            </span>
            <Show when={parsed().due}>
              <span class="chip chip-accent">
                <Icon name="calendar" size={12} />
                Fällig {formatDue(parsed().due, new Date(now()))}
              </span>
            </Show>
            <Show when={parsed().remind_at}>
              <span class="chip">
                <Icon name="bell" size={12} />
                Erinnerung
              </span>
            </Show>
            <Show when={parsed().unknownTags.length > 0}>
              <span class="chip chip-danger" title="Kein Bereich oder Projekt gefunden">
                {parsed().unknownTags.join(" ")} unbekannt
              </span>
            </Show>
          </Show>
        </div>
        <Show when={suggestions().length > 0}>
          <ul class="suggestions">
            <For each={suggestions()}>
              {(s) => (
                <li>
                  <button type="button" class="btn btn-ghost btn-small" onClick={() => complete(s.slug)}>
                    #{s.slug} <span class="muted">· {s.kind} {s.label}</span>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
        <div class="modal-actions">
          <button type="button" class="btn btn-ghost" onClick={() => setCaptureOpen(false)}>
            Abbrechen
          </button>
          <button type="submit" class="btn btn-primary" disabled={!text().trim()}>
            Erfassen
          </button>
        </div>
      </form>
    </Modal>
  );
}
