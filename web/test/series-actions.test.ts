// Recurring tasks through the UI's actions (contract 5.1, "Rhythmen"): every way of
// completing goes through the same path (hint "Nächste Instanz am …"), ending a series
// writes its ops in an order that survives split pushes, and the change history names
// the previous instance. Runs the real store and sync engine against an in-memory server
// that creates next instances per push like the real one (contract 3.1, simplified).

import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { describeValue, historyTaskRefs } from "../src/labels";
import { clearDue, endSeries, placeTask, setStatus } from "../src/state/actions";
import { NEXT_INSTANCE_WAIT_MS } from "../src/state/series";
import { configureServices, data, getEngine, startSync, taskIndex } from "../src/state/store";
import { dismissToast, dropRequest, setDropRequest, toasts } from "../src/state/ui";
import { isClosed } from "../src/sync/derived";
import { newId } from "../src/sync/ids";
import { MemoryLocalStore } from "../src/sync/local-store";
import { mergeOps } from "../src/sync/merge";
import { toTask } from "../src/sync/model";
import type { SubscriptionHandlers, SyncTransport } from "../src/sync/transport";
import type { JsonValue, Op, PullResponse, PushRequest, PushResponse, Snapshot, Task } from "../src/sync/types";
import { validateOps } from "../src/sync/validate";
import { op } from "./helpers";

const MONTHLY = { every: 1, unit: "month", day: "last", from: "due" } as const;
const NEXT_DUE = "2099-02-28";

class SeriesServer implements SyncTransport {
  log: Op[] = [];
  pushes: Op[][] = [];
  /** Whether a push that completes a recurring task creates the next instance right away. */
  createsNext = true;
  private handlers: SubscriptionHandlers | null = null;

  private tasks(ops: Op[] = this.log): Map<string, Task> {
    const out = new Map<string, Task>();
    for (const e of mergeOps(ops).values()) if (e.entity === "task") out.set(e.id, toTask(e));
    return out;
  }

  /** Pushes the server's next instance of `prevId` (as the real server would after a completion). */
  next(prevId: string, due = NEXT_DUE): string {
    const prev = this.tasks().get(prevId)!;
    const id = newId();
    const wall = Date.now();
    const fields: [string, JsonValue][] = [
      ["title", prev.title],
      ["location", { kind: "loose" }],
      ["due", due],
      ["recurrence", { ...MONTHLY }],
      ["recurrence_prev", prevId],
    ];
    for (const [field, value] of fields) this.log.push(op("task", id, field, value, wall, { node: "server" }));
    this.handlers?.onCursor(this.log.length);
    return id;
  }

  async snapshot(): Promise<Snapshot> {
    return {
      cursor: this.log.length,
      entities: [...mergeOps(this.log).values()].map((e) => ({
        entity: e.entity,
        entity_id: e.id,
        fields: Object.fromEntries(Object.entries(e.fields).map(([k, f]) => [k, { value: f.value, hlc: f.hlc }])),
      })),
    };
  }

  async push(req: PushRequest): Promise<PushResponse> {
    this.pushes.push([...req.ops]);
    this.apply(req.ops);
    return { accepted: req.ops.map((o) => o.op_id), rejected: [], warnings: [], cursor: this.log.length };
  }

  /** Stores ops as one push and creates next instances for the series it completed. */
  apply(ops: readonly Op[]): void {
    const before = this.tasks();
    this.log.push(...ops);
    if (!this.createsNext) return;
    const after = this.tasks();
    const live = [...after.values()].filter((t) => !t.deleted);
    for (const id of new Set(ops.filter((o) => o.entity === "task" && o.field === "status").map((o) => o.entity_id))) {
      const was = before.get(id);
      const t = after.get(id)!;
      const completed = isClosed(t.status) && !(was && isClosed(was.status));
      if (!completed || t.deleted || !t.recurrence || !t.due || t.parent_id) continue;
      if (live.some((o) => o.recurrence_prev === id)) continue;
      this.next(id);
    }
  }

  async pull(since: number, limit = 1000): Promise<PullResponse> {
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
}

const server = new SeriesServer();
const settle = async () => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};
const hints = () => toasts().filter((t) => t.message.startsWith("Nächste Instanz"));
const successors = (id: string) => [...server["tasks"]().values()].filter((t) => t.recurrence_prev === id && !t.deleted);

/** Creates a task through the engine and waits until the server has it. */
async function seed(fields: Record<string, JsonValue>): Promise<string> {
  const id = newId();
  expect(getEngine().change([{ entity: "task", id, fields: { location: { kind: "loose" }, ...fields } }])).toEqual([]);
  await vi.waitFor(() => expect(getEngine().pendingOps).toHaveLength(0));
  return id;
}

const recurring = (title: string) => seed({ title, due: "2099-01-31", recurrence: { ...MONTHLY } });

/** The fields of the last push that touched `id`, in oplog order. */
const lastPushFields = (id: string) =>
  server.pushes
    .filter((p) => p.some((o) => o.entity_id === id))
    .at(-1)!
    .filter((o) => o.entity_id === id)
    .map((o) => o.field);

beforeAll(async () => {
  configureServices({ transport: server, localStore: new MemoryLocalStore("web-series") });
  expect(await startSync()).toBe(true);
});

beforeEach(() => {
  for (const t of toasts()) dismissToast(t.id);
});

afterEach(() => {
  vi.useRealTimers();
  server.createsNext = true;
  setDropRequest(null);
});

describe("completing on the board", () => {
  it("dragging to Erledigt writes status and rank together and announces the next instance", async () => {
    const id = await recurring("Odoo Board");
    expect(placeTask(id, null, null, "done")).toBe(true);
    expect(lastPushFields(id).sort()).toEqual(["rank", "status"]);
    await vi.waitFor(() => expect(hints()).toHaveLength(1));
    expect(hints().at(-1)!.message).toBe("Nächste Instanz am Sa, 28.02.2099");
    expect(successors(id)).toHaveLength(1);
  });

  it("only reordering writes the rank and no status", async () => {
    const id = await seed({ title: "Nur Rang" });
    expect(placeTask(id, null, null, "open")).toBe(true);
    expect(lastPushFields(id)).toEqual(["rank"]);
    expect(placeTask(id, null, null)).toBe(true);
    expect(lastPushFields(id)).toEqual(["rank"]);
  });

  it("dropping a recurring task asks first and writes nothing yet", async () => {
    const id = await recurring("Verwerfen auf dem Board");
    const pushes = server.pushes.length;
    expect(placeTask(id, null, null, "dropped")).toBe(false);
    expect(dropRequest()?.taskId).toBe(id);
    await settle();
    expect(server.pushes.length).toBe(pushes);
    expect(data.tasks[id]!.status).toBe("open");
  });
});

describe("ending a series", () => {
  /** A fresh server holding only the given task, fed one op per push (the app's batches split anywhere). */
  function splitPushes(seedOps: Op[], ops: Op[]): SeriesServer {
    const s = new SeriesServer();
    s.log.push(...seedOps);
    for (const o of ops) s.apply([o]);
    return s;
  }

  it("„Serie beenden“ removes the rhythm before dropping, so even split pushes create no instance", async () => {
    const id = await recurring("Serie beenden");
    const seedOps = server.log.filter((o) => o.entity_id === id);
    expect(endSeries(id)).toBe(true);
    expect(lastPushFields(id)).toEqual(["recurrence", "status"]);
    const ops = server.pushes.at(-1)!.filter((o) => o.entity_id === id);
    expect(ops[0]!.hlc < ops[1]!.hlc).toBe(true);
    await settle();
    expect(successors(id)).toHaveLength(0);

    const split = splitPushes(seedOps, ops);
    expect([...split["tasks"]().values()].filter((t) => t.recurrence_prev === id)).toHaveLength(0);
    // The other order would have created one.
    const reversed = splitPushes(seedOps, [...ops].reverse());
    expect([...reversed["tasks"]().values()].filter((t) => t.recurrence_prev === id)).toHaveLength(1);
  });

  it("removing the due date removes the rhythm first, so each part of a split push is valid", async () => {
    const id = await recurring("Ohne Fälligkeit");
    const base = mergeOps(server.log);
    expect(clearDue(id)).toBe(true);
    expect(lastPushFields(id)).toEqual(["recurrence", "due"]);
    const [first, second] = server.pushes.at(-1)!.filter((o) => o.entity_id === id);
    const now = Date.now();
    expect(validateOps(base, [first!], now)).toEqual([]);
    base.applyAll([first!]);
    expect(validateOps(base, [second!], now)).toEqual([]);
    // The other order: the due date alone may not go.
    expect(validateOps(mergeOps(server.log.filter((o) => o !== first && o !== second)), [second!], now)).not.toEqual([]);
    expect(data.tasks[id]).toMatchObject({ due: null, recurrence: null });
  });

  it("a task without rhythm only loses its due date", async () => {
    const id = await seed({ title: "Einmalig", due: "2099-01-31" });
    expect(clearDue(id)).toBe(true);
    expect(lastPushFields(id)).toEqual(["due"]);
  });
});

describe("hint „Nächste Instanz am …“", () => {
  it("does not come when completing again while the next instance exists", async () => {
    const id = await recurring("Erneut abschließen");
    expect(setStatus(id, "done")).toBe(true);
    await vi.waitFor(() => expect(successors(id)).toHaveLength(1));
    await vi.waitFor(() => expect(hints()).toHaveLength(1));
    expect(setStatus(id, "open")).toBe(true);
    expect(setStatus(id, "done")).toBe(true);
    // Further changes reach the index; a waiting hint would now find the existing instance.
    await vi.waitFor(() => expect(getEngine().pendingOps).toHaveLength(0));
    await seed({ title: "Weitere Änderung" });
    expect(hints()).toHaveLength(1);
    expect(successors(id)).toHaveLength(1);
  });

  it("stays silent when the next instance arrives after 60 seconds", async () => {
    server.createsNext = false;
    const id = await recurring("Offline erledigt");
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    expect(setStatus(id, "done")).toBe(true);
    await settle();
    vi.advanceTimersByTime(NEXT_INSTANCE_WAIT_MS + 1);
    server.next(id);
    await vi.waitFor(() => expect(taskIndex().nextInstance(id)).toBeDefined());
    await settle();
    expect(hints()).toHaveLength(0);
  });

  it("comes when the next instance arrives within 60 seconds", async () => {
    server.createsNext = false;
    const id = await recurring("Später angelegt");
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    expect(setStatus(id, "done")).toBe(true);
    await settle();
    vi.advanceTimersByTime(NEXT_INSTANCE_WAIT_MS - 1000);
    server.next(id, "2099-03-31");
    await vi.waitFor(() => expect(hints()).toHaveLength(1));
    expect(hints().at(-1)!.message).toBe("Nächste Instanz am Di, 31.03.2099");
  });
});

describe("change history", () => {
  it("names the previous instance by reference or title, linkable while it lives", async () => {
    const id = await recurring("Vorgängerin");
    expect(describeValue("recurrence_prev", id)).toBe("„Vorgängerin“");
    expect(historyTaskRefs("recurrence_prev", id)).toEqual([{ id, task: data.tasks[id], label: "„Vorgängerin“" }]);
    const unknown = newId();
    expect(describeValue("recurrence_prev", unknown)).toBe("andere Aufgabe");
    expect(historyTaskRefs("recurrence_prev", unknown)![0]!.task).toBeUndefined();
  });

  it("resolves parent and blockers the same way", async () => {
    const parent = await seed({ title: "Eltern" });
    const blocker = await seed({ title: "Blocker" });
    expect(describeValue("parent_id", parent)).toBe("Eltern");
    expect(describeValue("blocked_by", [blocker, newId()])).toBe("„Blocker“, andere Aufgabe");
    expect(historyTaskRefs("blocked_by", [blocker])!.map((r) => r.task?.id)).toEqual([blocker]);
    expect(describeValue("blocked_by", [])).toBe("keine");
    expect(historyTaskRefs("blocked_by", [])).toBeNull();
    expect(historyTaskRefs("title", "x")).toBeNull();
  });
});
