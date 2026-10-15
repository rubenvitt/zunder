// Recurring tasks (contract 5.1, "Rhythmen"): set a rhythm in the detail, check the task
// off and see the server's next instance arrive with the right due date. The mock server
// creates follow-up instances like the real one (mock-recurrence.ts).

import { expect, test, type Page } from "@playwright/test";
import { formatDateLabel } from "../src/sync/dates";
import { entityFields, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

const detail = (page: Page) => page.getByRole("complementary", { name: "Aufgabendetail" });
/** A year that is always ahead, so the series never adjusts to today. */
const YEAR = new Date().getFullYear() + 1;
const FEB_LAST = new Date(YEAR, 2, 0).getDate();

/** Live tasks naming `id` as their previous instance. */
async function successors(page: Page, id: string): Promise<{ id: string; fields: Record<string, Json> }[]> {
  const res = await page.request.get("/api/snapshot");
  const body = (await res.json()) as { entities: { entity: string; entity_id: string; fields: Record<string, { value: Json }> }[] };
  return body.entities
    .filter((e) => e.entity === "task" && e.fields.recurrence_prev?.value === id && e.fields.deleted?.value !== true)
    .map((e) => ({ id: e.entity_id, fields: Object.fromEntries(Object.entries(e.fields).map(([k, v]) => [k, v.value])) }));
}

const openRow = (page: Page, title: string) => page.locator(".task-row:not(.closed)", { has: page.locator(".task-title", { hasText: title }) });

test("Rhythmus setzen, abhaken: die nächste Instanz erscheint mit der richtigen Fälligkeit", async ({ page }) => {
  const s = suffix();
  const title = `Odoo absenden und Stunden bestätigen ${s}`;
  const seeder = new Seeder();
  const id = seeder.task(title, { location: { kind: "loose" }, due: `${YEAR}-01-31`, links: [{ url: "https://odoo.example.org/" }] });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, title).locator(".task-main").click();

  const select = detail(page).getByRole("combobox", { name: "Wiederholen" });
  await expect(select).toBeEnabled();
  await select.selectOption({ label: "Monatlich am Monatsletzten" });
  await expect.poll(async () => (await entityFields(page, "task", id)).recurrence).toEqual({ every: 1, unit: "month", day: "last", from: "due" });
  await expect(row(page, title).locator(".chip-recurring")).toHaveAttribute("title", "Wiederkehrend: Monatlich am Monatsletzten");

  await detail(page).locator(".detail-head .check").click();
  const nextDue = `${YEAR}-02-${FEB_LAST}`;
  await expect(page.locator(".toast", { hasText: "Nächste Instanz" })).toHaveText(`Nächste Instanz am ${formatDateLabel(nextDue, new Date())}`);

  // The new instance: open, same place and links, next month's last day, same rhythm.
  await expect(openRow(page, title)).toHaveCount(1);
  const [next, ...more] = await successors(page, id);
  expect(more).toEqual([]);
  expect(next!.fields).toMatchObject({
    title,
    due: nextDue,
    status: "open",
    location: { kind: "loose" },
    links: [{ url: "https://odoo.example.org/" }],
    recurrence: { every: 1, unit: "month", day: "last", from: "due" },
  });
  await expect(openRow(page, title)).toHaveAttribute("data-task-id", next!.id);
  await expect(openRow(page, title).locator(".chip-recurring")).toBeVisible();

  // The series is linked both ways in the detail.
  await detail(page).getByRole("link", { name: "Nächste" }).click();
  await expect(detail(page).getByLabel("Fälligkeitsdatum")).toHaveValue(nextDue);
  await expect(detail(page).getByRole("link", { name: "Vorherige" })).toBeVisible();

  // Reopening and checking off again creates no second instance.
  await detail(page).getByRole("link", { name: "Vorherige" }).click();
  await detail(page).locator(".detail-head .check").click();
  await detail(page).locator(".detail-head .check").click();
  await expect.poll(async () => (await entityFields(page, "task", id)).status).toBe("done");
  await page.waitForTimeout(500);
  expect(await successors(page, id)).toHaveLength(1);
});

test("Ohne Fälligkeit ist Wiederholen deaktiviert, Unteraufgaben bieten es nicht an", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const parent = seeder.task(`Ohne Termin ${s}`, { location: { kind: "loose" } });
  seeder.task(`Unteraufgabe ${s}`, { parent_id: parent, due: `${YEAR}-03-01` });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, `Ohne Termin ${s}`).locator(".task-main").click();

  await expect(detail(page).getByRole("combobox", { name: "Wiederholen" })).toBeDisabled();
  await expect(detail(page).getByText("Erst eine Fälligkeit setzen")).toBeVisible();

  await detail(page).locator(".subtask-title", { hasText: `Unteraufgabe ${s}` }).click();
  await expect(detail(page).locator(".detail-title")).toHaveValue(`Unteraufgabe ${s}`);
  await expect(detail(page).getByRole("combobox", { name: "Wiederholen" })).toHaveCount(0);
});

test("Eigener Rhythmus: Intervall, Wochentage, Monatstag und Anker", async ({ page }) => {
  const s = suffix();
  const title = `Eigener Rhythmus ${s}`;
  const seeder = new Seeder();
  const id = seeder.task(title, { location: { kind: "loose" }, due: `${YEAR}-01-04` });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, title).locator(".task-main").click();
  const recurrence = async () => (await entityFields(page, "task", id)).recurrence;

  await detail(page).getByRole("combobox", { name: "Wiederholen" }).selectOption({ label: "Benutzerdefiniert …" });
  await expect.poll(recurrence).toEqual({ every: 1, unit: "week", from: "due" });
  const editor = detail(page).getByRole("group", { name: "Rhythmus anpassen" });
  await editor.getByRole("button", { name: "Montag" }).click();
  await expect.poll(recurrence).toEqual({ every: 1, unit: "week", weekdays: ["mo"], from: "due" });
  await editor.getByRole("button", { name: "Donnerstag" }).click();
  await expect(editor.getByRole("button", { name: "Donnerstag" })).toHaveAttribute("aria-pressed", "true");
  await expect.poll(recurrence).toEqual({ every: 1, unit: "week", weekdays: ["mo", "th"], from: "due" });
  await editor.getByLabel("Intervall").fill("2");
  await editor.getByLabel("Intervall").press("Enter");
  await expect.poll(recurrence).toEqual({ every: 2, unit: "week", weekdays: ["mo", "th"], from: "due" });
  await expect(detail(page).getByRole("combobox", { name: "Wiederholen" }).locator("option:checked")).toHaveText("Alle 2 Wochen (Mo, Do)");

  // Another unit drops the weekdays; months offer a day.
  await editor.getByLabel("Einheit").selectOption("month");
  await expect.poll(recurrence).toEqual({ every: 2, unit: "month", from: "due" });
  await editor.getByLabel("Tag im Monat").selectOption({ label: "am Monatsletzten" });
  await editor.getByLabel("Anker").selectOption({ label: "nach Erledigung" });
  await expect.poll(recurrence).toEqual({ every: 2, unit: "month", day: "last", from: "done" });
  await expect(editor).toContainText("Alle 2 Monate am Monatsletzten nach Erledigung");

  await detail(page).getByRole("combobox", { name: "Wiederholen" }).selectOption({ label: "Nicht wiederholen" });
  await expect.poll(recurrence).toBeNull();
  await expect(editor).toHaveCount(0);
  await expect(row(page, title).locator(".chip-recurring")).toHaveCount(0);
});

test("Verwerfen fragt: nur diese Instanz oder die ganze Serie", async ({ page }) => {
  const s = suffix();
  const title = `Wochenbericht ${s}`;
  const seeder = new Seeder();
  const id = seeder.task(title, { due: `${YEAR}-01-04`, recurrence: { every: 1, unit: "week", from: "due" } });
  await seeder.push(page);
  await openApp(page, "#/inbox");
  await row(page, title).locator(".task-main").focus();

  const dialog = page.getByRole("dialog", { name: "Wiederkehrende Aufgabe verwerfen" });
  await page.keyboard.press("d");
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  expect((await entityFields(page, "task", id)).status).not.toBe("dropped");

  // Enter: only this one; the next instance follows (in the inbox again).
  await row(page, title).locator(".task-main").focus();
  await page.keyboard.press("d");
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(page.locator(".toast", { hasText: "Verworfen, die nächste Instanz folgt." })).toBeVisible();
  await expect.poll(async () => (await entityFields(page, "task", id)).status).toBe("dropped");
  await expect.poll(async () => (await successors(page, id)).map((t) => t.fields.due)).toEqual([`${YEAR}-01-11`]);
  const [next] = await successors(page, id);
  await expect(row(page, title)).toHaveAttribute("data-task-id", next!.id);

  // "Serie beenden": dropped and without rhythm, no further instance.
  await row(page, title).locator(".task-main").focus();
  await page.keyboard.press("d");
  await dialog.getByRole("button", { name: "Serie beenden" }).click();
  await expect.poll(async () => entityFields(page, "task", next!.id)).toMatchObject({ status: "dropped", recurrence: null });
  await page.waitForTimeout(500);
  expect(await successors(page, next!.id)).toEqual([]);
});

test("Fälligkeit entfernen beendet nach Rückfrage die Serie", async ({ page }) => {
  const s = suffix();
  const title = `Serie ohne Termin ${s}`;
  const seeder = new Seeder();
  const id = seeder.task(title, { location: { kind: "loose" }, due: `${YEAR}-05-01`, recurrence: { every: 1, unit: "year", from: "due" } });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, title).locator(".task-main").click();
  await expect(detail(page).getByRole("combobox", { name: "Wiederholen" }).locator("option:checked")).toHaveText("Jährlich");

  page.once("dialog", (d) => {
    expect(d.message()).toBe("Ohne Fälligkeit endet die Serie.");
    void d.dismiss();
  });
  await detail(page).getByRole("button", { name: "Fälligkeit entfernen" }).click();
  await page.waitForTimeout(300);
  expect((await entityFields(page, "task", id)).due).toBe(`${YEAR}-05-01`);

  page.once("dialog", (d) => void d.accept());
  await detail(page).getByRole("button", { name: "Fälligkeit entfernen" }).click();
  await expect.poll(async () => entityFields(page, "task", id)).toMatchObject({ due: null, recurrence: null });
  await expect(detail(page).getByRole("combobox", { name: "Wiederholen" })).toBeDisabled();
});
