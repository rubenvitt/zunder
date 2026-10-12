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
// and a subset of the core validation with the core's rejection codes.

import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { dirname, extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { randomBytes, randomUUID } from "node:crypto";

const here = dirname(fileURLToPath(import.meta.url));
const DIST = resolve(process.env.MOCK_DIST ?? join(here, "..", "dist"));
const PORT = Number(process.env.MOCK_PORT ?? 4173);

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type EntityKind = "area" | "project" | "task";

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
    "flame_ended_at", "clickup_id", "rank", "ref", "deleted",
  ],
};
const ENUMS: Record<string, string[]> = {
  "task.status": ["open", "in_progress", "review", "blocked", "done", "dropped"],
  "task.type": ["aufgabe", "bug", "feature"],
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
let devices: Device[] = [];
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

function append(op: Op, source: string) {
  if (opIds.has(op.op_id)) return;
  opIds.add(op.op_id);
  log.push({ seq: log.length + 1, op, source, received_at: new Date().toISOString() });
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

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const HLC_RE = /^\d{13}:\d{5}:[a-z0-9-]{1,36}$/;
const REQUIRED: Record<string, string[]> = { area: ["name"], project: ["name", "area_id"], task: ["title"] };

/** Wire-format check; a malformed op fails the whole push with 400 (like the server). */
function wireError(op: unknown): string | null {
  if (!op || typeof op !== "object") return "Operation ist kein Objekt.";
  const o = op as Record<string, unknown>;
  if (!UUID_RE.test(String(o.op_id)) || !UUID_RE.test(String(o.entity_id))) return "Keine kanonische UUID.";
  if (!["area", "project", "task"].includes(String(o.entity))) return "Unbekannte Entitätsart.";
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
  if (op.value === null) {
    return REQUIRED[op.entity]!.includes(op.field) ? ["invalid_value", `Pflichtfeld „${op.field}“ kann nicht zurückgesetzt werden.`] : null;
  }
  const allowed = ENUMS[`${op.entity}.${op.field}`];
  if (allowed && !allowed.includes(String(op.value))) return ["invalid_value", `Ungültiger Wert für „${op.field}“.`];
  if ((op.field === "title" || op.field === "name") && (typeof op.value !== "string" || !op.value.trim())) {
    return ["invalid_value", `„${op.field}“ darf nicht leer sein.`];
  }
  if (op.entity === "project" && op.field === "key" && !/^[A-Z][A-Z0-9]{0,9}$/.test(String(op.value))) {
    return ["invalid_value", "Ungültiger Schlüssel."];
  }
  if (op.field === "deleted" && typeof op.value !== "boolean") return ["invalid_value", "„deleted“ muss true oder false sein."];
  if ((op.field === "parent_id" || op.field === "area_id") && !UUID_RE.test(String(op.value))) {
    return ["invalid_value", `„${op.field}“ muss eine UUID (klein, mit Bindestrichen) sein.`];
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
  if (entity === "task") {
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

function push(ops: Op[], source: string) {
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
  for (const op of ops) {
    const reason = verdict.get(op.op_id);
    if (reason) {
      rejected.push({ op_id: op.op_id, code: reason[0], message: reason[1] });
      continue;
    }
    if (groups.get(key(op.entity, op.entity_id))?.includes(op)) append(op, source);
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
    notify();
  }
  return { accepted, rejected, warnings, cursor: log.length };
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
    append({ op_id: randomUUID(), entity: "task", entity_id: e.id, field: "ref", value: `${k}-${n}`, hlc: serverHlc() }, "server");
  }
}

function serverOps(entity: EntityKind, id: string, fields: Record<string, Json>, source = "server") {
  for (const [field, value] of Object.entries(fields)) {
    append({ op_id: randomUUID(), entity, entity_id: id, field, value, hlc: serverHlc() }, source);
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
  tokens = [{ id: randomUUID(), name: "claude-code", created_at: new Date(Date.now() - 86400000 * 3).toISOString(), last_used_at: null }];
  oauthClients = [];
  timing = { token: null, project: null };
  devices = [];
  if (kind === "empty") return;
  const base = Date.now() - 3600_000;
  let n = 0;
  const op = (entity: EntityKind, id: string, field: string, value: Json) => {
    n += 1;
    append({ op_id: randomUUID(), entity, entity_id: id, field, value, hlc: `${String(base + n).padStart(13, "0")}:00000:import` }, "api:import");
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
        id: randomUUID(),
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
      const device: Device = { id: randomUUID(), name: String(body.name ?? "iPhone"), created_at: new Date().toISOString(), last_used_at: null };
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
        if (!UUID_RE.test(id)) return error(res, 400, "invalid_request", "Keine kanonische ID.");
        if (!state.has(key("task", id))) return error(res, 404, "not_found", "Aufgabe nicht gefunden.");
        return send(res, 200, log.filter((e) => e.op.entity === "task" && e.op.entity_id === id));
      }

      m = /^\/api\/tasks\/([^/]+)\/flame\/(start|stop)$/.exec(path);
      if (m && method === "POST") {
        const id = decodeURIComponent(m[1]!);
        if (!UUID_RE.test(id)) return error(res, 400, "invalid_request", "Keine kanonische ID.");
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
        const token: Token = { id: randomUUID(), name: body.name, created_at: new Date().toISOString(), last_used_at: null };
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
      if (path === "/api/settings/timing/check" && method === "POST") {
        if (!timing.token) return error(res, 409, "conflict", "Timing ist nicht eingerichtet.");
        return send(res, 200, { ok: true, error: null });
      }
      if (path === "/api/devices" && method === "GET") return send(res, 200, devices);
      m = /^\/api\/devices\/([^/]+)$/.exec(path);
      if (m && method === "DELETE") {
        const deviceId = decodeURIComponent(m[1]!);
        if (!devices.some((d) => d.id === deviceId)) return error(res, 404, "not_found", "Gerät nicht gefunden");
        devices = devices.filter((d) => d.id !== deviceId);
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
