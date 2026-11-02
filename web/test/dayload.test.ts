// Tagesvorschau (contract 5.1): which tasks weigh on a day ahead, the fixed threshold for a
// crowded day and the labels in Heute and when picking a day.

import { describe, expect, it } from "vitest";
import { crowdedDayLine, crowdedPickHint, dueCountLabel, longDayLabel } from "../src/labels";
import { CROWDED_DAY, dayLoads, dueOnDay, isCrowded, PREVIEW_DAYS } from "../src/sync/dayload";
import type { Task } from "../src/sync/types";

function task(id: string, extra: Partial<Task> = {}): Task {
  return {
    id,
    title: id,
    status: "open",
    due: null,
    rank: null,
    created_hlc: "1760000000000:00000:test",
    ...extra,
  } as Task;
}

const none = () => false;

describe("dayLoads", () => {
  it("lists the days after today, empty ones included, across a month end", () => {
    const loads = dayLoads([], "2026-10-28", PREVIEW_DAYS, none);
    expect(loads.map((l) => l.date)).toEqual(["2026-10-29", "2026-10-30", "2026-10-31", "2026-11-01", "2026-11-02", "2026-11-03", "2026-11-04"]);
    expect(loads.every((l) => l.tasks.length === 0)).toBe(true);
  });

  it("counts open tasks by their local due day, dates with and without time", () => {
    const tasks = [
      task("date", { due: "2026-10-30" }),
      task("time", { due: "2026-10-30T09:00:00+01:00" }),
      task("utc", { due: "2026-10-29T23:30:00Z" }), // 00:30 on the 30th in Berlin
      task("other", { due: "2026-10-31" }),
      task("today", { due: "2026-10-28" }),
      task("later", { due: "2026-11-20" }),
      task("undated"),
    ];
    const loads = dayLoads(tasks, "2026-10-28", 3, none);
    expect(loads.map((l) => [l.date, l.tasks.map((t) => t.id)])).toEqual([
      ["2026-10-29", []],
      ["2026-10-30", ["date", "time", "utc"]],
      ["2026-10-31", ["other"]],
    ]);
  });

  it("leaves out done, dropped and excluded tasks; blocked, waiting and subtasks count", () => {
    const due = "2026-10-30";
    const tasks = [
      task("open", { due }),
      task("progress", { due, status: "in_progress" }),
      task("blocked", { due, status: "blocked", waiting_on: "Anna" }),
      task("sub", { due, parent_id: "open" }),
      task("done", { due, status: "done" }),
      task("dropped", { due, status: "dropped" }),
      task("dev", { due }),
    ];
    const loads = dayLoads(tasks, "2026-10-29", 1, (t) => t.id === "dev");
    expect(loads[0]!.tasks.map((t) => t.id)).toEqual(["open", "progress", "blocked", "sub"]);
  });

  it("answers an invalid day with nothing", () => {
    expect(dayLoads([task("a", { due: "2026-10-30" })], "kaputt", 7, none)).toEqual([]);
  });
});

describe("dueOnDay", () => {
  it("counts the others on that day", () => {
    const tasks = [task("a", { due: "2026-10-30" }), task("b", { due: "2026-10-30T18:00:00+01:00" }), task("c", { due: "2026-10-31" }), task("d", { due: "2026-10-30", status: "done" })];
    expect(dueOnDay(tasks, "2026-10-30", none)).toBe(2);
    expect(dueOnDay(tasks, "2026-10-30", none, "a")).toBe(1);
    expect(dueOnDay(tasks, "2026-10-30", (t) => t.id === "b")).toBe(1);
  });
});

describe("crowded day", () => {
  it("is fixed at five", () => {
    expect(CROWDED_DAY).toBe(5);
    expect(isCrowded(4)).toBe(false);
    expect(isCrowded(5)).toBe(true);
    expect(isCrowded(9)).toBe(true);
  });
});

describe("labels", () => {
  it("name the day and the count", () => {
    expect(longDayLabel("2026-10-30")).toBe("Freitag, 30.10.");
    expect(crowdedDayLine("2026-10-30", 9)).toBe("Freitag, 30.10.: 9 Aufgaben fällig.");
    expect(dueCountLabel(1)).toBe("1 fällig");
    expect(crowdedPickHint(1)).toBe("An diesem Tag ist schon 1 Aufgabe fällig.");
    expect(crowdedPickHint(6)).toBe("An diesem Tag sind schon 6 Aufgaben fällig.");
  });
});
