// "Zurückgeben" and "Wartet auf andere" (contract 5.1): a Rauch task goes back to whoever asked,
// with a sentence to copy, whom it waits on and a reminder in three working days.

import { expect, test, type Page } from "@playwright/test";
import { toRfc3339Local } from "../src/sync/dates";
import { handbackRemindAt } from "../src/sync/handback";
import { entityFields, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const detail = (page: Page) => page.getByRole("complementary", { name: "Aufgabendetail" });
const clipboard = (page: Page) => page.evaluate(() => navigator.clipboard.readText());

/** Closes seeded Brand tasks again, so they leave Heute and later tests (real backend keeps data). */
async function close(page: Page, ...ids: string[]): Promise<void> {
  const seeder = new Seeder();
  for (const id of ids) seeder.raw("task", id, { status: "done" });
  await seeder.push(page);
}

test("Zurückgeben: Satz kopieren, auf jemanden warten, in drei Werktagen erinnern", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Bericht ${s}`, { location: { kind: "loose" }, weight: "rauch" });
  // Due first wins: this one beats every other open Rauch or Brand task.
  const offer = seeder.task(`Angebot ${s}`, { location: { kind: "loose" }, weight: "brand", due: "2001-01-01" });
  await seeder.push(page);
  try {
    await openApp(page, "#/lose");
    await row(page, `Bericht ${s}`).locator(".task-main").click();
    await detail(page).getByRole("button", { name: "Zurückgeben" }).click();

    const dialog = page.getByRole("dialog", { name: "Zurückgeben" });
    const sentence = `Ich kann „Bericht ${s}“ oder „Angebot ${s}“ diese Woche machen. Was davon ist dir wichtiger?`;
    await expect(dialog.getByLabel("Satz zum Verschicken")).toHaveValue(sentence);
    await expect(dialog.getByRole("checkbox", { name: /In 3 Werktagen erinnern/ })).toBeChecked();
    // The name is required.
    const submit = dialog.getByRole("button", { name: "Zurückgeben" });
    await expect(submit).toBeDisabled();
    await dialog.getByLabel("Wer entscheidet?").fill(`Anna ${s}`);
    await submit.click();
    await expect(dialog).toBeHidden();

    await expect.poll(() => clipboard(page)).toBe(sentence);
    await expect.poll(async () => {
      const f = await entityFields(page, "task", id);
      return [f.waiting_on, f.status, f.remind_at];
    }).toEqual([`Anna ${s}`, "blocked", handbackRemindAt(new Date())]);
    await expect(page.locator(".toast")).toContainText(`Satz kopiert. „Bericht ${s}“ wartet auf Anna ${s}. Erinnerung:`);
    await expect(row(page, `Bericht ${s}`).locator(".chip-waiting-on")).toHaveText(`wartet auf Anna ${s}`);
    await expect(detail(page).getByLabel("Wartet auf", { exact: true })).toHaveValue(`Anna ${s}`);
    // Handed back already: no second "Zurückgeben" while it waits, neither in the detail nor in the palette.
    await expect(detail(page).getByRole("button", { name: "Zurückgeben" })).toHaveCount(0);
    await page.keyboard.press("Control+k");
    await expect(page.locator(".palette-item").first()).toBeVisible();
    await expect(page.locator(".palette-item", { hasText: "Zurückgeben" })).toHaveCount(0);
    await page.keyboard.press("Escape");

    // Wartet auf andere: grouped by name; the answer clears it and the task opens again.
    await page.locator(".sidebar").getByRole("link", { name: "Wartet auf andere" }).click();
    await expect(page.getByRole("heading", { name: "Wartet auf andere" })).toBeVisible();
    const group = page.getByRole("list", { name: `Wartet auf Anna ${s}` });
    await expect(group.locator(".task-title")).toHaveText([`Bericht ${s}`]);
    await group.getByRole("button", { name: "Antwort ist da" }).click();
    await expect(group).toHaveCount(0);
    await expect.poll(async () => {
      const f = await entityFields(page, "task", id);
      return [f.waiting_on, f.status];
    }).toEqual([null, "open"]);
  } finally {
    await close(page, offer);
  }
});

/** A reminder early today (local time), so it is due in Heute. */
function todayEarly(): string {
  const d = new Date();
  return toRfc3339Local(new Date(d.getFullYear(), d.getMonth(), d.getDate(), 0, 5));
}

test("Wiedervorlage in Heute: wartet noch, nachfragen oder Antwort ist da, Dev-Projekte nicht", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const s = suffix();
  const seeder = new Seeder();
  const remind_at = todayEarly();
  const id = seeder.task(`Bericht ${s}`, { location: { kind: "loose" }, weight: "rauch", waiting_on: `Anna ${s}`, status: "blocked", remind_at });
  const plain = seeder.task(`Anruf ${s}`, { location: { kind: "loose" }, remind_at });
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`App ${s}`, area, { kind: "dev", repo: "example/app", view: "list" });
  const ticket = seeder.task(`Ticket ${s}`, { location: { kind: "project", id: project }, waiting_on: `Bob ${s}`, status: "blocked", remind_at });
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    const followUps = page.getByRole("list", { name: "Wiedervorlage" });
    const reminders = page.getByRole("list", { name: "Erinnerungen" });
    await expect(followUps.locator(".task-title")).toHaveText([`Bericht ${s}`]);
    await expect(row(page, `Bericht ${s}`).locator(".chip-waiting-on")).toHaveText(`wartet noch auf Anna ${s}`);
    // Without a name and in dev projects it stays a plain reminder.
    await expect(reminders.locator(".task-title", { hasText: `Anruf ${s}` })).toHaveCount(1);
    await expect(reminders.locator(".task-title", { hasText: `Ticket ${s}` })).toHaveCount(1);

    await followUps.getByRole("button", { name: "Nachfragen" }).click();
    await expect.poll(() => clipboard(page)).toBe(`Kurze Nachfrage zu „Bericht ${s}“: Wie wichtig ist es dir?`);
    await expect(page.locator(".toast")).toContainText(`Satz kopiert. „Bericht ${s}“ wartet weiter auf Anna ${s}.`);
    expect((await entityFields(page, "task", id)).waiting_on).toBe(`Anna ${s}`);

    await followUps.getByRole("button", { name: "Antwort ist da" }).click();
    await expect(followUps).toHaveCount(0);
    await expect.poll(async () => {
      const f = await entityFields(page, "task", id);
      return [f.waiting_on, f.status];
    }).toEqual([null, "open"]);
    // The reminder stays for today, now as a plain one.
    await expect(reminders.locator(".task-title", { hasText: `Bericht ${s}` })).toHaveCount(1);
  } finally {
    await close(page, id, plain, ticket);
  }
});

test("Ohne Häkchen keine Erinnerung, Abbrechen ändert nichts", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Umfrage ${s}`, { location: { kind: "loose" }, weight: "rauch" });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, `Umfrage ${s}`).locator(".task-main").click();

  const dialog = page.getByRole("dialog", { name: "Zurückgeben" });
  await detail(page).getByRole("button", { name: "Zurückgeben" }).click();
  await dialog.getByLabel("Wer entscheidet?").fill(`Ben ${s}`);
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  expect((await entityFields(page, "task", id)).waiting_on ?? null).toBeNull();

  await detail(page).getByRole("button", { name: "Zurückgeben" }).click();
  await dialog.getByLabel("Wer entscheidet?").fill(`Ben ${s}`);
  await dialog.getByRole("checkbox", { name: /In 3 Werktagen erinnern/ }).uncheck();
  await dialog.getByLabel("Wer entscheidet?").press("Enter");
  await expect(dialog).toBeHidden();
  await expect.poll(async () => {
    const f = await entityFields(page, "task", id);
    return [f.waiting_on, f.status, f.remind_at ?? null];
  }).toEqual([`Ben ${s}`, "blocked", null]);
});

test("Wartet auf: freies Feld an jeder Aufgabe, setzen blockiert, leeren öffnet", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Vorlage ${s}`, { location: { kind: "loose" }, waiting_on: `Carla ${s}` });
  const id = seeder.task(`Schlüssel ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, `Schlüssel ${s}`).locator(".task-main").click();

  // Not Rauch: no "Zurückgeben", but the field and the names used so far.
  await expect(detail(page).getByRole("button", { name: "Zurückgeben" })).toHaveCount(0);
  const field = detail(page).getByLabel("Wartet auf", { exact: true });
  await expect(detail(page).locator(`datalist option[value="Carla ${s}"]`)).toHaveCount(1);
  await field.fill(`  Dora ${s} `);
  await field.press("Enter");
  await expect.poll(async () => {
    const f = await entityFields(page, "task", id);
    return [f.waiting_on, f.status];
  }).toEqual([`Dora ${s}`, "blocked"]);
  await expect(row(page, `Schlüssel ${s}`).locator(".chip-waiting-on")).toHaveText(`wartet auf Dora ${s}`);

  await detail(page).getByRole("button", { name: "Wartet auf entfernen" }).click();
  await expect.poll(async () => {
    const f = await entityFields(page, "task", id);
    return [f.waiting_on, f.status];
  }).toEqual([null, "open"]);
  await expect(row(page, `Schlüssel ${s}`).locator(".chip-waiting-on")).toHaveCount(0);
});

test("Befehlspalette bietet Zurückgeben nur bei Rauch, Dev-Projekte nie", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Formular ${s}`, { location: { kind: "loose" }, weight: "rauch" });
  seeder.task(`Steuer ${s}`, { location: { kind: "loose" }, weight: "glut" });
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`App ${s}`, area, { kind: "dev", repo: "example/app", view: "list" });
  seeder.task(`Ticket ${s}`, { location: { kind: "project", id: project }, weight: "rauch" });
  await seeder.push(page);
  await openApp(page, "#/lose");

  const palette = page.locator(".palette-item", { hasText: "Zurückgeben" });
  await row(page, `Steuer ${s}`).locator(".task-main").click();
  await page.keyboard.press("Control+k");
  await expect(page.locator(".palette-item").first()).toBeVisible();
  await expect(palette).toHaveCount(0);
  await page.keyboard.press("Escape");

  await row(page, `Formular ${s}`).locator(".task-main").click();
  await page.keyboard.press("Control+k");
  await palette.click();
  await expect(page.getByRole("dialog", { name: "Zurückgeben" }).getByLabel("Satz zum Verschicken")).toHaveValue(new RegExp(`„Formular ${s}“`));
  await page.keyboard.press("Escape");

  await openApp(page, `#/bereiche/project/${project}`);
  await row(page, `Ticket ${s}`).locator(".task-main").click();
  await expect(detail(page).getByRole("group", { name: "Gewicht" })).toBeVisible();
  await expect(detail(page).getByRole("button", { name: "Zurückgeben" })).toHaveCount(0);
  await page.keyboard.press("Control+k");
  await expect(page.locator(".palette-item").first()).toBeVisible();
  await expect(palette).toHaveCount(0);
});

test("Wartet auf andere ist mobil ein Tab, die Antwort ein Knopf @mobile", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Rückruf ${s}`, { location: { kind: "loose" }, waiting_on: `Eva ${s}` });
  await seeder.push(page);
  await openApp(page, "#/heute");
  await page.locator(".tabbar").getByRole("button", { name: "Wartet" }).click();
  await expect(page.getByRole("heading", { name: "Wartet auf andere" })).toBeVisible();
  const group = page.getByRole("list", { name: `Wartet auf Eva ${s}` });
  await expect(group.getByRole("button", { name: "Antwort ist da" })).toBeVisible();
});
