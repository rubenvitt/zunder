// Recurrence rules of the core validation (contract 2.5, "Rhythmen") at the push API. The UI
// never sends such pushes, so the test pushes raw operations like a client would; the mock
// server has to reject them exactly like the real one (same codes and messages).

import { expect, test, type Page } from "@playwright/test";
import { entityFields, prepare, Seeder, suffix, type PushResult } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const YEAR = new Date().getFullYear() + 1;
const DAILY = { every: 1, unit: "day", from: "due" };
const NEEDS_DUE = "Ein Rhythmus braucht eine Fälligkeit. Bitte zusammen mit dem Rhythmus eine Fälligkeit setzen.";
const KEEPS_DUE = "Eine wiederkehrende Aufgabe braucht eine Fälligkeit. Bitte zuerst den Rhythmus entfernen.";
const ROOT_ONLY = "Nur Aufgaben ohne übergeordnete Aufgabe können wiederkehren.";

/** Code and message per rejected op; accepted ops are absent. */
function rejections(result: PushResult): Record<string, { code: string; message: string }> {
  return Object.fromEntries(result.rejected.map((r) => [r.op_id, { code: r.code, message: r.message }]));
}

/** All ops of entity `id` in the seeder, rejected with `code` and `message`. */
function allRejected(seeder: Seeder, id: string, fields: string[], code: string, message: string) {
  return Object.fromEntries(fields.flatMap((f) => seeder.opIds(id, f)).map((op) => [op, { code, message }]));
}

/** A live recurring root task with a due date. */
async function recurringTask(page: Page, title: string): Promise<string> {
  const seeder = new Seeder();
  const id = seeder.task(title, { location: { kind: "loose" }, due: `${YEAR}-03-10`, recurrence: DAILY });
  await seeder.push(page);
  return id;
}

test("Ungültige Form des Rhythmus wird abgelehnt, die übrigen Felder der Aufgabe mit", async ({ page }) => {
  const seeder = new Seeder();
  const id = seeder.task(`Rhythmus mit Wochentag ${suffix()}`, { due: `${YEAR}-03-10`, recurrence: { every: 1, unit: "day", weekdays: ["mo"], from: "due" } });
  const [recurrenceOp] = seeder.opIds(id, "recurrence");
  const others = [...seeder.opIds(id, "title"), ...seeder.opIds(id, "due")];
  const result = await seeder.send(page);

  expect(result.accepted).toEqual([]);
  const byOp = rejections(result);
  expect(byOp[recurrenceOp!]).toEqual({ code: "invalid_value", message: "Wochentage („weekdays“) gehen nur bei wöchentlichen Rhythmen." });
  for (const op of others) expect(byOp[op]?.code).toBe("entity_rejected");
  expect(await entityFields(page, "task", id)).toEqual({});
});

test("Ein Rhythmus braucht eine Fälligkeit", async ({ page }) => {
  const seeder = new Seeder();
  const id = seeder.task(`Rhythmus ohne Fälligkeit ${suffix()}`, { recurrence: DAILY });
  const expected = allRejected(seeder, id, ["title", "recurrence"], "invalid_value", NEEDS_DUE);
  const result = await seeder.send(page);

  expect(result.accepted).toEqual([]);
  expect(rejections(result)).toEqual(expected);
  expect(await entityFields(page, "task", id)).toEqual({});
});

test("Fälligkeit einer wiederkehrenden Aufgabe entfernen geht nur zusammen mit dem Rhythmus", async ({ page }) => {
  const id = await recurringTask(page, `Wiederkehrend ${suffix()}`);

  const removeDue = new Seeder();
  removeDue.raw("task", id, { due: null });
  const expected = allRejected(removeDue, id, ["due"], "invalid_value", KEEPS_DUE);
  expect(rejections(await removeDue.send(page))).toEqual(expected);
  expect(await entityFields(page, "task", id)).toMatchObject({ due: `${YEAR}-03-10`, recurrence: DAILY });

  const endSeries = new Seeder();
  endSeries.raw("task", id, { due: null, recurrence: null });
  const result = await endSeries.send(page);
  expect(result.rejected).toEqual([]);
  expect(result.accepted).toHaveLength(2);
  expect(await entityFields(page, "task", id)).toMatchObject({ due: null, recurrence: null });
});

test("Nur Wurzelaufgaben dürfen wiederkehren", async ({ page }) => {
  const parentSeeder = new Seeder();
  const parent = parentSeeder.task(`Elternaufgabe ${suffix()}`, { location: { kind: "loose" } });
  await parentSeeder.push(page);

  // A new subtask with a rhythm.
  const child = new Seeder();
  const childId = child.task(`Unteraufgabe ${suffix()}`, { parent_id: parent, due: `${YEAR}-03-10`, recurrence: DAILY });
  const expectedChild = allRejected(child, childId, ["title", "parent_id", "due", "recurrence"], "invalid_value", ROOT_ONLY);
  expect(rejections(await child.send(page))).toEqual(expectedChild);
  expect(await entityFields(page, "task", childId)).toEqual({});

  // Hanging a recurring task under another one.
  const id = await recurringTask(page, `Wiederkehrend ${suffix()}`);
  const move = new Seeder();
  move.raw("task", id, { parent_id: parent });
  const expectedMove = allRejected(move, id, ["parent_id"], "invalid_value", ROOT_ONLY);
  expect(rejections(await move.send(page))).toEqual(expectedMove);
  expect((await entityFields(page, "task", id)).parent_id).toBeUndefined();

  // Without the rhythm it may move.
  const moveEnding = new Seeder();
  moveEnding.raw("task", id, { parent_id: parent, recurrence: null });
  expect((await moveEnding.send(page)).rejected).toEqual([]);
  expect(await entityFields(page, "task", id)).toMatchObject({ parent_id: parent, recurrence: null });
});

test("recurrence_prev setzt nur der Server", async ({ page }) => {
  const previous = await recurringTask(page, `Vorgängerin ${suffix()}`);
  const seeder = new Seeder();
  const id = seeder.task(`Selbst angelegte Instanz ${suffix()}`, { due: `${YEAR}-03-11`, recurrence: DAILY, recurrence_prev: previous });
  const [prevOp] = seeder.opIds(id, "recurrence_prev");
  const others = ["title", "due", "recurrence"].flatMap((f) => seeder.opIds(id, f));
  const result = await seeder.send(page);

  expect(result.accepted).toEqual([]);
  const byOp = rejections(result);
  expect(byOp[prevOp!]).toEqual({
    code: "invalid_value",
    message: "Die Vorgängerin in einer Serie („recurrence_prev“) setzt nur der Server, wenn er die nächste Instanz anlegt.",
  });
  for (const op of others) expect(byOp[op]?.code).toBe("entity_rejected");
  expect(await entityFields(page, "task", id)).toEqual({});
});
