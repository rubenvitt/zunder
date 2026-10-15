import { describe, expect, it } from "vitest";
import type { AgentChange, Op } from "../src/sync/types";
import { describeChange } from "../src/agent-log";

function op(entity: Op["entity"], entity_id: string, field: string, value: Op["value"]): Op {
  return { op_id: `op-${field}`, entity, entity_id, field, value, hlc: "0001760000000000:00000:test" };
}

function change(entity: Op["entity"], created: boolean, fields: Record<string, Op["value"]>): AgentChange {
  const id = "t".repeat(24);
  return { entity, entity_id: id, created, ops: Object.entries(fields).map(([f, v]) => op(entity, id, f, v)) };
}

describe("describeChange", () => {
  it("names a created task by the title in the change", () => {
    const s = describeChange(change("task", true, { title: "Steuer", location: { kind: "inbox" }, rank: "a" }));
    expect([s.lead, s.name, s.verb]).toEqual(["Aufgabe", "Steuer", "angelegt"]);
    expect(s.details).toEqual([]);
  });

  it("lists changed fields with readable values and hides bookkeeping", () => {
    const s = describeChange(change("task", false, { status: "done", rank: "b", due: "2026-10-20" }));
    expect(s.verb).toBe("geändert");
    expect(s.details).toEqual(["Status → Erledigt", "Fällig → „2026-10-20“"]);
  });

  it("calls a tombstone a deletion", () => {
    const s = describeChange(change("task", false, { deleted: true }));
    expect(s.verb).toBe("gelöscht");
    expect(s.icon).toBe("trash");
    expect(s.details).toEqual([]);
  });

  it("shows a new comment with a snippet of its body", () => {
    const s = describeChange(change("comment", true, { task_id: "x".repeat(24), body: "PR ist offen" }));
    expect([s.lead, s.verb]).toEqual(["Kommentar zu", "geschrieben"]);
    expect(s.details).toEqual(["„PR ist offen“"]);
  });

  it("names projects and areas", () => {
    expect(describeChange(change("project", true, { name: "Zunder" })).name).toBe("Zunder");
    expect(describeChange(change("area", false, { name: "Gesundheit" })).details).toEqual(["Name → „Gesundheit“"]);
  });
});
