import { createMemo, createSignal, For, onMount, Show, untrack } from "solid-js";
import { captureContext } from "../capture/context";
import { parseCapture, slugify } from "../capture/parse";
import { formatDue } from "../sync/dates";
import { describeRecurrence, locationLabel } from "../labels";
import { createTask } from "../state/actions";
import { route } from "../state/router";
import { areaList, data, now, projectList } from "../state/store";
import { setCaptureOpen, toast } from "../state/ui";
import { Icon } from "./Icon";
import { Modal } from "./Modal";

export function QuickCapture() {
  const [text, setText] = createSignal("");
  // Preselected from the view the capture was opened in; a #tag overrides it.
  const [preset, setPreset] = createSignal(
    untrack(() =>
      captureContext(route(), {
        hasArea: (id) => !!data.areas[id] && !data.areas[id]!.deleted,
        hasProject: (id) => !!data.projects[id] && !data.projects[id]!.deleted,
      }),
    ),
  );
  let input: HTMLInputElement | undefined;

  const structure = createMemo(() => ({
    areas: areaList().filter((a) => a.state === "active"),
    projects: projectList().filter((p) => p.status !== "completed"),
  }));
  // × on the rhythm chip keeps its words in the title until the field is emptied.
  const [rhythmDismissed, setRhythmDismissed] = createSignal(false);
  const parsed = createMemo(() => parseCapture(text(), new Date(now()), structure(), { recurrence: !rhythmDismissed() }));
  const target = () => parsed().location ?? preset() ?? { kind: "inbox" as const };
  const fromPreset = () => !parsed().location && preset() !== null;

  function dismissRhythm() {
    setRhythmDismissed(true);
    input?.focus();
  }

  function clearPreset() {
    setPreset(null);
    input?.focus();
  }

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
    const location = target();
    // Quick capture only creates root tasks, so a rhythm is always allowed (contract 2.5).
    const id = createTask({ title: result.title, location, due: result.due, remind_at: result.remind_at, recurrence: result.recurrence });
    if (!id) return;
    toast(`Erfasst in ${locationLabel(location)}.`);
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
          onInput={(e) => {
            setText(e.currentTarget.value);
            if (!e.currentTarget.value.trim()) setRhythmDismissed(false);
          }}
          onKeyDown={(e) => {
            if (e.key === "Tab" && suggestions().length > 0) {
              e.preventDefault();
              complete(suggestions()[0]!.slug);
            } else if (e.key === "Backspace" && text() === "" && preset()) {
              e.preventDefault();
              clearPreset();
            }
          }}
          aria-label="Titel der neuen Aufgabe"
        />
        </div>
        <div class="capture-preview" aria-live="polite">
          <Show when={text().trim()} fallback={<span class="muted">Landet in</span>}>
            <span class="capture-title">{parsed().title}</span>
          </Show>
          <span class="chip capture-location" classList={{ "chip-accent": fromPreset() }}>
            <Icon name={target().kind === "inbox" ? "inbox" : "folder"} size={12} />
            {locationLabel(target())}
            <Show when={fromPreset()}>
              <button type="button" class="chip-clear" title="Vorauswahl entfernen (Rücktaste im leeren Feld)" aria-label="Vorauswahl entfernen, in die Inbox" onClick={clearPreset}>
                <Icon name="x" size={10} />
              </button>
            </Show>
          </span>
          <Show when={text().trim()} fallback={<span class="muted"><kbd>#</kbd> ordnet zu, „morgen 9:00“ setzt Fälligkeit, „jeden Freitag“ einen Rhythmus. <kbd>Enter</kbd> speichert.</span>}>
            <Show when={parsed().due}>
              <span class="chip chip-accent">
                <Icon name="calendar" size={12} />
                Fällig {formatDue(parsed().due, new Date(now()))}
              </span>
            </Show>
            <Show when={parsed().recurrence}>
              {(r) => (
                <span class="chip chip-accent capture-recurrence">
                  <Icon name="repeat" size={12} />
                  {describeRecurrence(r(), parsed().due)}
                  <button type="button" class="chip-clear" title="Rhythmus entfernen (Wörter bleiben im Titel)" aria-label="Rhythmus entfernen" onClick={dismissRhythm}>
                    <Icon name="x" size={10} />
                  </button>
                </span>
              )}
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
