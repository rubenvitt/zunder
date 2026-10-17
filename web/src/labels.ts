// German UI labels for the English codes used on the wire.

import type { AreaState, ForWhom, JsonValue, Location, PhoenixRole, ProjectKind, ProjectStatus, Recurrence, RecurrenceFrom, RecurrenceUnit, Task, TaskStatus, TaskType, WeekdayCode, Weight } from "./sync/types";
import { WEEKDAY_CODES } from "./sync/types";
import { formatDateLabel, parseDue } from "./sync/dates";
import { parseRecurrence, sameRecurrence, weekdayOf } from "./sync/recurrence";
import { data } from "./state/store";

export const STATUS_LABELS: Record<TaskStatus, string> = {
  open: "Offen",
  in_progress: "In Arbeit",
  review: "Review",
  blocked: "Blockiert",
  done: "Erledigt",
  dropped: "Verworfen",
};

export const TYPE_LABELS: Record<TaskType, string> = {
  aufgabe: "Aufgabe",
  bug: "Bug",
  feature: "Feature",
};

export const PROJECT_STATUS_LABELS: Record<ProjectStatus, string> = {
  active: "Aktiv",
  paused: "Pausiert",
  completed: "Abgeschlossen",
};

export const PROJECT_KIND_LABELS: Record<ProjectKind, string> = {
  standard: "Standard",
  dev: "Dev",
};

export const AREA_STATE_LABELS: Record<AreaState, string> = {
  active: "Aktiv",
  archived: "Archiviert",
};

export const PHOENIX_LABELS: Record<PhoenixRole, string> = {
  feuer_schritt: "Feuer-Schritt",
  flamme: "Flamme",
};

/** Weight after Covey's quadrants, in Zunder's fire images. */
export const WEIGHT_LABELS: Record<Weight, string> = {
  brand: "Brand",
  glut: "Glut",
  rauch: "Rauch",
  asche: "Asche",
};

/** What a weight means and what to do with it (tooltips, MCP wording alike). */
export const WEIGHT_HINTS: Record<Weight, string> = {
  brand: "Wichtig und dringlich: heute tun",
  glut: "Wichtig, nicht dringlich: einplanen, darf rutschen, nicht verlieren",
  rauch: "Dringlich nur fürs System: kurz halten, hinterfragen, zurückgeben oder ablehnen",
  asche: "Weder wichtig noch dringlich: darf liegen bleiben, streichen ist in Ordnung",
};

export const FOR_WHOM_LABELS: Record<ForWhom, string> = {
  mich: "für mich",
  uns: "für uns",
  andere: "für andere",
};

/** The question each choice answers, shown while nothing is picked. */
export const WEIGHT_QUESTION = "Ist es dir wichtig, und drängt es wirklich?";
export const FOR_WHOM_QUESTION = "Wer wartet, wenn es sich verschiebt?";

/** Whom it is for means who waits on it (a commitment), not who benefits. */
export const FOR_WHOM_HINTS: Record<ForWhom, string> = {
  mich: "Niemand wartet darauf: still verschiebbar",
  uns: "Partnerschaft oder Haushalt warten mit: verschieben heißt absprechen",
  andere: "Jemand wartet auf deine Zusage: wer verschiebt, sagt Bescheid",
};

/** Heute's quiet hint from COMMITMENT_LIMIT open commitments on (sync/commitments). */
export const COMMITMENT_HINT = "Viele Zusagen offen. Eine zurückgeben?";

/** "1 offene Zusage", "3 offene Zusagen": Heute's count of open commitments. */
export function commitmentsLabel(count: number): string {
  return count === 1 ? "1 offene Zusage" : `${count} offene Zusagen`;
}

/** "wartet auf Anna": whom a task waits on (waiting_on). */
export function waitingLabel(name: string): string {
  return `wartet auf ${name}`;
}

export function deferralsLabel(count: number): string {
  return count === 1 ? "einmal verschoben" : `${count}-mal verschoben`;
}

export const FIELD_LABELS: Record<string, string> = {
  title: "Titel",
  name: "Name",
  description: "Beschreibung",
  type: "Typ",
  status: "Status",
  location: "Stelle",
  parent_id: "Elternaufgabe",
  due: "Fällig",
  remind_at: "Erinnerung",
  vault_links: "Vault-Links",
  links: "Links",
  phoenix: "Phönix",
  claimed_by: "Reserviert von",
  claim_expires_at: "Reservierung bis",
  plan: "Plan",
  branch: "Branch",
  pr_url: "PR",
  timing_ref: "Timing",
  flame_started_at: "Flamme gestartet",
  flame_ended_at: "Flamme beendet",
  clickup_id: "ClickUp-ID",
  rank: "Reihenfolge",
  undated_ack: "Bewusst ohne Fälligkeit",
  recurrence: "Rhythmus",
  recurrence_prev: "Vorherige Instanz",
  blocked_by: "Blockiert durch",
  weight: "Gewicht",
  for_whom: "Für wen",
  deferrals: "Aufschübe",
  waiting_on: "Wartet auf",
  ref: "Kennung",
  key: "Schlüssel",
  deleted: "Gelöscht",
  area_id: "Bereich",
  state: "Zustand",
  kind: "Art",
  repo: "Repository",
  vault_path: "Vault-Pfad",
  body: "Text",
};

/**
 * Readable origin of a history entry. Sources per contract 3: `web`, `api:<token>`,
 * `mcp:<token>` (agents), `mcp:oauth:<client>` (apps connected via OAuth, e.g. claude.ai),
 * `app:<device>` (the Zunder app), `intake:<channel>` (tasks from another server through an
 * intake channel) and `server` (e.g. the Timing bridge).
 */
export function sourceLabel(source: string): string {
  if (source === "web") return "Web";
  if (source === "server") return "Server";
  const colon = source.indexOf(":");
  if (colon > 0) {
    const kind = source.slice(0, colon);
    const name = source.slice(colon + 1);
    if (kind === "api") return name === "import" ? "Import" : `API · ${name}`;
    if (kind === "app") return `App · ${name}`;
    if (kind === "intake") return `Eingang · ${name}`;
    if (kind === "mcp") return name.startsWith("oauth:") ? `Agent · ${name.slice(6)} (OAuth)` : `Agent · ${name}`;
  }
  return source;
}

/** Board columns; "review" only on dev boards. Dropped tasks are not shown on boards. */
export function boardStatuses(dev: boolean): TaskStatus[] {
  return dev ? ["open", "in_progress", "review", "blocked", "done"] : ["open", "in_progress", "blocked", "done"];
}

export function locationLabel(location: Location): string {
  switch (location.kind) {
    case "inbox":
      return "Inbox";
    case "loose":
      return "Lose";
    case "area":
      return data.areas[location.id]?.name ?? "Unbekannter Bereich";
    case "project": {
      const project = data.projects[location.id];
      if (!project) return "Unbekanntes Projekt";
      const area = data.areas[project.area_id];
      return area ? `${area.name} › ${project.name}` : project.name;
    }
  }
}

/** A task named by a field value, with its label (deleted tasks keep theirs, unknown ones are „andere Aufgabe“). */
export interface TaskRefLabel {
  id: string;
  task: Task | undefined;
  label: string;
}

/**
 * The tasks a field value names: `parent_id` (by title), `blocked_by` and
 * `recurrence_prev` (by reference, else title). Null for other fields, null and `[]`.
 */
export function historyTaskRefs(field: string, value: JsonValue): TaskRefLabel[] | null {
  let ids: string[];
  if ((field === "parent_id" || field === "recurrence_prev") && typeof value === "string") ids = [value];
  else if (field === "blocked_by" && Array.isArray(value) && value.length > 0) ids = value.map((id) => (typeof id === "string" ? id : ""));
  else return null;
  return ids.map((id) => {
    const task = id ? data.tasks[id] : undefined;
    const label = !task ? "andere Aufgabe" : field === "parent_id" ? task.title : (task.ref ?? `„${task.title}“`);
    return { id, task, label };
  });
}

/** A field value of an operation in words (change history, agent log). */
export function describeValue(field: string, value: JsonValue): string {
  if (value === null) return "entfernt";
  if (field === "status" && typeof value === "string") return STATUS_LABELS[value as TaskStatus] ?? value;
  if (field === "type" && typeof value === "string") return TYPE_LABELS[value as TaskType] ?? value;
  if (field === "weight" && typeof value === "string") return WEIGHT_LABELS[value as Weight] ?? value;
  if (field === "for_whom" && typeof value === "string") return FOR_WHOM_LABELS[value as ForWhom] ?? value;
  if (field === "location" && value && typeof value === "object" && !Array.isArray(value)) {
    const kind = value.kind;
    if (kind === "inbox" || kind === "loose") return locationLabel({ kind });
    if ((kind === "area" || kind === "project") && typeof value.id === "string") return locationLabel({ kind, id: value.id });
  }
  const refs = historyTaskRefs(field, value);
  if (refs) return refs.map((r) => r.label).join(", ");
  if (field === "blocked_by" && Array.isArray(value) && value.length === 0) return "keine";
  if (field === "deleted") return "ja";
  if (field === "recurrence") {
    const r = parseRecurrence(value);
    if (r) return describeRecurrence(r);
  }
  if (field === "phoenix" && value && typeof value === "object" && !Array.isArray(value)) {
    return `${PHOENIX_LABELS[value.role as keyof typeof PHOENIX_LABELS] ?? value.role} ${value.fire}`;
  }
  if (typeof value === "string") return value.length > 80 ? `„${value.slice(0, 79)}…“` : `„${value}“`;
  if (Array.isArray(value)) return `${value.length} Einträge`;
  return JSON.stringify(value);
}

// ---- Recurrence (contract 5.1, "Rhythmen") ----------------------------------------

export const WEEKDAY_LABELS: Record<WeekdayCode, string> = { mo: "Mo", tu: "Di", we: "Mi", th: "Do", fr: "Fr", sa: "Sa", su: "So" };

export const WEEKDAY_NAMES: Record<WeekdayCode, string> = {
  mo: "Montag",
  tu: "Dienstag",
  we: "Mittwoch",
  th: "Donnerstag",
  fr: "Freitag",
  sa: "Samstag",
  su: "Sonntag",
};

/** Unit as the editor names it ("alle 2 Wochen"). */
export const RECURRENCE_UNIT_LABELS: Record<RecurrenceUnit, { one: string; many: string }> = {
  day: { one: "Tag", many: "Tage" },
  week: { one: "Woche", many: "Wochen" },
  month: { one: "Monat", many: "Monate" },
  year: { one: "Jahr", many: "Jahre" },
};

export const RECURRENCE_FROM_LABELS: Record<RecurrenceFrom, string> = {
  due: "ab Fälligkeit",
  done: "nach Erledigung",
};

const EVERY_ONE: Record<RecurrenceUnit, string> = { day: "Täglich", week: "Wöchentlich", month: "Monatlich", year: "Jährlich" };
const WORKDAYS = "mo,tu,we,th,fr";

/**
 * A rhythm in words: "Täglich", "Alle 3 Monate nach Erledigung", "Wöchentlich (Mo, Do)",
 * "Werktags", "Monatlich am Monatsletzten", "Jährlich am 28.02." (the month of `due`) …
 */
export function describeRecurrence(r: Recurrence, due?: string | null): string {
  const days = WEEKDAY_CODES.filter((d) => r.weekdays?.includes(d));
  const workdays = r.every === 1 && r.unit === "week" && days.join(",") === WORKDAYS;
  let text = workdays ? "Werktags" : r.every === 1 ? EVERY_ONE[r.unit] : `Alle ${r.every} ${RECURRENCE_UNIT_LABELS[r.unit].many}`;
  if (!workdays && days.length > 0) text += ` (${days.map((d) => WEEKDAY_LABELS[d]).join(", ")})`;
  const month = yearlyDayMonth(r, due);
  if (r.day === "last") text += " am Monatsletzten";
  else if (r.day !== undefined) text += month ? ` ${yearlyDayLabel(r.day, month)}` : ` am ${r.day}.`;
  if (r.from === "done") text += " nach Erledigung";
  return text;
}

/**
 * Month ("02") in which a yearly rhythm takes its day, if the UI knows it: that of the due
 * date for `from: "due"`. With `from: "done"` the server uses the month of the completion
 * (contract 3.1), so the UI names no month.
 */
export function yearlyDayMonth(r: Recurrence, due?: string | null): string | undefined {
  return r.unit === "year" && r.from === "due" ? parseDue(due ?? null)?.date.slice(5, 7) : undefined;
}

/** Days the month ("02") can have at most: February 29, since a 29 comes back in leap years. */
export function monthMaxDays(month: string): number {
  return [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][Number(month) - 1] ?? 31;
}

/** "am 28.02." for a yearly day; a day beyond the month's length is its last ("am 30.04." for 31). */
export function yearlyDayLabel(day: number, month: string): string {
  return `am ${String(Math.min(day, monthMaxDays(month))).padStart(2, "0")}.${month}.`;
}

export interface RecurrencePreset {
  label: string;
  value: Recurrence;
}

/** Common rhythms offered in the detail; "weekly" falls on the weekday of the due date. */
export function recurrencePresets(due: string | null): RecurrencePreset[] {
  const date = parseDue(due)?.date;
  const weekday = date ? weekdayOf(date) : null;
  const out: RecurrencePreset[] = [{ label: "Täglich", value: { every: 1, unit: "day", from: "due" } }];
  if (weekday) out.push({ label: `Wöchentlich am ${WEEKDAY_NAMES[weekday]}`, value: { every: 1, unit: "week", weekdays: [weekday], from: "due" } });
  out.push(
    { label: "Alle 2 Wochen", value: { every: 2, unit: "week", from: "due" } },
    { label: "Monatlich", value: { every: 1, unit: "month", from: "due" } },
    { label: "Monatlich am Monatsletzten", value: { every: 1, unit: "month", day: "last", from: "due" } },
    { label: "Jährlich", value: { every: 1, unit: "year", from: "due" } },
  );
  return out;
}

export function matchPreset(presets: readonly RecurrencePreset[], r: Recurrence | null): RecurrencePreset | undefined {
  return r ? presets.find((p) => sameRecurrence(p.value, r)) : undefined;
}

/** Toast once the server delivered the next instance of a series. */
export function nextInstanceMessage(due: string | null, now: Date): string {
  const info = parseDue(due);
  if (!info) return "Nächste Instanz angelegt.";
  const label = formatDateLabel(info.date, now);
  const day = ["Heute", "Morgen", "Gestern"].includes(label) ? label.toLowerCase() : `am ${label}`;
  return `Nächste Instanz ${day}${info.time ? ` um ${info.time}` : ""}`;
}
