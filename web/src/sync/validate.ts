// Client-side mirror of the core validation (contract 2.5, zunder_core::validate)
// for instant feedback. Codes and rules follow the core; the server stays the
// authority. Shared validation vectors (kind "validate" in
// crates/zunder-core/tests/vectors) keep both sides in step.

import { isCanonicalId } from "./ids";
import { isRfc3339, isValidDue, isValidFire } from "./dates";
import { entityKey, EntityStore, wins, type EntityState } from "./merge";
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
} from "./types";
import { hlcWallMs } from "./hlc";
import { recurrenceProblem } from "./recurrence";

/** Rejection codes of the core (contract 2.5). */
export type RejectCode =
  | "unknown_field"
  | "invalid_value"
  | "missing_required"
  | "dev_only"
  | "dev_tasks_present"
  | "invalid_reference"
  | "has_dependents"
  | "cycle"
  | "future_hlc"
  | "kind_mismatch"
  | "entity_rejected";

export interface ValidationIssue {
  op_id?: string;
  entity_id: string;
  code: RejectCode;
  message: string;
}

export const TITLE_MAX = 500;
/** Most entries in a task's `blocked_by` (core: model::MAX_BLOCKED_BY). */
export const MAX_BLOCKED_BY = 100;
export const MAX_FUTURE_MS = 24 * 60 * 60 * 1000;

export const REQUIRED_FIELDS: Record<EntityKind, readonly string[]> = {
  area: ["name"],
  project: ["name", "area_id"],
  task: ["title"],
  comment: ["task_id", "body"],
};

const KIND_LABEL: Record<EntityKind, string> = { area: "Bereich", project: "Projekt", task: "Aufgabe", comment: "Kommentar" };
const DEFINITE: Record<EntityKind, string> = { area: "Der", project: "Das", task: "Die", comment: "Der" };
const INDEFINITE_ACC: Record<EntityKind, string> = {
  area: "einen Bereich",
  project: "ein Projekt",
  task: "eine Aufgabe",
  comment: "einen Kommentar",
};
const DEPENDENT: Record<EntityKind, string> = {
  area: "noch ein Bereich",
  project: "noch ein Projekt",
  task: "noch eine Aufgabe",
  comment: "noch ein Kommentar",
};

export { isCanonicalId } from "./ids";

function isObject(v: JsonValue | undefined): v is { [key: string]: JsonValue } {
  return !!v && typeof v === "object" && !Array.isArray(v);
}

function onlyKeys(v: { [key: string]: JsonValue }, allowed: readonly string[]): boolean {
  return Object.keys(v).every((k) => allowed.includes(k));
}

/** Absolute URL with any scheme (core: url::Url::parse succeeds). */
export function isAbsoluteUrl(v: string): boolean {
  try {
    new URL(v);
    return true;
  } catch {
    return false;
  }
}

/** Absolute http(s) URL with a host. */
export function isHttpUrl(v: string): boolean {
  try {
    const u = new URL(v);
    return (u.protocol === "http:" || u.protocol === "https:") && u.hostname !== "";
  } catch {
    return false;
  }
}

/** `owner/name` in GitHub style (core: model::is_valid_repo). */
export function isValidRepo(s: string): boolean {
  const slash = s.indexOf("/");
  if (slash < 0) return false;
  const owner = s.slice(0, slash);
  const name = s.slice(slash + 1);
  const ownerOk = owner.length >= 1 && owner.length <= 39 && /^[A-Za-z0-9-]+$/.test(owner) && !owner.startsWith("-") && !owner.endsWith("-");
  const nameOk = name.length >= 1 && name.length <= 100 && /^[A-Za-z0-9._-]+$/.test(name) && name !== "." && name !== "..";
  return ownerOk && nameOk;
}

export const MAX_KEY_CHARS = 10;

/** Project key: uppercase letter, then uppercase letters or digits (core: numbering::is_valid_key). */
export function isValidKey(s: string): boolean {
  return s.length >= 1 && s.length <= MAX_KEY_CHARS && /^[A-Z][A-Z0-9]*$/.test(s);
}

/** Task reference `KEY-N` (core: numbering::parse_ref). */
export function isValidRef(s: string): boolean {
  const dash = s.indexOf("-");
  if (dash < 0) return false;
  return isValidKey(s.slice(0, dash)) && /^[1-9][0-9]{0,8}$/.test(s.slice(dash + 1));
}

/** Strict location shape (core: RawLocation with deny_unknown_fields). */
export function parseStrictLocation(v: JsonValue | undefined): Location | null {
  if (!isObject(v) || !onlyKeys(v, ["kind", "id"])) return null;
  const id = v.id ?? null;
  if ((v.kind === "inbox" || v.kind === "loose") && id === null) return { kind: v.kind };
  if ((v.kind === "area" || v.kind === "project") && isCanonicalId(id)) return { kind: v.kind, id };
  return null;
}

type Shape =
  | "title"
  | "text"
  | "body"
  | "bool"
  | "id"
  | "location"
  | "due"
  | "timestamp"
  | "string_list"
  | "id_list"
  | "links"
  | "phoenix"
  | "recurrence"
  | "repo"
  | "http_url"
  | "project_key"
  | "task_ref"
  | readonly string[];

const SHAPES: Record<EntityKind, Record<string, Shape>> = {
  area: { name: "title", state: AREA_STATES, vault_path: "text", deleted: "bool" },
  project: {
    name: "title",
    area_id: "id",
    status: PROJECT_STATUSES,
    kind: PROJECT_KINDS,
    repo: "repo",
    description: "text",
    view: PROJECT_VIEWS,
    key: "project_key",
    deleted: "bool",
  },
  task: {
    title: "title",
    description: "text",
    type: TASK_TYPES,
    status: TASK_STATUSES,
    location: "location",
    parent_id: "id",
    due: "due",
    remind_at: "timestamp",
    vault_links: "string_list",
    links: "links",
    phoenix: "phoenix",
    claimed_by: "text",
    claim_expires_at: "timestamp",
    plan: "text",
    branch: "text",
    pr_url: "http_url",
    timing_ref: "text",
    flame_started_at: "timestamp",
    flame_ended_at: "timestamp",
    clickup_id: "text",
    rank: "text",
    undated_ack: "timestamp",
    recurrence: "recurrence",
    recurrence_prev: "id",
    blocked_by: "id_list",
    ref: "task_ref",
    deleted: "bool",
  },
  comment: { task_id: "id", body: "body", author: "title", deleted: "bool" },
};

const FIELD_LABELS: Record<string, string> = {
  title: "Titel",
  name: "Name",
  due: "Fälligkeit (Datum JJJJ-MM-TT oder Zeitpunkt)",
  remind_at: "Erinnerung",
  undated_ack: "Bewusst ohne Fälligkeit (Zeitpunkt)",
  blocked_by: "Blockiert durch (verschiedene Aufgaben-IDs)",
  recurrence_prev: "Vorgängerin in der Serie (Aufgaben-ID)",
  links: "Links (absolute URL)",
  phoenix: "Phönix (Rolle und Feuer wie 2026-W45)",
  pr_url: "PR-Link (http(s)-URL)",
  repo: "Repository (owner/name)",
  key: "Schlüssel (Großbuchstaben und Ziffern, z. B. ZUN)",
  location: "Stelle",
  parent_id: "Elternaufgabe",
  area_id: "Bereich",
  task_id: "Aufgabe",
  body: "Text",
  author: "Autor",
};

const label = (field: string) => FIELD_LABELS[field] ?? `„${field}“`;

function shapeOk(shape: Shape, v: JsonValue): boolean {
  if (typeof shape !== "string") return typeof v === "string" && shape.includes(v);
  switch (shape) {
    case "title":
    case "text":
    case "body":
      return typeof v === "string";
    case "bool":
      return typeof v === "boolean";
    case "id":
      return isCanonicalId(v);
    case "location":
      return parseStrictLocation(v) !== null;
    case "due":
      return typeof v === "string" && isValidDue(v);
    case "timestamp":
      return typeof v === "string" && isRfc3339(v);
    case "string_list":
      return Array.isArray(v) && v.every((x) => typeof x === "string" && x !== "");
    case "id_list":
      return Array.isArray(v) && v.length <= MAX_BLOCKED_BY && v.every(isCanonicalId) && new Set(v).size === v.length;
    case "links":
      return (
        Array.isArray(v) &&
        v.every(
          (x) =>
            isObject(x) &&
            onlyKeys(x, ["url", "title"]) &&
            typeof x.url === "string" &&
            isAbsoluteUrl(x.url) &&
            (x.title === undefined || x.title === null || typeof x.title === "string"),
        )
      );
    case "phoenix":
      return (
        isObject(v) &&
        onlyKeys(v, ["role", "fire"]) &&
        typeof v.role === "string" &&
        (PHOENIX_ROLES as readonly string[]).includes(v.role) &&
        typeof v.fire === "string" &&
        isValidFire(v.fire)
      );
    case "recurrence":
      return recurrenceProblem(v) === null;
    case "repo":
      return typeof v === "string" && isValidRepo(v);
    case "http_url":
      return typeof v === "string" && isHttpUrl(v);
    case "project_key":
      return typeof v === "string" && isValidKey(v);
    case "task_ref":
      return typeof v === "string" && isValidRef(v);
  }
}

/** Static check of a single field value (core: model::validate_field_value). */
export function checkField(entity: EntityKind, field: string, value: JsonValue): { code: RejectCode; message: string } | null {
  const shape = FIELDS_BY_ENTITY[entity].includes(field) ? SHAPES[entity][field] : undefined;
  if (!shape) return { code: "unknown_field", message: `Unbekanntes Feld „${field}“ für ${KIND_LABEL[entity]}.` };
  if (value === null) {
    if (REQUIRED_FIELDS[entity].includes(field)) {
      return { code: "invalid_value", message: `Pflichtfeld ${label(field)} kann nicht zurückgesetzt werden.` };
    }
    return null;
  }
  if (shape === "title" || shape === "body") {
    if (typeof value !== "string") return { code: "invalid_value", message: `${label(field)} muss ein Text sein.` };
    // Blank = only Unicode White_Space, the set Rust's str::trim strips (JS trim differs: it
    // keeps U+0085 and strips U+FEFF).
    if (/^\p{White_Space}*$/u.test(value)) {
      const message =
        entity === "comment"
          ? field === "body"
            ? "Ein Kommentar braucht Text."
            : `${label(field)} darf nicht leer sein.`
          : entity === "task"
            ? "Eine Aufgabe braucht einen Titel."
            : `${DEFINITE[entity]} ${KIND_LABEL[entity]} braucht einen Namen.`;
      return { code: "invalid_value", message };
    }
    if (shape === "body") return null;
    if ([...value].length > TITLE_MAX) {
      return { code: "invalid_value", message: `${label(field)} darf höchstens ${TITLE_MAX} Zeichen lang sein.` };
    }
    return null;
  }
  if (shape === "recurrence") {
    const problem = recurrenceProblem(value);
    return problem === null ? null : { code: "invalid_value", message: problem };
  }
  if (!shapeOk(shape, value)) return { code: "invalid_value", message: `Ungültiger Wert für ${label(field)}.` };
  return null;
}

// ---- Entity checks over a scratch state --------------------------------------

/** Read access to a state: the base store, optionally with touched entities overlaid. */
class View {
  private childIndex: Map<string, string[]> | null = null;

  constructor(
    private readonly base: EntityStore,
    private readonly touched: ReadonlyMap<string, EntityState> = new Map(),
  ) {}

  entity(kind: EntityKind, id: string): EntityState | undefined {
    const key = entityKey(kind, id);
    return this.touched.get(key) ?? this.base.getByKey(key);
  }

  live(kind: EntityKind, id: string): EntityState | undefined {
    const e = this.entity(kind, id);
    return e && !isDeleted(e) ? e : undefined;
  }

  *entities(kind?: EntityKind): Generator<EntityState> {
    for (const e of this.touched.values()) if (!kind || e.entity === kind) yield e;
    for (const e of this.base.values()) {
      if ((!kind || e.entity === kind) && !this.touched.has(entityKey(e.entity, e.id))) yield e;
    }
  }

  /** Root task id, or null when the chain is cyclic or the task is unknown. */
  rootTask(id: string): string | null {
    let current = this.entity("task", id);
    if (!current) return null;
    const seen = new Set([id]);
    for (;;) {
      const pid = parentId(current);
      const parent = pid ? this.entity("task", pid) : undefined;
      if (!parent) return current.id;
      if (seen.has(parent.id)) return null;
      seen.add(parent.id);
      current = parent;
    }
  }

  effectiveLocation(id: string): Location | null {
    const root = this.rootTask(id);
    const e = root ? this.entity("task", root) : undefined;
    return e ? ownLocation(e) : null;
  }

  isDevLocation(loc: Location): boolean {
    if (loc.kind !== "project") return false;
    const p = this.live("project", loc.id);
    return !!p && strValue(p, "kind") === "dev";
  }

  /** Live direct children (winning parent_id). */
  children(id: string): string[] {
    if (!this.childIndex) {
      this.childIndex = new Map();
      for (const e of this.entities("task")) {
        const pid = parentId(e);
        if (!pid) continue;
        let list = this.childIndex.get(pid);
        if (!list) this.childIndex.set(pid, (list = []));
        list.push(e.id);
      }
    }
    return (this.childIndex.get(id) ?? []).filter((c) => this.live("task", c));
  }

  /** All live descendants, cycle safe (core: State::descendants). */
  descendants(id: string): string[] {
    const out: string[] = [];
    const seen = new Set([id]);
    const stack = [...this.children(id)].reverse();
    while (stack.length > 0) {
      const c = stack.pop()!;
      if (seen.has(c)) continue;
      seen.add(c);
      out.push(c);
      stack.push(...[...this.children(c)].reverse());
    }
    return out;
  }
}

function value(e: EntityState, field: string): JsonValue | undefined {
  const v = e.fields[field]?.value;
  return v === null ? undefined : v;
}

function strValue(e: EntityState, field: string): string | undefined {
  const v = value(e, field);
  return typeof v === "string" ? v : undefined;
}

function isDeleted(e: EntityState): boolean {
  return value(e, "deleted") === true;
}

function parentId(e: EntityState): string | null {
  if (e.entity !== "task") return null;
  const v = strValue(e, "parent_id");
  return isCanonicalId(v) ? v : null;
}

function areaId(e: EntityState): string | null {
  if (e.entity !== "project") return null;
  const v = strValue(e, "area_id");
  return isCanonicalId(v) ? v : null;
}

function ownLocation(e: EntityState): Location {
  return parseStrictLocation(value(e, "location")) ?? { kind: "inbox" };
}

function target(loc: Location): [EntityKind, string] | null {
  return loc.kind === "area" || loc.kind === "project" ? [loc.kind, loc.id] : null;
}

/** The task's `blocked_by` IDs (malformed entries skipped, core: Entity::blocked_by). */
function blockedBy(e: EntityState): string[] {
  const v = value(e, "blocked_by");
  return e.entity === "task" && Array.isArray(v) ? v.filter((x): x is string => isCanonicalId(x)) : [];
}

function taskType(e: EntityState): string {
  return strValue(e, "type") ?? "aufgabe";
}

function taskStatus(e: EntityState): string {
  return strValue(e, "status") ?? "open";
}

type Key = string; // "kind/id"
const keyOf = (kind: EntityKind, id: string): Key => entityKey(kind, id);

class Checker {
  readonly blamed = new Map<Key, { code: RejectCode; message: string }>();

  constructor(
    private readonly before: View,
    private readonly scratch: View,
    private readonly changed: ReadonlyMap<Key, ReadonlySet<string>>,
  ) {}

  private touched(key: Key): boolean {
    return this.changed.has(key);
  }

  /** Restoring an entity counts as changing every field (all rules apply anew). */
  private changedAny(kind: EntityKind, id: string, names: readonly string[]): boolean {
    const set = this.changed.get(keyOf(kind, id));
    if (!set) return false;
    return names.some((n) => set.has(n)) || (set.has("deleted") && !!this.scratch.live(kind, id));
  }

  private blame(key: Key, code: RejectCode, message: string): void {
    if (this.touched(key) && !this.blamed.has(key)) this.blamed.set(key, { code, message });
  }

  check(kind: EntityKind, id: string): void {
    const e = this.scratch.entity(kind, id);
    if (!e || isDeleted(e)) return;
    const key = keyOf(kind, id);
    if (this.touched(key)) {
      for (const field of REQUIRED_FIELDS[kind]) {
        if (value(e, field) === undefined) {
          const message =
            kind === "task" && field === "title"
              ? "Eine Aufgabe braucht einen Titel."
              : kind === "project" && field === "area_id"
                ? "Ein Projekt gehört immer zu einem Bereich."
                : `Pflichtfeld „${field}“ fehlt für ${INDEFINITE_ACC[kind]}.`;
          this.blame(key, "missing_required", message);
          return;
        }
      }
    }
    if (kind === "project") this.checkProject(e);
    else if (kind === "task") this.checkTask(e);
    else if (kind === "comment") this.checkComment(e);
  }

  private brokenReference(kind: EntityKind, id: string, via: readonly string[], targetKind: EntityKind, targetId: string): void {
    if (this.scratch.live(targetKind, targetId)) return;
    const key = keyOf(kind, id);
    const targetKey = keyOf(targetKind, targetId);
    if (this.changedAny(kind, id, via)) {
      const what = `${DEFINITE[targetKind]} ${KIND_LABEL[targetKind]}`;
      const message = this.scratch.entity(targetKind, targetId) ? `${what} ist gelöscht.` : `${what} existiert nicht.`;
      this.blame(key, "invalid_reference", message);
    } else if (this.touched(targetKey)) {
      this.blame(
        targetKey,
        "has_dependents",
        `${DEFINITE[targetKind]} ${KIND_LABEL[targetKind]} kann nicht gelöscht werden, solange ${DEPENDENT[kind]} darauf verweist. Bitte zuerst verschieben oder mitlöschen.`,
      );
    } else {
      this.blame(key, "invalid_reference", `${DEFINITE[targetKind]} referenzierte ${KIND_LABEL[targetKind]} existiert nicht mehr.`);
    }
  }

  private checkProject(p: EntityState): void {
    const area = areaId(p);
    if (area) this.brokenReference("project", p.id, ["area_id", "deleted"], "area", area);
    const own = strValue(p, "key");
    if (own !== undefined && this.changedAny("project", p.id, ["key"])) {
      for (const other of this.scratch.entities("project")) {
        if (other.id !== p.id && !isDeleted(other) && strValue(other, "key") === own) {
          this.blame(keyOf("project", p.id), "invalid_value", `Der Schlüssel „${own}“ gehört schon zum Projekt „${strValue(other, "name") ?? ""}“.`);
          break;
        }
      }
    }
    if (value(p, "repo") !== undefined && strValue(p, "kind") !== "dev" && this.changedAny("project", p.id, ["repo", "kind"])) {
      this.blame(
        keyOf("project", p.id),
        "dev_only",
        "Ein Repo ist nur bei Dev-Projekten erlaubt. Bitte „repo“ zurücksetzen oder die Art „dev“ wählen.",
      );
    }
  }

  /** A comment needs a live task when written to one; it never blocks deleting its task. */
  private checkComment(c: EntityState): void {
    const task = strValue(c, "task_id");
    const via = ["task_id", "deleted"];
    if (isCanonicalId(task) && this.changedAny("comment", c.id, via)) this.brokenReference("comment", c.id, via, "task", task);
  }

  private checkTask(t: EntityState): void {
    const key = keyOf("task", t.id);
    const refFields = ["parent_id", "location", "deleted"];
    const parent = parentId(t);
    if (parent) {
      this.brokenReference("task", t.id, refFields, "task", parent);
      if (this.inCycle(t.id) && this.changedAny("task", t.id, ["parent_id"])) {
        this.blame(key, "cycle", "Die Aufgabe kann nicht unter sich selbst oder eine ihrer Unteraufgaben gehängt werden.");
        return;
      }
    } else {
      const tgt = target(ownLocation(t));
      if (tgt) this.brokenReference("task", t.id, refFields, tgt[0], tgt[1]);
    }
    if (this.changedAny("task", t.id, ["blocked_by"])) this.checkBlockedBy(t);
    if (value(t, "recurrence") !== undefined && this.changedAny("task", t.id, ["recurrence", "due", "parent_id"])) {
      this.checkRecurring(t);
    }

    const type = taskType(t);
    const devType = type === "bug" || type === "feature";
    const devStatus = taskStatus(t) === "review";
    if (!devType && !devStatus) return;
    const effective = this.scratch.effectiveLocation(t.id);
    if (!effective || this.scratch.isDevLocation(effective)) return;
    const what = devType ? `Typ „${type}“` : "Status „review“";
    const isRoot = parent === null;
    const ownCause =
      this.changedAny("task", t.id, ["type", "status", "parent_id", "deleted"]) || (isRoot && this.changedAny("task", t.id, ["location"]));
    if (ownCause) {
      const beforeLoc = this.before.effectiveLocation(t.id);
      const wasDev = !!beforeLoc && this.before.isDevLocation(beforeLoc);
      this.blame(
        key,
        "dev_only",
        wasDev ? `Eine Aufgabe mit ${what} darf nicht aus einem Dev-Projekt heraus verschoben werden.` : `${what} ist nur in Dev-Projekten erlaubt.`,
      );
      return;
    }

    const title = strValue(t, "title") ?? "";
    let responsible = false;
    const seen = new Set([t.id]);
    let current = parent;
    while (current) {
      if (seen.has(current)) break;
      seen.add(current);
      const ancestor = this.scratch.entity("task", current);
      if (!ancestor) break;
      const ancestorIsRoot = parentId(ancestor) === null;
      if (this.changedAny("task", current, ["parent_id"]) || (ancestorIsRoot && this.changedAny("task", current, ["location"]))) {
        responsible = true;
        this.blame(
          keyOf("task", current),
          "dev_tasks_present",
          `Unteraufgabe „${title}“ hat ${what} und darf nicht aus einem Dev-Projekt heraus verschoben werden.`,
        );
      }
      current = parentId(ancestor);
    }
    if (effective.kind === "project") {
      const pid = effective.id;
      if (this.changedAny("project", pid, ["deleted"]) && !this.scratch.live("project", pid)) {
        responsible = true;
      } else if (this.changedAny("project", pid, ["kind"])) {
        responsible = true;
        this.blame(
          keyOf("project", pid),
          "dev_tasks_present",
          `Das Projekt enthält noch die Aufgabe „${title}“ mit ${what} und kann deshalb kein Standard-Projekt werden.`,
        );
      }
    }
    if (!responsible && this.touched(key)) this.blame(key, "dev_only", `${what} ist nur in Dev-Projekten erlaubt.`);
  }

  /**
   * New entries of `blocked_by` must be other live tasks that do not wait for this one
   * themselves (core: Checker::check_blocked_by). Entries that were already there stay,
   * even if their task was deleted meanwhile.
   */
  private checkBlockedBy(t: EntityState): void {
    const key = keyOf("task", t.id);
    const prev = this.before.entity("task", t.id);
    const before = new Set(prev ? blockedBy(prev) : []);
    for (const blocker of blockedBy(t)) {
      if (before.has(blocker)) continue;
      if (blocker === t.id) {
        this.blame(key, "invalid_value", "Eine Aufgabe kann nicht von sich selbst blockiert werden.");
        return;
      }
      const other = this.scratch.live("task", blocker);
      if (!other) {
        this.blame(
          key,
          "invalid_reference",
          this.scratch.entity("task", blocker) ? "Die blockierende Aufgabe ist gelöscht." : "Die blockierende Aufgabe existiert nicht.",
        );
        return;
      }
      if (this.waitsFor(blocker, t.id)) {
        this.blame(
          key,
          "cycle",
          `„${strValue(other, "title") ?? ""}“ wartet selbst (direkt oder über andere Aufgaben) auf diese Aufgabe. Zwei Aufgaben können sich nicht gegenseitig blockieren.`,
        );
        return;
      }
    }
  }

  /**
   * A recurring task needs a due date and must be a root task (core: Checker::check_recurring).
   * Sending `recurrence: null` along lifts both rules. A missing due date is explained by what
   * the push did: setting the recurrence asks for a due date, removing it (or restoring) for
   * removing the recurrence.
   */
  private checkRecurring(t: EntityState): void {
    const key = keyOf("task", t.id);
    if (value(t, "due") === undefined) {
      const setsRecurrence = this.changed.get(key)?.has("recurrence") ?? false;
      this.blame(
        key,
        "invalid_value",
        setsRecurrence
          ? "Ein Rhythmus braucht eine Fälligkeit. Bitte zusammen mit dem Rhythmus eine Fälligkeit setzen."
          : "Eine wiederkehrende Aufgabe braucht eine Fälligkeit. Bitte zuerst den Rhythmus entfernen.",
      );
    } else if (value(t, "parent_id") !== undefined) {
      this.blame(key, "invalid_value", "Nur Aufgaben ohne übergeordnete Aufgabe können wiederkehren.");
    }
  }

  /** Whether live task `from` waits for `target` along `blocked_by`, through live tasks only. */
  private waitsFor(from: string, target: string): boolean {
    const seen = new Set([from]);
    const stack = [from];
    while (stack.length > 0) {
      const e = this.scratch.live("task", stack.pop()!);
      if (!e) continue;
      for (const next of blockedBy(e)) {
        if (next === target) return true;
        if (!seen.has(next)) {
          seen.add(next);
          stack.push(next);
        }
      }
    }
    return false;
  }

  private inCycle(id: string): boolean {
    const seen = new Set<string>();
    let e = this.scratch.entity("task", id);
    let current = e ? parentId(e) : null;
    while (current) {
      if (current === id) return true;
      if (seen.has(current)) return false;
      seen.add(current);
      e = this.scratch.entity("task", current);
      current = e ? parentId(e) : null;
    }
    return false;
  }
}

/** Touched entities plus every live entity whose validity may depend on them. */
function neighbourhood(scratch: View, changed: ReadonlyMap<Key, ReadonlySet<string>>): [EntityKind, string][] {
  const out = new Map<Key, [EntityKind, string]>();
  const touchedAreas = new Set<string>();
  const touchedProjects = new Set<string>();
  const touchedTasks: string[] = [];
  for (const key of changed.keys()) {
    const [kind, id] = key.split("/") as [EntityKind, string];
    out.set(key, [kind, id]);
    if (kind === "area") touchedAreas.add(id);
    else if (kind === "project") touchedProjects.add(id);
    else if (kind === "task") touchedTasks.push(id);
  }
  if (touchedAreas.size > 0) {
    for (const p of scratch.entities("project")) {
      const a = areaId(p);
      if (!isDeleted(p) && a && touchedAreas.has(a)) out.set(keyOf("project", p.id), ["project", p.id]);
    }
  }
  if (touchedAreas.size > 0 || touchedProjects.size > 0) {
    for (const t of scratch.entities("task")) {
      if (isDeleted(t)) continue;
      const own = ownLocation(t);
      const inArea = parentId(t) === null && own.kind === "area" && touchedAreas.has(own.id);
      let inProject = false;
      if (!inArea && touchedProjects.size > 0) {
        const eff = scratch.effectiveLocation(t.id);
        inProject = eff?.kind === "project" && touchedProjects.has(eff.id);
      }
      if (inArea || inProject) out.set(keyOf("task", t.id), ["task", t.id]);
    }
  }
  for (const id of touchedTasks) for (const d of scratch.descendants(id)) out.set(keyOf("task", d), ["task", d]);
  return [...out.values()];
}

/** Task fields only the server writes (core: Author::Server), with the reason shown to clients. */
const SERVER_ONLY = new Map<string, string>([
  ["ref", "Die Kennung („ref“) vergibt der Server, sobald die Aufgabe in einem Projekt mit Schlüssel liegt."],
  ["recurrence_prev", "Die Vorgängerin in einer Serie („recurrence_prev“) setzt nur der Server, wenn er die nächste Instanz anlegt."],
]);

/**
 * Validates one commit (ops for one or more entities) against the current state
 * like zunder_core::validate_push: per-op checks first, then entity rules on the
 * state with the commit applied. Returns at most one issue per entity (the
 * cause); the caller refuses the whole commit when there are any.
 */
export function validateOps(base: EntityStore, ops: readonly Op[], nowMs: number = Date.now()): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  const kindsInPush = new Map<string, Set<EntityKind>>();
  for (const op of ops) {
    let set = kindsInPush.get(op.entity_id);
    if (!set) kindsInPush.set(op.entity_id, (set = new Set()));
    set.add(op.entity);
  }
  const failed = new Set<Key>();
  const otherKinds = (kind: EntityKind, id: string) => (["area", "project", "task", "comment"] as const).filter((k) => k !== kind && base.has(k, id));
  for (const op of ops) {
    const key = keyOf(op.entity, op.entity_id);
    if (failed.has(key)) continue;
    let issue: { code: RejectCode; message: string } | null = null;
    if (hlcWallMs(op.hlc) > nowMs + MAX_FUTURE_MS) {
      issue = { code: "future_hlc", message: "Der Zeitstempel liegt mehr als 24 Stunden in der Zukunft. Bitte die Uhr des Geräts prüfen." };
    }
    issue ??= checkField(op.entity, op.field, op.value);
    const serverOnly = op.entity === "task" ? SERVER_ONLY.get(op.field) : undefined;
    if (!issue && serverOnly) issue = { code: "invalid_value", message: serverOnly };
    if (!issue) {
      const other = otherKinds(op.entity, op.entity_id)[0];
      if (other) {
        issue = {
          code: "kind_mismatch",
          message: `Die ID ist bereits als ${KIND_LABEL[other]} vergeben und kann nicht für ${INDEFINITE_ACC[op.entity]} verwendet werden.`,
        };
      } else if ((kindsInPush.get(op.entity_id)?.size ?? 0) > 1) {
        issue = { code: "kind_mismatch", message: "Dieselbe ID wird in diesem Push für verschiedene Entitätsarten verwendet." };
      }
    }
    if (issue) {
      failed.add(key);
      issues.push({ op_id: op.op_id, entity_id: op.entity_id, ...issue });
    }
  }
  if (issues.length > 0) return issues;

  // Apply the commit to copies of the touched entities.
  const touched = new Map<Key, EntityState>();
  const changed = new Map<Key, Set<string>>();
  for (const op of ops) {
    const key = keyOf(op.entity, op.entity_id);
    let e = touched.get(key);
    if (!e) {
      const orig = base.getByKey(key);
      e = { entity: op.entity, id: op.entity_id, fields: orig ? { ...orig.fields } : {}, created_hlc: orig?.created_hlc ?? op.hlc };
      touched.set(key, e);
    }
    const candidate = { value: op.value, hlc: op.hlc, op_id: op.op_id };
    const current = e.fields[op.field];
    if (!current || wins(candidate, current)) e.fields[op.field] = candidate;
    if (op.hlc < e.created_hlc) e.created_hlc = op.hlc;
    let set = changed.get(key);
    if (!set) changed.set(key, (set = new Set()));
    set.add(op.field);
  }
  const scratch = new View(base, touched);
  const checker = new Checker(new View(base), scratch, changed);
  for (const [kind, id] of neighbourhood(scratch, changed)) checker.check(kind, id);
  for (const [key, v] of checker.blamed) issues.push({ entity_id: key.slice(key.indexOf("/") + 1), ...v });
  return issues;
}

/**
 * Entity plus all live dependents, dependents last (core: OpBatch::delete_cascade):
 * a task with its descendants; a project with the tasks located in it; an area
 * with its projects (and their tasks) and the tasks located directly in it.
 * Comments stay: they are hidden with their task and return when it is restored.
 */
export function deleteCascade(base: EntityStore, kind: EntityKind, id: string): [EntityKind, string][] {
  const view = new View(base);
  const out: [EntityKind, string][] = [[kind, id]];
  if (kind === "task") {
    for (const d of view.descendants(id)) out.push(["task", d]);
  } else if (kind === "project") {
    for (const t of view.entities("task")) {
      if (isDeleted(t)) continue;
      const eff = view.effectiveLocation(t.id);
      if (eff?.kind === "project" && eff.id === id) out.push(["task", t.id]);
    }
  } else if (kind === "area") {
    for (const p of view.entities("project")) {
      if (!isDeleted(p) && areaId(p) === id) out.push(...deleteCascade(base, "project", p.id));
    }
    for (const t of view.entities("task")) {
      if (isDeleted(t)) continue;
      const own = ownLocation(t);
      if (parentId(t) === null && own.kind === "area" && own.id === id) {
        out.push(["task", t.id], ...view.descendants(t.id).map((d): [EntityKind, string] => ["task", d]));
      }
    }
  }
  const seen = new Set<string>();
  return out.filter(([k, i]) => {
    const key = keyOf(k, i);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
