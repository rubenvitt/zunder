// Client-side mirror of the most important core validations (contract 2.5) for
// instant feedback. The server stays the authority; this never replaces it.

import { isRfc3339, isValidDue, isValidFire } from "./dates";
import { entityKey, EntityStore, type EntityState } from "./merge";
import { parseLocation, toArea, toProject, toTask } from "./model";
import {
  AREA_STATES,
  FIELDS_BY_ENTITY,
  PHOENIX_ROLES,
  PROJECT_KINDS,
  PROJECT_STATUSES,
  PROJECT_VIEWS,
  TASK_STATUSES,
  TASK_TYPES,
  type EntityKind,
  type JsonValue,
  type Location,
  type Op,
  type Task,
} from "./types";

export interface ValidationIssue {
  op_id?: string;
  entity_id: string;
  code: string;
  message: string;
}

export const TITLE_MAX = 500;
const REPO_RE = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;

function isObject(v: JsonValue): v is { [key: string]: JsonValue } {
  return !!v && typeof v === "object" && !Array.isArray(v);
}

function isUrl(v: string): boolean {
  try {
    const u = new URL(v);
    return u.protocol === "http:" || u.protocol === "https:" || u.protocol === "obsidian:";
  } catch {
    return false;
  }
}

const optString = (v: JsonValue) => v === null || typeof v === "string";
const optEnum = (allowed: readonly string[]) => (v: JsonValue) =>
  v === null || (typeof v === "string" && allowed.includes(v));
const optRfc3339 = (v: JsonValue) => v === null || (typeof v === "string" && isRfc3339(v));

type Check = (v: JsonValue) => boolean;

const FIELD_CHECKS: Record<EntityKind, Record<string, Check>> = {
  area: {
    name: (v) => typeof v === "string",
    state: optEnum(AREA_STATES),
    vault_path: optString,
    deleted: (v) => typeof v === "boolean",
  },
  project: {
    name: (v) => typeof v === "string",
    area_id: (v) => typeof v === "string",
    status: optEnum(PROJECT_STATUSES),
    kind: optEnum(PROJECT_KINDS),
    repo: (v) => v === null || (typeof v === "string" && REPO_RE.test(v)),
    description: optString,
    view: optEnum(PROJECT_VIEWS),
    deleted: (v) => typeof v === "boolean",
  },
  task: {
    title: (v) => typeof v === "string",
    description: optString,
    type: optEnum(TASK_TYPES),
    status: optEnum(TASK_STATUSES),
    location: (v) => v === null || parseLocation(v) !== null,
    parent_id: optString,
    due: (v) => v === null || (typeof v === "string" && isValidDue(v)),
    remind_at: optRfc3339,
    vault_links: (v) => v === null || (Array.isArray(v) && v.every((x) => typeof x === "string")),
    links: (v) =>
      v === null ||
      (Array.isArray(v) &&
        v.every(
          (x) =>
            isObject(x) &&
            typeof x.url === "string" &&
            isUrl(x.url) &&
            (x.title === undefined || x.title === null || typeof x.title === "string"),
        )),
    phoenix: (v) =>
      v === null ||
      (isObject(v) &&
        typeof v.role === "string" &&
        (PHOENIX_ROLES as readonly string[]).includes(v.role) &&
        typeof v.fire === "string" &&
        isValidFire(v.fire)),
    claimed_by: optString,
    claim_expires_at: optRfc3339,
    plan: optString,
    branch: optString,
    pr_url: (v) => v === null || (typeof v === "string" && isUrl(v)),
    timing_ref: optString,
    flame_started_at: optRfc3339,
    flame_ended_at: optRfc3339,
    clickup_id: optString,
    rank: optString,
    deleted: (v) => typeof v === "boolean",
  },
};

const FIELD_LABELS: Record<string, string> = {
  due: "Fälligkeit (Datum JJJJ-MM-TT oder Zeitpunkt)",
  remind_at: "Erinnerung",
  links: "Links (gültige URL)",
  phoenix: "Phönix (Rolle und Feuer wie 2026-W45)",
  pr_url: "PR-Link (gültige URL)",
  repo: "Repository (owner/name)",
};

/** Static check of a single field value. */
export function checkField(entity: EntityKind, field: string, value: JsonValue): { code: string; message: string } | null {
  if (!FIELDS_BY_ENTITY[entity].includes(field)) {
    return { code: "unknown_field", message: `Unbekanntes Feld „${field}“.` };
  }
  const check = FIELD_CHECKS[entity][field];
  if (check && !check(value)) {
    return { code: "invalid_value", message: `Ungültiger Wert für ${FIELD_LABELS[field] ?? `„${field}“`}.` };
  }
  return null;
}

/** View over the base state with the proposed ops applied to copies of touched entities. */
class Overlay {
  private readonly touched = new Map<string, EntityState>();

  constructor(private readonly base: EntityStore) {}

  apply(op: Op): void {
    const key = entityKey(op.entity, op.entity_id);
    let e = this.touched.get(key);
    if (!e) {
      const orig = this.base.getByKey(key);
      e = { entity: op.entity, id: op.entity_id, fields: orig ? { ...orig.fields } : {} };
      this.touched.set(key, e);
    }
    const current = e.fields[op.field];
    if (!current || op.hlc > current.hlc || (op.hlc === current.hlc && op.op_id > current.op_id)) {
      e.fields[op.field] = { value: op.value, hlc: op.hlc, op_id: op.op_id };
    }
  }

  get(entity: EntityKind, id: string): EntityState | undefined {
    const key = entityKey(entity, id);
    return this.touched.get(key) ?? this.base.getByKey(key);
  }

  task(id: string): Task | undefined {
    const e = this.get("task", id);
    return e ? toTask(e) : undefined;
  }

  existsAlive(entity: EntityKind, id: string): boolean {
    const e = this.get(entity, id);
    return !!e && e.fields.deleted?.value !== true;
  }

  isDevProject(id: string): boolean {
    const e = this.get("project", id);
    return !!e && e.fields.deleted?.value !== true && toProject(e).kind === "dev";
  }

  effectiveLocation(task: Task): Location {
    let current = task;
    const seen = new Set([task.id]);
    while (current.parent_id) {
      const parent = this.task(current.parent_id);
      if (!parent || seen.has(parent.id)) break;
      seen.add(parent.id);
      current = parent;
    }
    return current.location;
  }

  hasCycle(task: Task): boolean {
    const seen = new Set([task.id]);
    let parentId = task.parent_id;
    while (parentId) {
      if (seen.has(parentId)) return true;
      seen.add(parentId);
      parentId = this.task(parentId)?.parent_id ?? null;
    }
    return false;
  }

  touchedEntities(): EntityState[] {
    return [...this.touched.values()];
  }

  /** Tasks (from base and overlay) whose effective location is the given project. */
  tasksInProject(projectId: string): Task[] {
    const out: Task[] = [];
    const seen = new Set<string>();
    const consider = (e: EntityState) => {
      if (e.entity !== "task" || seen.has(e.id)) return;
      seen.add(e.id);
      const t = toTask(e);
      if (t.deleted) return;
      const loc = this.effectiveLocation(t);
      if (loc.kind === "project" && loc.id === projectId) out.push(t);
    };
    for (const e of this.touched.values()) consider(e);
    for (const e of this.base.values()) if (!this.touched.has(entityKey(e.entity, e.id))) consider(e);
    return out;
  }
}

function isDevOnly(task: Task): boolean {
  return task.type === "bug" || task.type === "feature" || task.status === "review";
}

/**
 * Validates one commit (ops for one or more entities) against the current state.
 * Mirrors the per-entity granularity of the server: issues name the entity.
 */
export function validateOps(base: EntityStore, ops: readonly Op[]): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  const overlay = new Overlay(base);
  const touchedFields = new Map<string, Set<string>>();

  for (const op of ops) {
    const issue = checkField(op.entity, op.field, op.value);
    if (issue) issues.push({ op_id: op.op_id, entity_id: op.entity_id, ...issue });
    overlay.apply(op);
    const key = entityKey(op.entity, op.entity_id);
    let set = touchedFields.get(key);
    if (!set) touchedFields.set(key, (set = new Set()));
    set.add(op.field);
  }
  if (issues.length > 0) return issues;

  for (const e of overlay.touchedEntities()) {
    const fields = touchedFields.get(entityKey(e.entity, e.id)) ?? new Set<string>();
    const isNew = !base.has(e.entity, e.id);
    const push = (code: string, message: string) => issues.push({ entity_id: e.id, code, message });

    if (e.entity === "area") {
      const area = toArea(e);
      if (area.name.trim() === "") push("required", "Ein Bereich braucht einen Namen.");
    } else if (e.entity === "project") {
      const project = toProject(e);
      if (project.name.trim() === "") push("required", "Ein Projekt braucht einen Namen.");
      if (!project.area_id) push("required", "Ein Projekt gehört immer zu einem Bereich.");
      else if ((isNew || fields.has("area_id")) && !overlay.existsAlive("area", project.area_id)) {
        push("invalid_reference", "Der Bereich existiert nicht.");
      }
      if (fields.has("kind") && project.kind === "standard" && !isNew) {
        if (overlay.tasksInProject(project.id).some(isDevOnly)) {
          push("project_has_dev_tasks", "Das Projekt enthält noch Bugs, Features oder Review-Aufgaben.");
        }
      }
    } else {
      const task = toTask(e);
      const titleLength = [...task.title].length;
      if (task.title.trim() === "") push("required", "Eine Aufgabe braucht einen Titel.");
      else if (titleLength > TITLE_MAX) push("title_too_long", `Der Titel ist länger als ${TITLE_MAX} Zeichen.`);

      if ((isNew || fields.has("location")) && (task.location.kind === "area" || task.location.kind === "project")) {
        if (!overlay.existsAlive(task.location.kind, task.location.id)) {
          push("invalid_reference", task.location.kind === "area" ? "Der Bereich existiert nicht." : "Das Projekt existiert nicht.");
        }
      }
      if (fields.has("parent_id") && task.parent_id) {
        if (task.parent_id === task.id || overlay.hasCycle(task)) {
          push("parent_cycle", "Eine Aufgabe kann nicht unter sich selbst hängen.");
        } else if (!overlay.existsAlive("task", task.parent_id)) {
          push("invalid_reference", "Die Elternaufgabe existiert nicht.");
        }
      }
      const relevant = isNew || ["type", "status", "location", "parent_id"].some((f) => fields.has(f));
      if (relevant && isDevOnly(task) && !task.deleted) {
        const loc = overlay.effectiveLocation(task);
        const inDev = loc.kind === "project" && overlay.isDevProject(loc.id);
        if (!inDev) {
          const moved = !isNew && (fields.has("location") || fields.has("parent_id"));
          if (moved) {
            push("dev_task_move", "Bugs, Features und Review-Aufgaben können ein Dev-Projekt nicht verlassen.");
          } else if (task.status === "review") {
            push("dev_only_status", "Den Status „Review“ gibt es nur in Dev-Projekten.");
          } else {
            push("dev_only_type", "Bugs und Features gibt es nur in Dev-Projekten.");
          }
        }
      }
    }
  }
  return issues;
}

