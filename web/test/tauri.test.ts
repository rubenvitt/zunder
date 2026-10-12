import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  AppCommandError,
  EVENTS,
  TauriApp,
  TauriTransport,
  appErrorCode,
  tauriLocalStore,
  toAppError,
  type AppConfig,
  type ConfigUpdate,
  type LocalSnapshot,
  type SyncEvent,
  type TauriBackend,
} from "../src/platform/tauri";
import { isTauri } from "../src/platform/runtime";
import { SyncEngine } from "../src/sync/engine";
import { mergeOps } from "../src/sync/merge";
import { toTask } from "../src/sync/model";
import { isTransient, isUnauthorized, NetworkError } from "../src/sync/transport";
import type { Op, Rejection } from "../src/sync/types";
import { op } from "./helpers";

type Args = Record<string, unknown> | undefined;

/**
 * Stand-in for the Tauri app: the command and event contract of
 * apps/zunder-app (local store with confirmed + pending ops) without a webview.
 */
class FakeApp implements TauriBackend {
  calls: { cmd: string; args: Args }[] = [];
  ops: Op[] = [];
  pending = 0;
  cursor = 3;
  config: AppConfig = {
    server_url: null,
    has_api_token: false,
    node_id: "app-0123456789ab",
    cursor: 0,
    pending: 0,
    last_sync: null,
  };
  /** Local validation of local_apply: return a message to reject an op. */
  reject: (op: Op) => string | null = () => null;
  /** Errors to throw for the next call of a command (raw, as Tauri would). */
  failures = new Map<string, unknown>();
  private listeners = new Map<string, Set<(event: { payload: unknown }) => void>>();

  async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    this.calls.push({ cmd, args });
    if (this.failures.has(cmd)) {
      const failure = this.failures.get(cmd);
      this.failures.delete(cmd);
      throw failure;
    }
    return this.handle(cmd, args) as T;
  }

  private handle(cmd: string, args: Args): unknown {
    switch (cmd) {
      case "local_snapshot":
        return this.snapshot();
      case "local_apply": {
        const ops = (args?.ops ?? []) as Op[];
        const rejected: Rejection[] = [];
        const accepted: string[] = [];
        for (const o of ops) {
          const reason = this.reject(o);
          if (reason) rejected.push({ op_id: o.op_id, code: "invalid_value", message: reason });
          else {
            accepted.push(o.op_id);
            this.ops.push(o);
            this.pending += 1;
          }
        }
        return { accepted, rejected, warnings: [], pending: this.pending };
      }
      case "get_config":
        return { ...this.config, pending: this.pending };
      case "set_config": {
        const update = args?.update as ConfigUpdate;
        if (update.server_url !== undefined) this.config.server_url = update.server_url;
        if (update.api_token !== undefined) this.config.has_api_token = update.api_token !== null;
        return { ...this.config, pending: this.pending };
      }
      case "sync_now": {
        const report = { pushed: this.pending, rejected: [], warnings: [], pulled: 0, cursor: this.cursor, pending: 0, changed: false };
        this.pending = 0;
        this.config.last_sync = { at_ms: Date.now(), ok: true };
        this.emit(EVENTS.sync, { ok: true, report } satisfies SyncEvent);
        return report;
      }
      case "task_history":
        return [{ seq: 1, op: this.ops[0], source: "web", received_at: "2026-10-11T08:00:00Z" }];
      case "flame_start":
      case "flame_stop":
        return { running: cmd === "flame_start" };
      default:
        throw `command ${cmd} not found`;
    }
  }

  snapshot(): LocalSnapshot {
    const store = mergeOps(this.ops);
    return {
      cursor: this.cursor,
      pending: this.pending,
      entities: [...store.values()].map((e) => ({
        entity: e.entity,
        entity_id: e.id,
        created_hlc: e.created_hlc,
        fields: Object.fromEntries(Object.entries(e.fields).map(([k, f]) => [k, { value: f.value, hlc: f.hlc, op_id: f.op_id }])),
      })),
    };
  }

  async listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<() => void> {
    const set = this.listeners.get(event) ?? new Set();
    this.listeners.set(event, set);
    const h = handler as (event: { payload: unknown }) => void;
    set.add(h);
    return () => set.delete(h);
  }

  emit(event: string, payload: unknown): void {
    for (const h of this.listeners.get(event) ?? []) h({ payload });
  }

  listenerCount(event: string): number {
    return this.listeners.get(event)?.size ?? 0;
  }

  commands(): string[] {
    return this.calls.map((c) => c.cmd);
  }
}

const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe("runtime detection", () => {
  afterEach(() => {
    delete (globalThis as { window?: unknown }).window;
  });

  it("recognises the Tauri webview by __TAURI_INTERNALS__", () => {
    expect(isTauri()).toBe(false);
    (globalThis as { window?: unknown }).window = {};
    expect(isTauri()).toBe(false);
    (globalThis as { window?: unknown }).window = { __TAURI_INTERNALS__: { invoke() {} } };
    expect(isTauri()).toBe(true);
  });
});

describe("error mapping", () => {
  it("maps app error codes onto the engine's error types", () => {
    const network = toAppError({ code: "network", message: "Server nicht erreichbar: timeout" });
    expect(network).toBeInstanceOf(NetworkError);
    expect(isTransient(network)).toBe(true);
    expect(appErrorCode(network)).toBe("network");

    const unauthorized = toAppError({ code: "unauthorized", message: "Der Server lehnt den API-Token ab" });
    expect(isUnauthorized(unauthorized)).toBe(true);
    expect(appErrorCode(unauthorized)).toBe("unauthorized");

    const pending = toAppError({ code: "pending_ops", message: "2 lokale Änderungen …" });
    expect(pending).toBeInstanceOf(AppCommandError);
    expect(appErrorCode(pending)).toBe("pending_ops");
    expect(isTransient(pending)).toBe(false);

    const plain = toAppError("command flame_start not found");
    expect(appErrorCode(plain)).toBe("app");
    expect(plain.message).toBe("command flame_start not found");
  });
});

describe("TauriTransport", () => {
  let fake: FakeApp;
  let app: TauriApp;
  let pendingSeen: number[];
  let transport: TauriTransport;

  beforeEach(async () => {
    fake = new FakeApp();
    fake.ops.push(op("task", "t1", "title", "Lokal gespeichert", 1000, { node: "server" }));
    app = await TauriApp.connect(fake);
    pendingSeen = [];
    transport = new TauriTransport(app, { pending: (n) => void pendingSeen.push(n) });
  });

  afterEach(() => app.close());

  it("listens to both app events once connected", () => {
    expect(fake.listenerCount(EVENTS.changed)).toBe(1);
    expect(fake.listenerCount(EVENTS.sync)).toBe(1);
    app.close();
    expect(fake.listenerCount(EVENTS.changed)).toBe(0);
  });

  it("reads the local snapshot in the server's snapshot shape", async () => {
    fake.pending = 2;
    const snapshot = await transport.snapshot();
    expect(snapshot.cursor).toBe(3);
    expect(snapshot).not.toHaveProperty("pending");
    expect(snapshot.entities[0]!.fields.title!.value).toBe("Lokal gespeichert");
    expect(pendingSeen).toEqual([2]);
  });

  it("pushes through local_apply and reports the app's pending count", async () => {
    const ok = op("task", "n1", "title", "Neu", 2000, { node: "app-0123456789ab" });
    const bad = op("task", "n2", "title", "", 2001, { node: "app-0123456789ab" });
    fake.reject = (o) => (o.entity_id === "n2" ? "Titel fehlt" : null);
    const res = await transport.push({ client_id: "app-0123456789ab", ops: [ok, bad] });
    expect(fake.calls.at(-1)).toEqual({ cmd: "local_apply", args: { ops: [ok, bad] } });
    expect(res.accepted).toEqual([ok.op_id]);
    expect(res.rejected.map((r) => r.op_id)).toEqual([bad.op_id]);
    expect(pendingSeen).toEqual([1]);
  });

  it("has nothing to pull: changes arrive as zunder://changed", async () => {
    expect(await transport.pull(7)).toEqual({ ops: [], cursor: 7, has_more: false });
    expect(fake.calls).toHaveLength(0);

    const events: string[] = [];
    const off = transport.subscribe({
      onCursor: () => events.push("cursor"),
      onConnection: (c) => events.push(`connection:${c}`),
      onReset: () => events.push("reset"),
    });
    fake.emit(EVENTS.changed, { cursor: 9, pending: 4 });
    expect(events).toEqual(["connection:true", "reset"]);
    expect(pendingSeen).toEqual([4]);
    off();
    fake.emit(EVENTS.changed, { cursor: 10, pending: 0 });
    expect(events).toHaveLength(2);
  });

  it("calls history and flames through app commands with snake_case arguments", async () => {
    const entries = await transport.history("t1");
    expect(entries[0]!.source).toBe("web");
    await transport.flame("t1", "start");
    await transport.flame("t1", "stop");
    expect(fake.calls).toEqual([
      { cmd: "task_history", args: { task_id: "t1" } },
      { cmd: "flame_start", args: { task_id: "t1" } },
      { cmd: "flame_stop", args: { task_id: "t1" } },
    ]);
  });

  it("surfaces server errors of proxied calls with their German message", async () => {
    fake.failures.set("flame_stop", { code: "server", message: "Server-Fehler (409): Keine laufende Flamme" });
    await expect(transport.flame("t1", "stop")).rejects.toThrow("Server-Fehler (409): Keine laufende Flamme");
    fake.failures.set("task_history", { code: "network", message: "Server nicht erreichbar: dns" });
    await expect(transport.history("t1")).rejects.toBeInstanceOf(NetworkError);
  });

  it("refuses token management (needs a browser session)", async () => {
    await expect(transport.listTokens()).rejects.toThrow("Web-Oberfläche");
    await expect(transport.createToken("x")).rejects.toBeInstanceOf(AppCommandError);
    await expect(transport.revokeToken("x")).rejects.toBeInstanceOf(AppCommandError);
    await expect(transport.me()).resolves.toMatchObject({ user_id: "app" });
  });

  it("uses the app's node id for the engine's clock", async () => {
    const store = tauriLocalStore(await app.getConfig());
    expect(store.nodeId()).toBe("app-0123456789ab");
    expect(store.persistsState).toBe(false);
  });
});

describe("SyncEngine on top of the Tauri app", () => {
  let fake: FakeApp;
  let app: TauriApp;
  let engine: SyncEngine;
  let rejections: Rejection[][];

  const titleOf = (id: string) => {
    const e = engine.view.get("task", id);
    return e ? toTask(e).title : undefined;
  };

  beforeEach(async () => {
    fake = new FakeApp();
    fake.ops.push(op("task", "t1", "title", "Vom Gerät", 1000, { node: "server" }));
    app = await TauriApp.connect(fake);
    rejections = [];
    engine = new SyncEngine(new TauriTransport(app), tauriLocalStore(fake.config), {
      changed: () => {},
      rejected: (r) => void rejections.push([...r]),
    });
  });

  afterEach(() => {
    engine.stop();
    app.close();
  });

  it("starts from the local snapshot and stamps ops with the app's node", async () => {
    expect(await engine.start()).toBe(true);
    expect(titleOf("t1")).toBe("Vom Gerät");
    expect(engine.status).toBe("online");
    engine.change([{ entity: "task", id: "n1", fields: { title: "In der App erfasst" } }]);
    await flush();
    const apply = fake.calls.find((c) => c.cmd === "local_apply");
    const ops = apply?.args?.ops as Op[];
    expect(ops.map((o) => o.field)).toEqual(["title"]);
    expect(ops[0]!.hlc.endsWith(":app-0123456789ab")).toBe(true);
    expect(engine.pendingOps).toHaveLength(0);
    expect(fake.commands()).not.toContain("sync_now");
  });

  it("rolls back what the app rejects locally", async () => {
    await engine.start();
    fake.reject = () => "Ungültig.";
    engine.change([{ entity: "task", id: "t1", fields: { title: "Lokal geändert" } }]);
    expect(titleOf("t1")).toBe("Lokal geändert");
    await flush();
    await flush();
    expect(titleOf("t1")).toBe("Vom Gerät");
    expect(rejections.flat().map((r) => r.message)).toEqual(["Ungültig."]);
  });

  it("reloads the snapshot on zunder://changed (new data and app-side rollbacks)", async () => {
    await engine.start();
    engine.change([{ entity: "task", id: "n2", fields: { title: "Wird abgelehnt" } }]);
    await flush();
    expect(titleOf("n2")).toBe("Wird abgelehnt");

    // The app's sync pulled a rename and the server rejected n2 (rolled back locally).
    fake.ops = fake.ops.filter((o) => o.entity_id !== "n2");
    fake.ops.push(op("task", "t1", "title", "Vom Server umbenannt", 5_000_000_000_000, { node: "server" }));
    fake.cursor = 8;
    fake.emit(EVENTS.changed, { cursor: 8, pending: 0 });
    await flush();
    await flush();
    expect(titleOf("t1")).toBe("Vom Server umbenannt");
    expect(titleOf("n2")).toBeUndefined();
    expect(engine.cursor).toBe(8);
    // The clock moved past the reloaded state.
    expect(engine.clock.now() > "5000000000000:00000:server").toBe(true);
  });

  it("does not let a stale snapshot hide a change applied meanwhile", async () => {
    await engine.start();
    // local_snapshot answers with the state from before the local_apply below.
    const stale = fake.snapshot();
    let release: () => void = () => {};
    const original = fake.invoke.bind(fake);
    let first = true;
    fake.invoke = async <T,>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
      if (cmd === "local_snapshot" && first) {
        first = false;
        fake.calls.push({ cmd, args });
        await new Promise<void>((resolve) => (release = resolve));
        return stale as T;
      }
      return original<T>(cmd, args);
    };
    fake.emit(EVENTS.changed, { cursor: 3, pending: 0 });
    engine.change([{ entity: "task", id: "n3", fields: { title: "Während des Neuladens" } }]);
    await flush();
    release();
    await flush();
    await flush();
    expect(titleOf("n3")).toBe("Während des Neuladens");
  });
});
