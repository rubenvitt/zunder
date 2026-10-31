// Jev's proposal for an inbox task (contract 4.5): weight, for whom and place,
// each with its probability and a button to take it over. Only fetched on
// request, so the owner decides which task leaves the server.

import { createResource, createSignal, Show } from "solid-js";
import { FOR_WHOM_LABELS, WEIGHT_LABELS } from "../labels";
import { moveTask, updateTask } from "../state/actions";
import { api, data, now } from "../state/store";
import { toast } from "../state/ui";
import { formatInstant } from "../sync/dates";
import type { ForWhom, Location, PlaceProposal, Suggestion, Task, Weight } from "../sync/types";
import { Icon } from "./Icon";

function percent(p: number | null): string {
  return p === null ? "" : `${Math.round(p * 100)} %`;
}

/** The proposed place, if it still exists and is still active. */
function placeAvailable(place: PlaceProposal): boolean {
  if (place.kind === "project") return data.projects[place.id]?.status === "active";
  return data.areas[place.id]?.state === "active";
}

function placeLocation(place: PlaceProposal): Location {
  return place.kind === "project" ? { kind: "project", id: place.id } : { kind: "area", id: place.id };
}

export function JevSuggestion(props: { task: Task; onForWhom: (value: ForWhom) => void }) {
  const [stored, { mutate }] = createResource(
    () => props.task.id,
    async (id): Promise<Suggestion | null> => {
      try {
        return await api().taskSuggestion(id);
      } catch {
        return null;
      }
    },
  );
  const [busy, setBusy] = createSignal(false);

  async function ask() {
    if (busy()) return;
    setBusy(true);
    try {
      mutate(await api().suggestTask(props.task.id));
    } catch (err) {
      toast(err instanceof Error ? err.message : "Jev hat nicht geantwortet.", "error");
    } finally {
      setBusy(false);
    }
  }

  const weightOpen = (s: Suggestion) => s.weight !== null && props.task.weight !== s.weight;
  const forWhomOpen = (s: Suggestion) => s.for_whom !== null && props.task.for_whom !== s.for_whom;
  const placeOpen = (s: Suggestion) => s.place !== null && placeAvailable(s.place);
  const anyOpen = (s: Suggestion) => weightOpen(s) || forWhomOpen(s) || placeOpen(s);

  const takeWeight = (w: Weight) => updateTask(props.task.id, { weight: w });
  const takePlace = (p: PlaceProposal) => moveTask(props.task.id, placeLocation(p));

  function takeAll(s: Suggestion) {
    if (weightOpen(s)) takeWeight(s.weight!);
    if (forWhomOpen(s)) props.onForWhom(s.for_whom!);
    // Last: moving takes the task out of the inbox and with it this panel.
    if (placeOpen(s)) takePlace(s.place!);
  }

  return (
    <section class="jev-suggestion" aria-label="Vorschlag von Jev" data-testid="jev-suggestion">
      <Show
        when={stored()}
        fallback={
          <div class="jev-head">
            <span class="muted">Jev kann Gewicht, „für wen“ und Ort vorschlagen.</span>
            <button class="btn btn-small" disabled={busy()} onClick={() => void ask()}>
              <Icon name="target" size={14} /> {busy() ? "Fragt …" : "Vorschlag holen"}
            </button>
          </div>
        }
      >
        {(s) => (
          <>
            <div class="jev-head">
              <strong>Vorschlag von Jev</strong>
              <span class="muted" title={s().model}>
                {formatInstant(s().created_at, new Date(now()))}
              </span>
              <span class="spacer" />
              <Show when={anyOpen(s())}>
                <button class="btn btn-small btn-primary" onClick={() => takeAll(s())}>
                  Alles übernehmen
                </button>
              </Show>
              <button class="btn btn-small btn-ghost" disabled={busy()} onClick={() => void ask()} title="Jev noch einmal fragen">
                <Icon name="sync" size={14} /> {busy() ? "Fragt …" : "Neu fragen"}
              </button>
            </div>
            <ul class="jev-rows">
              <li>
                <span class="jev-label">Gewicht</span>
                <Show when={s().weight} fallback={<span class="muted">unklar</span>}>
                  {(w) => (
                    <>
                      <span>{WEIGHT_LABELS[w()]}</span>
                      <span class="muted">{percent(s().weight_probability)}</span>
                      <Show when={weightOpen(s())} fallback={<Icon name="check" size={14} title="Übernommen" />}>
                        <button class="btn btn-ghost btn-small" onClick={() => takeWeight(w())}>
                          Übernehmen
                        </button>
                      </Show>
                    </>
                  )}
                </Show>
              </li>
              <li>
                <span class="jev-label">Für wen</span>
                <Show when={s().for_whom} fallback={<span class="muted">unklar</span>}>
                  {(f) => (
                    <>
                      <span>{FOR_WHOM_LABELS[f()]}</span>
                      <span class="muted">{percent(s().for_whom_probability)}</span>
                      <Show when={forWhomOpen(s())} fallback={<Icon name="check" size={14} title="Übernommen" />}>
                        <button class="btn btn-ghost btn-small" onClick={() => props.onForWhom(f())}>
                          Übernehmen
                        </button>
                      </Show>
                    </>
                  )}
                </Show>
              </li>
              <li>
                <span class="jev-label">Ort</span>
                <Show when={s().place} fallback={<span class="muted">nichts passt</span>}>
                  {(p) => (
                    <>
                      <span>
                        <Icon name="folder" size={13} /> {p().name}
                      </span>
                      <span class="muted">{percent(s().place_probability)}</span>
                      <Show when={placeOpen(s())} fallback={<span class="muted">nicht mehr aktiv</span>}>
                        <button class="btn btn-ghost btn-small" onClick={() => takePlace(p())}>
                          Verschieben
                        </button>
                      </Show>
                    </>
                  )}
                </Show>
              </li>
            </ul>
          </>
        )}
      </Show>
    </section>
  );
}
