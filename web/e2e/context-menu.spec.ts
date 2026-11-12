// Right click on a task (contract 5.1, "Kontextmenü"): in the browser the web UI draws
// the menu; the desktop app's native menu is covered in tauri.spec.ts.

import { expect, test, type Page } from "@playwright/test";
import { localDate } from "../src/sync/dates";
import { deferDay } from "../src/sync/defer";
import { card, entityFields, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const menu = (page: Page) => page.getByRole("menu").first();

test("Rechtsklick auf eine Zeile: erledigen, Fälligkeit und Gewicht über Untermenüs", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const done = seeder.task(`Abhaken ${s}`, { location: { kind: "loose" } });
  const plan = seeder.task(`Planen ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, "#/lose");

  await row(page, `Abhaken ${s}`).locator(".task-main").click({ button: "right" });
  await expect(menu(page)).toBeVisible();
  // The row the menu belongs to is the selected one.
  await expect(row(page, `Abhaken ${s}`)).toHaveAttribute("aria-selected", "true");
  await menu(page).getByRole("menuitem", { name: "Erledigt" }).click();
  await expect(page.getByRole("menu")).toHaveCount(0);
  await expect.poll(async () => (await entityFields(page, "task", done)).status).toBe("done");

  // Submenu by pointer: Fälligkeit → Morgen.
  await row(page, `Planen ${s}`).locator(".task-title").click({ button: "right" });
  await menu(page).getByRole("menuitem", { name: "Fälligkeit" }).hover();
  await page.getByRole("menuitemcheckbox", { name: "Morgen" }).click();
  await expect.poll(async () => (await entityFields(page, "task", plan)).due).toBe(deferDay("tomorrow", localDate(new Date())));

  // Submenu by keyboard: Gewicht → Glut, the current choice carries the check mark.
  await row(page, `Planen ${s}`).locator(".task-main").click({ button: "right" });
  await menu(page).getByRole("menuitem", { name: "Gewicht" }).focus();
  await page.keyboard.press("ArrowRight");
  const weights = page.getByRole("menu").nth(1);
  await expect(weights.getByRole("menuitemcheckbox", { name: /^Ohne Gewicht/ })).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("ArrowDown");
  await expect(weights.getByRole("menuitemcheckbox", { name: /^Glut/ })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect.poll(async () => (await entityFields(page, "task", plan)).weight).toBe("glut");
  await expect(row(page, `Planen ${s}`).locator(".chip-weight-glut")).toBeVisible();
});

test("Kontextmenü: Esc schließt ohne Aktion, Tasten wirken solange nicht auf die Liste", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Bleibt offen ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, "#/lose");

  await row(page, `Bleibt offen ${s}`).locator(".task-main").click({ button: "right" });
  await expect(menu(page)).toBeVisible();
  // "x" inside the menu does not complete the selected task.
  await page.keyboard.press("x");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toHaveCount(0);
  expect((await entityFields(page, "task", id)).status ?? "open").toBe("open");

  // A click outside closes it as well.
  await row(page, `Bleibt offen ${s}`).locator(".task-main").click({ button: "right" });
  // Inside the rounded corner of the sheet, not on its cut-off tip.
  await page.locator("main").first().click({ position: { x: 30, y: 30 } });
  await expect(page.getByRole("menu")).toHaveCount(0);
});

test("Kontextmenü auf Board-Karten: verschieben über die Auswahl, löschen nach Rückfrage", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const areaId = seeder.area(`Garten ${s}`);
  const target = seeder.area(`Keller ${s}`);
  // Dev projects show the board.
  const projectId = seeder.project(`Beete ${s}`, areaId, { kind: "dev", repo: "example/beete" });
  const moved = seeder.task(`Umziehen ${s}`, { location: { kind: "project", id: projectId } });
  const gone = seeder.task(`Weg damit ${s}`, { location: { kind: "project", id: projectId } });
  await seeder.push(page);
  await openApp(page, `#/bereiche/project/${projectId}`);

  await card(page, `Umziehen ${s}`).click({ button: "right" });
  await menu(page).getByRole("menuitem", { name: "Verschieben …" }).click();
  await expect(page.getByRole("dialog")).toContainText("verschieben");
  await page.keyboard.type(`Keller ${s}`);
  await page.keyboard.press("Enter");
  await expect.poll(async () => (await entityFields(page, "task", moved)).location).toEqual({ kind: "area", id: target });

  page.once("dialog", (d) => void d.accept());
  await card(page, `Weg damit ${s}`).click({ button: "right" });
  await menu(page).getByRole("menuitem", { name: "Löschen …" }).click();
  await expect(card(page, `Weg damit ${s}`)).toHaveCount(0);
  await expect.poll(async () => (await entityFields(page, "task", gone)).deleted).toBe(true);
});

test("Textfelder behalten das Menü des Systems", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Mit Detail ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, `Mit Detail ${s}`).locator(".task-main").click();
  const detail = page.getByRole("complementary", { name: "Aufgabendetail" });
  await detail.getByLabel("Neue Unteraufgabe").click({ button: "right" });
  await expect(page.getByRole("menu")).toHaveCount(0);
});
