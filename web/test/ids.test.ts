import { describe, expect, it } from "vitest";
import { fromLegacy, isCanonicalId, newId, upgradeOp } from "../src/sync/ids";

describe("ids", () => {
  it("creates canonical CUID2 ids", () => {
    const ids = new Set(Array.from({ length: 200 }, () => newId()));
    expect(ids.size).toBe(200);
    for (const id of ids) expect(isCanonicalId(id)).toBe(true);
    expect(isCanonicalId("0192a000-0000-7000-8000-00000000e001")).toBe(false);
  });

  it("converts legacy UUIDs like the core", () => {
    // Same frozen value as zunder_core (vector 15 covers the merge path).
    expect(fromLegacy("0192a000-0000-7000-8000-00000000e001")).toBe("ssfsatfajv4rz72sw49qhw6r");
    const parent = "0192a000-0000-7000-8000-0c0000000001";
    const op = upgradeOp({
      op_id: "0192a000-0000-7000-8000-0f0000000001",
      entity: "task",
      entity_id: "0192a000-0000-7000-8000-0c0000000002",
      field: "parent_id",
      value: parent,
      hlc: "1760000000000:00000:web-a",
    });
    expect(op.value).toBe(fromLegacy(parent));
    expect(isCanonicalId(op.op_id) && isCanonicalId(op.entity_id)).toBe(true);
    const current = { op_id: newId(), entity: "task" as const, entity_id: newId(), field: "title", value: parent, hlc: "1760000000000:00000:web-a" };
    expect(upgradeOp(current)).toBe(current);
  });
});
