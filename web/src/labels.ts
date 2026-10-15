// German UI labels for the English codes used on the wire.

import type { AreaState, JsonValue, Location, PhoenixRole, ProjectKind, ProjectStatus, TaskStatus, TaskType } from "./sync/types";
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
  blocked_by: "Blockiert durch",
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

/** A field value of an operation in words (change history, agent log). */
export function describeValue(field: string, value: JsonValue): string {
  if (value === null) return "entfernt";
  if (field === "status" && typeof value === "string") return STATUS_LABELS[value as TaskStatus] ?? value;
  if (field === "type" && typeof value === "string") return TYPE_LABELS[value as TaskType] ?? value;
  if (field === "location" && value && typeof value === "object" && !Array.isArray(value)) {
    const kind = value.kind;
    if (kind === "inbox" || kind === "loose") return locationLabel({ kind });
    if ((kind === "area" || kind === "project") && typeof value.id === "string") return locationLabel({ kind, id: value.id });
  }
  if (field === "parent_id" && typeof value === "string") return data.tasks[value]?.title ?? "andere Aufgabe";
  if (field === "blocked_by" && Array.isArray(value)) {
    if (value.length === 0) return "keine";
    return value.map((id) => (typeof id === "string" && data.tasks[id] ? (data.tasks[id]!.ref ?? `„${data.tasks[id]!.title}“`) : "andere Aufgabe")).join(", ");
  }
  if (field === "deleted") return "ja";
  if (field === "phoenix" && value && typeof value === "object" && !Array.isArray(value)) {
    return `${PHOENIX_LABELS[value.role as keyof typeof PHOENIX_LABELS] ?? value.role} ${value.fire}`;
  }
  if (typeof value === "string") return value.length > 80 ? `„${value.slice(0, 79)}…“` : `„${value}“`;
  if (Array.isArray(value)) return `${value.length} Einträge`;
  return JSON.stringify(value);
}
