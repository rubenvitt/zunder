// Heute, Inbox and Lose.

import { createMemo, onMount, Show } from "solid-js";
import { EmptyState } from "../components/EmptyState";
import { TaskList } from "../components/TaskList";
import type { Task } from "../sync/types";
import { moveTask, setStatus, shorten } from "../state/actions";
import { inboxTasks, tasksAt, todayGroups } from "../state/selectors";
import { activeList, pickLocation, registerList, selectedId, setCaptureOpen, setSelectedId, toast } from "../state/ui";
import { now } from "../state/store";
import { locationLabel } from "../labels";

export function TodayView() {
  const groups = createMemo(todayGroups);
  const all = createMemo(() => {
    const g = groups();
    const seen = new Set<string>();
    return [...g.flames, ...g.overdue, ...g.dueToday, ...g.reminders].filter((t) => !seen.has(t.id) && !!seen.add(t.id));
  });
  onMount(() => registerList(all));
  const nothing = () => all().length === 0;
  return (
    <div class="view">
      <header class="view-head">
        <h1>Heute</h1>
        <span class="muted">{new Date(now()).toLocaleDateString("de-DE", { weekday: "long", day: "numeric", month: "long" })}</span>
      </header>
      <Show when={groups().flames.length > 0}>
        <section class="group group-flame">
          <h2>Laufende Flamme</h2>
          <TaskList tasks={groups().flames} showLocation label="Laufende Flamme" />
        </section>
      </Show>
      <Show when={groups().overdue.length > 0}>
        <section class="group">
          <h2 class="danger">Überfällig</h2>
          <TaskList tasks={groups().overdue} showLocation label="Überfällig" />
        </section>
      </Show>
      <Show when={groups().dueToday.length > 0}>
        <section class="group">
          <h2>Heute fällig</h2>
          <TaskList tasks={groups().dueToday} showLocation label="Heute fällig" />
        </section>
      </Show>
      <Show when={groups().reminders.length > 0}>
        <section class="group">
          <h2>Erinnerungen</h2>
          <TaskList tasks={groups().reminders} showLocation label="Erinnerungen" />
        </section>
      </Show>
      <Show when={nothing()}>
        <EmptyState icon="sun" title="Nichts fällig." hint="Kein Termin drängt. Ein guter Tag für eine Flamme.">
          <a class="btn btn-small" href="#/phoenix">
            Zu Phönix
          </a>
        </EmptyState>
      </Show>
    </div>
  );
}

/** Advance the selection before a task leaves the current list. */
function selectNextAfter(id: string): void {
  const list = activeList()();
  const i = list.findIndex((t) => t.id === id);
  const next = list[i + 1] ?? list[i - 1];
  setSelectedId(next && next.id !== id ? next.id : null);
}

export const inboxActions = {
  assign(task: Task) {
    pickLocation(`„${shorten(task.title)}“ zuordnen`, (loc) => {
      selectNextAfter(task.id);
      if (moveTask(task.id, loc)) toast(`Zugeordnet: ${locationLabel(loc)}`);
    });
  },
  loose(task: Task) {
    selectNextAfter(task.id);
    if (moveTask(task.id, { kind: "loose" })) toast(`„${shorten(task.title)}“ ist jetzt lose.`);
  },
  drop(task: Task) {
    selectNextAfter(task.id);
    if (setStatus(task.id, "dropped")) toast(`„${shorten(task.title)}“ verworfen.`);
  },
};

export function InboxView() {
  const tasks = createMemo(inboxTasks);
  onMount(() => {
    registerList(tasks);
    if (!selectedId() && tasks()[0]) setSelectedId(tasks()[0]!.id);
  });
  return (
    <div class="view">
      <header class="view-head">
        <h1>Inbox</h1>
        <span class="count-badge">{tasks().length}</span>
        <span class="muted hide-mobile">a zuordnen · l lose · d verwerfen</span>
      </header>
      <TaskList
        tasks={tasks()}
        label="Inbox"
        empty={
          <EmptyState icon="checkCircle" title="Inbox leer." hint="Alles eingeordnet. Neue Gedanken landen hier, bis du sie zuordnest.">
            {/* Phones have the big plus button instead. */}
            <button class="btn btn-small hide-mobile" onClick={() => setCaptureOpen(true)}>
              Aufgabe erfassen <kbd>n</kbd>
            </button>
          </EmptyState>
        }
        actions={(task) => [
          // Spec 7: assign, loose, drop with one key or one swipe each; on the phone the
          // swipes cover loose (right) and drop (left), assigning stays a visible button.
          { label: "Zuordnen", key: "a", mobile: true, run: () => inboxActions.assign(task) },
          { label: "Lose", key: "l", run: () => inboxActions.loose(task) },
          { label: "Verwerfen", key: "d", run: () => inboxActions.drop(task) },
        ]}
        swipe={(task) => ({
          right: { label: "Lose", run: () => inboxActions.loose(task) },
          left: { label: "Verwerfen", run: () => inboxActions.drop(task) },
        })}
      />
    </div>
  );
}

export function LooseView() {
  const tasks = createMemo(() => tasksAt({ kind: "loose" }));
  onMount(() => registerList(tasks));
  return (
    <div class="view">
      <header class="view-head">
        <h1>Lose</h1>
        <span class="count-badge">{tasks().length}</span>
      </header>
      <TaskList
        tasks={tasks()}
        label="Lose Aufgaben"
        empty={<EmptyState icon="loose" title="Keine losen Aufgaben." hint="Hier liegt, was bewusst zu keinem Bereich gehört. Aus der Inbox mit l hierher schieben." />}
      />
    </div>
  );
}
