// Materializes merged entity states into typed entities with contract defaults.
// Malformed values (which the server should never accept) fall back to defaults
// instead of crashing the UI.

import type { EntityState } from "./merge";
import {
  AREA_STATES,
  PHOENIX_ROLES,
  PROJECT_KINDS,
  PROJECT_STATUSES,
  PROJECT_VIEWS,
  TASK_STATUSES,
  TASK_TYPES,
  type Area,
  type Hlc,
  type JsonValue,
  type Link,
  type Location,
  type Phoenix,
  type Project,
  type Task,
} from "./types";

function value(e: EntityState, field: string): JsonValue | undefined {
  return e.fields[field]?.value;
}

function str(v: JsonValue | undefined): string | null {
  return typeof v === "string" ? v : null;
}

function oneOf<T extends string>(v: JsonValue | undefined, allowed: readonly T[], fallback: T): T {
  return typeof v === "string" && (allowed as readonly string[]).includes(v) ? (v as T) : fallback;
}

/** created = smallest HLC ever applied (contract 2.4), updated = largest field HLC. */
function hlcRange(e: EntityState): { min: Hlc; max: Hlc } {
  let max = e.created_hlc;
  for (const f of Object.values(e.fields)) if (f.hlc > max) max = f.hlc;
  return { min: e.created_hlc, max };
}

export function parseLocation(v: JsonValue | undefined): Location | null {
  if (!v || typeof v !== "object" || Array.isArray(v)) return null;
  const kind = v.kind;
  if (kind === "inbox" || kind === "loose") return { kind };
  if ((kind === "area" || kind === "project") && typeof v.id === "string") return { kind, id: v.id };
  return null;
}

export function parseLinks(v: JsonValue | undefined): Link[] {
  if (!Array.isArray(v)) return [];
  const out: Link[] = [];
  for (const item of v) {
    if (item && typeof item === "object" && !Array.isArray(item) && typeof item.url === "string") {
      out.push(typeof item.title === "string" ? { url: item.url, title: item.title } : { url: item.url });
    }
  }
  return out;
}

export function parsePhoenix(v: JsonValue | undefined): Phoenix | null {
  if (!v || typeof v !== "object" || Array.isArray(v)) return null;
  if (typeof v.fire !== "string") return null;
  const role = v.role;
  if (typeof role !== "string" || !(PHOENIX_ROLES as readonly string[]).includes(role)) return null;
  return { role: role as Phoenix["role"], fire: v.fire };
}

function stringArray(v: JsonValue | undefined): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

export function toArea(e: EntityState): Area {
  const { min, max } = hlcRange(e);
  return {
    id: e.id,
    deleted: value(e, "deleted") === true,
    created_hlc: min,
    updated_hlc: max,
    name: str(value(e, "name")) ?? "",
    state: oneOf(value(e, "state"), AREA_STATES, "active"),
    vault_path: str(value(e, "vault_path")),
  };
}

export function toProject(e: EntityState): Project {
  const { min, max } = hlcRange(e);
  const kind = oneOf(value(e, "kind"), PROJECT_KINDS, "standard");
  return {
    id: e.id,
    deleted: value(e, "deleted") === true,
    created_hlc: min,
    updated_hlc: max,
    name: str(value(e, "name")) ?? "",
    area_id: str(value(e, "area_id")) ?? "",
    status: oneOf(value(e, "status"), PROJECT_STATUSES, "active"),
    kind,
    repo: str(value(e, "repo")),
    description: str(value(e, "description")),
    view: oneOf(value(e, "view"), PROJECT_VIEWS, kind === "dev" ? "board" : "list"),
  };
}

export function toTask(e: EntityState): Task {
  const { min, max } = hlcRange(e);
  return {
    id: e.id,
    deleted: value(e, "deleted") === true,
    created_hlc: min,
    updated_hlc: max,
    title: str(value(e, "title")) ?? "",
    description: str(value(e, "description")),
    type: oneOf(value(e, "type"), TASK_TYPES, "aufgabe"),
    status: oneOf(value(e, "status"), TASK_STATUSES, "open"),
    location: parseLocation(value(e, "location")) ?? { kind: "inbox" },
    parent_id: str(value(e, "parent_id")),
    due: str(value(e, "due")),
    remind_at: str(value(e, "remind_at")),
    vault_links: stringArray(value(e, "vault_links")),
    links: parseLinks(value(e, "links")),
    phoenix: parsePhoenix(value(e, "phoenix")),
    claimed_by: str(value(e, "claimed_by")),
    claim_expires_at: str(value(e, "claim_expires_at")),
    plan: str(value(e, "plan")),
    branch: str(value(e, "branch")),
    pr_url: str(value(e, "pr_url")),
    timing_ref: str(value(e, "timing_ref")),
    flame_started_at: str(value(e, "flame_started_at")),
    flame_ended_at: str(value(e, "flame_ended_at")),
    clickup_id: str(value(e, "clickup_id")),
    rank: str(value(e, "rank")),
    status_hlc: e.fields.status?.hlc ?? min,
  };
}
