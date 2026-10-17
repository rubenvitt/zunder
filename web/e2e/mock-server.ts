// Small in-memory mock of the Zunder HTTP API (contract section 3) for e2e tests
// and UI development without the Rust server. Serves the built web/dist.
//
//   node e2e/mock-server.ts             (port from MOCK_PORT, default 4173; Node >= 22.18 strips types)
//
// Test-only control endpoints (never part of the real API):
//   POST /__test/reset   {"seed": "demo" | "empty", "auth": true | false}
//   POST /__test/ops     {"ops": [Op], "source": "mcp:test"}   simulate another client
//   GET  /__test/state   materialized state for assertions
//   POST /__test/oauth-client {"name": "Claude"}   a connected OAuth app (token page)
//   POST /__test/device  {"name": "iPhone"}        a logged-in app device (token page)
//
// Mirrors the real server where the UI depends on it (contract 3 and 3.1): error
// format {"error":{"code","message"}}, CSRF header X-Zunder-Client: web for
// state-changing requests, snapshot with created_hlc/op_id, history entries with
// source/received_at, flame responses (409 without a running flame), token rules,
// "Erst für dich" settings (/api/settings/focus, reset with the seed),
// intake channels (management under /api/intake/channels, POST /intake/v1/tasks with
// a zi_ token: idempotent per external_id, 409 for a dead or completed target, 429
// after 30 tasks per hour), follow-up instances of recurring tasks after each push
// (3.1, calendar in mock-recurrence.ts, time zone of the mock process), the status rule
// for `waiting_on` (not for `blocked_by`), and a subset of the core validation with the
// core's rejection codes.

import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { dirname, extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { randomBytes } from "node:crypto";
import { createId } from "@paralleldrive/cuid2";
import { nextOccurrence, type MockRecurrence } from "./mock-recurrence.ts";

const here = dirname(fileURLToPath(import.meta.url));
const DIST = resolve(process.env.MOCK_DIST ?? join(here, "..", "dist"));
const PORT = Number(process.env.MOCK_PORT ?? 4173);

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type EntityKind = "area" | "project" | "task" | "comment";

interface Op {
  op_id: string;
  entity: EntityKind;
  entity_id: string;
  field: string;
  value: Json;
  hlc: string;
}

interface FieldState {
  value: Json;
  hlc: string;
  op_id: string;
}

interface Entity {
  entity: EntityKind;
  id: string;
  fields: Record<string, FieldState>;
  created_hlc: string;
}

interface Token {
  id: string;
  name: string;
  created_at: string;
  last_used_at: string | null;
}

interface OAuthClient {
  id: string;
  name: string;
  redirect_uris: string[];
  created_at: string;
  last_used_at: string | null;
}

interface Device {
  id: string;
  name: string;
  created_at: string;
  last_used_at: string | null;
}

interface IntakeChannel {
  id: string;
  name: string;
  project_id: string;
  token: string;
  created_at: string;
  last_used_at: string | null;
  /** external_id → created task and when. */
  submissions: Map<string, { task_id: string; at: number }>;
}

interface Rejection {
  op_id: string;
  code: string;
  message: string;
}

const FIELDS: Record<string, string[]> = {
  area: ["name", "state", "vault_path", "deleted"],
  project: ["name", "area_id", "status", "kind", "repo", "description", "view", "key", "deleted"],
  task: [
    "title", "description", "type", "status", "location", "parent_id", "due", "remind_at", "vault_links", "links",
    "phoenix", "claimed_by", "claim_expires_at", "plan", "branch", "pr_url", "timing_ref", "flame_started_at",
    "flame_ended_at", "clickup_id", "rank", "undated_ack", "recurrence", "recurrence_prev", "blocked_by", "weight",
    "for_whom", "deferrals", "waiting_on", "ref", "deleted",
  ],
  comment: ["task_id", "body", "author", "deleted"],
};
const ENUMS: Record<string, string[]> = {
  "task.status": ["open", "in_progress", "review", "blocked", "done", "dropped"],
  "task.type": ["aufgabe", "bug", "feature"],
  "task.weight": ["brand", "glut", "rauch", "asche"],
  "task.for_whom": ["mich", "uns", "andere"],
  "project.status": ["active", "paused", "completed"],
  "project.kind": ["standard", "dev"],
  "project.view": ["list", "board"],
  "area.state": ["active", "archived"],
};

// ---- State -------------------------------------------------------------------

let log: { seq: number; op: Op; source: string; received_at: string }[] = [];
let state = new Map<string, Entity>();
let opIds = new Set<string>();
let tokens: Token[] = [];
let oauthClients: OAuthClient[] = [];
/** Timing settings from the settings view (the mock has no environment variables). */
let timing: { token: string | null; project: string | null } = { token: null, project: null };
/** Projects the mock's Timing account has (for the project picker). */
const TIMING_PROJECTS = [
  { self: "/projects/1", title: "Arbeit", title_chain: ["Arbeit"], color: "#E5532D" },
  { self: "/projects/2", title: "Zunder", title_chain: ["Arbeit", "Zunder"], color: "#2A5DB0" },
  { self: "/projects/3", title: "Privat", title_chain: ["Privat"], color: null },
];
let devices: Device[] = [];
let intakeChannels: IntakeChannel[] = [];
/** Calendar feed: creation time of the token, null without a feed. */
let calendarCreatedAt: string | null = null;
/** "Erst für dich" (GET/PUT /api/settings/focus): days as the client sends them. */
let focus: { dismissed_on: string | null; chosen: { on: string; task_id: string } | null } = { dismissed_on: null, chosen: null };
let authenticated = true;
const sseClients = new Set<ServerResponse>();

let clockWall = 0;
let clockCounter = 0;
function serverHlc() {
  const now = Date.now();
  if (now > clockWall) {
    clockWall = now;
    clockCounter = 0;
  } else clockCounter += 1;
  return `${String(clockWall).padStart(13, "0")}:${String(clockCounter).padStart(5, "0")}:server`;
}
function observe(hlc: string) {
  const [w = 0, c = 0] = hlc.split(":").map(Number);
  if (w > clockWall || (w === clockWall && c > clockCounter)) {
    clockWall = w;
    clockCounter = c;
  }
}

function key(entity: string, id: string): string {
  return `${entity}/${id}`;
}

function applyToState(op: Op) {
  const k = key(op.entity, op.entity_id);
  let e = state.get(k);
  if (!e) state.set(k, (e = { entity: op.entity, id: op.entity_id, fields: {}, created_hlc: op.hlc }));
  if (op.hlc < e.created_hlc) e.created_hlc = op.hlc;
  const cur = e.fields[op.field];
  if (!cur || op.hlc > cur.hlc || (op.hlc === cur.hlc && op.op_id > cur.op_id)) {
    e.fields[op.field] = { value: op.value, hlc: op.hlc, op_id: op.op_id };
  }
}

function append(op: Op, source: string, receivedAt = new Date().toISOString()) {
  if (opIds.has(op.op_id)) return;
  opIds.add(op.op_id);
  log.push({ seq: log.length + 1, op, source, received_at: receivedAt });
  applyToState(op);
  observe(op.hlc);
}

function notify() {
  const cursor = log.length;
  for (const res of sseClients) res.write(`event: cursor\ndata: ${cursor}\n\n`);
}

function alive(entity: string, id: string): boolean {
  const e = state.get(key(entity, id));
  return !!e && e.fields.deleted?.value !== true;
}

// ---- Validation (subset of contract 2.5, core codes) ------------------------------

const ID_RE = /^[a-z][a-z0-9]{23}$/;

/** A real calendar day "YYYY-MM-DD". */
function isCalendarDay(value: string): boolean {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!m) return false;
  const d = new Date(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3])));
  return d.getUTCFullYear() === Number(m[1]) && d.getUTCMonth() === Number(m[2]) - 1 && d.getUTCDate() === Number(m[3]);
}
const HLC_RE = /^\d{13}:\d{5}:[a-z0-9-]{1,36}$/;
const REQUIRED: Record<string, string[]> = { area: ["name"], project: ["name", "area_id"], task: ["title"], comment: ["task_id", "body"] };

/** Wire-format check; a malformed op fails the whole push with 400 (like the server). */
function wireError(op: unknown): string | null {
  if (!op || typeof op !== "object") return "Operation ist kein Objekt.";
  const o = op as Record<string, unknown>;
  if (!ID_RE.test(String(o.op_id)) || !ID_RE.test(String(o.entity_id))) return "Keine kanonische ID.";
  if (!["area", "project", "task", "comment"].includes(String(o.entity))) return "Unbekannte Entitätsart.";
  if (typeof o.field !== "string") return "Feld fehlt.";
  if (!("value" in o)) return "value fehlt.";
  if (typeof o.hlc !== "string" || !HLC_RE.test(o.hlc)) return "Ungültiger HLC.";
  return null;
}

function checkOp(op: Op): [string, string] | null {
  const known = FIELDS[op.entity]!;
  if (!known.includes(op.field)) return ["unknown_field", `Unbekanntes Feld „${op.field}“.`];
  if (Number(op.hlc.slice(0, 13)) > Date.now() + 24 * 3600 * 1000) {
    return ["future_hlc", "Der Zeitstempel liegt mehr als 24 Stunden in der Zukunft. Bitte die Uhr des Geräts prüfen."];
  }
  if (op.entity === "task" && op.field === "ref") {
    return ["invalid_value", "Die Kennung („ref“) vergibt der Server, sobald die Aufgabe in einem Projekt mit Schlüssel liegt."];
  }
  if (op.entity === "task" && op.field === "recurrence_prev") {
    return ["invalid_value", "Die Vorgängerin in einer Serie („recurrence_prev“) setzt nur der Server, wenn er die nächste Instanz anlegt."];
  }
  if (op.value === null) {
    return REQUIRED[op.entity]!.includes(op.field) ? ["invalid_value", `Pflichtfeld „${op.field}“ kann nicht zurückgesetzt werden.`] : null;
  }
  const allowed = ENUMS[`${op.entity}.${op.field}`];
  if (allowed && !allowed.includes(String(op.value))) return ["invalid_value", `Ungültiger Wert für „${op.field}“.`];
  if (["title", "name", "body", "author", "waiting_on"].includes(op.field) && (typeof op.value !== "string" || !op.value.trim())) {
    return ["invalid_value", `„${op.field}“ darf nicht leer sein.`];
  }
  if (op.field === "waiting_on" && [...String(op.value)].length > 500) {
    return ["invalid_value", "„waiting_on“ darf höchstens 500 Zeichen lang sein."];
  }
  if (op.entity === "project" && op.field === "key" && !/^[A-Z][A-Z0-9]{0,9}$/.test(String(op.value))) {
    return ["invalid_value", "Ungültiger Schlüssel."];
  }
  if (op.field === "deleted" && typeof op.value !== "boolean") return ["invalid_value", "„deleted“ muss true oder false sein."];
  if (op.field === "deferrals" && !(Number.isInteger(op.value) && (op.value as number) >= 0 && (op.value as number) <= 9999)) {
    return ["invalid_value", "„deferrals“ muss eine ganze Zahl von 0 bis 9999 sein."];
  }
  if (op.field === "blocked_by") {
    const ids = Array.isArray(op.value) ? op.value.map(String) : null;
    if (!ids || !ids.every((v) => ID_RE.test(v)) || new Set(ids).size !== ids.length || ids.length > 100) {
      return ["invalid_value", "„blocked_by“ muss eine Liste verschiedener Aufgaben-IDs sein (höchstens 100)."];
    }
  }
  if (["parent_id", "area_id", "task_id"].includes(op.field) && !ID_RE.test(String(op.value))) {
    return ["invalid_value", `„${op.field}“ muss eine ID sein.`];
  }
  return null;
}

function fieldsAfter(entity: EntityKind, id: string, pending: Map<string, Op[]>): Record<string, Json> {
  const fields: Record<string, Json> = {};
  for (const [f, st] of Object.entries(state.get(key(entity, id))?.fields ?? {})) fields[f] = st.value;
  for (const op of pending.get(key(entity, id)) ?? []) fields[op.field] = op.value;
  return fields;
}

function aliveAfter(entity: EntityKind, id: string, pending: Map<string, Op[]>): boolean {
  if (!state.has(key(entity, id)) && !pending.has(key(entity, id))) return false;
  return fieldsAfter(entity, id, pending).deleted !== true;
}

function effectiveProject(id: string, pending: Map<string, Op[]>): string | null {
  const seen = new Set<string>();
  let current = id;
  for (;;) {
    const f = fieldsAfter("task", current, pending);
    const parent = typeof f.parent_id === "string" ? f.parent_id : null;
    if (parent && !seen.has(parent) && (state.has(key("task", parent)) || pending.has(key("task", parent)))) {
      seen.add(current);
      current = parent;
      continue;
    }
    const loc = f.location as { kind?: string; id?: string } | null | undefined;
    return loc?.kind === "project" ? String(loc.id) : null;
  }
}

function checkEntity(entity: EntityKind, id: string, pending: Map<string, Op[]>): [string, string] | null {
  const fields = fieldsAfter(entity, id, pending);
  if (fields.deleted === true) {
    // Deleting needs the live dependents gone (deleted in the same push or before).
    const dependents = [...new Set([...state.keys(), ...pending.keys()])].filter((k) => {
      const [kind, other] = k.split("/") as [EntityKind, string];
      if (!aliveAfter(kind, other, pending)) return false;
      const f = fieldsAfter(kind, other, pending);
      const loc = f.location as { kind?: string; id?: string } | null | undefined;
      if (entity === "area") return (kind === "project" && f.area_id === id) || (kind === "task" && !f.parent_id && loc?.kind === "area" && loc.id === id);
      if (entity === "project") return kind === "task" && !f.parent_id && loc?.kind === "project" && loc.id === id;
      return kind === "task" && f.parent_id === id;
    });
    return dependents.length > 0
      ? ["has_dependents", "Kann nicht gelöscht werden, solange noch etwas darauf verweist. Bitte zuerst verschieben oder mitlöschen."]
      : null;
  }
  for (const f of REQUIRED[entity]!) if (fields[f] === undefined || fields[f] === null) return ["missing_required", `Pflichtfeld „${f}“ fehlt.`];
  if (entity === "comment") {
    // Comments never block deleting their task; writing one needs a live task.
    const writes = (pending.get(key(entity, id)) ?? []).some((op) => op.field === "task_id" || op.field === "deleted");
    if (writes && !aliveAfter("task", String(fields.task_id), pending)) return ["invalid_reference", "Die Aufgabe existiert nicht."];
  } else if (entity === "task") {
    const loc = fields.location as { kind?: string; id?: string } | null | undefined;
    const parent = typeof fields.parent_id === "string" ? fields.parent_id : null;
    if (parent && !aliveAfter("task", parent, pending)) return ["invalid_reference", "Die Aufgabe existiert nicht."];
    if (!parent && (loc?.kind === "area" || loc?.kind === "project") && !aliveAfter(loc.kind, String(loc.id), pending)) {
      return ["invalid_reference", loc.kind === "area" ? "Der Bereich existiert nicht." : "Das Projekt existiert nicht."];
    }
    if (fields.type === "bug" || fields.type === "feature" || fields.status === "review") {
      const project = effectiveProject(id, pending);
      const dev = project && aliveAfter("project", project, pending) && fieldsAfter("project", project, pending).kind === "dev";
      if (!dev) return ["dev_only", "Bugs, Features und Review gibt es nur in Dev-Projekten."];
    }
  } else if (entity === "project") {
    if (!aliveAfter("area", String(fields.area_id), pending)) return ["invalid_reference", "Der Bereich existiert nicht."];
    if (fields.repo && fields.kind !== "dev") return ["dev_only", "Ein Repo ist nur bei Dev-Projekten erlaubt."];
    if (typeof fields.key === "string") {
      const holder = [...new Set([...state.keys(), ...pending.keys()])].find((k) => {
        const [kind, other] = k.split("/") as [EntityKind, string];
        return kind === "project" && other !== id && aliveAfter(kind, other, pending) && fieldsAfter(kind, other, pending).key === fields.key;
      });
      if (holder) return ["invalid_value", `Der Schlüssel „${fields.key}“ gehört schon zum Projekt „${fieldsAfter("project", holder.split("/")[1]!, pending).name}“.`];
    }
  }
  return null;
}

/** GET /api/agent-log: MCP commits (same source and receive time), newest first, never split. */
function agentLog(before: number, limit: number) {
  const rows = log.filter((e) => e.source.startsWith("mcp:") && e.seq < before).reverse();
  let taken = rows.slice(0, limit);
  const last = taken[taken.length - 1];
  if (taken.length === limit && last) {
    taken = taken.concat(rows.slice(limit).filter((e) => e.source === last.source && e.received_at === last.received_at));
  }
  const actions: { seq: number; source: string; received_at: string; changes: { entity: EntityKind; entity_id: string; created: boolean; ops: Op[] }[]; first: number }[] = [];
  for (const row of taken) {
    let action = actions[actions.length - 1];
    if (!action || action.source !== row.source || action.received_at !== row.received_at) {
      action = { seq: row.seq, source: row.source, received_at: row.received_at, changes: [], first: row.seq };
      actions.push(action);
    }
    action.first = row.seq;
    let change = action.changes.find((c) => c.entity === row.op.entity && c.entity_id === row.op.entity_id);
    if (!change) {
      change = { entity: row.op.entity, entity_id: row.op.entity_id, created: false, ops: [] };
      action.changes.push(change);
    }
    change.ops.unshift(row.op);
  }
  for (const action of actions) {
    action.changes.reverse();
    for (const change of action.changes) {
      change.created = !log.some((e) => e.seq < action.first && e.op.entity === change.entity && e.op.entity_id === change.entity_id);
    }
  }
  return {
    actions: actions.map(({ first: _first, ...action }) => action),
    next_before: taken.length >= limit && actions.length > 0 ? actions[actions.length - 1]!.first : null,
  };
}

function push(ops: Op[], source: string) {
  // One push is one commit: all its operations share the receive time (agent log).
  const receivedAt = new Date().toISOString();
  const accepted: string[] = [];
  const rejected: Rejection[] = [];
  const warnings: { entity_id: string; code: string; message: string }[] = [];
  const groups = new Map<string, Op[]>();
  const seen = new Set<string>();
  for (const op of ops) {
    if (opIds.has(op.op_id) || seen.has(op.op_id)) continue;
    seen.add(op.op_id);
    const k = key(op.entity, op.entity_id);
    let group = groups.get(k);
    if (!group) groups.set(k, (group = []));
    group.push(op);
  }
  const verdict = new Map<string, [string, string]>();
  for (const [k, group] of groups) {
    for (const op of group) {
      const reason = checkOp(op);
      if (reason) {
        verdict.set(op.op_id, reason);
        for (const other of group) {
          if (!verdict.has(other.op_id)) verdict.set(other.op_id, ["entity_rejected", `Abgelehnt, weil eine andere Änderung ungültig ist: ${reason[1]}`]);
        }
        groups.delete(k);
        break;
      }
    }
  }
  // Entity rules on the state with all remaining groups applied, until stable.
  for (let changed = true; changed; ) {
    changed = false;
    for (const [k, group] of groups) {
      const first = group[0]!;
      const reason = checkEntity(first.entity, first.entity_id, groups);
      if (reason) {
        for (const op of group) verdict.set(op.op_id, reason);
        groups.delete(k);
        changed = true;
      }
    }
  }
  const completions = completionsBefore([...groups.values()].flat());
  const waiting = waitingBefore([...groups.values()].flat());
  for (const op of ops) {
    const reason = verdict.get(op.op_id);
    if (reason) {
      rejected.push({ op_id: op.op_id, code: reason[0], message: reason[1] });
      continue;
    }
    if (groups.get(key(op.entity, op.entity_id))?.includes(op)) append(op, source, receivedAt);
    accepted.push(op.op_id);
  }
  for (const group of groups.values()) {
    const first = group[0]!;
    if (first.entity === "task" && group.some((op) => op.field === "status" && op.value === "done")) {
      const open = [...state.values()].filter(
        (e) => e.entity === "task" && e.fields.parent_id?.value === first.entity_id && e.fields.deleted?.value !== true &&
          !["done", "dropped"].includes(String(e.fields.status?.value ?? "open")),
      );
      if (open.length) {
        warnings.push({ entity_id: first.entity_id, code: "open_subtasks", message: `Aufgabe ist erledigt, hat aber noch ${open.length} offene Unteraufgabe(n).` });
      }
    }
  }
  if (groups.size > 0) {
    assignRefs();
    followWaiting(waiting);
    if (followUps(completions) > 0) assignRefs();
    notify();
  }
  return { accepted, rejected, warnings, cursor: log.length };
}

const CLOSED = ["done", "dropped"];

/**
 * Tasks a push may complete (zunder_core::recurrence::CompletionsBefore): open before
 * the push, or reopened by a newer status op in the push (created and checked off in one push).
 */
function completionsBefore(ops: Op[]): Set<string> {
  const out = new Set<string>();
  for (const op of ops) {
    if (op.entity !== "task" || op.field !== "status" || out.has(op.entity_id)) continue;
    const e = state.get(key("task", op.entity_id));
    const status = e?.fields.status;
    const openBefore = !!e && e.fields.deleted?.value !== true && !CLOSED.includes(String(status?.value ?? "open"));
    const reopens = !CLOSED.includes(String(op.value)) && (!status || op.hlc > status.hlc || (op.hlc === status.hlc && op.op_id > status.op_id));
    if (openBefore || reopens) out.add(op.entity_id);
  }
  return out;
}

/**
 * Follow-up instances of the series whose current instance the push completed
 * (contract 3.1, "Folgeinstanzen"), written with source `server`. Returns how many.
 */
function followUps(candidates: Set<string>): number {
  let created = 0;
  const successors = new Set(
    [...state.values()]
      .filter((e) => e.entity === "task" && e.fields.deleted?.value !== true && typeof e.fields.recurrence_prev?.value === "string")
      .map((e) => String(e.fields.recurrence_prev!.value)),
  );
  for (const id of [...candidates].sort()) {
    const e = state.get(key("task", id));
    if (!e || e.fields.deleted?.value === true || successors.has(id)) continue;
    const f = Object.fromEntries(Object.entries(e.fields).map(([k, v]) => [k, v.value])) as Record<string, Json>;
    if (!CLOSED.includes(String(f.status)) || f.parent_id || !f.recurrence || typeof f.due !== "string") continue;
    const completedMs = Number(e.fields.status!.hlc.slice(0, 13));
    const next = nextOccurrence(f.recurrence as unknown as MockRecurrence, f.due, typeof f.remind_at === "string" ? f.remind_at : null, completedMs, Date.now());
    if (!next) continue;
    const fields: Record<string, Json> = {};
    for (const k of ["title", "description", "type", "location"]) if (f[k] !== undefined && f[k] !== null) fields[k] = f[k]!;
    for (const k of ["links", "vault_links"]) if (Array.isArray(f[k]) && (f[k] as Json[]).length > 0) fields[k] = f[k]!;
    fields.due = next.due;
    if (next.remind_at) fields.remind_at = next.remind_at;
    fields.status = "open";
    fields.recurrence = next.recurrence as unknown as Json;
    fields.recurrence_prev = id;
    const instance = createId();
    for (const [field, value] of Object.entries(fields)) {
      append({ op_id: createId(), entity: "task", entity_id: instance, field, value, hlc: serverHlc() }, "server");
    }
    created += 1;
  }
  return created;
}

/**
 * Tasks whose `waiting_on` the push changes, with status and waiting before it. Only this part
 * of "Status folgt Abhängigkeiten" (zunder_core::blocking) is mirrored: the mock does not follow
 * `blocked_by` (contract 3.1).
 */
function waitingBefore(ops: Op[]): Map<string, { status: string; waiting: boolean }> {
  const out = new Map<string, { status: string; waiting: boolean }>();
  for (const op of ops) {
    if (op.entity !== "task" || op.field !== "waiting_on" || out.has(op.entity_id)) continue;
    const e = state.get(key("task", op.entity_id));
    const live = !!e && e.fields.deleted?.value !== true;
    out.set(op.entity_id, { status: live ? String(e.fields.status?.value ?? "open") : "", waiting: live && typeof e.fields.waiting_on?.value === "string" });
  }
  return out;
}

/**
 * An open task that starts waiting on someone becomes blocked; a blocked one whose name is
 * cleared opens again unless an open blocker holds it. A push that set the status keeps it.
 */
function followWaiting(before: Map<string, { status: string; waiting: boolean }>) {
  for (const [id, was] of [...before].sort(([a], [b]) => (a < b ? -1 : 1))) {
    const e = state.get(key("task", id));
    if (!e || e.fields.deleted?.value === true) continue;
    const status = String(e.fields.status?.value ?? "open");
    if (was.status && was.status !== status) continue;
    const waiting = typeof e.fields.waiting_on?.value === "string";
    const blockers = Array.isArray(e.fields.blocked_by?.value) ? (e.fields.blocked_by.value as Json[]).map(String) : [];
    const openBlocker = blockers.some((b) => alive("task", b) && !CLOSED.includes(String(state.get(key("task", b))!.fields.status?.value ?? "open")));
    let next: string | null = null;
    if (status === "open" && waiting && !was.waiting) next = "blocked";
    if (status === "blocked" && was.waiting && !waiting && !openBlocker) next = "open";
    if (next) append({ op_id: createId(), entity: "task", entity_id: id, field: "status", value: next, hlc: serverHlc() }, "server");
  }
}

/** Task references like the server (zunder_core::numbering): per key in creation order, never reused. */
function assignRefs() {
  const keyOf = (projectId: string | null) => {
    const p = projectId ? state.get(key("project", projectId)) : undefined;
    const k = p && p.fields.deleted?.value !== true ? p.fields.key?.value : undefined;
    return typeof k === "string" ? k : null;
  };
  const tasks = [...state.values()].filter((e) => e.entity === "task");
  const pending = tasks
    .filter((e) => e.fields.deleted?.value !== true && typeof e.fields.ref?.value !== "string")
    .map((e) => ({ e, k: keyOf(effectiveProject(e.id, new Map())) }))
    .filter((x): x is { e: Entity; k: string } => x.k !== null)
    .sort((a, b) => (a.e.created_hlc < b.e.created_hlc ? -1 : a.e.created_hlc > b.e.created_hlc ? 1 : a.e.id < b.e.id ? -1 : 1));
  const last = new Map<string, number>();
  for (const e of tasks) {
    const m = /^([A-Z][A-Z0-9]*)-([1-9][0-9]*)$/.exec(String(e.fields.ref?.value ?? ""));
    if (m) last.set(m[1]!, Math.max(last.get(m[1]!) ?? 0, Number(m[2])));
  }
  for (const { e, k } of pending) {
    const n = (last.get(k) ?? 0) + 1;
    last.set(k, n);
    append({ op_id: createId(), entity: "task", entity_id: e.id, field: "ref", value: `${k}-${n}`, hlc: serverHlc() }, "server");
  }
}

function serverOps(entity: EntityKind, id: string, fields: Record<string, Json>, source = "server") {
  for (const [field, value] of Object.entries(fields)) {
    append({ op_id: createId(), entity, entity_id: id, field, value, hlc: serverHlc() }, source);
  }
  notify();
}

// ---- Seed --------------------------------------------------------------------------

function isoWeek(d: Date): string {
  const date = new Date(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()));
  const day = date.getUTCDay() || 7;
  date.setUTCDate(date.getUTCDate() + 4 - day);
  const yearStart = Date.UTC(date.getUTCFullYear(), 0, 1);
  const week = Math.ceil(((date.getTime() - yearStart) / 86400000 + 1) / 7);
  return `${date.getUTCFullYear()}-W${String(week).padStart(2, "0")}`;
}

function localDate(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

const SEED_IDS = {
  areaHealth: "0192a000-0000-7000-8000-000000000001",
  areaWork: "0192a000-0000-7000-8000-000000000002",
  areaHome: "0192a000-0000-7000-8000-000000000003",
  projDev: "0192a000-0000-7000-8000-000000000010",
  projMove: "0192a000-0000-7000-8000-000000000011",
};

function timingSettings() {
  const overridden = timing.token !== null || timing.project !== null;
  return {
    configured: timing.token !== null,
    token_source: timing.token !== null ? "settings" : null,
    token_hint: timing.token !== null ? (timing.token.length >= 12 ? `…${timing.token.slice(-4)}` : "…") : null,
    project: timing.project || null,
    project_source: timing.project !== null ? "settings" : null,
    overridden,
    env_token: false,
    env_project: null,
  };
}

function seed(kind: string) {
  log = [];
  state = new Map();
  opIds = new Set();
  tokens = [{ id: createId(), name: "claude-code", created_at: new Date(Date.now() - 86400000 * 3).toISOString(), last_used_at: null }];
  oauthClients = [];
  timing = { token: null, project: null };
  devices = [];
  intakeChannels = [];
  calendarCreatedAt = null;
  focus = { dismissed_on: null, chosen: null };
  if (kind === "empty") return;
  const base = Date.now() - 3600_000;
  let n = 0;
  const op = (entity: EntityKind, id: string, field: string, value: Json) => {
    n += 1;
    append({ op_id: createId(), entity, entity_id: id, field, value, hlc: `${String(base + n).padStart(13, "0")}:00000:import` }, "api:import");
  };
  const today = new Date();
  const fire = isoWeek(today);
  const I = SEED_IDS;
  op("area", I.areaHealth, "name", "Gesundheit");
  op("area", I.areaWork, "name", "Arbeit");
  op("area", I.areaHome, "name", "Haushalt");
  op("project", I.projDev, "name", "Zunder");
  op("project", I.projDev, "area_id", I.areaWork);
  op("project", I.projDev, "kind", "dev");
  op("project", I.projDev, "repo", "example/zunder");
  op("project", I.projMove, "name", "Umzug");
  op("project", I.projMove, "area_id", I.areaHome);

  const task = (id: string, fields: Record<string, Json>) => {
    for (const [f, v] of Object.entries(fields)) op("task", id, f, v);
  };
  const tid = (i: number) => `0192a000-0000-7000-8000-0000000001${String(i).padStart(2, "0")}`;
  task(tid(1), { title: "Steuerunterlagen sortieren" });
  task(tid(2), { title: "Fahrradschloss kaufen" });
  task(tid(3), { title: "Artikel über Rust lesen" });
  task(tid(4), { title: "Kartons besorgen", location: { kind: "project", id: I.projMove } });
  task(tid(5), { title: "Ummelden beim Bürgeramt", location: { kind: "project", id: I.projMove }, due: localDate(new Date(today.getTime() - 86400000)) });
  task(tid(6), { title: "Vorsorgetermin vereinbaren", location: { kind: "area", id: I.areaHealth }, due: localDate(today) });
  task(tid(7), { title: "Sync-Engine fertigstellen", location: { kind: "project", id: I.projDev }, type: "feature", status: "in_progress",
    claimed_by: "agent-1", claim_expires_at: new Date(Date.now() + 2 * 3600_000).toISOString(), branch: "feature/sync-engine",
    pr_url: "https://github.com/example/zunder/pull/1", phoenix: { role: "flamme", fire } });
  task(tid(8), { title: "Absturz beim Login", location: { kind: "project", id: I.projDev }, type: "bug" });
  task(tid(9), { title: "Board-Tastatursteuerung", location: { kind: "project", id: I.projDev }, type: "feature" });
  task(tid(10), { title: "Teilschritt A", parent_id: tid(9) });
  task(tid(11), { title: "Teilschritt B", parent_id: tid(9), status: "done" });
  task(tid(12), { title: "Wochenplanung", location: { kind: "loose" }, phoenix: { role: "feuer_schritt", fire } });
  task(tid(13), { title: "Deep Work: Schreiben", location: { kind: "area", id: I.areaWork }, phoenix: { role: "flamme", fire } });
}

// ---- HTTP ------------------------------------------------------------------------

function send(res: ServerResponse, status: number, body?: unknown, headers: Record<string, string> = {}) {
  const data = body === undefined ? "" : JSON.stringify(body);
  res.writeHead(status, { "Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store", ...headers });
  res.end(data);
}

function error(res: ServerResponse, status: number, code: string, message: string) {
  send(res, status, { error: { code, message } });
}

async function readJson(req: IncomingMessage): Promise<Record<string, any>> {
  let body = "";
  for await (const chunk of req) body += chunk;
  return body ? JSON.parse(body) : {};
}

const MIME: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".svg": "image/svg+xml",
  ".json": "application/json",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".webmanifest": "application/manifest+json",
};

function serveStatic(res: ServerResponse, pathname: string) {
  let file = normalize(join(DIST, decodeURIComponent(pathname)));
  if (!file.startsWith(DIST)) return error(res, 403, "forbidden", "Verboten.");
  if (!existsSync(file) || statSync(file).isDirectory()) {
    // SPA fallback only for paths without a file extension (like the server).
    if (pathname.startsWith("/assets/") || extname(pathname) !== "") return error(res, 404, "not_found", "Datei nicht gefunden.");
    file = join(DIST, "index.html");
  }
  if (!existsSync(file)) return error(res, 500, "internal", "web/dist fehlt – zuerst `npm run build`.");
  const immutable = file.includes(`${join(DIST, "assets")}`);
  res.writeHead(200, {
    "Content-Type": MIME[extname(file)] ?? "application/octet-stream",
    "Cache-Control": immutable ? "public, max-age=31536000, immutable" : "no-cache",
  });
  createReadStream(file).pipe(res);
}

seed("demo");

function isFlameRunning(e: Entity | undefined): boolean {
  const started = e?.fields.flame_started_at?.value;
  const ended = e?.fields.flame_ended_at?.value;
  if (typeof started !== "string") return false;
  return typeof ended !== "string" || Date.parse(ended) < Date.parse(started);
}

function flameOutcome(id: string, running: boolean, stopped: string[], unchanged: boolean) {
  const e = state.get(key("task", id));
  return {
    task: { id, title: e?.fields.title?.value ?? "", flame_running: running },
    running,
    stopped_task_ids: stopped,
    unchanged,
    timing: { configured: false, used: false, entry: null, error: null },
    warnings: [],
  };
}

function intakeInfo(c: IntakeChannel) {
  const name = state.get(key("project", c.project_id))?.fields.name?.value;
  return {
    id: c.id,
    name: c.name,
    project_id: c.project_id,
    project_name: typeof name === "string" ? name : null,
    created_at: c.created_at,
    last_used_at: c.last_used_at,
  };
}

/** Why the project takes no intake tasks (dead or completed), null when it does. */
function intakeTargetProblem(projectId: string): string | null {
  if (!alive("project", projectId)) return "Das Zielprojekt gibt es nicht (mehr)";
  if (state.get(key("project", projectId))?.fields.status?.value === "completed") return "Das Zielprojekt ist abgeschlossen";
  return null;
}

const INTAKE_FIELDS = ["external_id", "title", "description", "type", "links"];

/** Field checks of POST /intake/v1/tasks; the core rules run in push(). */
function intakeBodyError(body: Record<string, any>): string | null {
  if (typeof body !== "object" || body === null || Array.isArray(body)) return "Der Body muss ein JSON-Objekt sein";
  const unknown = Object.keys(body).find((k) => !INTAKE_FIELDS.includes(k));
  if (unknown) return `Unbekanntes Feld „${unknown}“`;
  if (typeof body.external_id !== "string" || !/^[A-Za-z0-9_-]{1,128}$/.test(body.external_id)) return "„external_id“ ist ungültig";
  if (typeof body.title !== "string" || body.title.trim() === "" || [...body.title.trim()].length > 200) return "„title“ ist ungültig";
  if (body.description != null && (typeof body.description !== "string" || [...body.description].length > 20000)) return "„description“ ist ungültig";
  if (body.type != null && !["aufgabe", "bug", "feature"].includes(body.type)) return "„type“ muss „aufgabe“, „bug“ oder „feature“ sein";
  if (body.links != null) {
    if (!Array.isArray(body.links)) return "„links“ muss eine Liste sein";
    if (body.links.length > 5) return "Höchstens 5 Links erlaubt";
    for (const link of body.links) {
      if (typeof link?.url !== "string") continue;
      let ok = false;
      try {
        ok = ["http:", "https:"].includes(new URL(link.url).protocol);
      } catch {
        ok = false;
      }
      if (!ok) return "Links müssen http- oder https-Adressen sein";
    }
  }
  return null;
}

function tokenNameError(name: unknown): string | null {
  if (typeof name !== "string" || name.trim() === "" || [...name].length > 64) return "Der Name muss 1 bis 64 Zeichen lang sein.";
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(name)) return "Der Name darf keine Steuerzeichen enthalten.";
  if (name === "bootstrap") return "Der Name „bootstrap“ ist reserviert.";
  return null;
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  const path = url.pathname;
  const method = req.method ?? "GET";
  const safe = method === "GET" || method === "HEAD" || method === "OPTIONS";
  const csrfOk = safe || req.headers["x-zunder-client"] === "web";
  try {
    // Test control
    if (path === "/__test/reset" && method === "POST") {
      const body = await readJson(req);
      seed(body.seed ?? "demo");
      authenticated = body.auth ?? true;
      return send(res, 200, { ok: true, cursor: log.length });
    }
    if (path === "/__test/ops" && method === "POST") {
      const body = await readJson(req);
      return send(res, 200, push(body.ops ?? [], body.source ?? "mcp:test"));
    }
    if (path === "/__test/oauth-client" && method === "POST") {
      const body = await readJson(req);
      const client: OAuthClient = {
        id: createId(),
        name: String(body.name ?? "Claude"),
        redirect_uris: ["https://claude.ai/api/mcp/auth_callback"],
        created_at: new Date().toISOString(),
        last_used_at: new Date().toISOString(),
      };
      oauthClients.push(client);
      return send(res, 201, client);
    }
    if (path === "/__test/device" && method === "POST") {
      const body = await readJson(req);
      const device: Device = { id: createId(), name: String(body.name ?? "iPhone"), created_at: new Date().toISOString(), last_used_at: null };
      devices.push(device);
      return send(res, 201, device);
    }
    if (path === "/__test/state" && method === "GET") {
      const out: Record<string, Record<string, Json>> = {};
      for (const [k, e] of state) out[k] = Object.fromEntries(Object.entries(e.fields).map(([f, s]) => [f, s.value]));
      return send(res, 200, { cursor: log.length, entities: out });
    }

    if (path === "/healthz") return send(res, 200, { status: "ok", version: "mock" });
    if (path === "/auth/login" && method === "GET") {
      // Like dev auth: log in directly and return to a local path.
      authenticated = true;
      const target = url.searchParams.get("return_to") ?? "/";
      const local = target.startsWith("/") && !target.startsWith("//") && !target.includes("\\") ? target : "/";
      res.writeHead(303, { Location: local, "Cache-Control": "no-store" });
      return res.end();
    }
    if (path === "/auth/logout" && method === "POST") {
      if (!csrfOk) return error(res, 403, "forbidden", "Header X-Zunder-Client fehlt.");
      authenticated = false;
      return send(res, 204);
    }

    if (path === "/intake/v1/tasks" && method === "POST") {
      // Only the token of an active channel; no session, no CSRF header.
      const auth = /^Bearer\s+(\S+)$/i.exec(req.headers.authorization ?? "");
      const channel = auth ? intakeChannels.find((c) => c.token === auth[1]) : undefined;
      if (!channel) {
        res.setHeader("WWW-Authenticate", 'Bearer realm="zunder"');
        return error(res, 401, "unauthorized", "Anmeldung erforderlich");
      }
      channel.last_used_at = new Date().toISOString();
      let body: Record<string, any>;
      try {
        body = await readJson(req);
      } catch {
        return error(res, 400, "invalid_request", "Kein gültiges JSON.");
      }
      const problem = intakeBodyError(body);
      if (problem) return error(res, 400, "invalid_request", problem);
      const refOf = (id: string) => {
        const r = state.get(key("task", id))?.fields.ref?.value;
        return typeof r === "string" ? r : null;
      };
      const known = channel.submissions.get(body.external_id);
      if (known) return send(res, 200, { id: known.task_id, ref: refOf(known.task_id) });
      const hourAgo = Date.now() - 3600_000;
      const recent = [...channel.submissions.values()].filter((s) => s.at >= hourAgo);
      if (recent.length >= 30) {
        const oldest = Math.min(...recent.map((s) => s.at));
        res.setHeader("Retry-After", String(Math.max(1, Math.ceil((oldest + 3600_000 - Date.now()) / 1000))));
        return error(res, 429, "too_many_requests", "Mehr als 30 Aufgaben in der letzten Stunde über diesen Eingang");
      }
      const target = intakeTargetProblem(channel.project_id);
      if (target) return error(res, 409, "conflict", target);
      const id = createId();
      const fields: Record<string, Json> = {
        title: body.title.trim(),
        type: body.type ?? "aufgabe",
        status: "open",
        location: { kind: "project", id: channel.project_id },
      };
      if (typeof body.description === "string" && body.description.trim()) fields.description = body.description;
      if (Array.isArray(body.links) && body.links.length > 0) fields.links = body.links;
      const ops = Object.entries(fields).map(([field, value]) => ({ op_id: createId(), entity: "task" as const, entity_id: id, field, value, hlc: serverHlc() }));
      const result = push(ops, `intake:${channel.name}`);
      const cause = result.rejected.find((r) => r.code !== "entity_rejected") ?? result.rejected[0];
      if (cause) return error(res, 400, "invalid_request", cause.message);
      channel.submissions.set(body.external_id, { task_id: id, at: Date.now() });
      return send(res, 201, { id, ref: refOf(id) });
    }

    if (path.startsWith("/api/")) {
      if (!authenticated) {
        res.setHeader("WWW-Authenticate", "Bearer");
        return error(res, 401, "unauthorized", "Nicht angemeldet.");
      }
      if (!csrfOk) return error(res, 403, "forbidden", "Header X-Zunder-Client fehlt.");

      if (path === "/api/me") return send(res, 200, { user_id: "mock-user", name: "Testperson", email: "test@example.org" });

      if (path === "/api/snapshot") {
        const entities = [...state.values()].map((e) => ({
          entity: e.entity,
          entity_id: e.id,
          created_hlc: e.created_hlc,
          fields: Object.fromEntries(Object.entries(e.fields).map(([f, s]) => [f, { value: s.value, hlc: s.hlc, op_id: s.op_id }])),
        }));
        return send(res, 200, { cursor: log.length, entities });
      }

      if (path === "/api/sync/push" && method === "POST") {
        let body: Record<string, any>;
        try {
          body = await readJson(req);
        } catch {
          return error(res, 400, "invalid_request", "Kein gültiges JSON.");
        }
        if (!Array.isArray(body.ops)) return error(res, 400, "invalid_request", "ops fehlt.");
        for (const op of body.ops) {
          const problem = wireError(op);
          if (problem) return error(res, 400, "invalid_request", problem);
        }
        return send(res, 200, push(body.ops, "web"));
      }

      if (path === "/api/sync/pull") {
        const since = Number(url.searchParams.get("since") ?? 0);
        const limit = Math.min(Number(url.searchParams.get("limit") ?? 1000), 5000);
        if (!Number.isInteger(since) || since < 0) return error(res, 400, "invalid_request", "since darf nicht negativ sein");
        if (!Number.isInteger(limit) || limit < 1) return error(res, 400, "invalid_request", "limit muss mindestens 1 sein");
        const slice = log.slice(since, since + limit);
        return send(res, 200, {
          ops: slice.map((e) => ({ seq: e.seq, op: e.op })),
          cursor: slice.length > 0 ? slice[slice.length - 1]!.seq : log.length,
          has_more: since + slice.length < log.length,
        });
      }

      if (path === "/api/sync/events") {
        res.writeHead(200, { "Content-Type": "text/event-stream", "Cache-Control": "no-store", Connection: "keep-alive" });
        res.write(`event: cursor\ndata: ${log.length}\n\n`);
        sseClients.add(res);
        const keepAlive = setInterval(() => res.write(": keep-alive\n\n"), 15000);
        req.on("close", () => {
          clearInterval(keepAlive);
          sseClients.delete(res);
        });
        return;
      }

      let m = /^\/api\/tasks\/([^/]+)\/history$/.exec(path);
      if (m) {
        const id = decodeURIComponent(m[1]!);
        if (!ID_RE.test(id)) return error(res, 400, "invalid_request", "Keine kanonische ID.");
        if (!state.has(key("task", id))) return error(res, 404, "not_found", "Aufgabe nicht gefunden.");
        return send(res, 200, log.filter((e) => e.op.entity === "task" && e.op.entity_id === id));
      }

      if (path === "/api/agent-log" && method === "GET") {
        const beforeParam = url.searchParams.get("before");
        const before = beforeParam === null ? Infinity : Number(beforeParam);
        const limit = Math.min(Number(url.searchParams.get("limit") ?? 200), 1000);
        if (!(before >= 1)) return error(res, 400, "invalid_request", "before muss mindestens 1 sein");
        if (!Number.isInteger(limit) || limit < 1) return error(res, 400, "invalid_request", "limit muss mindestens 1 sein");
        return send(res, 200, agentLog(before, limit));
      }

      m = /^\/api\/tasks\/([^/]+)\/flame\/(start|stop)$/.exec(path);
      if (m && method === "POST") {
        const id = decodeURIComponent(m[1]!);
        if (!ID_RE.test(id)) return error(res, 400, "invalid_request", "Keine kanonische ID.");
        if (!alive("task", id)) return error(res, 404, "not_found", "Aufgabe nicht gefunden.");
        const at = new Date().toISOString();
        const running = isFlameRunning(state.get(key("task", id)));
        if (m[2] === "start") {
          if (running) return send(res, 200, flameOutcome(id, true, [], true));
          const others = [...state.values()].filter((e) => e.entity === "task" && e.id !== id && e.fields.deleted?.value !== true && isFlameRunning(e));
          for (const other of others) serverOps("task", other.id, { flame_ended_at: at });
          serverOps("task", id, { flame_started_at: at, flame_ended_at: null });
          return send(res, 200, flameOutcome(id, true, others.map((o) => o.id), false));
        }
        if (!running) return error(res, 409, "conflict", "Auf dieser Aufgabe läuft keine Flamme.");
        serverOps("task", id, { flame_ended_at: at });
        return send(res, 200, flameOutcome(id, false, [], false));
      }

      if (path === "/api/tokens" && method === "GET") return send(res, 200, tokens);
      if (path === "/api/tokens" && method === "POST") {
        const body = await readJson(req);
        const problem = tokenNameError(body.name);
        if (problem) return error(res, 400, "invalid_request", problem);
        if (tokens.some((t) => t.name === body.name)) return error(res, 409, "conflict", `Es gibt bereits einen Token „${body.name}“.`);
        const token: Token = { id: createId(), name: body.name, created_at: new Date().toISOString(), last_used_at: null };
        tokens.push(token);
        return send(res, 201, { ...token, token: `zt_${randomBytes(32).toString("base64url")}` });
      }
      m = /^\/api\/tokens\/([^/]+)$/.exec(path);
      if (m && method === "DELETE") {
        const tokenId = decodeURIComponent(m[1]!);
        if (!tokens.some((t) => t.id === tokenId)) return error(res, 404, "not_found", "Token nicht gefunden.");
        tokens = tokens.filter((t) => t.id !== tokenId);
        return send(res, 204);
      }
      if (path === "/api/settings/timing") {
        if (method === "PUT") {
          const body = await readJson(req);
          const token = typeof body.token === "string" ? body.token.trim() : "";
          if (/\s/.test(token)) return error(res, 400, "invalid_request", "Der Timing-Token darf keine Leer- oder Steuerzeichen enthalten");
          if (token) timing.token = token;
          if (typeof body.project === "string") timing.project = body.project.trim();
        } else if (method === "DELETE") timing = { token: null, project: null };
        else if (method !== "GET") return error(res, 405, "invalid_request", "Methode nicht erlaubt.");
        return send(res, 200, timingSettings());
      }
      if (path === "/api/settings/timing/projects" && method === "GET") {
        if (!timing.token) return error(res, 409, "conflict", "Timing ist nicht eingerichtet.");
        return send(res, 200, TIMING_PROJECTS);
      }
      if (path === "/api/settings/timing/check" && method === "POST") {
        if (!timing.token) return error(res, 409, "conflict", "Timing ist nicht eingerichtet.");
        return send(res, 200, { ok: true, error: null });
      }
      if (path === "/api/settings/focus") {
        if (method === "PUT") {
          const body = await readJson(req);
          const unknown = Object.keys(body).find((k) => k !== "dismissed_on" && k !== "chosen");
          if (unknown) return error(res, 400, "invalid_request", `Unbekanntes Feld „${unknown}“`);
          const day = (v: unknown) => (typeof v === "string" && isCalendarDay(v.trim()) ? v.trim() : null);
          const next = { ...focus };
          if (body.dismissed_on !== undefined) {
            if (body.dismissed_on !== null && !day(body.dismissed_on)) return error(res, 400, "invalid_request", "„dismissed_on“ muss ein Tag JJJJ-MM-TT sein");
            next.dismissed_on = body.dismissed_on === null ? null : day(body.dismissed_on);
          }
          if (body.chosen !== undefined) {
            const c = body.chosen as { on?: unknown; task_id?: unknown } | null;
            if (c !== null) {
              if (typeof c !== "object" || Array.isArray(c) || Object.keys(c).some((k) => k !== "on" && k !== "task_id")) return error(res, 400, "invalid_request", "„chosen“ braucht on und task_id");
              if (!day(c.on)) return error(res, 400, "invalid_request", "„chosen.on“ muss ein Tag JJJJ-MM-TT sein");
              if (typeof c.task_id !== "string" || !ID_RE.test(c.task_id.trim())) return error(res, 400, "invalid_request", "„chosen.task_id“ muss eine Aufgaben-ID sein");
              next.chosen = { on: day(c.on)!, task_id: c.task_id.trim() };
            } else next.chosen = null;
          }
          focus = next;
        } else if (method !== "GET") return error(res, 405, "invalid_request", "Methode nicht erlaubt.");
        return send(res, 200, focus);
      }
      if (path === "/api/settings/calendar") {
        if (method === "POST") {
          calendarCreatedAt = new Date().toISOString();
          const url = `http://${req.headers.host ?? "localhost"}/calendar/zc_${randomBytes(32).toString("base64url")}.ics`;
          return send(res, 201, { enabled: true, created_at: calendarCreatedAt, url, url_all: `${url}?scope=all` });
        }
        if (method === "DELETE") calendarCreatedAt = null;
        else if (method !== "GET") return error(res, 405, "invalid_request", "Methode nicht erlaubt.");
        return send(res, 200, { enabled: calendarCreatedAt !== null, created_at: calendarCreatedAt });
      }
      if (path === "/api/devices" && method === "GET") return send(res, 200, devices);
      m = /^\/api\/devices\/([^/]+)$/.exec(path);
      if (m && method === "DELETE") {
        const deviceId = decodeURIComponent(m[1]!);
        if (!devices.some((d) => d.id === deviceId)) return error(res, 404, "not_found", "Gerät nicht gefunden");
        devices = devices.filter((d) => d.id !== deviceId);
        return send(res, 204);
      }
      if (path === "/api/intake/channels" && method === "GET") return send(res, 200, intakeChannels.map(intakeInfo));
      if (path === "/api/intake/channels" && method === "POST") {
        const body = await readJson(req);
        const unknown = Object.keys(body).find((k) => k !== "name" && k !== "project_id");
        if (unknown) return error(res, 400, "invalid_request", `Unbekanntes Feld „${unknown}“`);
        const name = typeof body.name === "string" ? body.name.trim() : "";
        const problem = tokenNameError(name);
        if (problem && name !== "bootstrap") return error(res, 400, "invalid_request", problem);
        if (typeof body.project_id !== "string" || !ID_RE.test(body.project_id)) return error(res, 400, "invalid_request", "Ungültige Projekt-ID");
        const target = intakeTargetProblem(body.project_id);
        if (target) return error(res, 409, "conflict", target);
        if (intakeChannels.some((c) => c.name === name)) return error(res, 409, "conflict", `Es gibt bereits einen Eingang mit dem Namen „${name}“`);
        const channel: IntakeChannel = {
          id: createId(),
          name,
          project_id: body.project_id,
          token: `zi_${randomBytes(32).toString("base64url")}`,
          created_at: new Date().toISOString(),
          last_used_at: null,
          submissions: new Map(),
        };
        intakeChannels.push(channel);
        return send(res, 201, { ...intakeInfo(channel), token: channel.token });
      }
      m = /^\/api\/intake\/channels\/([^/]+)$/.exec(path);
      if (m && method === "DELETE") {
        const channelId = decodeURIComponent(m[1]!);
        if (!intakeChannels.some((c) => c.id === channelId)) return error(res, 404, "not_found", "Eingang nicht gefunden");
        intakeChannels = intakeChannels.filter((c) => c.id !== channelId);
        return send(res, 204);
      }

      if (path === "/api/oauth/clients" && method === "GET") return send(res, 200, oauthClients);
      m = /^\/api\/oauth\/clients\/([^/]+)$/.exec(path);
      if (m && method === "DELETE") {
        const clientId = decodeURIComponent(m[1]!);
        if (!oauthClients.some((c) => c.id === clientId)) return error(res, 404, "not_found", "Verbundener Client nicht gefunden");
        oauthClients = oauthClients.filter((c) => c.id !== clientId);
        return send(res, 204);
      }
      return error(res, 404, "not_found", "Unbekannter API-Pfad");
    }

    if (method !== "GET" && method !== "HEAD") return error(res, 405, "invalid_request", "Methode nicht erlaubt.");
    return serveStatic(res, path);
  } catch (err) {
    console.error(err);
    return error(res, 500, "internal", "Interner Fehler im Mock-Server.");
  }
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`Zunder-Mock läuft auf http://127.0.0.1:${PORT} (dist: ${DIST})`);
});
