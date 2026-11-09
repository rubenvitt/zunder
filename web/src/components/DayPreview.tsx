// Tagesvorschau in Heute (contract 5.1): the next seven days with how many tasks fall due on
// each, so a pile-up of small things shows early. A crowded day is marked and named; a click
// on a day lists its tasks right here, where the detail can spread them out.

import { createMemo, createSignal, For, Show } from "solid-js";
import { CROWDED_DAY, isCrowded, PREVIEW_DAYS, type DayLoad } from "../sync/dayload";
import { dateFromLocal, localDate } from "../sync/dates";
import { CROWDED_HINT, crowdedDayLine, dueCountLabel, longDayLabel } from "../labels";
import { dayLoadsAhead } from "../state/selectors";
import { now } from "../state/store";
import { TaskList } from "./TaskList";

const WEEKDAYS = ["So", "Mo", "Di", "Mi", "Do", "Fr", "Sa"];

/** The day whose tasks the preview lists (local "YYYY-MM-DD"), or null. Per session only. */
const [openDay, setOpenDay] = createSignal<string | null>(null);

/** The next days with their tasks, reactive (date changes at midnight, tasks on every change). */
export function previewLoads(): DayLoad[] {
  return dayLoadsAhead(localDate(new Date(now())), PREVIEW_DAYS);
}

/** Tasks of the day the preview lists, for keyboard navigation in Heute. */
export function openDayTasks(loads: DayLoad[]): DayLoad["tasks"] {
  const day = openDay();
  return loads.find((l) => l.date === day)?.tasks ?? [];
}

function dayLabel(date: string): { weekday: string; day: string } {
  const d = dateFromLocal(date)!;
  return { weekday: WEEKDAYS[d.getDay()]!, day: `${d.getDate()}.${d.getMonth() + 1}.` };
}

export function DayPreview(props: { loads: DayLoad[] }) {
  const crowded = createMemo(() => props.loads.filter((l) => isCrowded(l.tasks.length)));
  const open = createMemo(() => props.loads.find((l) => l.date === openDay()) ?? null);
  const toggle = (date: string) => setOpenDay((d) => (d === date ? null : date));
  return (
    <section class="day-preview" aria-label="Nächste Tage">
      <h2>Nächste Tage</h2>
      <ol class="day-strip">
        <For each={props.loads}>
          {(load) => {
            const label = dayLabel(load.date);
            const count = () => load.tasks.length;
            return (
              <li>
                <button
                  type="button"
                  class="day-cell"
                  classList={{ "day-crowded": isCrowded(count()), "day-empty": count() === 0, "day-open": openDay() === load.date }}
                  data-date={load.date}
                  data-count={count()}
                  aria-pressed={openDay() === load.date}
                  aria-label={`${longDayLabel(load.date)}: ${dueCountLabel(count())}`}
                  disabled={count() === 0}
                  // The meter at the foot of the cell: full once the day counts as crowded.
                  style={{ "--load": Math.min(count() / CROWDED_DAY, 1) }}
                  onClick={() => toggle(load.date)}
                >
                  <span class="day-weekday">{label.weekday}</span>
                  <span class="day-date">{label.day}</span>
                  <span class="day-count">{count()}</span>
                </button>
              </li>
            );
          }}
        </For>
      </ol>
      <For each={crowded()}>
        {(load) => (
          <p class="day-crowded-line" data-date={load.date}>
            <button type="button" class="link-button" onClick={() => setOpenDay(load.date)}>
              <strong>{crowdedDayLine(load.date, load.tasks.length)}</strong> <span class="muted">{CROWDED_HINT}</span>
            </button>
          </p>
        )}
      </For>
      <Show when={open()}>
        {(load) => <TaskList tasks={load().tasks} showLocation label={`Fällig ${longDayLabel(load().date)}`} />}
      </Show>
    </section>
  );
}
