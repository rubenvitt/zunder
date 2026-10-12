import { describe, expect, it } from "vitest";
import { cmpJson, EntityStore, mergeOps, wins } from "../src/sync/merge";
import { op, permutations, shuffled } from "./helpers";

const T = "v00070008000000000000001";

describe("LWW merge", () => {
  it("later HLC wins per field", () => {
    const store = mergeOps([op("task", T, "title", "alt", 1000), op("task", T, "title", "neu", 2000)]);
    expect(store.toValues()[`task/${T}`]).toEqual({ title: "neu" });
  });

  it("fields are independent", () => {
    const store = mergeOps([
      op("task", T, "title", "A", 3000),
      op("task", T, "status", "done", 1000),
      op("task", T, "title", "B", 2000),
      op("task", T, "status", "open", 500),
    ]);
    expect(store.toValues()[`task/${T}`]).toEqual({ title: "A", status: "done" });
  });

  it("op_id breaks HLC ties (greater wins), independent of order", () => {
    const a = op("task", T, "title", "kleiner", 1000, { opId: "v0007000800000000000000a" });
    const b = op("task", T, "title", "größer", 1000, { opId: "v0007000800000000000000b" });
    expect(mergeOps([a, b]).toValues()[`task/${T}`]!.title).toBe("größer");
    expect(mergeOps([b, a]).toValues()[`task/${T}`]!.title).toBe("größer");
  });

  it("null resets a field but still participates in LWW", () => {
    const store = mergeOps([op("task", T, "due", "2026-10-12", 1000), op("task", T, "due", null, 2000)]);
    expect(store.toValues()[`task/${T}`]).toEqual({ due: null });
  });

  it("is idempotent", () => {
    const ops = [op("task", T, "title", "x", 1), op("task", T, "title", "y", 2)];
    const store = new EntityStore();
    store.applyAll(ops);
    const before = JSON.stringify(store.toValues());
    expect(store.applyAll(ops).size).toBe(0);
    expect(JSON.stringify(store.toValues())).toBe(before);
  });

  it("is order independent for every permutation", () => {
    const ops = [
      op("task", T, "title", "a", 10, { node: "web-a" }),
      op("task", T, "title", "b", 10, { node: "web-b" }),
      op("task", T, "status", "done", 11),
      op("area", T, "name", "gleiche id, andere Entität", 5),
      op("task", T, "deleted", true, 12),
    ];
    const reference = JSON.stringify(mergeOps(ops).toValues());
    for (const perm of permutations(ops, 120)) expect(JSON.stringify(sortKeys(mergeOps(perm).toValues()))).toBe(JSON.stringify(sortKeys(JSON.parse(reference))));
  });

  it("random interleavings converge (associativity via split stores)", () => {
    const ops = Array.from({ length: 60 }, (_, i) =>
      op("task", i % 2 ? T : "v00070008000000000000002", i % 3 ? "title" : "status", `v${i}`, 1000 + (i % 7), {
        counter: i % 4,
        node: i % 5 ? "web-a" : "server",
      }),
    );
    const reference = sortKeys(mergeOps(ops).toValues());
    for (let seed = 1; seed <= 20; seed++) {
      const mixed = shuffled(ops, seed);
      const left = mergeOps(mixed.slice(0, 30));
      const right = mergeOps(mixed.slice(30));
      // Merge right into left field by field.
      for (const e of right.values()) for (const [f, s] of Object.entries(e.fields)) left.applyField(e.entity, e.id, f, s);
      expect(sortKeys(left.toValues())).toEqual(reference);
    }
  });

  it("snapshot fields without op_id lose HLC ties against real ops", () => {
    expect(wins({ value: null, hlc: "x", op_id: "01" }, { value: null, hlc: "x", op_id: "" })).toBe(true);
    const store = new EntityStore();
    store.loadSnapshot({
      cursor: 1,
      entities: [{ entity: "task", entity_id: T, fields: { title: { value: "snap", hlc: "0000000001000:00000:server" } } }],
    });
    expect(store.apply(op("task", T, "title", "alt", 999))).toBe(false);
    expect(store.apply(op("task", T, "title", "neu", 1001))).toBe(true);
  });

  it("identical HLC and op_id fall back to a fixed order on the value", () => {
    const a = op("task", T, "title", "A", 5, { opId: "v000700080000000000000ff" });
    const b = { ...a, value: "B" };
    expect(mergeOps([a, b]).toValues()).toEqual(mergeOps([b, a]).toValues());
    expect(mergeOps([a, b]).get("task", T)!.fields.title!.value).toBe("B");
    expect(cmpJson(null, false)).toBeLessThan(0);
    expect(cmpJson(true, false)).toBeGreaterThan(0);
    expect(cmpJson({ b: 1 }, { a: 2 })).toBeGreaterThan(0);
    expect(cmpJson([1, 2], [1])).toBeGreaterThan(0);
  });

  it("created_hlc is the smallest HLC ever applied, also from the snapshot", () => {
    const store = mergeOps([op("task", T, "title", "erst", 100), op("task", T, "title", "später", 200)]);
    expect(store.get("task", T)!.created_hlc).toBe("0000000000100:00000:test");
    const snap = new EntityStore();
    snap.loadSnapshot({
      cursor: 2,
      entities: [
        {
          entity: "task",
          entity_id: T,
          created_hlc: "0000000000100:00000:test",
          fields: { title: { value: "später", hlc: "0000000000200:00000:test", op_id: "x" } },
        },
      ],
    });
    expect(snap.get("task", T)!.created_hlc).toBe("0000000000100:00000:test");
  });

  it("clone is independent", () => {
    const store = mergeOps([op("task", T, "title", "a", 1)]);
    const copy = store.clone();
    copy.apply(op("task", T, "title", "b", 2));
    expect(store.toValues()[`task/${T}`]!.title).toBe("a");
    expect(copy.toValues()[`task/${T}`]!.title).toBe("b");
  });
});

function sortKeys<T>(value: T): T {
  if (Array.isArray(value)) return value.map(sortKeys) as T;
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((k) => [k, sortKeys((value as Record<string, unknown>)[k])]),
    ) as T;
  }
  return value;
}
