// Keyboard and focus: shortcuts only act on what is visible, focused controls keep
// their native Enter, dialogs keep the focus, the detail panel takes and returns it.

import { expect, test } from "@playwright/test";
import { entityFields, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const detail = (page: import("@playwright/test").Page) => page.getByRole("complementary", { name: "Aufgabendetail" });

test("Tastenkürzel wirken nicht auf unsichtbare Aufgaben der vorigen Sicht", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const one = seeder.task(`Inbox eins ${s}`);
  const two = seeder.task(`Inbox zwei ${s}`);
  await seeder.push(page);
  await openApp(page, "#/inbox");
  await expect(row(page, `Inbox eins ${s}`)).toBeVisible();

  // Bereiche without a selection shows no list.
  await page.keyboard.press("g");
  await page.keyboard.press("b");
  await expect(page.locator(".split-main")).toContainText("Bereich oder Projekt links auswählen.");
  for (const key of ["j", "Enter", "x", "k", "x"]) await page.keyboard.press(key);

  // The token page has no list either.
  await page.keyboard.press("g");
  await page.keyboard.press("i");
  await expect(row(page, `Inbox eins ${s}`)).toBeVisible();
  await page.locator(".sidebar").getByRole("link", { name: "API-Tokens" }).click();
  await expect(page.locator(".view-head h1")).toBeVisible();
  for (const key of ["j", "x", "k", "x"]) await page.keyboard.press(key);

  await expect(detail(page)).toBeHidden();
  await expect(page.locator(".toast")).toHaveCount(0);
  for (const id of [one, two]) expect((await entityFields(page, "task", id)).status ?? "open").toBe("open");
});

test("Enter auf einem fokussierten Knopf löst dessen Aktion aus", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Knopf ${s}`);
  await seeder.push(page);
  await openApp(page, "#/inbox");
  const r = row(page, `Knopf ${s}`);
  await r.locator(".task-main").focus();

  // Row action: the location picker opens, not the detail.
  await r.getByRole("button", { name: "Zuordnen", exact: true }).focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog")).toContainText("zuordnen");
  await expect(detail(page)).toBeHidden();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toBeHidden();

  // The detail's close button closes it.
  await r.locator(".task-main").click();
  await expect(detail(page)).toBeVisible();
  await detail(page).getByRole("button", { name: "Schließen (Esc)" }).focus();
  await page.keyboard.press("Enter");
  await expect(detail(page)).toBeHidden();

  // The sidebar button opens quick capture although a task is selected.
  await page.locator(".sidebar").getByRole("button", { name: "Neue Aufgabe" }).focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog", { name: "Schnellerfassung" })).toBeVisible();
});

test("Nach Klick auf einen Navigationslink öffnen j und Enter die Auswahl", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Lose eins ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, "#/heute");
  await page.locator(".sidebar").getByRole("link", { name: "Lose" }).click();
  await expect(row(page, `Lose eins ${s}`)).toBeVisible();
  await page.keyboard.press("j");
  await page.keyboard.press("Enter");
  await expect(detail(page)).toBeVisible();
});

test("Dialoge halten den Fokus (Tab und Shift+Tab)", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("n");
  const dialog = page.getByRole("dialog", { name: "Schnellerfassung" });
  await expect(dialog).toBeVisible();
  await page.keyboard.type("Fokus bleibt drin");
  const inside = () => page.evaluate(() => !!document.activeElement?.closest("[role=dialog]"));
  for (let i = 0; i < 6; i++) {
    await page.keyboard.press("Tab");
    expect(await inside()).toBe(true);
  }
  for (let i = 0; i < 6; i++) {
    await page.keyboard.press("Shift+Tab");
    expect(await inside()).toBe(true);
  }
});

test("Detail nimmt beim Öffnen den Fokus und gibt ihn beim Schließen zurück", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Fokus ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, `Fokus ${s}`).locator(".task-main").click();
  await expect(detail(page)).toBeVisible();
  expect(await page.evaluate(() => !!document.activeElement?.closest(".detail"))).toBe(true);
  // Shortcuts keep working from the panel.
  await page.keyboard.press("Escape");
  await expect(detail(page)).toBeHidden();
  await expect.poll(() => page.evaluate(() => document.activeElement?.closest("[data-task-id]")?.getAttribute("data-task-id"))).toBe(id);
});
