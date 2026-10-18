// Heute, Inbox, Lose, Wartet auf andere and Offene Zusagen.

import { createMemo, For, onMount, Show } from "solid-js";
import { EmptyState } from "../components/EmptyState";
import { TaskList } from "../components/TaskList";
import { FocusSection } from "../components/FocusSection";
import { WeekMirror } from "../components/WeekMirror";
import type { DeferTarget } from "../sync/defer";
import type { Task } from "../sync/types";
import { canHandBack, deferTask, dropTask, moveTask, setWaitingOn, shorten } from "../state/actions";
import { commitmentTasks, inboxTasks, tasksAt, todayGroups, waitingGroups } from "../state/selectors";
import { activeList, copyText, pickLocation, registerList, selectedId, setCaptureOpen, setHandBackRequest, setSelectedId, toast } from "../state/ui";
import { followUpSentence } from "../sync/handback";
import { now } from "../state/store";
import { refreshFocus } from "../state/focus";
import { tooManyCommitments } from "../sync/commitments";
import { COMMITMENT_HINT, commitmentsLabel, locationLabel } from "../labels";

/** "Heute nicht" from the Heute list (s: tomorrow, Shift+S: next week, swipe right). */
export const todayActions = {
  later(task: Task, target: DeferTarget = "tomorrow") {
    deferTask(task.id, target, () => selectNextAfter(task.id));
  },
};

// One row action keeps the titles wide; next week is Shift+S and in the detail.
const todayRowActions = (task: Task) => [{ label: "Heute nicht", key: "s", run: () => todayActions.later(task) }];

const todaySwipe = (task: Task) => ({ right: { label: "Heute nicht", run: () => todayActions.later(task) } });

/** "Antwort ist da": clears whom it waits on; the server opens the task unless a blocker holds it. */
function answered(task: Task): void {
  selectNextAfter(task.id);
  if (setWaitingOn(task.id, "")) toast(`„${shorten(task.title)}“ wartet nicht mehr.`);
}

/**
 * Wiedervorlage (contract 5.1): the reminder is due, the answer is not. Both actions are
 * offers; leaving the row alone is fine, it goes with the day.
 */
const followUpActions = (task: Task) => [
  { label: "Antwort ist da", mobile: true, run: () => answered(task) },
  {
    label: "Nachfragen",
    mobile: true,
    run: () => copyText(followUpSentence(task.title), `Satz kopiert. „${shorten(task.title)}“ wartet weiter auf ${task.waiting_on?.trim()}.`),
  },
  { label: "Heute nicht", key: "s", run: () => todayActions.later(task) },
];

export function TodayView() {
  const groups = createMemo(todayGroups);
  const all = createMemo(() => {
    const g = groups();
    const seen = new Set<string>();
    return [...g.flames, ...g.overdue, ...g.dueToday, ...g.reminders, ...g.followUps].filter((t) => !seen.has(t.id) && !!seen.add(t.id));
  });
  // Zusagen-Bremse: how many promises are open, shown before they press (contract 5.1).
  const commitments = createMemo(() => commitmentTasks().length);
  onMount(() => {
    registerList(all);
    // Another device may have clicked "Erst für dich" away or picked a task today.
    void refreshFocus();
  });
  const nothing = () => all().length === 0;
  return (
    <div class="view">
      <header class="view-head">
        <h1>Heute</h1>
        <span class="muted">{new Date(now()).toLocaleDateString("de-DE", { weekday: "long", day: "numeric", month: "long" })}</span>
        <Show when={commitments() > 0}>
          <span class="spacer" />
          <span class="commitments">
            <a class="commitments-count" href="#/zusagen" title="Jemand wartet auf deine Zusage: Liste öffnen">
              {commitmentsLabel(commitments())}
            </a>
            <Show when={tooManyCommitments(commitments())}>
              <a class="commitments-hint" href="#/zusagen">
                {COMMITMENT_HINT}
              </a>
            </Show>
          </span>
        </Show>
      </header>
      <FocusSection />
      <Show when={groups().flames.length > 0}>
        <section class="group group-flame">
          <h2>Laufende Flamme</h2>
          <TaskList tasks={groups().flames} showLocation label="Laufende Flamme" />
        </section>
      </Show>
      <Show when={groups().overdue.length > 0}>
        <section class="group">
          <h2 class="danger">Überfällig</h2>
          <TaskList tasks={groups().overdue} showLocation label="Überfällig" actions={todayRowActions} swipe={todaySwipe} />
        </section>
      </Show>
      <Show when={groups().dueToday.length > 0}>
        <section class="group">
          <h2>Heute fällig</h2>
          <TaskList tasks={groups().dueToday} showLocation label="Heute fällig" actions={todayRowActions} swipe={todaySwipe} />
        </section>
      </Show>
      <Show when={groups().reminders.length > 0}>
        <section class="group">
          <h2>Erinnerungen</h2>
          <TaskList tasks={groups().reminders} showLocation label="Erinnerungen" actions={todayRowActions} swipe={todaySwipe} />
        </section>
      </Show>
      <Show when={groups().followUps.length > 0}>
        <section class="group">
          <h2>Wiedervorlage</h2>
          <p class="muted group-hint">Noch keine Antwort. Nachfragen, wenn es dir hilft; sonst wartet es einfach weiter.</p>
          <TaskList tasks={groups().followUps} showLocation waitingStill label="Wiedervorlage" actions={followUpActions} swipe={todaySwipe} />
        </section>
      </Show>
      <Show when={nothing()}>
        <EmptyState icon="sun" title="Nichts fällig." hint="Kein Termin drängt. Ein guter Tag für eine Flamme.">
          <a class="btn btn-small" href="#/phoenix">
            Zu Phönix
          </a>
        </EmptyState>
      </Show>
      <WeekMirror />
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
    // A recurring task asks first (only this instance or the whole series).
    dropTask(task.id, () => selectNextAfter(task.id));
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

/**
 * Wartet auf andere (contract 5.1): what was handed back or waits on someone's decision,
 * grouped by name. "Antwort ist da" clears the name; the server then opens the task again
 * unless a blocker still holds it.
 */
export function WaitingView() {
  const groups = createMemo(waitingGroups);
  const all = createMemo(() => groups().flatMap((g) => g.tasks));
  onMount(() => registerList(all));
  return (
    <div class="view">
      <header class="view-head">
        <h1>Wartet auf andere</h1>
        <span class="count-badge">{all().length}</span>
      </header>
      <For each={groups()}>
        {(group) => (
          <section class="group">
            <h2>{group.name}</h2>
            <TaskList
              tasks={group.tasks}
              showLocation
              label={`Wartet auf ${group.name}`}
              actions={(task) => [{ label: "Antwort ist da", mobile: true, run: () => answered(task) }]}
            />
          </section>
        )}
      </For>
      <Show when={all().length === 0}>
        <EmptyState
          icon="user"
          title="Niemand ist am Zug."
          hint="Hier steht, was du zurückgegeben hast oder wo jemand anderes entscheidet. Im Detail unter „Wartet auf“ eintragen."
        />
      </Show>
    </div>
  );
}

/**
 * Offene Zusagen (contract 5.1): everything someone waits on you for, reached from the count in
 * Heute's head. Rauch rows can be handed back right here (the dialog of "Zurückgeben").
 */
export function CommitmentsView() {
  const tasks = createMemo(commitmentTasks);
  onMount(() => registerList(tasks));
  return (
    <div class="view">
      <header class="view-head">
        <h1>Offene Zusagen</h1>
        <span class="count-badge">{tasks().length}</span>
        <Show when={tooManyCommitments(tasks().length)}>
          <span class="muted">Rauch kannst du hier zurückgeben.</span>
        </Show>
      </header>
      <TaskList
        tasks={tasks()}
        showLocation
        label="Offene Zusagen"
        empty={<EmptyState icon="user" title="Keine offenen Zusagen." hint="Hier steht, was du anderen zugesagt hast: Für wen „andere“, noch nicht erledigt." />}
        actions={(task) => (canHandBack(task) ? [{ label: "Zurückgeben", mobile: true, run: () => setHandBackRequest({ taskId: task.id }) }] : [])}
      />
    </div>
  );
}
