import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SyncEngine, type EngineListener } from "../src/sync/engine";
import { MemoryLocalStore } from "../src/sync/local-store";
import { mergeOps } from "../src/sync/merge";
import { toTask } from "../src/sync/model";
import { HttpError, NetworkError, type SubscriptionHandlers, type SyncTransport } from "../src/sync/transport";
import type { Op, PullResponse, PushRequest, PushResponse, Rejection, Snapshot } from "../src/sync/types";
import { op } from "./helpers";

/** In-memory server: op log with seq numbers, optional rejection rule, failure injection. */
class FakeServer implements SyncTransport {
  log: Op[] = [];
  reject: (op: Op) => string | null = () => null;
  failNext = 0;
  /** Refuses the whole request (e.g. 400 for a malformed op, 413 for a too large body). */
  refuseRequest: (req: PushRequest) => HttpError | null = () => null;
  unauthorized = false;
  handlers: SubscriptionHandlers | null = null;
  pushes: PushRequest[] = [];

  private guard() {
    if (this.unauthorized) throw new HttpError(401, "unauthorized", "Nicht angemeldet.");
    if (this.failNext > 0) {
      this.failNext -= 1;
      throw new NetworkError("offline");
    }
  }

  async snapshot(): Promise<Snapshot> {
    this.guard();
    const store = mergeOps(this.log);
    return {
      cursor: this.log.length,
      entities: [...store.values()].map((e) => ({
        entity: e.entity,
        entity_id: e.id,
        fields: Object.fromEntries(Object.entries(e.fields).map(([k, f]) => [k, { value: f.value, hlc: f.hlc }])),
      })),
    };
  }

  async push(req: PushRequest): Promise<PushResponse> {
    this.guard();
    this.pushes.push(req);
    const refused = this.refuseRequest(req);
    if (refused) throw refused;
    const rejected: Rejection[] = [];
    const accepted: string[] = [];
    // Core rule (contract 2.5): one invalid op rejects all ops of that entity in this push.
    const failed = new Map<string, string>();
    for (const o of req.ops) {
      const reason = this.reject(o);
      if (reason && !failed.has(o.entity_id)) failed.set(o.entity_id, reason);
    }
    for (const o of req.ops) {
      const reason = this.reject(o);
      if (reason) rejected.push({ op_id: o.op_id, code: "invalid", message: reason });
      else if (failed.has(o.entity_id)) rejected.push({ op_id: o.op_id, code: "entity_rejected", message: "Abgelehnt mit der Entität." });
      else {
        accepted.push(o.op_id);
        if (!this.log.some((l) => l.op_id === o.op_id)) this.log.push(o);
      }
    }
    return { accepted, rejected, warnings: [], cursor: this.log.length };
  }

  async pull(since: number, limit = 1000): Promise<PullResponse> {
    this.guard();
    const ops = this.log.slice(since, since + limit).map((o, i) => ({ seq: since + i + 1, op: o }));
    return { ops, cursor: since + ops.length, has_more: since + ops.length < this.log.length };
  }

  subscribe(handlers: SubscriptionHandlers): () => void {
    this.handlers = handlers;
    handlers.onConnection(true);
    return () => {
      this.handlers = null;
    };
  }

  /** Simulate another client writing. */
  external(o: Op) {
    this.log.push(o);
    this.handlers?.onCursor(this.log.length);
  }
}

const P1 = "01900000-0000-7000-8000-0000000000a1";
const A1 = "01900000-0000-7000-8000-0000000000b1";
const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));
const titleOf = (engine: SyncEngine, id: string) => {
  const e = engine.view.get("task", id);
  return e ? toTask(e).title : undefined;
};

describe("SyncEngine", () => {
  let server: FakeServer;
  let store: MemoryLocalStore;
  let listener: EngineListener;
  let engine: SyncEngine;
  const rejections: Rejection[][] = [];

  beforeEach(() => {
    server = new FakeServer();
    server.log.push(op("task", "t1", "title", "Vom Server", 1000, { node: "server" }));
    store = new MemoryLocalStore("web-test");
    rejections.length = 0;
    listener = { changed: () => {}, rejected: (r) => void rejections.push([...r]) };
    engine = new SyncEngine(server, store, listener, { backoffMs: [5], pollIntervalMs: 60_000 });
  });

  afterEach(() => engine.stop());

  it("loads the snapshot and raises the clock past it", async () => {
    expect(await engine.start()).toBe(true);
    expect(titleOf(engine, "t1")).toBe("Vom Server");
    expect(engine.cursor).toBe(1);
    expect(engine.clock.now() > "0000000001000:00000:server").toBe(true);
  });

  it("applies changes optimistically and pushes them as one group", async () => {
    await engine.start();
    const issues = engine.change([{ entity: "task", id: "n1", fields: { title: "Neu", status: "open" } }]);
    expect(issues).toEqual([]);
    expect(titleOf(engine, "n1")).toBe("Neu");
    await flush();
    expect(server.pushes).toHaveLength(1);
    expect(server.pushes[0]!.ops.map((o) => o.field)).toEqual(["title", "status"]);
    expect(server.pushes[0]!.client_id).toBe("web-test");
    expect(engine.pendingOps).toHaveLength(0);
    expect(store.pending).toEqual([]);
  });

  it("refuses invalid changes client-side without queueing", async () => {
    await engine.start();
    const issues = engine.change([{ entity: "task", id: "t1", fields: { title: "" } }]);
    expect(issues.map((i) => i.code)).toEqual(["invalid_value"]);
    expect(engine.pendingOps).toHaveLength(0);
    expect(titleOf(engine, "t1")).toBe("Vom Server");
  });

  it("rolls back rejected ops and keeps other pending ops", async () => {
    await engine.start();
    server.reject = (o) => (o.entity_id === "t1" ? "Nein." : null);
    engine.change([{ entity: "task", id: "t1", fields: { title: "Lokal" } }]);
    engine.change([{ entity: "task", id: "n2", fields: { title: "Bleibt" } }]);
    expect(titleOf(engine, "t1")).toBe("Lokal");
    await flush();
    await flush();
    expect(titleOf(engine, "t1")).toBe("Vom Server");
    expect(titleOf(engine, "n2")).toBe("Bleibt");
    expect(rejections.flat().map((r) => r.message)).toEqual(["Nein."]);
  });

  it("pulls remote changes on SSE cursor events", async () => {
    await engine.start();
    server.external(op("task", "t1", "title", "Von woanders", 5_000_000_000_000, { node: "server" }));
    await flush();
    await flush();
    expect(titleOf(engine, "t1")).toBe("Von woanders");
    expect(engine.cursor).toBe(2);
  });

  it("a pending local edit wins over an older remote op, rollback reveals it", async () => {
    await engine.start();
    server.failNext = 1; // the push fails, the op stays pending
    engine.change([{ entity: "task", id: "t1", fields: { title: "Lokal" } }]);
    await flush();
    server.external(op("task", "t1", "title", "Älter", 1500, { node: "server" }));
    await flush();
    await flush();
    expect(titleOf(engine, "t1")).toBe("Lokal");
  });

  it("retries pushes with backoff while offline and persists the queue", async () => {
    await engine.start();
    server.failNext = 2;
    engine.change([{ entity: "task", id: "n3", fields: { title: "Offline erfasst" } }]);
    await flush();
    expect(store.pending.flat()).toHaveLength(1);
    expect(engine.status).toBe("offline");
    await vi.waitFor(() => expect(server.log.some((o) => o.entity_id === "n3")).toBe(true), { timeout: 500, interval: 5 });
    await vi.waitFor(() => expect(engine.status).toBe("online"));
    expect(store.pending).toEqual([]);
  });

  it("restores the pending queue after a reload and re-pushes idempotently", async () => {
    const pendingOp = op("task", "n4", "title", "Vor dem Reload", 2000, { node: "web-test" });
    store.pending = [[pendingOp]];
    await engine.start();
    expect(titleOf(engine, "n4")).toBe("Vor dem Reload");
    await flush();
    expect(server.log.filter((o) => o.op_id === pendingOp.op_id)).toHaveLength(1);
    expect(engine.clock.now() > pendingOp.hlc).toBe(true);
  });

  it("reports unauthorized at start", async () => {
    server.unauthorized = true;
    expect(await engine.start()).toBe(false);
    expect(engine.status).toBe("unauthorized");
  });

  it("splits large queues into batches at group boundaries", async () => {
    engine.stop();
    engine = new SyncEngine(server, store, listener, { maxBatchOps: 3, pollIntervalMs: 60_000 });
    store.pending = [
      [op("task", "a", "title", "A", 2000), op("task", "a", "status", "open", 2001)],
      [op("task", "b", "title", "B", 2002), op("task", "b", "status", "open", 2003)],
    ];
    await engine.start();
    await vi.waitFor(() => expect(server.pushes).toHaveLength(2));
    expect(server.pushes.map((p) => p.ops.length)).toEqual([2, 2]);
  });

  it("never puts two groups that touch the same entity into one push", async () => {
    server.log.push(op("project", P1, "name", "Projekt", 1001, { node: "server" }));
    server.log.push(op("area", A1, "name", "Bereich", 999, { node: "server" }));
    server.log.push(op("project", P1, "area_id", A1, 1002, { node: "server" }));
    await engine.start();
    server.failNext = 1; // offline: both groups queue up
    engine.change([{ entity: "task", id: "t1", fields: { title: "Neu" } }]);
    engine.change([{ entity: "task", id: "n9", fields: { title: "Andere Aufgabe" } }]);
    expect(engine.change([{ entity: "task", id: "t1", fields: { location: { kind: "project", id: P1 } } }])).toEqual([]);
    await flush();
    expect(engine.status).toBe("offline");
    // Meanwhile another device deletes the project: the move becomes invalid.
    server.reject = (o) => (o.field === "location" ? "Das Projekt gibt es nicht mehr." : null);
    engine.retryNow();
    await vi.waitFor(() => expect(engine.pendingOps).toHaveLength(0));
    expect(server.pushes.slice(-2).map((p) => p.ops.map((o) => `${o.entity_id}.${o.field}`))).toEqual([
      ["t1.title", "n9.title"],
      ["t1.location"],
    ]);
    expect(rejections.flat().map((r) => r.code)).toEqual(["invalid"]);
    // The rename was valid on its own and survives on the server and in the view.
    expect(mergeOps(server.log).get("task", "t1")!.fields.title!.value).toBe("Neu");
    expect(titleOf(engine, "t1")).toBe("Neu");
  });

  it("a whole-request refusal only drops the group that fails on its own", async () => {
    await engine.start();
    server.failNext = 1;
    engine.change([{ entity: "task", id: "g1", fields: { title: "Eins" } }]);
    engine.change([{ entity: "task", id: "g2", fields: { title: "Kaputt" } }]);
    engine.change([{ entity: "task", id: "g3", fields: { title: "Drei" } }]);
    await flush();
    server.refuseRequest = (req) => (req.ops.some((o) => o.value === "Kaputt") ? new HttpError(400, "invalid_request", "Ungültige Anfrage.") : null);
    engine.retryNow();
    await vi.waitFor(() => expect(engine.pendingOps).toHaveLength(0));
    expect(titleOf(engine, "g1")).toBe("Eins");
    expect(titleOf(engine, "g2")).toBeUndefined();
    expect(titleOf(engine, "g3")).toBe("Drei");
    expect(server.log.map((o) => o.entity_id).filter((id) => id.startsWith("g"))).toEqual(["g1", "g3"]);
    expect(rejections.flat().map((r) => r.code)).toEqual(["invalid_request"]);
  });

  it("splits batches by size and replaces lone surrogates before queueing", async () => {
    engine.stop();
    engine = new SyncEngine(server, store, listener, { maxBatchBytes: 600, pollIntervalMs: 60_000 });
    await engine.start();
    server.failNext = 1;
    engine.change([{ entity: "task", id: "s1", fields: { title: "a\ud800b", description: "x".repeat(300) } }]);
    engine.change([{ entity: "task", id: "s2", fields: { title: "Zwei", description: "y".repeat(300) } }]);
    await flush();
    engine.retryNow();
    await vi.waitFor(() => expect(engine.pendingOps).toHaveLength(0));
    expect(server.pushes.slice(-2).map((p) => p.ops.map((o) => o.entity_id))).toEqual([
      ["s1", "s1"],
      ["s2", "s2"],
    ]);
    expect(titleOf(engine, "s1")).toBe("a\ufffdb");
    expect(JSON.stringify(server.pushes.at(-2))).not.toContain("\\ud800");
  });
});
