import { expect, test } from "@playwright/test";
import { entityFields, isMock, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

test("Schnellerfassung: nur Titel landet in der Inbox", async ({ page }) => {
  await openApp(page);
  const title = `Milch kaufen ${suffix()}`;
  await page.keyboard.press("n");
  await page.getByLabel("Titel der neuen Aufgabe").fill(title);
  await page.keyboard.press("Enter");
  await expect(page.locator(".toast", { hasText: "Erfasst in Inbox" })).toBeVisible();
  await page.keyboard.press("g");
  await page.keyboard.press("i");
  await expect(row(page, title)).toBeVisible();
});

test("Schnellerfassung mit Kurzschreibweise setzt Fälligkeit, Erinnerung und Bereich", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const areaId = seeder.area(`Gesundheit ${s}`);
  await seeder.push(page);
  await openApp(page);

  await page.keyboard.press("n");
  const input = page.getByLabel("Titel der neuen Aufgabe");
  await input.fill(`Zahnarzt anrufen morgen 9:00 #gesundheit-${s}`);
  // Live preview of the parsed shorthand.
  await expect(page.locator(".capture-preview")).toContainText("Zahnarzt anrufen");
  await expect(page.locator(".capture-preview")).toContainText(`Gesundheit ${s}`);
  await expect(page.locator(".capture-preview")).toContainText("Morgen 09:00");
  await page.keyboard.press("Enter");

  await page.goto(`/#/bereiche/area/${areaId}`);
  const taskRow = row(page, "Zahnarzt anrufen");
  await expect(taskRow).toBeVisible();
  await expect(taskRow.locator(".chip", { hasText: "Morgen 09:00" })).toBeVisible();

  const id = await taskRow.getAttribute("data-task-id");
  await expect
    .poll(async () => entityFields(page, "task", id!))
    .toMatchObject({ title: "Zahnarzt anrufen", location: { kind: "area", id: areaId } });
  const fields = await entityFields(page, "task", id!);
  expect(fields.due).toMatch(/T09:00:00[+-]\d{2}:\d{2}$/);
  expect(fields.remind_at).toBe(fields.due);
});

test("Abgelehnte Operation wird zurückgerollt und gemeldet", async ({ page }) => {
  test.skip(!isMock, "Nutzt die Testregel des Mock-Servers");
  await openApp(page, "#/inbox");
  await page.keyboard.press("n");
  await page.getByLabel("Titel der neuen Aufgabe").fill("Darf nicht bleiben [ablehnen]");
  await page.keyboard.press("Enter");
  await expect(page.locator(".toast-error", { hasText: "Änderung abgelehnt" })).toBeVisible();
  await expect(row(page, "Darf nicht bleiben")).toHaveCount(0);
});

test("Clientseitige Validierung verhindert leere Titel im Detail", async ({ page }) => {
  const seeder = new Seeder();
  const title = `Umbenennen ${suffix()}`;
  seeder.task(title);
  await seeder.push(page);
  await openApp(page, "#/inbox");
  await row(page, title).locator(".task-main").click();
  const field = page.getByLabel("Titel", { exact: true });
  await field.fill("");
  await field.press("Enter");
  await expect(page.locator(".toast-error", { hasText: "Eine Aufgabe braucht einen Titel." })).toBeVisible();
  await expect(row(page, title)).toBeVisible();
});
