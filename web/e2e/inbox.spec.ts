import { expect, test } from "@playwright/test";
import { entityFields, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

test("Inbox abarbeiten mit l, d und a", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const areaId = seeder.area(`Haushalt ${s}`);
  const loose = seeder.task(`Lose machen ${s}`);
  const drop = seeder.task(`Verwerfen ${s}`);
  const assign = seeder.task(`Zuordnen ${s}`);
  await seeder.push(page);
  await openApp(page, "#/inbox");

  await row(page, `Lose machen ${s}`).locator(".task-main").focus();
  await page.keyboard.press("l");
  await expect(row(page, `Lose machen ${s}`)).toHaveCount(0);

  await row(page, `Verwerfen ${s}`).locator(".task-main").focus();
  await page.keyboard.press("d");
  await expect(row(page, `Verwerfen ${s}`)).toHaveCount(0);

  await row(page, `Zuordnen ${s}`).locator(".task-main").focus();
  await page.keyboard.press("a");
  const picker = page.getByRole("dialog");
  await expect(picker).toContainText("zuordnen");
  await page.keyboard.type(`Haushalt ${s}`);
  await page.keyboard.press("Enter");
  await expect(row(page, `Zuordnen ${s}`)).toHaveCount(0);

  await expect.poll(async () => (await entityFields(page, "task", loose)).location).toEqual({ kind: "loose" });
  await expect.poll(async () => (await entityFields(page, "task", drop)).status).toBe("dropped");
  await expect.poll(async () => (await entityFields(page, "task", assign)).location).toEqual({ kind: "area", id: areaId });

  await page.keyboard.press("g");
  await page.keyboard.press("l");
  await expect(row(page, `Lose machen ${s}`)).toBeVisible();
});

test("x markiert die ausgewählte Aufgabe als erledigt, Detail zeigt Protokoll", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const parent = seeder.task(`Eltern ${s}`, { location: { kind: "loose" } });
  seeder.task(`Kind offen ${s}`, { parent_id: parent });
  await seeder.push(page);
  await openApp(page, "#/lose");

  await row(page, `Eltern ${s}`).locator(".task-main").click();
  const detail = page.getByRole("complementary", { name: "Aufgabendetail" });
  await expect(detail).toBeVisible();
  await expect(detail.locator(".subtask", { hasText: `Kind offen ${s}` })).toBeVisible();
  await expect(detail.locator("h3", { hasText: "Unteraufgaben" })).toContainText("0/1");

  await detail.getByLabel("Neue Unteraufgabe").fill(`Kind zwei ${s}`);
  await detail.getByLabel("Neue Unteraufgabe").press("Enter");
  await expect(detail.locator("h3", { hasText: "Unteraufgaben" })).toContainText("0/2");

  await page.locator("body").click({ position: { x: 5, y: 5 } });
  await row(page, `Eltern ${s}`).locator(".task-main").focus();
  await page.keyboard.press("x");
  await expect(row(page, `Eltern ${s}`)).toHaveAttribute("data-status", "done");
  await expect(page.locator(".toast-warning", { hasText: "Unteraufgaben sind noch offen" })).toBeVisible();
  await expect(detail.locator(".notice-warning")).toBeVisible();
  await expect(detail.locator(".history-list li").first()).toContainText("Status");
});
