// "Wiederholen" in the task detail (contract 5.1, "Rhythmen"): a few presets, a custom
// editor (interval, unit, weekdays, day of the month, anchor) and "Nicht wiederholen".
// Every change writes the whole recurrence object in one operation.

import { createMemo, createSignal, For, Show } from "solid-js";
import { MAX_RECURRENCE_EVERY, recurrenceValue, toggleWeekday, withDay, withEvery, withFrom, withUnit } from "../sync/recurrence";
import { RECURRENCE_FROMS, RECURRENCE_UNITS, WEEKDAY_CODES, type Recurrence, type RecurrenceFrom, type RecurrenceUnit, type Task } from "../sync/types";
import {
  describeRecurrence,
  matchPreset,
  RECURRENCE_FROM_LABELS,
  RECURRENCE_UNIT_LABELS,
  recurrencePresets,
  WEEKDAY_LABELS,
  WEEKDAY_NAMES,
} from "../labels";
import { updateTask } from "../state/actions";
import { taskIndex } from "../state/store";
import { taskHref } from "./TaskLinks";

const CUSTOM = "custom";
const MONTH_DAYS = Array.from({ length: 31 }, (_, i) => String(i + 1));
const NONE = "";
/** Default when "Benutzerdefiniert …" is chosen without a rhythm. */
const CUSTOM_DEFAULT: Recurrence = { every: 1, unit: "week", from: "due" };

/** The preset select, the hint without a due date and the button that unfolds the editor. */
export function RecurrenceField(props: { task: Task; editorOpen: boolean; onToggleEditor: (open: boolean) => void }) {
  const presets = createMemo(() => recurrencePresets(props.task.due));
  const preset = () => matchPreset(presets(), props.task.recurrence);
  const write = (r: Recurrence | null) => updateTask(props.task.id, { recurrence: r ? recurrenceValue(r) : null });
  const selected = () => {
    if (!props.task.recurrence) return NONE;
    const p = preset();
    return p ? String(presets().indexOf(p)) : CUSTOM;
  };
  return (
    <span class="prop-inline">
      <select
        class="prop-select recurrence-select"
        classList={{ unset: !props.task.recurrence }}
        aria-label="Wiederholen"
        disabled={!props.task.due}
        title={props.task.due ? undefined : "Erst eine Fälligkeit setzen"}
        onChange={(e) => {
          const value = e.currentTarget.value;
          if (value === NONE) {
            if (write(null)) props.onToggleEditor(false);
          } else if (value === CUSTOM) {
            if (props.task.recurrence || write(CUSTOM_DEFAULT)) props.onToggleEditor(true);
          } else {
            write(presets()[Number(value)]!.value);
          }
          // Show what is stored: refused, or "Benutzerdefiniert …" on a preset (only opens the editor).
          e.currentTarget.value = selected();
        }}
      >
        {/* `selected` per option: their computed values arrive after a `value` on the select would. */}
        <option value={NONE} selected={selected() === NONE}>
          Nicht wiederholen
        </option>
        <For each={presets()}>
          {(p, i) => (
            <option value={String(i())} selected={selected() === String(i())}>
              {p.label}
            </option>
          )}
        </For>
        <option value={CUSTOM} selected={selected() === CUSTOM}>{props.task.recurrence && !preset() ? describeRecurrence(props.task.recurrence) : "Benutzerdefiniert …"}</option>
      </select>
      <Show when={!props.task.due}>
        <span class="muted small">Erst eine Fälligkeit setzen</span>
      </Show>
      <Show when={props.task.recurrence && props.task.due}>
        <button class="btn btn-ghost btn-small" aria-expanded={props.editorOpen} onClick={() => props.onToggleEditor(!props.editorOpen)}>
          {props.editorOpen ? "Fertig" : "Anpassen"}
        </button>
      </Show>
    </span>
  );
}

/** Interval, unit, weekdays (weeks), day of the month (months) and anchor of a rhythm. */
export function RecurrenceEditor(props: { task: Task; recurrence: Recurrence }) {
  const r = () => props.recurrence;
  const write = (next: Recurrence) => updateTask(props.task.id, { recurrence: recurrenceValue(next) });
  const [everyDraft, setEveryDraft] = createSignal<string | null>(null);
  const commitEvery = (value: string) => {
    setEveryDraft(null);
    const n = Number(value);
    if (Number.isInteger(n) && n >= 1 && n <= MAX_RECURRENCE_EVERY && n !== r().every) write(withEvery(r(), n));
  };
  const dayValue = () => (r().day === undefined ? "" : String(r().day));
  return (
    <div class="recurrence-editor" role="group" aria-label="Rhythmus anpassen">
      <div class="recurrence-line">
        <span class="muted">Alle</span>
        <input
          class="prop-input recurrence-every"
          type="number"
          inputmode="numeric"
          min="1"
          max={MAX_RECURRENCE_EVERY}
          aria-label="Intervall"
          value={everyDraft() ?? String(r().every)}
          onInput={(e) => setEveryDraft(e.currentTarget.value)}
          onChange={(e) => commitEvery(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape" && everyDraft() !== null) {
              e.stopPropagation();
              setEveryDraft(null);
            }
          }}
        />
        <select class="prop-select" aria-label="Einheit" onChange={(e) => write(withUnit(r(), e.currentTarget.value as RecurrenceUnit))}>
          <For each={RECURRENCE_UNITS}>{(u) => <option value={u} selected={r().unit === u}>{r().every === 1 ? RECURRENCE_UNIT_LABELS[u].one : RECURRENCE_UNIT_LABELS[u].many}</option>}</For>
        </select>
        <select class="prop-select" aria-label="Anker" onChange={(e) => write(withFrom(r(), e.currentTarget.value as RecurrenceFrom))}>
          <For each={RECURRENCE_FROMS}>{(f) => <option value={f} selected={r().from === f}>{RECURRENCE_FROM_LABELS[f]}</option>}</For>
        </select>
      </div>
      <Show when={r().unit === "week"}>
        <div class="recurrence-line weekday-toggles" role="group" aria-label="Wochentage">
          <For each={WEEKDAY_CODES}>
            {(d) => (
              <button
                type="button"
                class="chip chip-button chip-filter weekday-toggle"
                aria-pressed={!!r().weekdays?.includes(d)}
                aria-label={WEEKDAY_NAMES[d]}
                title={WEEKDAY_NAMES[d]}
                onClick={() => write(toggleWeekday(r(), d))}
              >
                {WEEKDAY_LABELS[d]}
              </button>
            )}
          </For>
        </div>
      </Show>
      <Show when={r().unit === "month"}>
        <div class="recurrence-line">
          <select
            class="prop-select"
            aria-label="Tag im Monat"
            onChange={(e) => {
              const v = e.currentTarget.value;
              write(withDay(r(), v === "" ? undefined : v === "last" ? "last" : Number(v)));
            }}
          >
            <option value="" selected={dayValue() === ""}>
              {r().from === "done" ? "am Tag der Erledigung" : "am Tag der Fälligkeit"}
            </option>
            <For each={MONTH_DAYS}>
              {(d) => (
                <option value={d} selected={dayValue() === d}>
                  am {d}.
                </option>
              )}
            </For>
            <option value="last" selected={dayValue() === "last"}>
              am Monatsletzten
            </option>
          </select>
        </div>
      </Show>
      <p class="recurrence-summary muted small">
        {describeRecurrence(r())}. Wer die Aufgabe erledigt oder verwirft, bekommt die nächste Instanz.
      </p>
    </div>
  );
}

/** "Vorherige" and "Nächste" instance of the series, as task links. */
export function SeriesLinks(props: { task: Task }) {
  const previous = () => taskIndex().previousInstance(props.task);
  const next = () => taskIndex().nextInstance(props.task.id);
  const link = (label: string, t: Task) => (
    // Plain clicks open the task in the panel (TaskHoverCard handles every task link).
    <a class="task-link series-link" href={taskHref(t)} data-task-id={t.id}>
      {label}
    </a>
  );
  return (
    <Show when={previous() || next()}>
      <span class="prop-inline series-links">
        <Show when={previous()}>{(p) => link("Vorherige", p())}</Show>
        <Show when={next()}>{(n) => link("Nächste", n())}</Show>
      </span>
    </Show>
  );
}
