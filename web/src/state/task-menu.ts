// Right click on a task (row, card, subtask, task link): the actions of the detail and
// the row in one menu. The listener lives in state/context-menu.ts.

import { dateFromLocal, formatDateLabel, parseDue, toRfc3339Local } from "../sync/dates";
import { isClosed, isDevLocation } from "../sync/derived";
import { deferDay, offersNextWeek } from "../sync/defer";
import { WEIGHTS, type JsonValue, type Task, type TaskStatus, type Weight } from "../sync/types";
import { boardStatuses, locationLabel, STATUS_LABELS, WEIGHT_HINTS, WEIGHT_LABELS } from "../labels";
import { taskUrl } from "../components/TaskLinks";
import { clearDue, deferTask, deleteTask, dependentsCount, dropTask, moveTask, setStatus, shorten, updateTask } from "./actions";
import type { MenuEntry } from "./menu";
import { data, now, taskIndex } from "./store";
import { copyTaskId, copyText, detailId, openTask, pickLocation, selectedId, setDetailId, setSelectedId, toast } from "./ui";

/** Sets the status the way the check button and the detail do (dropping asks for a series). */
function changeStatus(task: Task, status: TaskStatus): void {
  if (status === "dropped") {
    dropTask(task.id);
    return;
  }
  if (setStatus(task.id, status)) toast(`„${shorten(task.title)}“ → ${STATUS_LABELS[status]}`);
}

/** A new due day; a due time of day stays, a deliberate "undated" ends. */
function setDueDay(task: Task, day: string): void {
  const time = parseDue(task.due)?.time;
  let due = day;
  if (time) {
    const d = dateFromLocal(day)!;
    const [hh, mm] = time.split(":").map(Number);
    due = toRfc3339Local(new Date(d.getFullYear(), d.getMonth(), d.getDate(), hh!, mm!));
  }
  const fields: Record<string, JsonValue> = { due };
  if (task.undated_ack) fields.undated_ack = null;
  if (updateTask(task.id, fields)) toast(`„${shorten(task.title)}“ fällig ${dayPhrase(day)}.`);
}

/** "heute", "morgen", "Mo, 19.10." for the middle of a sentence. */
function dayPhrase(day: string): string {
  const label = formatDateLabel(day, new Date(now()));
  return /^(Heute|Morgen|Gestern)$/.test(label) ? label.toLowerCase() : `am ${label}`;
}

/** Removing the due date of a recurring task ends its series, after asking (as in the detail). */
function removeDue(task: Task): void {
  if (task.recurrence && !confirm("Ohne Fälligkeit endet die Serie.")) return;
  if (clearDue(task.id)) toast(`„${shorten(task.title)}“ ohne Fälligkeit.`);
}

/** "Brand (wichtig und dringlich)": the name with the first half of its hint. */
function weightLabel(w: Weight): string {
  const hint = WEIGHT_HINTS[w].split(":")[0]!;
  return `${WEIGHT_LABELS[w]} (${hint.charAt(0).toLowerCase()}${hint.slice(1)})`;
}

function setWeight(task: Task, weight: Weight | null): void {
  updateTask(task.id, { weight });
}

function removeTask(task: Task): void {
  const count = dependentsCount("task", task.id);
  const msg = count > 0 ? `„${task.title}“ und ${count} Unteraufgabe(n) löschen?` : `„${task.title}“ löschen?`;
  if (!confirm(msg) || !deleteTask(task.id)) return;
  if (detailId() === task.id) setDetailId(null);
  if (selectedId() === task.id) setSelectedId(null);
  toast(`„${shorten(task.title)}“ gelöscht.`);
}

/** The menu for one task. `today` is the local date ("YYYY-MM-DD"). */
export function taskMenuItems(task: Task, today: string): MenuEntry[] {
  const dev = isDevLocation(taskIndex().location(task), (id) => data.projects[id]);
  const closed = isClosed(task.status);
  const dueDate = parseDue(task.due)?.date ?? null;
  const tomorrow = deferDay("tomorrow", today);
  const nextWeek = deferDay("next_week", today);

  const dueItems: MenuEntry[] = [
    { kind: "item", label: "Heute", checked: dueDate === today, run: () => setDueDay(task, today) },
    { kind: "item", label: "Morgen", checked: dueDate === tomorrow, run: () => setDueDay(task, tomorrow) },
  ];
  if (offersNextWeek(today)) dueItems.push({ kind: "item", label: "Nächste Woche", checked: dueDate === nextWeek, run: () => setDueDay(task, nextWeek) });
  dueItems.push({ kind: "separator" }, { kind: "item", label: "Ohne Fälligkeit", disabled: !task.due, run: () => removeDue(task) });

  const statuses = [...boardStatuses(dev), "dropped" as const];
  if (!statuses.includes(task.status)) statuses.splice(statuses.length - 1, 0, task.status);

  const items: MenuEntry[] = [
    { kind: "item", label: "Öffnen", hint: "↵", run: () => openTask(task.id) },
    closed
      ? { kind: "item", label: "Wieder öffnen", run: () => changeStatus(task, "open") }
      : { kind: "item", label: "Erledigt", hint: "x", run: () => changeStatus(task, "done") },
    {
      kind: "submenu",
      label: "Status",
      items: statuses.map((s) => ({ kind: "item", label: STATUS_LABELS[s], checked: task.status === s, run: () => changeStatus(task, s) })),
    },
    { kind: "separator" },
    { kind: "submenu", label: "Fälligkeit", items: dueItems },
  ];

  // The personal reading ("Heute nicht", weight) belongs outside dev projects, as in the detail.
  if (!dev && !closed) {
    const later: MenuEntry[] = [{ kind: "item", label: "Morgen", run: () => deferTask(task.id, "tomorrow") }];
    if (offersNextWeek(today)) later.push({ kind: "item", label: "Nächste Woche", run: () => deferTask(task.id, "next_week") });
    items.push({ kind: "submenu", label: "Heute nicht", items: later });
  }
  if (!dev || task.weight) {
    items.push({
      kind: "submenu",
      label: "Gewicht",
      items: [
        ...WEIGHTS.map((w): MenuEntry => ({ kind: "item", label: weightLabel(w), checked: task.weight === w, run: () => setWeight(task, w) })),
        { kind: "separator" },
        { kind: "item", label: "Ohne Gewicht", checked: !task.weight, run: () => setWeight(task, null) },
      ],
    });
  }

  items.push(
    {
      kind: "item",
      label: task.parent_id ? "Herauslösen und verschieben …" : "Verschieben …",
      run: () =>
        pickLocation(`„${shorten(task.title)}“ verschieben`, (loc) => {
          if (moveTask(task.id, loc)) toast(`Verschoben: ${locationLabel(loc)}`);
        }),
    },
    { kind: "separator" },
  );
  if (task.ref) items.push({ kind: "item", label: `Kennung kopieren (${task.ref})`, hint: "y", run: () => copyTaskId(task) });
  items.push(
    { kind: "item", label: "ID kopieren", hint: task.ref ? undefined : "y", run: () => copyTaskId({ id: task.id, ref: null }) },
    { kind: "item", label: "Link kopieren", run: () => copyText(taskUrl(task), "Link kopiert") },
    { kind: "separator" },
    { kind: "item", label: "Löschen …", danger: true, run: () => removeTask(task) },
  );
  return items;
}
