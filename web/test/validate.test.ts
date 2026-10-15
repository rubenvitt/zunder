import { describe, expect, it } from "vitest";
import { mergeOps } from "../src/sync/merge";
import { checkField, deleteCascade, validateOps } from "../src/sync/validate";
import { toTask } from "../src/sync/model";
import { op } from "./helpers";

const id = (n: number) => `v${String(n).padStart(23, "0")}`;
const A1 = id(1);
const DEV = id(2);
const STD = id(3);
const BUG = id(4);
const T1 = id(5);
const GONE = id(6);
const NEW = id(7);
const CHILD = id(8);
const MISSING = id(99);

const base = () =>
  mergeOps([
    op("area", A1, "name", "Arbeit", 1),
    op("project", DEV, "name", "Zunder", 2),
    op("project", DEV, "area_id", A1, 3),
    op("project", DEV, "kind", "dev", 4),
    op("project", STD, "name", "Haushalt", 5),
    op("project", STD, "area_id", A1, 6),
    op("task", BUG, "title", "Absturz", 7),
    op("task", BUG, "type", "bug", 8),
    op("task", BUG, "location", { kind: "project", id: DEV }, 9),
    op("task", T1, "title", "Normal", 10),
    op("task", GONE, "title", "Weg", 11),
    op("task", GONE, "deleted", true, 12),
  ]);

const codes = (issues: { code: string }[]) => issues.map((i) => i.code);

describe("field checks", () => {
  it("rejects unknown fields and wrong types like the core", () => {
    expect(checkField("task", "prio", 1)?.code).toBe("unknown_field");
    expect(checkField("task", "status", "fertig")?.code).toBe("invalid_value");
    expect(checkField("task", "due", "12.10.2026")?.code).toBe("invalid_value");
    expect(checkField("task", "due", "2026-10-12")).toBeNull();
    expect(checkField("task", "due", "2026-10-12T09:00:00+02:00")).toBeNull();
    expect(checkField("task", "remind_at", "2026-10-12")?.code).toBe("invalid_value");
    expect(checkField("task", "phoenix", { role: "flamme", fire: "2026-W41" })).toBeNull();
    expect(checkField("task", "phoenix", { role: "flamme", fire: "W41" })?.code).toBe("invalid_value");
    expect(checkField("task", "phoenix", { role: "flamme", fire: "2026-W41", x: 1 })?.code).toBe("invalid_value");
    expect(checkField("task", "links", [{ url: "https://example.org", title: "X" }])).toBeNull();
    expect(checkField("task", "links", [{ url: "mailto:a@example.org" }])).toBeNull();
    expect(checkField("task", "links", [{ url: "kein link" }])?.code).toBe("invalid_value");
    expect(checkField("task", "links", [{ url: "https://example.org", extra: 1 }])?.code).toBe("invalid_value");
    expect(checkField("task", "pr_url", "ftp://example.org/x")?.code).toBe("invalid_value");
    expect(checkField("task", "vault_links", ["a.md", ""])?.code).toBe("invalid_value");
    expect(checkField("task", "location", { kind: "inbox", extra: true })?.code).toBe("invalid_value");
    expect(checkField("task", "location", { kind: "area", id: "A1" })?.code).toBe("invalid_value");
    expect(checkField("task", "parent_id", "kein-uuid")?.code).toBe("invalid_value");
    expect(checkField("project", "repo", "owner/name")).toBeNull();
    expect(checkField("project", "repo", "owner")?.code).toBe("invalid_value");
    expect(checkField("project", "repo", "-owner/name")?.code).toBe("invalid_value");
  });

  it("required fields are not nullable, optional ones are; deleted accepts true/false/null", () => {
    expect(checkField("task", "title", null)?.code).toBe("invalid_value");
    expect(checkField("project", "area_id", null)?.code).toBe("invalid_value");
    expect(checkField("area", "name", null)?.code).toBe("invalid_value");
    expect(checkField("task", "due", null)).toBeNull();
    expect(checkField("task", "deleted", true)).toBeNull();
    expect(checkField("task", "deleted", false)).toBeNull();
    expect(checkField("task", "deleted", null)).toBeNull();
    expect(checkField("task", "deleted", "ja")?.code).toBe("invalid_value");
  });
});

describe("entity rules", () => {
  it("requires a non-empty title of at most 500 characters", () => {
    expect(codes(validateOps(base(), [op("task", NEW, "status", "open", 100)]))).toEqual(["missing_required"]);
    expect(codes(validateOps(base(), [op("task", T1, "title", "  ", 100)]))).toEqual(["invalid_value"]);
    expect(validateOps(base(), [op("task", T1, "title", "  ", 100)])[0]!.message).toBe("Eine Aufgabe braucht einen Titel.");
    expect(codes(validateOps(base(), [op("task", NEW, "title", "ä".repeat(500), 100)]))).toEqual([]);
    // Same whitespace set as Rust's str::trim (Unicode White_Space), not JS trim().
    for (const blank of ["\u0085", "\u2028", " \t", "\u3000"]) expect(checkField("task", "title", blank)?.code).toBe("invalid_value");
    expect(checkField("task", "title", "\ufeff")).toBeNull();
    expect(checkField("area", "name", "\u0085")?.code).toBe("invalid_value");
    expect(codes(validateOps(base(), [op("task", NEW, "title", "ä".repeat(501), 100)]))).toEqual(["invalid_value"]);
  });

  it("checks references", () => {
    expect(codes(validateOps(base(), [op("task", NEW, "title", "X", 100), op("task", NEW, "location", { kind: "area", id: MISSING }, 101)]))).toEqual([
      "invalid_reference",
    ]);
    expect(codes(validateOps(base(), [op("task", T1, "parent_id", GONE, 100)]))).toEqual(["invalid_reference"]);
    expect(codes(validateOps(base(), [op("project", NEW, "name", "P", 100)]))).toEqual(["missing_required"]);
    expect(codes(validateOps(base(), [op("project", NEW, "name", "P", 100), op("project", NEW, "area_id", A1, 101)]))).toEqual([]);
  });

  it("forbids parent cycles", () => {
    const store = base();
    store.apply(op("task", CHILD, "title", "Kind", 50));
    store.apply(op("task", CHILD, "parent_id", T1, 51));
    expect(codes(validateOps(store, [op("task", T1, "parent_id", CHILD, 100)]))).toEqual(["cycle"]);
    expect(codes(validateOps(store, [op("task", T1, "parent_id", T1, 100)]))).toEqual(["cycle"]);
  });

  it("allows bug/feature/review only in dev projects", () => {
    expect(codes(validateOps(base(), [op("task", T1, "type", "feature", 100)]))).toEqual(["dev_only"]);
    expect(codes(validateOps(base(), [op("task", T1, "status", "review", 100)]))).toEqual(["dev_only"]);
    expect(codes(validateOps(base(), [op("task", BUG, "status", "review", 100)]))).toEqual([]);
    expect(codes(validateOps(base(), [op("task", BUG, "location", { kind: "inbox" }, 100)]))).toEqual(["dev_only"]);
    expect(codes(validateOps(base(), [op("project", STD, "repo", "owner/name", 100)]))).toEqual(["dev_only"]);
  });

  it("subtasks inherit the dev context of their root; moving the parent is blamed", () => {
    expect(
      codes(validateOps(base(), [op("task", NEW, "title", "Teil", 100), op("task", NEW, "parent_id", BUG, 101), op("task", NEW, "type", "bug", 102)])),
    ).toEqual([]);
    const store = base();
    store.apply(op("task", CHILD, "title", "Teil", 50));
    store.apply(op("task", CHILD, "parent_id", T1, 51));
    store.apply(op("task", T1, "location", { kind: "project", id: DEV }, 52));
    store.apply(op("task", CHILD, "type", "bug", 53));
    const issues = validateOps(store, [op("task", T1, "location", { kind: "inbox" }, 100)]);
    expect(issues.map((i) => [i.entity_id, i.code])).toEqual([[T1, "dev_tasks_present"]]);
  });

  it("blocks switching a project with dev tasks back to standard", () => {
    expect(codes(validateOps(base(), [op("project", DEV, "kind", "standard", 100)]))).toEqual(["dev_tasks_present"]);
    expect(codes(validateOps(base(), [op("project", STD, "kind", "dev", 100)]))).toEqual([]);
  });

  it("refuses deleting referenced entities unless the dependents go along", () => {
    const store = base();
    store.apply(op("task", CHILD, "title", "Kind", 50));
    store.apply(op("task", CHILD, "parent_id", T1, 51));
    expect(codes(validateOps(store, [op("task", T1, "deleted", true, 100)]))).toEqual(["has_dependents"]);
    expect(codes(validateOps(store, [op("task", T1, "deleted", true, 100), op("task", CHILD, "deleted", true, 101)]))).toEqual([]);
    expect(codes(validateOps(store, [op("area", A1, "deleted", true, 100)]))).toEqual(["has_dependents"]);
  });

  it("restoring with false or null applies all rules again", () => {
    expect(codes(validateOps(base(), [op("task", GONE, "deleted", false, 100)]))).toEqual([]);
    expect(codes(validateOps(base(), [op("task", GONE, "deleted", null, 100)]))).toEqual([]);
    const store = base();
    store.apply(op("task", CHILD, "title", "Kind", 50));
    store.apply(op("task", CHILD, "parent_id", GONE, 51));
    store.apply(op("task", CHILD, "deleted", true, 52));
    expect(codes(validateOps(store, [op("task", CHILD, "deleted", false, 100)]))).toEqual(["invalid_reference"]);
  });

  it("an id belongs to exactly one entity kind", () => {
    expect(codes(validateOps(base(), [op("task", A1, "title", "X", 100)]))).toEqual(["kind_mismatch"]);
  });
});

describe("recurrence", () => {
  const monthlyLast = { every: 1, unit: "month", day: "last", from: "due" };

  it("names the violated rule of a malformed recurrence like the core", () => {
    expect(checkField("task", "recurrence", monthlyLast)).toBeNull();
    expect(checkField("task", "recurrence", null)).toBeNull();
    expect(checkField("task", "recurrence", { every: 1, unit: "day", weekdays: ["mo"], from: "due" })?.message).toBe(
      "Wochentage („weekdays“) gehen nur bei wöchentlichen Rhythmen.",
    );
    expect(checkField("task", "recurrence", { every: 1, unit: "year", day: 29, from: "due" })?.message).toContain("monatlichen");
    expect(checkField("task", "recurrence", { every: 1000, unit: "day", from: "due" })?.message).toContain("1 bis 999");
    expect(checkField("task", "recurrence", { every: 1, unit: "day", from: "due", until: "x" })?.message).toContain("„until“");
    expect(checkField("task", "recurrence_prev", T1)).toBeNull();
  });

  it("needs a due date, a root task and leaves recurrence_prev to the server", () => {
    const store = base();
    const issues = validateOps(store, [op("task", T1, "recurrence", monthlyLast, 100)]);
    expect(issues.map((i) => i.message)).toEqual(["Eine wiederkehrende Aufgabe braucht eine Fälligkeit. Bitte zuerst den Rhythmus entfernen."]);
    expect(
      codes(validateOps(store, [op("task", T1, "due", "2026-10-31", 100), op("task", T1, "recurrence", monthlyLast, 101), op("task", T1, "parent_id", BUG, 102)])),
    ).toEqual(["invalid_value"]);
    expect(codes(validateOps(store, [op("task", T1, "due", "2026-10-31", 100), op("task", T1, "recurrence", monthlyLast, 101)]))).toEqual([]);
    expect(validateOps(store, [op("task", T1, "recurrence_prev", BUG, 100)])[0]!.message).toContain("nur der Server");
  });

  it("materializes the typed recurrence and drops malformed ones", () => {
    const store = mergeOps([
      op("task", T1, "title", "Odoo", 1),
      op("task", T1, "recurrence", monthlyLast, 2),
      op("task", T1, "recurrence_prev", BUG, 3),
      op("task", NEW, "title", "Kaputt", 4),
      op("task", NEW, "recurrence", { every: 1, unit: "month", day: 32, from: "due" }, 5),
    ]);
    const task = toTask(store.get("task", T1)!);
    expect(task.recurrence).toEqual(monthlyLast);
    expect(task.recurrence_prev).toBe(BUG);
    expect(toTask(store.get("task", NEW)!).recurrence).toBeNull();
  });
});

describe("delete cascade", () => {
  it("collects live dependents like OpBatch::delete_cascade", () => {
    const store = base();
    store.apply(op("task", CHILD, "title", "Kind", 50));
    store.apply(op("task", CHILD, "parent_id", BUG, 51));
    store.apply(op("task", NEW, "title", "Rasen", 52));
    store.apply(op("task", NEW, "location", { kind: "area", id: A1 }, 53));
    expect(deleteCascade(store, "task", BUG)).toEqual([
      ["task", BUG],
      ["task", CHILD],
    ]);
    expect(new Set(deleteCascade(store, "area", A1).map(([, i]) => i))).toEqual(new Set([A1, DEV, STD, BUG, CHILD, NEW]));
    const all = deleteCascade(store, "area", A1);
    const ops = all.map(([kind, i], n) => op(kind, i, "deleted", true, 200 + n));
    expect(validateOps(store, ops)).toEqual([]);
  });
});
