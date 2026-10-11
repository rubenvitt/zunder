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
    const rejected: Rejection[] = [];
    const accepted: string[] = [];
    for (const o of req.ops) {
      const reason = this.reject(o);
      if (reason) rejected.push({ op_id: o.op_id, code: "invalid", message: reason });
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
    expect(issues.map((i) => i.code)).toEqual(["required"]);
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
});
