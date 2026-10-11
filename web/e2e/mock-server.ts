// Small in-memory mock of the Zunder HTTP API (contract section 3) for e2e tests
// and UI development without the Rust server. Serves the built web/dist.
//
//   node e2e/mock-server.ts             (port from MOCK_PORT, default 4173; Node >= 22.18 strips types)
//
// Test-only control endpoints (never part of the real API):
//   POST /__test/reset   {"seed": "demo" | "empty", "auth": true | false}
//   POST /__test/ops     {"ops": [Op], "source": "mcp:test"}   simulate another client
//   GET  /__test/state   materialized state for assertions
// Mock-only rule: a task title containing "[ablehnen]" is rejected, to exercise rollback.

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
}

interface Token {
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
  project: ["name", "area_id", "status", "kind", "repo", "description", "view", "deleted"],
  task: [
    "title", "description", "type", "status", "location", "parent_id", "due", "remind_at", "vault_links", "links",
    "phoenix", "claimed_by", "claim_expires_at", "plan", "branch", "pr_url", "timing_ref", "flame_started_at",
    "flame_ended_at", "clickup_id", "rank", "deleted",
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

let log: { seq: number; op: Op; source: string }[] = [];
let state = new Map<string, Entity>();
let opIds = new Set<string>();
let tokens: Token[] = [];
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
  if (!e) state.set(k, (e = { entity: op.entity, id: op.entity_id, fields: {} }));
  const cur = e.fields[op.field];
  if (!cur || op.hlc > cur.hlc || (op.hlc === cur.hlc && op.op_id > cur.op_id)) {
    e.fields[op.field] = { value: op.value, hlc: op.hlc, op_id: op.op_id };
  }
}

function append(op: Op, source: string) {
  if (opIds.has(op.op_id)) return;
  opIds.add(op.op_id);
  log.push({ seq: log.length + 1, op, source });
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

// ---- Validation (subset of contract 2.5) ----------------------------------------

function checkOp(op: Op): [string, string] | null {
  const known = FIELDS[op.entity];
  if (!known) return ["invalid_entity", "Unbekannte Entität."];
  if (!known.includes(op.field)) return ["unknown_field", `Unbekanntes Feld „${op.field}“.`];
  if (typeof op.hlc !== "string" || !/^\d{13}:\d{5}:[a-z0-9-]{1,36}$/.test(op.hlc)) return ["invalid_hlc", "Ungültiger Zeitstempel."];
  if (Number(op.hlc.slice(0, 13)) > Date.now() + 24 * 3600 * 1000) return ["hlc_in_future", "Zeitstempel liegt zu weit in der Zukunft."];
  const allowed = ENUMS[`${op.entity}.${op.field}`];
  if (allowed && op.value !== null && !allowed.includes(String(op.value))) return ["invalid_value", `Ungültiger Wert für „${op.field}“.`];
  if ((op.field === "title" || op.field === "name") && (typeof op.value !== "string" || !op.value.trim())) {
    return ["required", "Pflichtfeld darf nicht leer sein."];
  }
  if (op.entity === "task" && op.field === "title" && typeof op.value === "string" && op.value.includes("[ablehnen]")) {
    return ["test_reject", "Vom Testserver abgelehnt."];
  }
  return null;
}

function checkEntity(entity: EntityKind, id: string, ops: Op[]): [string, string] | null {
  const existing = state.get(key(entity, id));
  const fields: Record<string, Json> = {};
  for (const [f, st] of Object.entries(existing?.fields ?? {})) fields[f] = st.value;
  for (const op of ops) fields[op.field] = op.value;
  const str = (f: string) => (typeof fields[f] === "string" ? (fields[f] as string) : "");
  if (entity === "task") {
    if (!str("title").trim()) return ["required", "Eine Aufgabe braucht einen Titel."];
    const loc = fields.location;
    if (loc && typeof loc === "object" && !Array.isArray(loc) && (loc.kind === "area" || loc.kind === "project")) {
      if (!alive(loc.kind, String(loc.id))) return ["invalid_reference", "Die Stelle existiert nicht."];
    }
    if (str("parent_id") && !alive("task", str("parent_id"))) return ["invalid_reference", "Die Elternaufgabe existiert nicht."];
  } else if (entity === "project") {
    if (!str("name")) return ["required", "Ein Projekt braucht einen Namen."];
    if (!str("area_id") || !alive("area", str("area_id"))) return ["invalid_reference", "Der Bereich existiert nicht."];
  } else if (!str("name")) return ["required", "Ein Bereich braucht einen Namen."];
  return null;
}

function push(ops: Op[], source: string) {
  const accepted: string[] = [];
  const rejected: Rejection[] = [];
  const warnings: { entity_id: string; code: string; message: string }[] = [];
  const groups = new Map<string, Op[]>();
  for (const op of ops) {
    if (opIds.has(op.op_id)) {
      accepted.push(op.op_id);
      continue;
    }
    const k = key(op.entity, op.entity_id);
    let group = groups.get(k);
    if (!group) groups.set(k, (group = []));
    group.push(op);
  }
  for (const group of groups.values()) {
    const first = group[0]!;
    let reason: [string, string] | null = null;
    for (const op of group) if ((reason = checkOp(op))) break;
    reason ??= checkEntity(first.entity, first.entity_id, group);
    if (reason) {
      for (const op of group) rejected.push({ op_id: op.op_id, code: reason[0], message: reason[1] });
      continue;
    }
    for (const op of group) {
      append(op, source);
      accepted.push(op.op_id);
    }
    if (first.entity === "task" && group.some((op) => op.field === "status" && op.value === "done")) {
      const open = [...state.values()].filter(
        (e) => e.entity === "task" && e.fields.parent_id?.value === first.entity_id && e.fields.deleted?.value !== true &&
          !["done", "dropped"].includes(String(e.fields.status?.value ?? "open")),
      );
      if (open.length) warnings.push({ entity_id: first.entity_id, code: "open_subtasks", message: "Es gibt noch offene Unteraufgaben." });
    }
  }
  if (accepted.length) notify();
  return { accepted, rejected, warnings, cursor: log.length };
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

function seed(kind: string) {
  log = [];
  state = new Map();
  opIds = new Set();
  tokens = [{ id: randomUUID(), name: "claude-code", created_at: new Date(Date.now() - 86400000 * 3).toISOString(), last_used_at: null }];
  if (kind === "empty") return;
  const base = Date.now() - 3600_000;
  let n = 0;
  const op = (entity: EntityKind, id: string, field: string, value: Json) => {
    n += 1;
    append({ op_id: randomUUID(), entity, entity_id: id, field, value, hlc: `${String(base + n).padStart(13, "0")}:00000:import` }, "import");
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
  if (!existsSync(file) || statSync(file).isDirectory()) file = join(DIST, "index.html");
  if (!existsSync(file)) return error(res, 500, "no_dist", "web/dist fehlt – zuerst `npm run build`.");
  const immutable = file.includes(`${join(DIST, "assets")}`);
  res.writeHead(200, {
    "Content-Type": MIME[extname(file)] ?? "application/octet-stream",
    "Cache-Control": immutable ? "public, max-age=31536000, immutable" : "no-cache",
  });
  createReadStream(file).pipe(res);
}

seed("demo");

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  const path = url.pathname;
  const method = req.method ?? "GET";
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
    if (path === "/__test/state" && method === "GET") {
      const out: Record<string, Record<string, Json>> = {};
      for (const [k, e] of state) out[k] = Object.fromEntries(Object.entries(e.fields).map(([f, s]) => [f, s.value]));
      return send(res, 200, { cursor: log.length, entities: out });
    }

    if (path === "/healthz") return send(res, 200, { status: "ok", version: "mock" });
    if (path === "/auth/login") {
      authenticated = true;
      res.writeHead(302, { Location: "/" });
      return res.end();
    }
    if (path === "/auth/logout" && method === "POST") {
      authenticated = false;
      return send(res, 204);
    }

    if (path.startsWith("/api/")) {
      if (!authenticated) return error(res, 401, "unauthorized", "Nicht angemeldet.");

      if (path === "/api/me") return send(res, 200, { user_id: "mock-user", name: "Testperson", email: "test@example.org" });

      if (path === "/api/snapshot") {
        const entities = [...state.values()].map((e) => ({
          entity: e.entity,
          entity_id: e.id,
          fields: Object.fromEntries(Object.entries(e.fields).map(([f, s]) => [f, { value: s.value, hlc: s.hlc }])),
        }));
        return send(res, 200, { cursor: log.length, entities });
      }

      if (path === "/api/sync/push" && method === "POST") {
        const body = await readJson(req);
        if (!Array.isArray(body.ops)) return error(res, 400, "bad_request", "ops fehlt.");
        return send(res, 200, push(body.ops, "web"));
      }

      if (path === "/api/sync/pull") {
        const since = Number(url.searchParams.get("since") ?? 0);
        const limit = Number(url.searchParams.get("limit") ?? 1000);
        const slice = log.slice(since, since + limit);
        return send(res, 200, {
          ops: slice.map((e) => ({ seq: e.seq, op: e.op })),
          cursor: since + slice.length,
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
        return send(res, 200, log.filter((e) => e.op.entity === "task" && e.op.entity_id === id));
      }

      m = /^\/api\/tasks\/([^/]+)\/flame\/(start|stop)$/.exec(path);
      if (m && method === "POST") {
        const id = decodeURIComponent(m[1]!);
        if (!alive("task", id)) return error(res, 404, "not_found", "Aufgabe nicht gefunden.");
        const at = new Date().toISOString();
        if (m[2] === "start") serverOps("task", id, { flame_started_at: at, flame_ended_at: null });
        else serverOps("task", id, { flame_ended_at: at });
        return send(res, 200, { ok: true, cursor: log.length });
      }

      if (path === "/api/tokens" && method === "GET") return send(res, 200, tokens);
      if (path === "/api/tokens" && method === "POST") {
        const body = await readJson(req);
        if (!body.name?.trim()) return error(res, 400, "required", "Name fehlt.");
        const token: Token = { id: randomUUID(), name: body.name.trim(), created_at: new Date().toISOString(), last_used_at: null };
        tokens.push(token);
        return send(res, 201, { ...token, token: `zt_${randomBytes(32).toString("base64url")}` });
      }
      m = /^\/api\/tokens\/([^/]+)$/.exec(path);
      if (m && method === "DELETE") {
        const tokenId = decodeURIComponent(m[1]!);
        tokens = tokens.filter((t) => t.id !== tokenId);
        return send(res, 204);
      }
      return error(res, 404, "not_found", "Unbekannter Endpunkt.");
    }

    if (method !== "GET") return error(res, 405, "method_not_allowed", "Methode nicht erlaubt.");
    return serveStatic(res, path);
  } catch (err) {
    console.error(err);
    return error(res, 500, "internal", "Interner Fehler im Mock-Server.");
  }
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`Zunder-Mock läuft auf http://127.0.0.1:${PORT} (dist: ${DIST})`);
});
