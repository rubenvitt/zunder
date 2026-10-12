// German UI labels for the English codes used on the wire.

import type { AreaState, Location, PhoenixRole, ProjectKind, ProjectStatus, TaskStatus, TaskType } from "./sync/types";
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
  deleted: "Gelöscht",
};

/**
 * Readable origin of a history entry. Sources per contract 3: `web`, `api:<token>`,
 * `mcp:<token>` (agents), `mcp:oauth:<client>` (apps connected via OAuth, e.g. claude.ai)
 * and `server` (e.g. the Timing bridge).
 */
export function sourceLabel(source: string): string {
  if (source === "web") return "Web";
  if (source === "server") return "Server";
  const colon = source.indexOf(":");
  if (colon > 0) {
    const kind = source.slice(0, colon);
    const name = source.slice(colon + 1);
    if (kind === "api") return name === "import" ? "Import" : `API · ${name}`;
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
