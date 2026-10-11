// Phone layout (Pixel 7 project, tests tagged @mobile).

import { expect, test } from "@playwright/test";
import { entityFields, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

test("Mobil: Inbox-Aufgabe direkt aus der Liste zuordnen @mobile", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Mobilbereich ${s}`);
  const id = seeder.task(`Mobil zuordnen ${s}`);
  await seeder.push(page);
  await openApp(page, "#/inbox");
  const r = row(page, `Mobil zuordnen ${s}`);
  // Loose and drop are swipes; assigning is a visible button, the others stay hidden.
  await expect(r.getByRole("button", { name: "Zuordnen", exact: true })).toBeVisible();
  await expect(r.getByRole("button", { name: "Lose", exact: true })).toBeHidden();
  await r.getByRole("button", { name: "Zuordnen", exact: true }).tap();
  await expect(page.getByRole("dialog")).toContainText("zuordnen");
  await page.keyboard.type(`Mobilbereich ${s}`);
  await page.keyboard.press("Enter");
  await expect(r).toHaveCount(0);
  await expect.poll(async () => (await entityFields(page, "task", id)).location).toEqual({ kind: "area", id: area });
});

test("Mobil: das Detail nimmt den Fokus, die verdeckte Liste ist inert @mobile", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Mobil Detail ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, `Mobil Detail ${s}`).locator(".task-main").tap();
  const detail = page.getByRole("complementary", { name: "Aufgabendetail" });
  await expect(detail).toBeVisible();
  expect(await page.evaluate(() => !!document.activeElement?.closest(".detail"))).toBe(true);
  await expect(page.locator("main.main")).toHaveJSProperty("inert", true);
  await detail.getByRole("button", { name: "Schließen (Esc)" }).tap();
  await expect(detail).toBeHidden();
  await expect(page.locator("main.main")).toHaveJSProperty("inert", false);
  await expect.poll(() => page.evaluate(() => document.activeElement?.closest("[data-task-id]")?.getAttribute("data-task-id"))).toBe(id);
});
