import { expect, test } from "@playwright/test";
import { entityFields, openApp, prepare, row, Seeder, suffix } from "./fixtures";

function currentFire(): string {
  const d = new Date();
  const date = new Date(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()));
  const day = date.getUTCDay() || 7;
  date.setUTCDate(date.getUTCDate() + 4 - day);
  const week = Math.ceil(((date.getTime() - Date.UTC(date.getUTCFullYear(), 0, 1)) / 86400000 + 1) / 7);
  return `${date.getUTCFullYear()}-W${String(week).padStart(2, "0")}`;
}

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

test("Flamme starten und beenden", async ({ page }) => {
  const s = suffix();
  const fire = currentFire();
  const seeder = new Seeder();
  const area = seeder.area(`Schreiben ${s}`);
  const flameId = seeder.task(`Deep Work ${s}`, { location: { kind: "area", id: area }, phoenix: { role: "flamme", fire } });
  seeder.task(`Schritt ${s}`, { location: { kind: "loose" }, phoenix: { role: "feuer_schritt", fire } });
  await seeder.push(page);

  await openApp(page, "#/phoenix");
  await expect(page.locator(".fire-label")).toHaveText(`Feuer ${fire}`);
  await expect(row(page, `Schritt ${s}`)).toBeVisible();

  const button = page.locator(`[data-flame-for="${flameId}"]`);
  await expect(button).toHaveText(/Flamme starten/);
  await button.click();
  await expect(button).toHaveText(/Flamme beenden/);
  await expect.poll(async () => (await entityFields(page, "task", flameId)).flame_started_at).toBeTruthy();

  await page.keyboard.press("g");
  await page.keyboard.press("h");
  await expect(page.locator(".group-flame")).toContainText(`Deep Work ${s}`);

  await page.keyboard.press("g");
  await page.keyboard.press("p");
  await button.click();
  await expect(button).toHaveText(/Flamme starten/);
  await expect.poll(async () => (await entityFields(page, "task", flameId)).flame_ended_at).toBeTruthy();
});
