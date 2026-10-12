// The web UI inside the Tauri app, with a stand-in for the app injected as
// window.__TAURI_INTERNALS__ (the same hook @tauri-apps/api uses). It follows
// the command and event contract of apps/zunder-app: local store with pending
// ops, set_config/sync_now, zunder://changed and zunder://sync. Needs no server;
// the page must not make a single /api request.

import { expect, test, type Page } from "@playwright/test";
import { newId, row } from "./fixtures";

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
interface WireOp {
  op_id: string;
  entity: string;
  entity_id: string;
  field: string;
  value: Json;
  hlc: string;
}

interface FakeAppHandle {
  calls: { cmd: string; args: Record<string, unknown> }[];
  ops: WireOp[];
  pending: number;
  emit(event: string, payload: unknown): void;
  serverOp(entity: string, id: string, field: string, value: Json): void;
  /** The browser came back with a device token (deep link). */
  completeLogin(): void;
}

declare global {
  interface Window {
    __ZUNDER_FAKE_APP__: FakeAppHandle;
  }
}

function currentFire(): string {
  // The browser runs in Europe/Berlin (playwright.config.ts), the test process may not.
  const [y, m, d] = new Intl.DateTimeFormat("en-CA", { timeZone: "Europe/Berlin" }).format(new Date()).split("-").map(Number);
  const date = new Date(Date.UTC(y!, m! - 1, d!));
  const day = date.getUTCDay() || 7;
  date.setUTCDate(date.getUTCDate() + 4 - day);
  const week = Math.ceil(((date.getTime() - Date.UTC(date.getUTCFullYear(), 0, 1)) / 86400000 + 1) / 7);
  return `${date.getUTCFullYear()}-W${String(week).padStart(2, "0")}`;
}

/** Runs in the page before any app code. */
function installFakeApp(seed: { serverOps: WireOp[]; validToken: string }) {
  let counter = 0;
  const hlc = () => `${String(Date.now()).padStart(13, "0")}:${String(++counter % 100000).padStart(5, "0")}:server`;
  // In the page: an ID in the wire form (letter + 23 base-36 characters).
  const uuid = () => "a" + Array.from(crypto.getRandomValues(new Uint8Array(23)), (b) => (b % 36).toString(36)).join("");
  const config = {
    server_url: null as string | null,
    has_api_token: false,
    token: "",
    node_id: "app-0a1b2c3d4e5f",
    cursor: 0,
    last_sync: null as null | { at_ms: number; ok: boolean; error?: string },
  };
  const callbacks = new Map<number, (event: unknown) => void>();
  const listeners: { event: string; id: number; handler: number }[] = [];
  let nextId = 1;

  const fake = {
    calls: [] as { cmd: string; args: Record<string, unknown> }[],
    ops: [] as WireOp[],
    pending: 0,
    emit(event: string, payload: unknown) {
      for (const l of listeners.filter((x) => x.event === event)) callbacks.get(l.handler)?.({ event, id: l.id, payload });
    },
    serverOp(entity: string, id: string, field: string, value: Json) {
      fake.ops.push({ op_id: uuid(), entity, entity_id: id, field, value, hlc: hlc() });
      config.cursor += 1;
    },
    completeLogin() {
      config.server_url = loginServer;
      config.token = seed.validToken;
      config.has_api_token = true;
      fake.emit("zunder://login", { ok: true });
    },
  };
  let loginServer: string | null = null;

  function snapshot() {
    const entities = new Map<string, { entity: string; entity_id: string; created_hlc: string; fields: Record<string, { value: Json; hlc: string; op_id: string }> }>();
    for (const op of fake.ops) {
      const key = `${op.entity}/${op.entity_id}`;
      const e = entities.get(key) ?? { entity: op.entity, entity_id: op.entity_id, created_hlc: op.hlc, fields: {} };
      entities.set(key, e);
      const current = e.fields[op.field];
      if (!current || current.hlc < op.hlc) e.fields[op.field] = { value: op.value, hlc: op.hlc, op_id: op.op_id };
      if (op.hlc < e.created_hlc) e.created_hlc = op.hlc;
    }
    return { cursor: config.cursor, entities: [...entities.values()], pending: fake.pending };
  }

  const view = () => ({
    server_url: config.server_url,
    has_api_token: config.has_api_token,
    node_id: config.node_id,
    cursor: config.cursor,
    pending: fake.pending,
    last_sync: config.last_sync,
  });

  function sync() {
    if (!config.server_url || !config.has_api_token) throw { code: "not_configured", message: "Es ist noch kein Server eingerichtet" };
    if (config.token !== seed.validToken) {
      const error = { code: "unauthorized", message: "Der Server lehnt den API-Token ab" };
      config.last_sync = { at_ms: Date.now(), ok: false, error: error.message };
      fake.emit("zunder://sync", { ok: false, error });
      throw error;
    }
    let changed = false;
    if (config.cursor === 0) {
      for (const op of seed.serverOps) fake.ops.push(op);
      config.cursor = seed.serverOps.length;
      changed = true;
    }
    const report = { pushed: fake.pending, rejected: [], warnings: [], pulled: changed ? seed.serverOps.length : 0, cursor: config.cursor, pending: 0, changed };
    fake.pending = 0;
    config.last_sync = { at_ms: Date.now(), ok: true };
    fake.emit("zunder://sync", { ok: true, report });
    if (changed) fake.emit("zunder://changed", { cursor: config.cursor, pending: 0 });
    return report;
  }

  function handle(cmd: string, args: Record<string, unknown>): unknown {
    switch (cmd) {
      case "plugin:event|listen": {
        const id = nextId++;
        listeners.push({ event: args.event as string, id, handler: args.handler as number });
        return id;
      }
      case "get_config":
        return view();
      case "set_config": {
        const update = args.update as { server_url?: string | null; api_token?: string | null };
        if (update.server_url !== undefined) config.server_url = update.server_url ? update.server_url.replace(/\/$/, "") : null;
        if (update.api_token !== undefined) {
          config.token = update.api_token ?? "";
          config.has_api_token = !!update.api_token;
        }
        return view();
      }
      case "login_start":
        loginServer = (args.server_url as string).replace(/\/$/, "");
        return null;
      case "sync_now":
        return sync();
      case "local_snapshot":
        return snapshot();
      case "local_apply": {
        const ops = args.ops as WireOp[];
        fake.ops.push(...ops);
        fake.pending += ops.length;
        return { accepted: ops.map((o) => o.op_id), rejected: [], warnings: [], pending: fake.pending };
      }
      case "task_history": {
        const id = args.task_id as string;
        return fake.ops.filter((o) => o.entity === "task" && o.entity_id === id).map((op, i) => ({ seq: i + 1, op, source: "api:app", received_at: new Date().toISOString() }));
      }
      case "flame_start":
      case "flame_stop": {
        const id = args.task_id as string;
        if (cmd === "flame_start") {
          fake.serverOp("task", id, "flame_started_at", new Date().toISOString());
          fake.serverOp("task", id, "flame_ended_at", null);
        } else fake.serverOp("task", id, "flame_ended_at", new Date(Date.now() + 1000).toISOString());
        fake.emit("zunder://changed", { cursor: config.cursor, pending: fake.pending });
        return { running: cmd === "flame_start" };
      }
      default:
        throw `command ${cmd} not found`;
    }
  }

  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
    transformCallback(callback: (event: unknown) => void) {
      const id = nextId++;
      callbacks.set(id, callback);
      return id;
    },
    async invoke(cmd: string, args: Record<string, unknown> = {}) {
      fake.calls.push({ cmd, args: JSON.parse(JSON.stringify(args)) as Record<string, unknown> });
      return handle(cmd, args);
    },
  };
  (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener(event: string, id: number) {
      const i = listeners.findIndex((l) => l.event === event && l.id === id);
      if (i >= 0) listeners.splice(i, 1);
    },
  };
  (window as unknown as { __ZUNDER_FAKE_APP__: unknown }).__ZUNDER_FAKE_APP__ = fake;
}

function serverOps(fire: string) {
  let n = 0;
  const op = (entity: string, entity_id: string, field: string, value: Json): WireOp => ({
    op_id: newId(),
    entity,
    entity_id,
    field,
    value,
    hlc: `${String(Date.now() - 60_000).padStart(13, "0")}:${String(++n).padStart(5, "0")}:web-e2e`,
  });
  const area = newId();
  const inboxTask = newId();
  const flameTask = newId();
  return {
    flameTask,
    ops: [
      op("area", area, "name", "Schreiben"),
      op("task", inboxTask, "title", "Vom Server geladen"),
      op("task", flameTask, "title", "Deep Work"),
      op("task", flameTask, "location", { kind: "area", id: area }),
      op("task", flameTask, "phoenix", { role: "flamme", fire }),
    ],
  };
}

async function openInApp(page: Page, ops: WireOp[]) {
  const apiRequests: string[] = [];
  page.on("request", (req) => {
    if (new URL(req.url()).pathname.startsWith("/api/") || new URL(req.url()).pathname.startsWith("/auth/")) apiRequests.push(req.url());
  });
  // Cast: the recursive Json type is too deep for addInitScript's argument typing.
  const seed: unknown = { serverOps: ops, validToken: "zt_richtig" };
  await page.addInitScript(installFakeApp as (seed: unknown) => void, seed);
  await page.goto("/#/inbox");
  await expect(page.locator("html[data-ready]")).toBeAttached();
  return apiRequests;
}

/** Hash navigation without reloading (a reload would reset the fake app). */
const go = (page: Page, hash: string) => page.evaluate((h) => void (location.hash = h), hash);

const calls = (page: Page) => page.evaluate(() => window.__ZUNDER_FAKE_APP__.calls.map((c) => c.cmd));

test("App: Einrichtung mit Server und Token statt Anmeldung", async ({ page }) => {
  const seed = serverOps(currentFire());
  const apiRequests = await openInApp(page, seed.ops);

  await expect(page.getByRole("heading", { name: "Zunder einrichten" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Anmelden" })).toHaveCount(0);

  await page.getByLabel("Server-Adresse").fill("https://zunder.example.org/");
  await page.getByLabel("API-Token").fill("zt_falsch");
  await page.getByRole("button", { name: "Speichern und verbinden" }).click();
  await expect(page.getByRole("alert")).toContainText("Der Server lehnt den API-Token ab");

  // The stored token stays; a new one replaces it.
  await expect(page.getByLabel("API-Token")).toHaveAttribute("placeholder", /Gespeichert/);
  await page.getByLabel("API-Token").fill("zt_richtig");
  await page.getByRole("button", { name: "Speichern und verbinden" }).click();

  await expect(page.locator(".app")).toBeVisible();
  await expect(row(page, "Vom Server geladen")).toBeVisible();
  const setConfig = await page.evaluate(() => window.__ZUNDER_FAKE_APP__.calls.filter((c) => c.cmd === "set_config").map((c) => c.args));
  expect(setConfig[0]).toEqual({ update: { server_url: "https://zunder.example.org/", api_token: "zt_falsch" } });
  expect(await calls(page)).toContain("sync_now");
  expect(apiRequests).toEqual([]);
});

test("App: Anmeldung im Browser statt Token", async ({ page }) => {
  const seed = serverOps(currentFire());
  const apiRequests = await openInApp(page, seed.ops);
  await page.getByLabel("Server-Adresse").fill("https://zunder.example.org");
  await page.getByLabel("Gerätename").fill("Testgerät");
  await page.getByRole("button", { name: "Im Browser anmelden" }).click();
  await expect(page.getByText("Melde dich im Browser an und erlaube")).toBeVisible();
  const started = await page.evaluate(() => window.__ZUNDER_FAKE_APP__.calls.filter((c) => c.cmd === "login_start").map((c) => c.args));
  expect(started).toEqual([{ server_url: "https://zunder.example.org", device_name: "Testgerät", discard_pending: false }]);

  await page.evaluate(() => window.__ZUNDER_FAKE_APP__.completeLogin());
  await expect(page.locator(".app")).toBeVisible();
  await expect(row(page, "Vom Server geladen")).toBeVisible();
  expect(apiRequests).toEqual([]);
});

test("App: Änderungen gehen an den lokalen Speicher, Sync-Ereignisse aktualisieren die Sicht", async ({ page }) => {
  const fire = currentFire();
  const seed = serverOps(fire);
  const apiRequests = await openInApp(page, seed.ops);
  // Configure through the UI once, then work in the app.
  await page.getByLabel("Server-Adresse").fill("https://zunder.example.org");
  await page.getByLabel("API-Token").fill("zt_richtig");
  await page.getByRole("button", { name: "Speichern und verbinden" }).click();
  await expect(row(page, "Vom Server geladen")).toBeVisible();

  // Quick capture → local_apply with HLCs of the app's node.
  await page.keyboard.press("n");
  await page.getByLabel("Titel der neuen Aufgabe").fill("In der App erfasst");
  await page.keyboard.press("Enter");
  await expect(row(page, "In der App erfasst")).toBeVisible();
  const applied = await page.evaluate(() => window.__ZUNDER_FAKE_APP__.calls.filter((c) => c.cmd === "local_apply").flatMap((c) => c.args.ops as { hlc: string }[]));
  expect(applied.length).toBeGreaterThan(0);
  expect(applied.every((o) => o.hlc.endsWith(":app-0a1b2c3d4e5f"))).toBe(true);
  await expect(page.locator(".sidebar .sync")).toContainText("Synchronisiert");

  // The app's background sync pulled a rename: zunder://changed → reload.
  await page.evaluate(() => {
    const fake = window.__ZUNDER_FAKE_APP__;
    const task = fake.ops.find((o) => o.value === "Vom Server geladen")!;
    fake.serverOp("task", task.entity_id, "title", "Auf einem anderen Gerät umbenannt");
    fake.pending = 0;
    fake.emit("zunder://changed", { cursor: 99, pending: 0 });
    fake.emit("zunder://sync", { ok: true, report: { pushed: 2, rejected: [], warnings: [], pulled: 1, cursor: 99, pending: 0, changed: true } });
  });
  await expect(row(page, "Auf einem anderen Gerät umbenannt")).toBeVisible();
  await expect(page.locator(".sidebar .sync .sync-label")).toHaveText("Synchron");

  // A rejected op reported by the app's sync shows up as a toast.
  await page.evaluate(() =>
    window.__ZUNDER_FAKE_APP__.emit("zunder://sync", {
      ok: true,
      report: { pushed: 0, rejected: [{ op_id: "x", code: "invalid_reference", message: "Der Bereich existiert nicht mehr." }], warnings: [], pulled: 0, cursor: 99, pending: 0, changed: false },
    }),
  );
  await expect(page.locator(".toast", { hasText: "Der Bereich existiert nicht mehr." })).toBeVisible();

  // Offline: the indicator says so.
  await page.evaluate(() => window.__ZUNDER_FAKE_APP__.emit("zunder://sync", { ok: false, error: { code: "network", message: "Server nicht erreichbar: dns" } }));
  await expect(page.locator(".sidebar .sync")).toContainText("Offline");

  // Flames go through the app's commands; the server's flame fields arrive via zunder://changed.
  await go(page, "#/phoenix");
  const button = page.locator(`[data-flame-for="${seed.flameTask}"]`);
  await expect(button).toHaveText(/Flamme starten/);
  await button.click();
  await expect(button).toHaveText(/Flamme beenden/);
  const flameCall = await page.evaluate(() => window.__ZUNDER_FAKE_APP__.calls.find((c) => c.cmd === "flame_start")?.args);
  expect(flameCall).toEqual({ task_id: seed.flameTask });

  // History via task_history.
  await go(page, "#/inbox");
  await row(page, "Auf einem anderen Gerät umbenannt").click();
  await expect(page.locator(".history-list li").first()).toContainText("API · app");

  // Connection view instead of token management.
  await go(page, "#/verbindung");
  await expect(page.getByRole("heading", { name: "Verbindung" })).toBeVisible();
  await expect(page.getByTestId("last-sync")).toContainText("Zuletzt abgeglichen");
  await page.getByRole("button", { name: "Jetzt abgleichen" }).click();
  await expect(page.locator(".toast", { hasText: "Abgeglichen." })).toBeVisible();

  expect(apiRequests).toEqual([]);
});
