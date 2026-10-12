// Phone layout (Pixel 7 project, tests tagged @mobile).

import { expect, test, type Page } from "@playwright/test";
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

/** A one-finger horizontal swipe (Chromium only: touch input over CDP). */
async function swipe(page: Page, fromX: number, toX: number, y: number): Promise<void> {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x: fromX, y }] });
  for (let i = 1; i <= 10; i++) {
    await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [{ x: fromX + ((toX - fromX) * i) / 10, y }] });
  }
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  await cdp.detach();
}

test("Mobil: Zurück führt von der Unteraufgabe zur Elternaufgabe und schließt dann das Detail @mobile", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const parent = seeder.task(`Mobil Eltern ${s}`, { location: { kind: "loose" } });
  seeder.task(`Mobil Kind ${s}`, { parent_id: parent });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, `Mobil Eltern ${s}`).locator(".task-main").tap();
  const detail = page.getByRole("complementary", { name: "Aufgabendetail" });
  const title = detail.getByLabel("Titel", { exact: true });
  await expect(title).toHaveValue(`Mobil Eltern ${s}`);
  await detail.locator(".subtask .link-button", { hasText: `Mobil Kind ${s}` }).tap();
  await expect(title).toHaveValue(`Mobil Kind ${s}`);

  await page.goBack();
  await expect(title).toHaveValue(`Mobil Eltern ${s}`);
  await page.goBack();
  await expect(detail).toBeHidden();
  await expect(page.locator("main.main")).toHaveJSProperty("inert", false);
  expect(new URL(page.url()).hash).toBe("#/lose");
  // Forward opens it again, as anywhere else.
  await page.goForward();
  await expect(title).toHaveValue(`Mobil Eltern ${s}`);
});

test("Mobil: Wischen vom linken Rand schließt das Detail, Schließen hinterlässt keinen Verlaufseintrag @mobile", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Mobil Wischen ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, "#/lose");
  const detail = page.getByRole("complementary", { name: "Aufgabendetail" });
  const open = async () => {
    await row(page, `Mobil Wischen ${s}`).locator(".task-main").tap();
    await expect(detail).toBeVisible();
    expect(await page.evaluate(() => history.state)).toMatchObject({ zunderDepth: 1 });
  };

  // The close button takes the entry back out: Back afterwards does not hit an invisible step.
  await open();
  await detail.getByRole("button", { name: "Schließen (Esc)" }).tap();
  await expect(detail).toBeHidden();
  await expect.poll(() => page.evaluate(() => history.state)).toBeNull();

  // A swipe that does not start at the edge leaves the detail open (it may be scrolling or editing).
  await open();
  await swipe(page, 150, 380, 400);
  await page.waitForTimeout(400);
  await expect(detail).toBeVisible();

  // From the edge it goes back.
  await swipe(page, 4, 300, 400);
  await expect(detail).toBeHidden();
  await expect(page.locator("main.main")).toHaveJSProperty("inert", false);
  await expect.poll(() => page.evaluate(() => history.state)).toBeNull();
  expect(new URL(page.url()).hash).toBe("#/lose");
});
