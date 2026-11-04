// Jev's proposal for an open task (contract 4.5, ZUN-56): weight, for whom and,
// in the inbox, the place, as chips to take over with one tap. Asked by itself
// as soon as the detail opens and one of those is still empty; what Jev is sure
// about (Suggestion.auto) is set right away and can be undone here. No numbers,
// no model names: the owner decides at a glance.

import { createEffect, createResource, createSignal, For, on, onCleanup, Show } from "solid-js";
import { FOR_WHOM_LABELS, WEIGHT_LABELS } from "../labels";
import { moveTask, updateTask } from "../state/actions";
import { api, data, pendingCount } from "../state/store";
import { toast } from "../state/ui";
import type { ForWhom, Location, PlaceProposal, Suggestion, SuggestionAuto, Task, Weight } from "../sync/types";
import { Icon } from "./Icon";

/** Below this probability a chip says "vielleicht". */
const UNSURE_BELOW = 0.6;

/** The proposed place, if it still exists and is still active. */
function placeAvailable(place: PlaceProposal): boolean {
  if (place.kind === "project") return data.projects[place.id]?.status === "active";
  return data.areas[place.id]?.state === "active";
}

function placeLocation(place: PlaceProposal): Location {
  return place.kind === "project" ? { kind: "project", id: place.id } : { kind: "area", id: place.id };
}

/** The server or the app refuses this task for Jev (private area, dev project, no key): stay quiet. */
function refused(err: unknown): boolean {
  const code = (err as { code?: unknown } | null)?.code;
  return code === "conflict" || code === "forbidden" || code === "unsupported" || code === "not_found";
}

/** Whether Jev has something to say about the task: an empty weight, for whom or place. */
export function wantsSuggestion(task: Task, inInbox: boolean): boolean {
  return task.weight === null || task.for_whom === null || inInbox;
}

interface Chip {
  key: "weight" | "for_whom" | "place";
  label: string;
  hint: string;
  unsure: boolean;
  take: () => void;
}

export function JevSuggestion(props: { task: Task; inInbox: boolean; onForWhom: (value: ForWhom) => void }) {
  const taskId = () => props.task.id;
  const [stored, { mutate }] = createResource(taskId, async (id): Promise<Suggestion | null> => {
    try {
      return await api().taskSuggestion(id);
    } catch {
      return null;
    }
  });
  const [busy, setBusy] = createSignal(false);
  const [failed, setFailed] = createSignal(false);
  const [hidden, setHidden] = createSignal(false);
  // Fields Jev set by itself in this view, with the value to undo to (always empty).
  const [autoSet, setAutoSet] = createSignal<SuggestionAuto[]>([]);
  let askedFor: string | null = null;
  // An answer that arrives after the panel is gone (task moved into a dev project, detail
  // closed) must not set anything: its "Rückgängig" would be gone too.
  let disposed = false;
  onCleanup(() => {
    disposed = true;
  });

  createEffect(
    on(taskId, () => {
      setFailed(false);
      setHidden(false);
      setAutoSet([]);
    }),
  );

  async function ask(manual: boolean) {
    if (busy()) return;
    const id = taskId();
    setBusy(true);
    setFailed(false);
    try {
      const s = await api().suggestTask(id);
      if (disposed || id !== taskId()) return;
      mutate(s);
      applySure(s);
    } catch (err) {
      if (disposed || id !== taskId()) return;
      if (refused(err)) setHidden(true);
      else if (manual) toast(err instanceof Error ? err.message : "Jev hat nicht geantwortet.", "error");
      else setFailed(true);
    } finally {
      setBusy(false);
    }
  }

  /** Sets what Jev is sure about, only into fields that are still empty. */
  function applySure(s: Suggestion) {
    const done: SuggestionAuto[] = [];
    for (const field of s.auto ?? []) {
      if (field === "weight" && s.weight && props.task.weight === null) {
        if (updateTask(props.task.id, { weight: s.weight })) done.push(field);
      } else if (field === "for_whom" && s.for_whom && props.task.for_whom === null) {
        props.onForWhom(s.for_whom);
        done.push(field);
      }
    }
    setAutoSet(done);
  }

  // Asks by itself once per task and view: nothing stored yet, something still empty, and
  // nothing waiting to be sent (the server only knows tasks it has received).
  createEffect(() => {
    if (stored.loading || stored() !== null || busy() || hidden()) return;
    if (!wantsSuggestion(props.task, props.inInbox) || pendingCount() > 0) return;
    if (askedFor === taskId()) return;
    askedFor = taskId();
    void ask(false);
  });

  const placeOpen = (s: Suggestion) => props.inInbox && s.place !== null && placeAvailable(s.place);

  const chips = (s: Suggestion): Chip[] => {
    const out: Chip[] = [];
    if (s.weight && props.task.weight === null) {
      const w: Weight = s.weight;
      out.push({
        key: "weight",
        label: WEIGHT_LABELS[w],
        hint: "Gewicht setzen",
        unsure: (s.weight_probability ?? 0) < UNSURE_BELOW,
        take: () => updateTask(props.task.id, { weight: w }),
      });
    }
    if (s.for_whom && props.task.for_whom === null) {
      const f: ForWhom = s.for_whom;
      out.push({
        key: "for_whom",
        label: FOR_WHOM_LABELS[f],
        hint: "„Für wen“ setzen",
        unsure: (s.for_whom_probability ?? 0) < UNSURE_BELOW,
        take: () => props.onForWhom(f),
      });
    }
    if (placeOpen(s)) {
      const p = s.place!;
      out.push({
        key: "place",
        label: p.name,
        hint: `Nach „${p.name}“ verschieben`,
        unsure: (s.place_probability ?? 0) < UNSURE_BELOW,
        take: () => moveTask(props.task.id, placeLocation(p)),
      });
    }
    return out;
  };

  function takeAll(list: Chip[]) {
    // Last: moving takes the task out of the inbox.
    for (const chip of [...list].sort((a, b) => Number(a.key === "place") - Number(b.key === "place"))) chip.take();
  }

  // What Jev set by itself and is still in place, for "Rückgängig".
  const setByJev = (s: Suggestion | null | undefined) =>
    autoSet().filter((f) => (f === "weight" ? s?.weight && props.task.weight === s.weight : s?.for_whom && props.task.for_whom === s.for_whom));

  function undo(field: SuggestionAuto) {
    if (field === "weight") updateTask(props.task.id, { weight: null });
    else updateTask(props.task.id, { for_whom: null });
    setAutoSet((list) => list.filter((f) => f !== field));
  }

  const autoLabel = (s: Suggestion, field: SuggestionAuto) =>
    field === "weight" ? WEIGHT_LABELS[s.weight!] : FOR_WHOM_LABELS[s.for_whom!];

  const visible = () => {
    if (hidden()) return false;
    if (busy() || failed()) return true;
    const s = stored();
    return !!s && (chips(s).length > 0 || setByJev(s).length > 0);
  };

  return (
    <Show when={visible()}>
      <section class="jev-suggestion" aria-label="Vorschlag von Jev" data-testid="jev-suggestion" aria-busy={busy()}>
        <span class="jev-title">
          <Icon name="target" size={14} /> Jev
        </span>
        <Show when={busy()}>
          <span class="muted jev-thinking">überlegt …</span>
        </Show>
        <Show when={!busy() && failed()}>
          <span class="muted">gerade nicht erreichbar</span>
          <button class="btn btn-ghost btn-small" onClick={() => void ask(true)}>
            Noch einmal
          </button>
        </Show>
        <Show when={!busy() && stored()}>
          {(s) => (
            <>
              <For each={setByJev(s())}>
                {(field) => (
                  <span class="jev-chip jev-chip-set" data-testid="jev-auto">
                    <Icon name="check" size={13} /> {autoLabel(s(), field)}
                    <button class="jev-undo" onClick={() => undo(field)} title="Von Jev gesetzt: zurücknehmen">
                      Rückgängig
                    </button>
                  </span>
                )}
              </For>
              <For each={chips(s())}>
                {(chip) => (
                  <button
                    class="jev-chip"
                    classList={{ "jev-chip-unsure": chip.unsure, [`jev-chip-${chip.key}`]: true }}
                    title={chip.unsure ? `${chip.hint} (Jev ist unsicher)` : chip.hint}
                    onClick={() => chip.take()}
                  >
                    <Show when={chip.key === "place"}>
                      <Icon name="folder" size={13} />
                    </Show>
                    {chip.label}
                    <Show when={chip.unsure}>
                      <span class="jev-maybe">?</span>
                    </Show>
                  </button>
                )}
              </For>
              <Show when={chips(s()).length > 1}>
                <button class="btn btn-small btn-primary jev-all" onClick={() => takeAll(chips(s()))}>
                  Alle übernehmen
                </button>
              </Show>
            </>
          )}
        </Show>
      </section>
    </Show>
  );
}
