import { expect, test, type Page } from "@playwright/test";
import { entityFields, openApp, prepare, row, Seeder, suffix, WEB_HEADERS } from "./fixtures";

function currentFire(): string {
  // The browser runs in Europe/Berlin (playwright.config.ts), the test process may not.
  const [y, m, d] = new Intl.DateTimeFormat("en-CA", { timeZone: "Europe/Berlin" }).format(new Date()).split("-").map(Number);
  const date = new Date(Date.UTC(y!, m! - 1, d!));
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
  try {
    await flameRun(page, s, fire, flameId);
  } finally {
    // The real backend keeps the flame, and a running flame is Heute's focus in later tests.
    const res = await page.request.post(`/api/tasks/${flameId}/flame/stop`, { headers: WEB_HEADERS });
    expect([200, 409]).toContain(res.status());
  }
});

async function flameRun(page: Page, s: string, fire: string, flameId: string): Promise<void> {
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
  // The running flame is Heute's focus, glowing.
  const box = page.getByRole("region", { name: "Fokus" });
  await expect(box).toHaveClass(/focus-burning/);
  await expect(box.locator(".task-title")).toHaveText(`Deep Work ${s}`);
  await expect(box.getByRole("button", { name: "Flamme beenden" })).toBeVisible();

  await page.keyboard.press("g");
  await page.keyboard.press("p");
  await button.click();
  await expect(button).toHaveText(/Flamme starten/);
  await expect.poll(async () => (await entityFields(page, "task", flameId)).flame_ended_at).toBeTruthy();

  // A stopped flame starts again.
  await button.click();
  await expect(button).toHaveText(/Flamme beenden/);
  await expect.poll(async () => (await entityFields(page, "task", flameId)).flame_ended_at ?? null).toBeNull();
}
