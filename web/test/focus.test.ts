// "Erst für dich" (contract 5.1): which Glut task stands at the top of Heute, what the empty
// state offers, and how a dismissal or a pick holds for one day, also offline.

import { describe, expect, it } from "vitest";
import {
  changeLocally,
  chosenFor,
  confirmed,
  FOCUS_CHOICES,
  fromServer,
  isDismissed,
  isFocusCandidate,
  NO_FOCUS_SETTINGS,
  parseFocusLocal,
  pickFocus,
  type FocusLocal,
} from "../src/sync/focus";
import type { Task, TaskStatus } from "../src/sync/types";

const TODAY = "2026-10-17";

function task(id: string, extra: Partial<Task> = {}): Task {
  return {
    id,
    title: id,
    status: "open",
    weight: "glut",
    for_whom: "mich",
    waiting_on: null,
    due: null,
    deferrals: 0,
    created_hlc: "1760000000000:00000:test",
    ...extra,
  } as Task;
}

const none = () => false;
const ids = (tasks: Task[]) => tasks.map((t) => t.id);

describe("isFocusCandidate", () => {
  it("takes Glut that is open or in progress", () => {
    expect(isFocusCandidate(task("t"), TODAY)).toBe(true);
    expect(isFocusCandidate(task("t", { status: "in_progress" }), TODAY)).toBe(true);
    for (const weight of ["brand", "rauch", "asche", null] as const) expect(isFocusCandidate(task("t", { weight }), TODAY)).toBe(false);
  });

  it("not what cannot be done today: blocked, waiting on someone, in review, closed", () => {
    for (const status of ["blocked", "review", "done", "dropped"] as TaskStatus[]) expect(isFocusCandidate(task("t", { status }), TODAY), status).toBe(false);
    expect(isFocusCandidate(task("t", { waiting_on: "Anna" }), TODAY)).toBe(false);
  });

  it("not what was put off past today with Heute nicht; a plain later deadline is fine", () => {
    expect(isFocusCandidate(task("t", { deferrals: 1, due: "2026-10-18" }), TODAY)).toBe(false);
    expect(isFocusCandidate(task("t", { deferrals: 1, due: "2026-10-18T09:00:00+02:00" }), TODAY)).toBe(false);
    expect(isFocusCandidate(task("t", { deferrals: 1, due: TODAY }), TODAY)).toBe(true);
    expect(isFocusCandidate(task("t", { deferrals: 2, due: "2026-10-10" }), TODAY)).toBe(true);
    expect(isFocusCandidate(task("t", { due: "2026-11-30" }), TODAY)).toBe(true);
  });
});

describe("pickFocus", () => {
  it("Glut for oneself: the earliest due first, else the oldest; ties by ID", () => {
    const tasks = [
      task("undated-new", { created_hlc: "1760000000009:00000:test" }),
      task("later", { due: "2026-10-30" }),
      task("sooner", { due: "2026-10-20T08:00:00+02:00" }),
      task("undated-old", { created_hlc: "1760000000001:00000:test" }),
    ];
    expect(pickFocus(tasks, TODAY, none).task?.id).toBe("sooner");
    expect(pickFocus(tasks.filter((t) => !t.due), TODAY, none).task?.id).toBe("undated-old");
    expect(pickFocus([task("b"), task("a")], TODAY, none).task?.id).toBe("a");
    expect(pickFocus([task("a"), task("b")], TODAY, none).task?.id).toBe("a");
  });

  it("only for oneself at the top; uns and unset are offered, others never", () => {
    const tasks = [task("uns", { for_whom: "uns" }), task("unset", { for_whom: null }), task("andere", { for_whom: "andere" })];
    const focus = pickFocus(tasks, TODAY, none);
    expect(focus.task).toBeNull();
    expect(ids(focus.choices)).toEqual(["uns", "unset"]);
  });

  it("leaves out what the caller excludes (deleted, dev projects)", () => {
    const tasks = [task("dev", { due: "2026-10-01" }), task("mine")];
    expect(pickFocus(tasks, TODAY, (t) => t.id === "dev").task?.id).toBe("mine");
    expect(pickFocus(tasks, TODAY, () => true)).toEqual({ task: null, chosen: false, choices: [] });
  });

  it("offers the best few to pick, in the same order", () => {
    const tasks = ["e", "d", "c", "b", "a"].map((id, i) => task(id, { for_whom: null, due: `2026-10-2${i}` }));
    const focus = pickFocus(tasks, TODAY, none);
    expect(focus.choices).toHaveLength(FOCUS_CHOICES);
    expect(ids(focus.choices)).toEqual(["e", "d", "c"]);
  });

  it("a task picked for today wins while it can still be done", () => {
    const tasks = [task("mine", { due: "2026-10-01" }), task("picked", { for_whom: "uns" })];
    expect(pickFocus(tasks, TODAY, none, "picked")).toMatchObject({ task: { id: "picked" }, chosen: true, choices: [] });
    const done = [tasks[0]!, task("picked", { for_whom: "uns", status: "done" })];
    expect(pickFocus(done, TODAY, none, "picked")).toMatchObject({ task: { id: "mine" }, chosen: false });
    expect(pickFocus(tasks, TODAY, none, "unknown").task?.id).toBe("mine");
  });
});

describe("one day only", () => {
  it("a dismissal or a pick holds on its day, not the next", () => {
    const settings = { dismissed_on: TODAY, chosen: { on: TODAY, task_id: "t1" } };
    expect(isDismissed(settings, TODAY)).toBe(true);
    expect(chosenFor(settings, TODAY)).toBe("t1");
    expect(isDismissed(settings, "2026-10-18")).toBe(false);
    expect(chosenFor(settings, "2026-10-18")).toBeNull();
    expect(isDismissed(NO_FOCUS_SETTINGS, TODAY)).toBe(false);
  });
});

describe("offline and in step with the server", () => {
  const start: FocusLocal = { known: NO_FOCUS_SETTINGS, pending: null };

  it("a local change holds at once and waits to be sent", () => {
    const local = changeLocally(start, { dismissed_on: TODAY });
    expect(local.known.dismissed_on).toBe(TODAY);
    expect(local.pending).toEqual({ dismissed_on: TODAY });
    // Still offline: a second change joins the first.
    const more = changeLocally(local, { chosen: { on: TODAY, task_id: "t1" } });
    expect(more.pending).toEqual({ dismissed_on: TODAY, chosen: { on: TODAY, task_id: "t1" } });
  });

  it("a value read from the server keeps unsent changes on top", () => {
    const local = changeLocally(start, { dismissed_on: TODAY });
    const server = { dismissed_on: "2026-10-16", chosen: { on: TODAY, task_id: "other-device" } };
    expect(fromServer(local, server).known).toEqual({ dismissed_on: TODAY, chosen: { on: TODAY, task_id: "other-device" } });
    expect(fromServer(start, server)).toEqual({ known: server, pending: null });
  });

  it("confirmation clears what was sent, not what changed meanwhile", () => {
    const sent = { dismissed_on: TODAY };
    const local = changeLocally(changeLocally(start, sent), { chosen: { on: TODAY, task_id: "t2" } });
    const server = { dismissed_on: TODAY, chosen: null };
    const after = confirmed(local, sent, server);
    expect(after.pending).toEqual({ chosen: { on: TODAY, task_id: "t2" } });
    expect(after.known).toEqual({ dismissed_on: TODAY, chosen: { on: TODAY, task_id: "t2" } });
    expect(confirmed(changeLocally(start, sent), sent, server)).toEqual({ known: server, pending: null });
    // Undone while on its way: the undo is still to be sent.
    const undone = changeLocally(changeLocally(start, sent), { dismissed_on: null });
    expect(confirmed(undone, sent, server).pending).toEqual({ dismissed_on: null });
  });

  it("reads stored state defensively", () => {
    expect(parseFocusLocal(null)).toEqual(start);
    expect(parseFocusLocal("kaputt")).toEqual(start);
    expect(parseFocusLocal(JSON.stringify({ known: { dismissed_on: 3, chosen: { on: TODAY } } }))).toEqual(start);
    const stored: FocusLocal = { known: { dismissed_on: TODAY, chosen: null }, pending: { dismissed_on: TODAY } };
    expect(parseFocusLocal(JSON.stringify(stored))).toEqual(stored);
  });
});
