// Search field and filter chips above a list or board (project, area, Dev, Lose).
// f focuses the field; Esc clears it, a second Esc leaves it; Enter or ↓ jumps to the first hit.

import { createEffect, createSignal, For, on, onCleanup, Show, type Accessor } from "solid-js";
import { WEIGHT_KEYS, type WeightKey } from "../sync/balance";
import { EMPTY_FILTER, isFilterActive, matchesFilter, type TaskFilter } from "../sync/task-filter";
import { TASK_TYPES, type Task, type TaskType } from "../sync/types";
import { TYPE_LABELS, WEIGHT_HINTS, WEIGHT_LABELS } from "../labels";
import { taskIndex } from "../state/store";
import { activeList, filterInput, setFilterInput, setSelectedId } from "../state/ui";
import { EmptyState } from "./EmptyState";
import { Icon } from "./Icon";
import { PLAIN_INPUT } from "./inputs";

export interface TaskFilterState {
  filter: Accessor<TaskFilter>;
  setQuery(query: string): void;
  toggleType(type: TaskType): void;
  toggleWeight(weight: WeightKey): void;
  reset(): void;
  active: Accessor<boolean>;
  /** The tasks that pass, in their given order. */
  apply(tasks: readonly Task[]): Task[];
}

function toggled<T>(list: readonly T[], item: T): T[] {
  return list.includes(item) ? list.filter((x) => x !== item) : [...list, item];
}

/** Filter state of one view; it starts over when `scope` changes (another project or area). */
export function createTaskFilter(scope?: Accessor<string>): TaskFilterState {
  const [filter, setFilter] = createSignal<TaskFilter>(EMPTY_FILTER);
  const reset = () => setFilter(EMPTY_FILTER);
  if (scope) createEffect(on(scope, reset, { defer: true }));
  const active = () => isFilterActive(filter());
  return {
    filter,
    setQuery: (query) => setFilter((f) => ({ ...f, query })),
    toggleType: (type) => setFilter((f) => ({ ...f, types: toggled(f.types, type) })),
    toggleWeight: (weight) => setFilter((f) => ({ ...f, weights: toggled(f.weights, weight) })),
    reset,
    active,
    apply(tasks) {
      const f = filter();
      if (!isFilterActive(f)) return [...tasks];
      const index = taskIndex();
      return tasks.filter((t) => matchesFilter(t, f, () => index.descendantsOf(t.id)));
    },
  };
}

export interface TaskFilterBarProps {
  state: TaskFilterState;
  /** Type chips (Dev projects and the Dev board). */
  types?: boolean;
  /** Weight chips (everything outside dev projects). */
  weights?: boolean;
  /** Tasks after filtering and before; the bar shows "3 von 12" while a filter is set. */
  shown: number;
  total: number;
  label: string;
}

export function TaskFilterBar(props: TaskFilterBarProps) {
  let input: HTMLInputElement | undefined;
  const focus = () => {
    input?.focus();
    input?.select();
  };
  setFilterInput(focus);
  onCleanup(() => filterInput() === focus && setFilterInput(null));
  const f = () => props.state.filter();

  function onKeyDown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === "Escape" && f().query) {
      // First Esc clears, the second one (global handler) leaves the field.
      e.preventDefault();
      props.state.setQuery("");
      return;
    }
    if (e.key === "Enter" || e.key === "ArrowDown") {
      const first = activeList()()[0];
      e.preventDefault();
      if (!first) return;
      setSelectedId(first.id);
      input?.blur();
      // List rows focus their main button, board cards themselves.
      const el = document.querySelector<HTMLElement>(`[data-task-id="${CSS.escape(first.id)}"]`);
      (el?.querySelector<HTMLElement>(".task-main") ?? el)?.focus({ preventScroll: true });
    }
  }

  return (
    <div class="task-filter" role="search" aria-label={props.label}>
      <label class="task-filter-search" classList={{ filled: !!f().query }}>
        <Icon name="search" size={16} />
        <input
          ref={input}
          class="task-filter-input"
          type="search"
          {...PLAIN_INPUT}
          placeholder="Suchen …"
          aria-label={`${props.label}: Suchen`}
          title="Suchen (f)"
          value={f().query}
          onInput={(e) => props.state.setQuery(e.currentTarget.value)}
          onKeyDown={onKeyDown}
        />
        <Show when={f().query}>
          <button type="button" class="btn btn-ghost btn-xs" aria-label="Suche leeren" title="Suche leeren (Esc)" onClick={() => (props.state.setQuery(""), input?.focus())}>
            <Icon name="x" size={14} />
          </button>
        </Show>
      </label>
      <Show when={props.types || props.weights}>
        <div class="task-filter-chips filter-chips" role="group" aria-label="Filter">
          <Show when={props.types}>
            <For each={TASK_TYPES}>
              {(type) => (
                <button type="button" class="chip chip-button chip-filter" data-type={type} aria-pressed={f().types.includes(type)} onClick={() => props.state.toggleType(type)}>
                  <Show when={type !== "aufgabe"}>
                    <Icon name={type === "bug" ? "bug" : "star"} size={13} />
                  </Show>
                  {TYPE_LABELS[type]}
                </button>
              )}
            </For>
          </Show>
          <Show when={props.weights}>
            <For each={WEIGHT_KEYS}>
              {(weight) => (
                <button
                  type="button"
                  class="chip chip-button chip-filter"
                  data-weight={weight}
                  aria-pressed={f().weights.includes(weight)}
                  title={weight === "none" ? "Noch nicht eingeteilt" : WEIGHT_HINTS[weight]}
                  onClick={() => props.state.toggleWeight(weight)}
                >
                  {weight === "none" ? "Ohne Gewicht" : WEIGHT_LABELS[weight]}
                </button>
              )}
            </For>
          </Show>
        </div>
      </Show>
      <Show when={props.state.active()}>
        <span class="task-filter-count muted" aria-live="polite">
          {props.shown} von {props.total}
        </span>
        <button type="button" class="btn btn-ghost btn-small" onClick={() => props.state.reset()}>
          Zurücksetzen
        </button>
      </Show>
    </div>
  );
}

/** Empty list while a filter is set: say so instead of "no tasks yet". */
export function NoMatches(props: { state: TaskFilterState }) {
  return (
    <EmptyState compact icon="search" title="Keine Treffer." hint="Nichts passt zu Suche und Filter.">
      <button class="btn btn-small" onClick={() => props.state.reset()}>
        Filter zurücksetzen
      </button>
    </EmptyState>
  );
}
