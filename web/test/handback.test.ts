// "Zurückgeben" (contract 5.1): sentence, second task and reminder in three working days.

import { describe, expect, it } from "vitest";
import { handbackRemindAt, handbackSentence, offersHandBack, pickAlternative, waitingNames, workdaysLater } from "../src/sync/handback";
import type { Task } from "../src/sync/types";

function task(id: string, extra: Partial<Task> = {}): Task {
  return {
    id,
    title: id,
    status: "open",
    weight: "rauch",
    waiting_on: null,
    due: null,
    created_hlc: "1760000000000:00000:test",
    ...extra,
  } as Task;
}

describe("offersHandBack", () => {
  it("offers Rauch that is not closed, outside dev projects", () => {
    expect(offersHandBack(task("t"), false)).toBe(true);
    expect(offersHandBack(task("t", { status: "in_progress" }), false)).toBe(true);
    expect(offersHandBack(task("t", { status: "blocked" }), false)).toBe(true);
    expect(offersHandBack(task("t", { weight: "brand" }), false)).toBe(false);
    expect(offersHandBack(task("t", { weight: null }), false)).toBe(false);
    expect(offersHandBack(task("t", { status: "done" }), false)).toBe(false);
    expect(offersHandBack(task("t", { status: "dropped" }), false)).toBe(false);
    expect(offersHandBack(task("t"), true)).toBe(false);
  });

  it("not once the task waits on someone: it was handed back already", () => {
    expect(offersHandBack(task("t", { status: "blocked", waiting_on: "Anna" }), false)).toBe(false);
    expect(offersHandBack(task("t", { waiting_on: "Anna" }), false)).toBe(false);
  });
});

describe("workdaysLater", () => {
  it("skips weekends and lands at 9:00 local time", () => {
    // Friday → Wednesday, Monday → Thursday, Saturday and Sunday → Wednesday.
    expect(workdaysLater(new Date(2026, 9, 16, 17, 45), 3)).toEqual(new Date(2026, 9, 21, 9));
    expect(workdaysLater(new Date(2026, 9, 19, 8), 3)).toEqual(new Date(2026, 9, 22, 9));
    expect(workdaysLater(new Date(2026, 9, 17, 12), 3)).toEqual(new Date(2026, 9, 21, 9));
    expect(workdaysLater(new Date(2026, 9, 18, 23, 59), 3)).toEqual(new Date(2026, 9, 21, 9));
    // Wednesday → Monday.
    expect(workdaysLater(new Date(2026, 9, 21, 10), 3)).toEqual(new Date(2026, 9, 26, 9));
  });

  it("writes the reminder with the local offset, also across the change to winter time", () => {
    expect(handbackRemindAt(new Date(2026, 9, 16, 17))).toBe("2026-10-21T09:00:00+02:00");
    expect(handbackRemindAt(new Date(2026, 9, 23, 17))).toBe("2026-10-28T09:00:00+01:00");
  });
});

describe("handbackSentence", () => {
  it("offers a choice when there is a second task", () => {
    expect(handbackSentence("Bericht", "Folien")).toBe("Ich kann „Bericht“ oder „Folien“ diese Woche machen. Was davon ist dir wichtiger?");
  });

  it("asks plainly without one", () => {
    expect(handbackSentence("Bericht", null)).toBe("Ich schaffe „Bericht“ diese Woche nicht. Wie wichtig ist es dir?");
  });
});

describe("pickAlternative", () => {
  const current = task("current");
  const none = () => false;

  it("takes open Rauch or Brand tasks only, never the task itself", () => {
    const candidates = [
      current,
      task("glut", { weight: "glut" }),
      task("unweighed", { weight: null }),
      task("done", { status: "done" }),
      task("blocked", { status: "blocked" }),
      task("waiting", { waiting_on: "Anna" }),
    ];
    expect(pickAlternative(current, candidates, none)).toBeNull();
    expect(pickAlternative(current, [...candidates, task("brand", { weight: "brand", status: "in_progress" })], none)?.id).toBe("brand");
  });

  it("prefers the task due first, then the oldest", () => {
    const old = task("old", { created_hlc: "1750000000000:00000:test" });
    const later = task("later", { due: "2026-10-30" });
    const sooner = task("sooner", { due: "2026-10-20T08:00:00+02:00", weight: "brand" });
    expect(pickAlternative(current, [old, later, sooner], none)?.id).toBe("sooner");
    expect(pickAlternative(current, [task("young", { created_hlc: "1770000000000:00000:test" }), old], none)?.id).toBe("old");
  });

  it("leaves out what the caller excludes (dev projects, deleted)", () => {
    const dev = task("dev", { due: "2026-01-01" });
    const other = task("other");
    expect(pickAlternative(current, [dev, other], (t) => t.id === "dev")?.id).toBe("other");
  });
});

describe("waitingNames", () => {
  it("lists each name once, sorted", () => {
    const tasks = [task("a", { waiting_on: "Ben" }), task("b", { waiting_on: "anna" }), task("c", { waiting_on: "Anna " }), task("d")];
    expect(waitingNames(tasks)).toEqual(["anna", "Ben"]);
  });
});
