// Heute, Inbox, Lose, Wartet auf andere and Offene Zusagen.

import { createMemo, For, onCleanup, onMount, Show } from "solid-js";
import { EmptyState } from "../components/EmptyState";
import { TaskList } from "../components/TaskList";
import { createTaskFilter, NoMatches, TaskFilterBar } from "../components/TaskFilter";
import { TodayFocus, todayFocus } from "../components/TodayFocus";
import { createTodayDrag, TodayOrder } from "../components/TodayOrder";
import { WeekMirror } from "../components/WeekMirror";
import { DayPreview, openDayTasks, previewLoads } from "../components/DayPreview";
import type { DeferTarget } from "../sync/defer";
import type { Task } from "../sync/types";
import { canHandBack, deferTask, dropTask, moveTask, placeInToday, setWaitingOn, shorten } from "../state/actions";
import { commitmentTasks, inboxTasks, tasksAt, todaySections, waitingGroups } from "../state/selectors";
import { activeList, copyText, pickLocation, registerList, selectedId, setActiveReorder, setCaptureOpen, setHandBackRequest, setSelectedId, toast } from "../state/ui";
import { followUpSentence } from "../sync/handback";
import { now } from "../state/store";
import { focusSettings, focusToday, refreshFocus } from "../state/focus";
import { isDismissed } from "../sync/focus";
import { tooManyCommitments } from "../sync/commitments";
import { COMMITMENT_HINT, commitmentsLabel, locationLabel, TODAY_REST } from "../labels";

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
  const sections = createMemo(todaySections);
  const today = createMemo(focusToday);
  // The box: the flame, the plan's first task or, without a plan, "Erst für dich" (design 5.2).
  const focus = createMemo(() => todayFocus(sections(), today(), isDismissed(focusSettings(), today())));
  const focusId = () => focus().task?.id;
  const rest = createMemo(() => sections().list.filter((t) => t.id !== focusId()));
  const reminders = createMemo(() => sections().reminders.filter((t) => t.id !== focusId()));
  const followUps = createMemo(() => sections().followUps.filter((t) => t.id !== focusId()));
  const boxShown = () => !!focus().task || focus().choices !== null || focus().planDone;
  // Tagesvorschau: the next days and their due tasks (contract 5.1).
  const loads = createMemo(previewLoads);
  const all = createMemo(() => {
    const seen = new Set<string>();
    const box = focus().task;
    return [...(box ? [box] : []), ...rest(), ...reminders(), ...followUps(), ...openDayTasks(loads())].filter((t) => !seen.has(t.id) && !!seen.add(t.id));
  });
  // Zusagen-Bremse: how many promises are open, shown before they press (contract 5.1).
  const commitments = createMemo(() => commitmentTasks().length);
  const drag = createTodayDrag(() => sections().list);
  /** Shift+J/K: one step down or up in the whole order, past the box as well (design 5.3). */
  const reorder = (dy: -1 | 1) => {
    const id = selectedId();
    const items = sections().list;
    const i = items.findIndex((t) => t.id === id);
    if (!id || i < 0) return;
    const order = items.filter((t) => t.id !== id);
    const at = Math.max(0, Math.min(order.length, i + dy));
    if (at !== i) placeInToday(id, order[at - 1] ?? null, order[at] ?? null);
  };
  onMount(() => {
    registerList(all);
    setActiveReorder(reorder);
    // Another device may have clicked "Erst für dich" away today.
    void refreshFocus();
  });
  onCleanup(() => setActiveReorder(null));
  const nothing = () => !focus().task && rest().length + reminders().length + followUps().length === 0;
  return (
    <div class="view">
      <header class="view-head today-head">
        <h1>Heute</h1>
        <span class="muted">{new Date(now()).toLocaleDateString("de-DE", { weekday: "long", day: "numeric", month: "long" })}</span>
        <Show when={commitments() > 0}>
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
      <Show when={boxShown()}>
        <TodayFocus state={focus()} drag={drag} actions={todayRowActions} swipe={todaySwipe} />
      </Show>
      <Show when={rest().length > 0}>
        <section class="group today-rest" aria-labelledby={boxShown() ? "today-rest-heading" : undefined}>
          <Show when={boxShown()}>
            <h2 id="today-rest-heading">{TODAY_REST}</h2>
          </Show>
          <TodayOrder tasks={rest()} items={sections().list} drag={drag} label={TODAY_REST} actions={todayRowActions} swipe={todaySwipe} />
        </section>
      </Show>
      <Show when={reminders().length > 0}>
        <section class="group">
          <h2 class="tone tone-violet">
            Erinnerungen <span class="count">{reminders().length}</span>
          </h2>
          <TaskList tasks={reminders()} showLocation quietDue label="Erinnerungen" actions={todayRowActions} swipe={todaySwipe} />
        </section>
      </Show>
      <Show when={followUps().length > 0}>
        <section class="group">
          <h2 class="tone tone-info">
            Wiedervorlage <span class="count">{followUps().length}</span>
          </h2>
          <p class="muted group-hint">Noch keine Antwort. Nachfragen, wenn es dir hilft; sonst wartet es einfach weiter.</p>
          <TaskList tasks={followUps()} showLocation waitingStill quietDue label="Wiedervorlage" actions={followUpActions} swipe={todaySwipe} />
        </section>
      </Show>
      <Show when={nothing() && !focus().planDone}>
        <EmptyState
          icon="sun"
          figure={{ name: "slow", label: "Kisten auf einem Förderband, das gemächlich läuft", play: true }}
          title="Nichts fällig." hint="Kein Termin drängt. Ein guter Tag für eine Flamme.">
          <a class="btn btn-small" href="#/phoenix">
            Zu Phönix
          </a>
        </EmptyState>
      </Show>
      <DayPreview loads={loads()} />
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
        <span class="key-hints hide-mobile">
          <span>
            <kbd>a</kbd> zuordnen
          </span>
          <span>
            <kbd>l</kbd> lose
          </span>
          <span>
            <kbd>d</kbd> verwerfen
          </span>
        </span>
      </header>
      <TaskList
        tasks={tasks()}
        label="Inbox"
        empty={
          <EmptyState
            icon="checkCircle"
            figure={{ name: "basket", label: "Ein leerer Drahtkorb" }}
            title="Inbox leer." hint="Alles eingeordnet. Neue Gedanken landen hier, bis du sie zuordnest.">
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
  const all = createMemo(() => tasksAt({ kind: "loose" }));
  const filter = createTaskFilter();
  const tasks = createMemo(() => filter.apply(all()));
  onMount(() => registerList(tasks));
  return (
    <div class="view">
      <header class="view-head">
        <h1>Lose</h1>
        <span class="count-badge">{all().length}</span>
      </header>
      <Show when={all().length > 0 || filter.active()}>
        <TaskFilterBar state={filter} weights shown={tasks().length} total={all().length} label="Lose Aufgaben" />
      </Show>
      <TaskList
        tasks={tasks()}
        label="Lose Aufgaben"
        empty={filter.active() ? <NoMatches state={filter} /> : <EmptyState icon="loose" figure={{ name: "rail", label: "Eine Kleiderstange mit leeren Bügeln" }} title="Keine losen Aufgaben." hint="Hier liegt, was bewusst zu keinem Bereich gehört. Aus der Inbox mit l hierher schieben." />}
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
            <h2 class="tone tone-info">
              {group.name} <span class="count">{group.tasks.length}</span>
            </h2>
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
          figure={{ name: "elevator", label: "Ein Aufzug, auf den niemand wartet" }}
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
        empty={<EmptyState icon="user" figure={{ name: "padlock", label: "Ein geöffnetes Vorhängeschloss" }} title="Keine offenen Zusagen." hint="Hier steht, was du anderen zugesagt hast: Für wen „andere“, noch nicht erledigt." />}
        actions={(task) => (canHandBack(task) ? [{ label: "Zurückgeben", mobile: true, run: () => setHandBackRequest({ taskId: task.id }) }] : [])}
      />
    </div>
  );
}
