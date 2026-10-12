import { describe, expect, it, vi } from "vitest";
import { SyncEngine } from "../src/sync/engine";
import { BrowserLocalStore, type PageLocks } from "../src/sync/local-store";
import { mergeOps } from "../src/sync/merge";
import { NetworkError, type SubscriptionHandlers, type SyncTransport } from "../src/sync/transport";
import type { Op, PullResponse, PushRequest, PushResponse, Snapshot } from "../src/sync/types";

/** localStorage stand-in shared by all "tabs" of one browser. */
class FakeStorage implements Storage {
  private map = new Map<string, string>();
  get length() {
    return this.map.size;
  }
  key(i: number) {
    return [...this.map.keys()][i] ?? null;
  }
  getItem(key: string) {
    return this.map.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.map.set(key, value);
  }
  removeItem(key: string) {
    this.map.delete(key);
  }
  clear() {
    this.map.clear();
  }
  keys() {
    return [...this.map.keys()];
  }
}

/** Web Locks stand-in: a lock is held until its page is "closed". */
class FakeLocks implements PageLocks {
  held = new Set<string>();
  async request(name: string): Promise<unknown> {
    this.held.add(name);
    return new Promise(() => {});
  }
  async query() {
    return { held: [...this.held].map((name) => ({ name })) };
  }
  close(store: BrowserLocalStore) {
    const page = store.nodeId().split("-").at(-1)!;
    this.held.delete(`zunder-page-${page}`);
  }
}

class Server implements SyncTransport {
  log: Op[] = [];
  online = false;
  pushes: PushRequest[] = [];
  async snapshot(): Promise<Snapshot> {
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
    if (!this.online) throw new NetworkError("offline");
    this.pushes.push(req);
    for (const o of req.ops) if (!this.log.some((l) => l.op_id === o.op_id)) this.log.push(o);
    return { accepted: req.ops.map((o) => o.op_id), rejected: [], warnings: [], cursor: this.log.length };
  }
  async pull(since: number): Promise<PullResponse> {
    const ops = this.log.slice(since).map((o, i) => ({ seq: since + i + 1, op: o }));
    return { ops, cursor: since + ops.length, has_more: false };
  }
  subscribe(handlers: SubscriptionHandlers): () => void {
    handlers.onConnection(true);
    return () => {};
  }
  titles(): string[] {
    return this.log.filter((o) => o.field === "title").map((o) => String(o.value));
  }
}

const TASK = "v00070008000000000000001";
const tick = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

function tab(server: Server, store: BrowserLocalStore) {
  return new SyncEngine(server, store, { changed: () => {} }, { backoffMs: [60_000], pollIntervalMs: 60_000 });
}

const variants: [string, () => FakeLocks | null][] = [
  ["Web Locks", () => new FakeLocks()],
  ["heartbeats", () => null],
];

describe.each(variants)("BrowserLocalStore across tabs (%s)", (_, makeLocks) => {
  it("keeps the unpushed edits of a closed tab and pushes them from the next one", async () => {
    const storage = new FakeStorage();
    const locks = makeLocks();
    const server = new Server();
    server.log.push({ op_id: "v00070008000000000000000", entity: "task", entity_id: TASK, field: "title", value: "Alt", hlc: "0000000001000:00000:server" });

    const storeA = new BrowserLocalStore({ storage, locks });
    const storeB = new BrowserLocalStore({ storage, locks });
    expect(storeA.nodeId()).not.toBe(storeB.nodeId());
    expect(storeA.nodeId()).toMatch(/^web-[0-9a-f]{12}-[0-9a-f]{8}$/);
    // Both pages belong to the same browser.
    expect(storeA.nodeId().slice(0, 16)).toBe(storeB.nodeId().slice(0, 16));

    const a = tab(server, storeA);
    const b = tab(server, storeB);
    await a.start();
    await b.start();
    expect(a.change([{ entity: "task", id: TASK, fields: { title: "Edit in Tab A" } }])).toEqual([]);
    expect(b.change([{ entity: "task", id: TASK, fields: { description: "Edit in Tab B" } }])).toEqual([]);
    await tick();

    // Tab A is closed before its push went through; tab B stays open.
    a.stop();
    if (locks) locks.close(storeA);
    else for (const key of storage.keys()) if (key.startsWith("zunder.alive.v1.") && key.endsWith(storeA.nodeId().split("-").at(-1)!)) storage.setItem(key, "0");
    server.online = true;

    // The next tab that starts adopts A's queue (B is alive, its queue stays with B).
    const storeC = new BrowserLocalStore({ storage, locks });
    const c = tab(server, storeC);
    await c.start();
    await vi.waitFor(() => expect(server.titles()).toContain("Edit in Tab A"));
    expect(server.log.some((o) => o.value === "Edit in Tab B")).toBe(false);
    b.retryNow();
    await vi.waitFor(() => expect(server.log.some((o) => o.value === "Edit in Tab B")).toBe(true));
    await vi.waitFor(() => expect(storage.keys().filter((k) => k.startsWith("zunder.pending."))).toEqual([]));
    b.stop();
    c.stop();
  });

  it("a long-lived tab adopts the queue of a tab closed meanwhile", async () => {
    const storage = new FakeStorage();
    const locks = makeLocks();
    const server = new Server();
    const storeA = new BrowserLocalStore({ storage, locks });
    const storeB = new BrowserLocalStore({ storage, locks });
    const a = tab(server, storeA);
    const b = tab(server, storeB);
    await a.start();
    await b.start();
    a.change([{ entity: "task", id: TASK, fields: { title: "Nur in A" } }]);
    await tick();
    a.stop();
    if (locks) locks.close(storeA);
    else for (const key of storage.keys()) if (key.startsWith("zunder.alive.v1.") && key.endsWith(storeA.nodeId().split("-").at(-1)!)) storage.setItem(key, "0");
    server.online = true;
    b.retryNow(); // e.g. back online / visible again
    await vi.waitFor(() => expect(server.titles()).toEqual(["Nur in A"]));
    b.stop();
  });

  it("adopts the queue of the former single-key format", async () => {
    const storage = new FakeStorage();
    const legacy: Op = { op_id: "v0007000800000000000000a", entity: "task", entity_id: TASK, field: "title", value: "Von früher", hlc: "0000000002000:00000:web-aaaaaaaaaaaa" };
    storage.setItem("zunder.pending.v1", JSON.stringify([[legacy]]));
    const store = new BrowserLocalStore({ storage, locks: makeLocks() });
    expect(await store.loadPending()).toEqual([[legacy]]);
    expect(storage.getItem("zunder.pending.v1")).toBeNull();
    expect(await store.loadPending()).toEqual([[legacy]]);
  });
});
