import { describe, expect, it } from "vitest";
import { mergeOps } from "../src/sync/merge";
import { checkField, validateOps } from "../src/sync/validate";
import { op } from "./helpers";

const base = () =>
  mergeOps([
    op("area", "a1", "name", "Arbeit", 1),
    op("project", "dev", "name", "Zunder", 2),
    op("project", "dev", "area_id", "a1", 3),
    op("project", "dev", "kind", "dev", 4),
    op("project", "std", "name", "Haushalt", 5),
    op("project", "std", "area_id", "a1", 6),
    op("task", "bug", "title", "Absturz", 7),
    op("task", "bug", "type", "bug", 8),
    op("task", "bug", "location", { kind: "project", id: "dev" }, 9),
    op("task", "t1", "title", "Normal", 10),
    op("task", "gone", "title", "Weg", 11),
    op("task", "gone", "deleted", true, 12),
  ]);

const codes = (issues: { code: string }[]) => issues.map((i) => i.code);

describe("field checks", () => {
  it("rejects unknown fields and wrong types", () => {
    expect(checkField("task", "prio", 1)?.code).toBe("unknown_field");
    expect(checkField("task", "status", "fertig")?.code).toBe("invalid_value");
    expect(checkField("task", "due", "12.10.2026")?.code).toBe("invalid_value");
    expect(checkField("task", "due", "2026-10-12")).toBeNull();
    expect(checkField("task", "due", "2026-10-12T09:00:00+02:00")).toBeNull();
    expect(checkField("task", "remind_at", "2026-10-12")?.code).toBe("invalid_value");
    expect(checkField("task", "phoenix", { role: "flamme", fire: "2026-W41" })).toBeNull();
    expect(checkField("task", "phoenix", { role: "flamme", fire: "W41" })?.code).toBe("invalid_value");
    expect(checkField("task", "links", [{ url: "https://example.org", title: "X" }])).toBeNull();
    expect(checkField("task", "links", [{ url: "kein link" }])?.code).toBe("invalid_value");
    expect(checkField("project", "repo", "owner/name")).toBeNull();
    expect(checkField("project", "repo", "owner")?.code).toBe("invalid_value");
  });
});

describe("entity rules", () => {
  it("requires a non-empty title of at most 500 characters", () => {
    expect(codes(validateOps(base(), [op("task", "n", "status", "open", 100)]))).toEqual(["required"]);
    expect(codes(validateOps(base(), [op("task", "t1", "title", "  ", 100)]))).toEqual(["required"]);
    expect(codes(validateOps(base(), [op("task", "n", "title", "ä".repeat(500), 100)]))).toEqual([]);
    expect(codes(validateOps(base(), [op("task", "n", "title", "ä".repeat(501), 100)]))).toEqual(["title_too_long"]);
  });

  it("checks references", () => {
    expect(codes(validateOps(base(), [op("task", "n", "title", "X", 100), op("task", "n", "location", { kind: "area", id: "nope" }, 101)]))).toEqual([
      "invalid_reference",
    ]);
    expect(codes(validateOps(base(), [op("task", "t1", "parent_id", "gone", 100)]))).toEqual(["invalid_reference"]);
    expect(codes(validateOps(base(), [op("project", "p", "name", "P", 100)]))).toEqual(["required"]);
    expect(codes(validateOps(base(), [op("project", "p", "name", "P", 100), op("project", "p", "area_id", "a1", 101)]))).toEqual([]);
  });

  it("forbids parent cycles", () => {
    const store = base();
    store.apply(op("task", "child", "title", "Kind", 50));
    store.apply(op("task", "child", "parent_id", "t1", 51));
    expect(codes(validateOps(store, [op("task", "t1", "parent_id", "child", 100)]))).toEqual(["parent_cycle"]);
    expect(codes(validateOps(store, [op("task", "t1", "parent_id", "t1", 100)]))).toEqual(["parent_cycle"]);
  });

  it("allows bug/feature/review only in dev projects", () => {
    expect(codes(validateOps(base(), [op("task", "t1", "type", "feature", 100)]))).toEqual(["dev_only_type"]);
    expect(codes(validateOps(base(), [op("task", "t1", "status", "review", 100)]))).toEqual(["dev_only_status"]);
    expect(codes(validateOps(base(), [op("task", "bug", "status", "review", 100)]))).toEqual([]);
    expect(codes(validateOps(base(), [op("task", "bug", "location", { kind: "inbox" }, 100)]))).toEqual(["dev_task_move"]);
  });

  it("subtasks inherit the dev context of their root", () => {
    expect(codes(validateOps(base(), [op("task", "s", "title", "Teil", 100), op("task", "s", "parent_id", "bug", 101), op("task", "s", "type", "bug", 102)]))).toEqual([]);
  });

  it("blocks switching a project with dev tasks back to standard", () => {
    expect(codes(validateOps(base(), [op("project", "dev", "kind", "standard", 100)]))).toEqual(["project_has_dev_tasks"]);
    expect(codes(validateOps(base(), [op("project", "std", "kind", "dev", 100)]))).toEqual([]);
  });
});
