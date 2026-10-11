// Task detail: inline fields commit only real edits, drafts stay with their task,
// unsafe links are not clickable, flame/Timing feedback, links to single tasks.

import { expect, test, type Page } from "@playwright/test";
import { entityFields, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const detail = (page: Page) => page.getByRole("complementary", { name: "Aufgabendetail" });

async function openLoose(page: Page, title: string) {
  await openApp(page, "#/lose");
  await row(page, title).locator(".task-main").click();
  await expect(detail(page)).toBeVisible();
}

test("Fokus ohne Tippen überschreibt keine Änderung von außen", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Agentenziel ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openLoose(page, `Agentenziel ${s}`);

  const description = detail(page).getByLabel("Beschreibung (Markdown)");
  await description.click();
  // An agent writes the description while the field has the focus.
  seeder.raw("task", id, { description: "Plan vom Agenten" });
  await seeder.push(page);
  await expect(description).toHaveValue("Plan vom Agenten");
  await detail(page).locator("h3", { hasText: "Links" }).click();
  await page.waitForTimeout(300);
  expect((await entityFields(page, "task", id)).description).toBe("Plan vom Agenten");

  // A real edit is still committed on blur.
  await description.fill("Eigener Text");
  await detail(page).locator("h3", { hasText: "Links" }).click();
  await expect.poll(async () => (await entityFields(page, "task", id)).description).toBe("Eigener Text");
});

test("Ein abgelehnter Wert verschwindet aus dem Feld", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Behält Titel ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openLoose(page, `Behält Titel ${s}`);
  const title = detail(page).getByLabel("Titel", { exact: true });
  await title.fill("");
  await title.press("Enter");
  await expect(page.locator(".toast-error", { hasText: "Eine Aufgabe braucht einen Titel." })).toBeVisible();
  await expect(title).toHaveValue(`Behält Titel ${s}`);
});

test("Entwürfe bleiben nicht im Detail einer anderen Aufgabe stehen", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const parent = seeder.task(`Eltern ${s}`, { location: { kind: "loose" } });
  seeder.task(`Kind A ${s}`, { parent_id: parent });
  await seeder.push(page);
  await openLoose(page, `Eltern ${s}`);

  await detail(page).getByLabel("Neue Unteraufgabe").fill("Gehört zu Eltern");
  await detail(page).getByLabel("Link-URL").fill("https://example.org/eltern");
  await detail(page).locator(".subtask .link-button", { hasText: `Kind A ${s}` }).click();
  await expect(detail(page).getByLabel("Titel", { exact: true })).toHaveValue(`Kind A ${s}`);
  await expect(detail(page).getByLabel("Neue Unteraufgabe")).toHaveValue("");
  await expect(detail(page).getByLabel("Link-URL")).toHaveValue("");
});

test("Links mit anderen Schemata als http(s)/mailto sind nicht klickbar", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Links ${s}`, {
    location: { kind: "loose" },
    links: [{ url: "javascript:alert(1)", title: "Böse" }, { url: "https://example.org/", title: "Beispiel" }],
  });
  await seeder.push(page);
  await openLoose(page, `Links ${s}`);
  const list = detail(page).locator(".link-list").first();
  await expect(list).toContainText("Böse");
  await expect(list.locator('a[href^="javascript:"]')).toHaveCount(0);
  await expect(list.getByRole("link", { name: "Beispiel" })).toHaveAttribute("href", "https://example.org/");
});

test("Flamme: ein Timing-Fehler wird gemeldet statt „Flamme gestartet“", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Flamme ${s}`, { location: { kind: "loose" }, phoenix: { role: "flamme", fire: "2026-W42" } });
  await seeder.push(page);
  await page.route("**/api/tasks/*/flame/start", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        running: true,
        unchanged: false,
        stopped_task_ids: [],
        timing: { configured: true, used: false, entry: null, error: "Timing-Token abgelaufen." },
        warnings: [],
      }),
    }),
  );
  await openLoose(page, `Flamme ${s}`);
  await detail(page).getByRole("button", { name: "Flamme starten" }).click();
  await expect(page.locator(".toast-warning")).toHaveText(/Flamme läuft, aber Timing: Timing-Token abgelaufen\./);
  await expect(page.locator(".toast", { hasText: "Flamme gestartet." })).toHaveCount(0);
});

test("Erledigt mit offenen Unteraufgaben zeigt genau einen Hinweis", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const parent = seeder.task(`Eltern ${s}`, { location: { kind: "loose" } });
  seeder.task(`Kind ${s}`, { parent_id: parent });
  await seeder.push(page);
  await openApp(page, "#/lose");
  const pushed = page.waitForResponse((r) => r.url().endsWith("/api/sync/push") && r.request().method() === "POST");
  await row(page, `Eltern ${s}`).locator(".check").click();
  const body = (await (await pushed).json()) as { warnings: { code: string }[] };
  expect(body.warnings.map((w) => w.code)).toContain("open_subtasks");
  await page.waitForTimeout(200);
  await expect(page.locator(".toast-warning")).toHaveCount(1);
});

test("#/aufgabe/<id> öffnet die Aufgabe über ihrer Sicht", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Verlinkt ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, `#/aufgabe/${id}`);
  await expect(page.locator(".view-head h1")).toHaveText("Lose");
  await expect(detail(page).getByLabel("Titel", { exact: true })).toHaveValue(`Verlinkt ${s}`);
  await expect(detail(page).getByRole("link", { name: "Link" })).toHaveAttribute("href", `#/aufgabe/${id}`);

  await page.goto("/#/aufgabe/01900000-0000-7000-8000-00000000dead");
  await expect(page.locator(".toast-error", { hasText: "Aufgabe nicht gefunden" })).toBeVisible();
  await expect(page.locator(".view-head h1")).toHaveText("Heute");
});

test("Heute: das Datum im Kopf folgt der Uhr über Mitternacht", async ({ page }) => {
  await page.clock.install({ time: new Date("2026-10-11T23:59:00+02:00") });
  await openApp(page, "#/heute");
  await expect(page.locator(".view-head .muted")).toHaveText("Sonntag, 11. Oktober");
  await page.clock.fastForward("02:00");
  await expect(page.locator(".view-head .muted")).toHaveText("Montag, 12. Oktober");
});

test("Dunkles Design: Text auf Fehler-Toasts ist dunkel", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Dunkel ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openLoose(page, `Dunkel ${s}`);
  const title = detail(page).getByLabel("Titel", { exact: true });
  await title.fill("");
  await title.press("Enter");
  const toast = page.locator(".toast-error").first();
  await expect(toast).toBeVisible();
  expect(await toast.evaluate((el) => getComputedStyle(el).color)).toBe("rgb(22, 19, 15)");
});
