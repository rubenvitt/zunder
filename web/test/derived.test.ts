import { describe, expect, it } from "vitest";
import { fireOf, isValidFire, shiftFire, formatDue, parseDue } from "../src/sync/dates";
import {
  HIDE_CLOSED_AFTER_MS,
  compareTasks,
  isClaimActive,
  isFlameRunning,
  isHiddenByAge,
  progressOf,
  statusChangedMs,
  TaskIndex,
} from "../src/sync/derived";
import { mergeOps } from "../src/sync/merge";
import { toProject, toTask } from "../src/sync/model";
import { defaultRank, rankBetween } from "../src/sync/rank";
import type { Op, Task } from "../src/sync/types";
import { op } from "./helpers";

function tasksFrom(ops: Op[]): Task[] {
  return [...mergeOps(ops).values()].filter((e) => e.entity === "task").map(toTask);
}

describe("materialization", () => {
  it("applies contract defaults", () => {
    const [t] = tasksFrom([op("task", "t1", "title", "X", 1000)]);
    expect(t).toMatchObject({ type: "aufgabe", status: "open", location: { kind: "inbox" }, links: [], vault_links: [], phoenix: null });
    expect(t!.created_hlc).toBe(t!.status_hlc);
  });

  it("derives created/updated/status_changed from field HLCs", () => {
    const [t] = tasksFrom([op("task", "t1", "title", "X", 1000), op("task", "t1", "status", "done", 5000), op("task", "t1", "description", "Y", 9000)]);
    expect(t!.created_hlc.startsWith("0000000001000")).toBe(true);
    expect(t!.updated_hlc.startsWith("0000000009000")).toBe(true);
    expect(statusChangedMs(t!)).toBe(5000);
  });

  it("dev projects default to the board view", () => {
    const store = mergeOps([op("project", "p1", "name", "Dev", 1), op("project", "p1", "kind", "dev", 1, { counter: 1 })]);
    expect(toProject(store.get("project", "p1")!).view).toBe("board");
    const plain = mergeOps([op("project", "p2", "name", "Std", 1)]);
    expect(toProject(plain.get("project", "p2")!).view).toBe("list");
  });

  it("tolerates malformed values", () => {
    const [t] = tasksFrom([op("task", "t1", "status", "kaputt", 1), op("task", "t1", "location", { kind: "area" }, 1, { counter: 1 })]);
    expect(t!.status).toBe("open");
    expect(t!.location).toEqual({ kind: "inbox" });
  });
});

describe("TaskIndex", () => {
  const ops = [
    op("task", "root", "title", "Root", 1),
    op("task", "root", "location", { kind: "project", id: "p1" }, 2),
    op("task", "a", "title", "A", 3),
    op("task", "a", "parent_id", "root", 4),
    op("task", "a", "location", { kind: "loose" }, 5),
    op("task", "b", "title", "B", 6),
    op("task", "b", "parent_id", "a", 7),
    op("task", "c", "title", "C", 8),
    op("task", "c", "parent_id", "root", 9),
    op("task", "c", "status", "done", 10),
    op("task", "d", "title", "D", 11),
    op("task", "d", "parent_id", "root", 12),
    op("task", "d", "status", "dropped", 13),
  ];
  const index = new TaskIndex(tasksFrom(ops));

  it("effective location is the root ancestor's", () => {
    expect(index.location(index.byId.get("b")!)).toEqual({ kind: "project", id: "p1" });
    expect(index.location(index.byId.get("a")!)).toEqual({ kind: "project", id: "p1" });
  });

  it("progress counts direct children, dropped excluded", () => {
    expect(index.progress("root")).toEqual({ done: 1, total: 2 });
    expect(index.progress("a")).toEqual({ done: 0, total: 1 });
    expect(progressOf([])).toEqual({ done: 0, total: 0 });
  });

  it("descendants and deletion propagate", () => {
    expect(index.descendantsOf("root").map((t) => t.id).sort()).toEqual(["a", "b", "c", "d"]);
    const withDeleted = new TaskIndex(tasksFrom([...ops, op("task", "a", "deleted", true, 20)]));
    expect(withDeleted.isDeleted(withDeleted.byId.get("b")!)).toBe(true);
    expect(withDeleted.isDeleted(withDeleted.byId.get("c")!)).toBe(false);
  });

  it("survives parent cycles", () => {
    const cyc = new TaskIndex(
      tasksFrom([
        op("task", "x", "title", "X", 1),
        op("task", "x", "parent_id", "y", 2),
        op("task", "y", "title", "Y", 3),
        op("task", "y", "parent_id", "x", 4),
      ]),
    );
    expect(cyc.location(cyc.byId.get("x")!)).toEqual({ kind: "inbox" });
    expect(cyc.isDeleted(cyc.byId.get("x")!)).toBe(false);
    expect(cyc.descendantsOf("x").map((t) => t.id)).toEqual(["y"]);
  });
});

describe("3-day hiding", () => {
  const [done] = tasksFrom([op("task", "t", "title", "T", 1_000), op("task", "t", "status", "done", 10_000)]);
  it("hides closed tasks 3 days after the status change", () => {
    expect(isHiddenByAge(done!, 10_000 + HIDE_CLOSED_AFTER_MS)).toBe(false);
    expect(isHiddenByAge(done!, 10_001 + HIDE_CLOSED_AFTER_MS)).toBe(true);
  });
  it("never hides open tasks", () => {
    const [open] = tasksFrom([op("task", "t", "title", "T", 1_000)]);
    expect(isHiddenByAge(open!, 1e15)).toBe(false);
  });
});

describe("flames and claims", () => {
  const base = tasksFrom([op("task", "t", "title", "T", 1)])[0]!;
  it("flame runs while started and not ended after start", () => {
    expect(isFlameRunning({ ...base, flame_started_at: "2026-10-11T10:00:00+02:00" })).toBe(true);
    expect(isFlameRunning({ ...base, flame_started_at: "2026-10-11T10:00:00+02:00", flame_ended_at: "2026-10-11T11:00:00+02:00" })).toBe(false);
    expect(isFlameRunning({ ...base, flame_started_at: "2026-10-11T12:00:00+02:00", flame_ended_at: "2026-10-11T11:00:00+02:00" })).toBe(true);
  });
  it("claims expire", () => {
    const now = Date.parse("2026-10-11T12:00:00Z");
    expect(isClaimActive({ ...base, claimed_by: "agent", claim_expires_at: "2026-10-11T13:00:00Z" }, now)).toBe(true);
    expect(isClaimActive({ ...base, claimed_by: "agent", claim_expires_at: "2026-10-11T11:00:00Z" }, now)).toBe(false);
    expect(isClaimActive(base, now)).toBe(false);
  });
});

describe("ranks", () => {
  it("rankBetween stays strictly between", () => {
    const cases: [string | null, string | null][] = [
      [null, null],
      [null, "a"],
      ["a", null],
      ["a", "b"],
      ["a", "a1"],
      ["az", "b"],
      ["abc", "abd"],
      [defaultRank(1000), defaultRank(1001)],
      // Fixed-width decimal ranks of the ClickUp import (contract 6).
      ["0000000000001.000000", "0000000000001.500000"],
      ["0000000000001.500000", "0000000000002.000000"],
      ["0000000000001.999999", "0000000000002.000000"],
      [null, "0000000000000.000000"],
      ["0000000000099.000000", null],
      ["0000000000099.000000", defaultRank(1_760_000_000_000)],
      ["a.", "a.0"],
      ["", "0.5"],
      ["z", null],
      ["zz~", null],
    ];
    for (const [a, b] of cases) {
      const r = rankBetween(a, b);
      if (a !== null) expect(r > a, `${r} > ${a}`).toBe(true);
      if (b !== null) expect(r < b, `${r} < ${b}`).toBe(true);
      // Below a rank made only of "0" and "." there is no rank ending in [1-9a-z].
      if (b === null || /[1-9a-z]/.test(b.slice((a ?? "").length))) expect(r.endsWith("0"), r).toBe(false);
    }
  });

  it("inserts repeatedly between import ranks", () => {
    let lo = "0000000000001.000000";
    let hi = "0000000000001.000001";
    for (let i = 0; i < 100; i++) {
      const r = rankBetween(lo, hi);
      expect(r > lo && r < hi, `${lo} < ${r} < ${hi}`).toBe(true);
      expect(r.endsWith("0")).toBe(false);
      if (i % 2 === 0) lo = r;
      else hi = r;
    }
  });

  it("supports repeated insertion at the same spot", () => {
    let lo = "a";
    const hi = "b";
    for (let i = 0; i < 200; i++) {
      const r = rankBetween(lo, hi);
      expect(r > lo && r < hi).toBe(true);
      lo = r;
    }
  });

  it("orders unranked tasks by creation, ranked ones by rank", () => {
    const [a, b] = tasksFrom([op("task", "a", "title", "A", 2000), op("task", "b", "title", "B", 1000)]);
    expect([a!, b!].sort(compareTasks).map((t) => t.id)).toEqual(["b", "a"]);
    const moved = { ...a!, rank: rankBetween(null, defaultRank(1000)) };
    expect([b!, moved].sort(compareTasks).map((t) => t.id)).toEqual(["a", "b"]);
  });
});

describe("date helpers", () => {
  it("computes ISO week fire ids", () => {
    expect(fireOf(new Date(2026, 9, 11))).toBe("2026-W41");
    expect(fireOf(new Date(2026, 0, 1))).toBe("2026-W01");
    expect(fireOf(new Date(2027, 0, 1))).toBe("2026-W53");
    expect(shiftFire("2026-W41", 1)).toBe("2026-W42");
    expect(shiftFire("2026-W53", 1)).toBe("2027-W01");
    expect(shiftFire("2026-W01", -1)).toBe("2025-W52");
    expect(isValidFire("2026-W53")).toBe(true);
    expect(isValidFire("2025-W53")).toBe(false);
  });

  it("parses and formats due values", () => {
    const now = new Date(2026, 9, 11, 12);
    expect(parseDue("2026-10-12")).toMatchObject({ date: "2026-10-12", time: null });
    expect(parseDue("2026-10-12T07:00:00Z")).toMatchObject({ date: "2026-10-12", time: "09:00" });
    expect(formatDue("2026-10-12", now)).toBe("Morgen");
    expect(formatDue("2026-10-10T08:00:00+02:00", now)).toBe("Gestern 08:00");
    expect(formatDue("2026-10-20", now)).toBe("Di, 20.10.");
    expect(formatDue("2027-01-05", now)).toBe("Di, 05.01.2027");
  });
});
