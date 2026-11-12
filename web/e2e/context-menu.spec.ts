// Right click on a task (contract 5.1, "Kontextmenü"): in the browser the web UI draws
// the menu; the desktop app's native menu is covered in tauri.spec.ts.

import { expect, test, type Page } from "@playwright/test";
import { localDate } from "../src/sync/dates";
import { deferDay } from "../src/sync/defer";
import { card, entityFields, newId, openApp, prepare, row, Seeder, suffix } from "./fixtures";

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

test("Kontextmenü auf Bereichen und Projekten: Ansicht, Status, Zustand, Bearbeiten", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const areaId = seeder.area(`Haus ${s}`);
  const projectId = seeder.project(`Dach ${s}`, areaId);
  await seeder.push(page);
  await openApp(page, `#/bereiche/area/${areaId}`);
  const tree = page.getByRole("navigation", { name: "Bereiche und Projekte" });

  // Project in the tree: view and status in submenus.
  await tree.getByRole("link", { name: `Dach ${s}` }).click({ button: "right" });
  await menu(page).getByRole("menuitem", { name: "Ansicht" }).hover();
  await page.getByRole("menuitemcheckbox", { name: "Board" }).click();
  await expect.poll(async () => (await entityFields(page, "project", projectId)).view).toBe("board");
  await tree.getByRole("link", { name: `Dach ${s}` }).click({ button: "right" });
  await menu(page).getByRole("menuitem", { name: "Status" }).hover();
  await page.getByRole("menuitemcheckbox", { name: "Pausiert" }).click();
  await expect.poll(async () => (await entityFields(page, "project", projectId)).status).toBe("paused");

  // The project chip in the area opens the same menu; "Öffnen" navigates.
  await page.locator(".project-chips").getByRole("button", { name: new RegExp(`Dach ${s}`) }).click({ button: "right" });
  await menu(page).getByRole("menuitem", { name: "Öffnen", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`#/bereiche/project/${projectId}$`));

  // Area: "Neues Projekt …" opens the dialog for this area, "Zustand" archives.
  await tree.getByRole("link", { name: `Haus ${s}` }).click({ button: "right" });
  await menu(page).getByRole("menuitem", { name: "Neues Projekt …" }).click();
  await expect(page.getByRole("dialog")).toContainText("Neues Projekt");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await tree.getByRole("link", { name: `Haus ${s}` }).click({ button: "right" });
  await menu(page).getByRole("menuitem", { name: "Zustand" }).hover();
  await page.getByRole("menuitemcheckbox", { name: "Archiviert" }).click();
  await expect.poll(async () => (await entityFields(page, "area", areaId)).state).toBe("archived");
});

test("Kontextmenü auf Kommentaren: löschen nach Rückfrage", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const taskId = seeder.task(`Kommentiert ${s}`, { location: { kind: "loose" } });
  const commentId = seeder.raw("comment", newId(), { task_id: taskId, body: `Notiz ${s}` });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, `Kommentiert ${s}`).locator(".task-main").click();
  const comment = page.locator(`[data-comment-id="${commentId}"]`);
  // The detail scrolls smoothly, and scrolling closes the menu: first bring the comment into view.
  await comment.scrollIntoViewIfNeeded();
  await expect
    .poll(async () => {
      const before = (await comment.boundingBox())?.y;
      await page.waitForTimeout(100);
      return before === (await comment.boundingBox())?.y;
    })
    .toBe(true);
  await comment.locator(".comment-author").click({ button: "right" });
  page.once("dialog", (d) => void d.accept());
  await menu(page).getByRole("menuitem", { name: "Löschen …" }).click();
  await expect(comment).toHaveCount(0);
  await expect.poll(async () => (await entityFields(page, "comment", commentId)).deleted).toBe(true);
});

test.describe("Touch", () => {
  test.use({ hasTouch: true });

  test("Langes Drücken öffnet das Menü, der Finger öffnet danach nicht die Aufgabe", async ({ page }) => {
    const s = suffix();
    const seeder = new Seeder();
    seeder.task(`Gedrückt ${s}`, { location: { kind: "loose" } });
    await seeder.push(page);
    await openApp(page, "#/lose");

    const target = row(page, `Gedrückt ${s}`).locator(".task-title");
    const box = (await target.boundingBox())!;
    const at = { clientX: box.x + 10, clientY: box.y + box.height / 2, pointerType: "touch", isPrimary: true, pointerId: 7, bubbles: true };
    await target.dispatchEvent("pointerdown", at);
    await page.waitForTimeout(700);
    await expect(menu(page)).toBeVisible();
    await target.dispatchEvent("pointerup", at);
    await target.dispatchEvent("click", { clientX: at.clientX, clientY: at.clientY, bubbles: true });
    await expect(menu(page)).toBeVisible();
    await expect(page.getByRole("complementary", { name: "Aufgabendetail" })).toHaveCount(0);

    // A short tap is still a tap: no menu.
    await page.keyboard.press("Escape");
    await target.dispatchEvent("pointerdown", at);
    await page.waitForTimeout(100);
    await target.dispatchEvent("pointerup", at);
    await page.waitForTimeout(600);
    await expect(page.getByRole("menu")).toHaveCount(0);
  });
});
