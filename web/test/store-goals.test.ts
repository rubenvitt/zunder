// Goals in the Solid store: snapshot load and live ops fill `data.goals` like comments,
// `goalList` is ordered like the goal input (rank, missing last; name; ID) and hides deleted goals.

import { beforeAll, describe, expect, it, vi } from "vitest";
import { configureServices, data, goalList, startSync } from "../src/state/store";
import { MemoryLocalStore } from "../src/sync/local-store";
import type { SubscriptionHandlers, SyncTransport } from "../src/sync/transport";
import type { Op, PullResponse, PushRequest, PushResponse, Snapshot } from "../src/sync/types";
import { op } from "./helpers";

const id = (n: number) => `g${String(n).padStart(23, "0")}`;

class GoalServer implements SyncTransport {
  log: Op[] = [];
  handlers: SubscriptionHandlers | null = null;
  async snapshot(): Promise<Snapshot> {
    return {
      cursor: this.log.length,
      entities: this.log.map((o) => ({ entity: o.entity, entity_id: o.entity_id, fields: { [o.field]: { value: o.value, hlc: o.hlc } } })),
    };
  }
  async push(req: PushRequest): Promise<PushResponse> {
    this.log.push(...req.ops);
    return { accepted: req.ops.map((o) => o.op_id), rejected: [], warnings: [], cursor: this.log.length };
  }
  async pull(since: number): Promise<PullResponse> {
    const ops = this.log.slice(since).map((o, i) => ({ seq: since + i + 1, op: o }));
    return { ops, cursor: this.log.length, has_more: false };
  }
  subscribe(handlers: SubscriptionHandlers): () => void {
    this.handlers = handlers;
    handlers.onConnection(true);
    return () => {
      this.handlers = null;
    };
  }
}

const server = new GoalServer();
const T = 1_790_000_000_000;
server.log.push(
  op("goal", id(1), "name", "Sport", T),
  op("goal", id(2), "name", "Lesen", T),
  op("goal", id(2), "rank", "a", T),
  op("goal", id(3), "name", "Alt", T),
  op("goal", id(3), "deleted", true, T + 1),
  op("goal", id(4), "name", "Anfang", T),
  op("goal", id(4), "rank", "a", T),
);

beforeAll(async () => {
  configureServices({ transport: server, localStore: new MemoryLocalStore("web-store-goals") });
  expect(await startSync()).toBe(true);
});

describe("goals in the store", () => {
  it("loads goals from the snapshot, deleted ones included in data", () => {
    expect(Object.keys(data.goals)).toHaveLength(4);
    expect(data.goals[id(3)]!.deleted).toBe(true);
  });

  it("lists live goals by rank (missing last), then name, then ID", () => {
    expect(goalList().map((g) => g.name)).toEqual(["Anfang", "Lesen", "Sport"]);
  });

  it("applies live ops to data.goals and the list", async () => {
    const live = op("goal", id(5), "name", "Neu", T + 5);
    const rank = op("goal", id(5), "rank", "0", T + 6);
    server.log.push(live, rank);
    server.handlers!.onCursor(server.log.length);
    await vi.waitFor(() => expect(data.goals[id(5)]?.name).toBe("Neu"));
    expect(goalList().map((g) => g.name)).toEqual(["Neu", "Anfang", "Lesen", "Sport"]);
  });
});
