// Logbuch „Erledigt“ (design 7): days newest first with the day's flame minutes, what was done and
// below it what was only worked on. Dev tasks and dropped ones are left out, older days load in steps
// of 14. Flames come from a flame that started a few minutes ago (the server alone writes flame_log;
// a running flame counts until now). The real backend keeps data between tests, so seeded tasks are
// deleted in `finally`.

import { expect, test, type Page } from "@playwright/test";
import { toRfc3339Local } from "../src/sync/dates";
import { newId, openApp, prepare, Seeder, suffix, WEB_HEADERS } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
  // A flame of a few minutes must have started on today's date.
  test.skip(new Date().getHours() === 0 && new Date().getMinutes() < 8, "Die Flamme begänne gestern.");
});

const loose = { kind: "loose" };
const logRow = (page: Page, title: string) => page.locator(".log-row", { has: page.locator(".log-title", { hasText: title }) });
const sevenMinutesAgo = () => toRfc3339Local(new Date(Date.now() - 7 * 60_000));

async function remove(page: Page, tasks: string[]): Promise<void> {
  const seeder = new Seeder();
  for (const id of tasks) seeder.raw("task", id, { deleted: true });
  await seeder.push(page);
}

test("zeigt erledigte Aufgaben mit Minuten und Tagessumme", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const goal = seeder.raw("goal", newId(), { name: `Sport ${s}`, emoji: "🏃", state: "active", rank: "a" });
  const ids = [
    seeder.task(`Gebrannt ${s}`, { location: loose, goal_id: goal, status: "done", flame_started_at: sevenMinutesAgo() }),
    seeder.task(`Still ${s}`, { location: loose, status: "done" }),
  ];
  await seeder.push(page);
  try {
    await openApp(page, "#/erledigt");
    await expect(page.getByRole("heading", { name: "Erledigt", level: 1 })).toBeVisible();
    const today = page.locator(".log-day", { has: logRow(page, `Gebrannt ${s}`) });
    const head = today.locator(".log-day-head");
    await expect(head).toContainText(new Date().toLocaleDateString("de-DE", { weekday: "long", day: "numeric", month: "long" }));
    await expect(head.locator(".log-day-minutes")).toHaveText(/^\d+ min$/);

    const burned = logRow(page, `Gebrannt ${s}`);
    await expect(burned.locator(".log-mark")).toBeVisible();
    await expect(burned.locator(".goal-mark")).toHaveText("🏃");
    await expect(burned.locator(".log-minutes")).toHaveText(/^[67] min$/);
    // No minutes, no figure.
    const quiet = logRow(page, `Still ${s}`);
    await expect(quiet).toBeVisible();
    await expect(quiet.locator(".log-minutes")).toHaveCount(0);

    // A click opens the task like in other lists.
    await burned.getByRole("button").first().click();
    await expect(page.getByRole("complementary", { name: "Aufgabendetail" })).toContainText(`Gebrannt ${s}`);
  } finally {
    await remove(page, ids);
  }
});

test("Daran gearbeitet steht unter dem Erledigten", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const ids = [
    seeder.task(`Erledigt ${s}`, { location: loose, status: "done" }),
    seeder.task(`Angefangen ${s}`, { location: loose, flame_started_at: sevenMinutesAgo() }),
  ];
  await seeder.push(page);
  try {
    await openApp(page, "#/erledigt");
    const day = page.locator(".log-day", { has: logRow(page, `Angefangen ${s}`) });
    const done = day.locator(".log-done");
    const worked = day.locator(".log-worked");
    await expect(done).toContainText(`Erledigt ${s}`);
    await expect(done).not.toContainText(`Angefangen ${s}`);
    await expect(worked.getByText("Daran gearbeitet")).toBeVisible();
    await expect(worked).toContainText(`Angefangen ${s}`);
    await expect(worked).not.toContainText(`Erledigt ${s}`);
    await expect(logRow(page, `Angefangen ${s}`).locator(".log-minutes")).toHaveText(/^[67] min$/);
    // The sub-label comes after the done rows.
    const doneBox = (await done.boundingBox())!;
    const workedBox = (await worked.boundingBox())!;
    expect(workedBox.y).toBeGreaterThan(doneBox.y);
  } finally {
    await remove(page, ids);
  }
});

test("Dev-Aufgaben und verworfene fehlen", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Bereich ${s}`);
  const dev = seeder.project(`Dienst ${s}`, area, { kind: "dev" });
  const ids = [
    seeder.task(`Sichtbar ${s}`, { location: loose, status: "done" }),
    seeder.task(`Dev ${s}`, { location: { kind: "project", id: dev }, status: "done", flame_started_at: sevenMinutesAgo() }),
    seeder.task(`Verworfen ${s}`, { location: loose, status: "dropped", flame_started_at: sevenMinutesAgo() }),
  ];
  await seeder.push(page);
  try {
    await openApp(page, "#/erledigt");
    await expect(logRow(page, `Sichtbar ${s}`)).toBeVisible();
    await expect(page.locator(".log-row")).not.toHaveCount(0);
    await expect(page.getByText(`Dev ${s}`)).toHaveCount(0);
    await expect(page.getByText(`Verworfen ${s}`)).toHaveCount(0);
  } finally {
    await remove(page, ids);
  }
});

test("Ältere zeigen lädt weitere Tage", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const ids = [seeder.task(`Heute ${s}`, { location: loose, status: "done" })];
  // Done 20 days ago: the status operation carries that day's HLC.
  const old = newId();
  const then = Date.now() - 20 * 86_400_000;
  const ops = [
    { field: "title", value: `Vor Wochen ${s}`, wall: Date.now() },
    { field: "location", value: loose, wall: Date.now() },
    { field: "status", value: "done", wall: then },
  ].map((o, i) => ({
    op_id: newId(),
    entity: "task",
    entity_id: old,
    field: o.field,
    value: o.value,
    hlc: `${String(o.wall).padStart(13, "0")}:${String(i).padStart(5, "0")}:e2e`,
  }));
  await seeder.push(page);
  const res = await page.request.post("/api/sync/push", { data: { client_id: "e2e", ops }, headers: WEB_HEADERS });
  expect(res.ok(), await res.text()).toBe(true);
  ids.push(old);
  try {
    await openApp(page, "#/erledigt");
    await expect(logRow(page, `Heute ${s}`)).toBeVisible();
    await expect(page.getByText(`Vor Wochen ${s}`)).toHaveCount(0);
    await page.getByRole("button", { name: "Ältere zeigen" }).click();
    await expect(logRow(page, `Vor Wochen ${s}`)).toBeVisible();
    await expect(logRow(page, `Heute ${s}`)).toBeVisible();
  } finally {
    await remove(page, ids);
  }
});

test("Zeilen bleiben beim Neuberechnen der Uhr stehen", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const ids = [seeder.task(`Fokus ${s}`, { location: loose, status: "done" })];
  await seeder.push(page);
  try {
    await page.clock.install();
    await openApp(page, "#/erledigt");
    const button = logRow(page, `Fokus ${s}`).locator(".log-open");
    await button.focus();
    await expect(button).toBeFocused();
    await button.evaluate((el) => ((el as HTMLElement & { marker?: string }).marker = "same"));
    // The store's clock ticks every 30 s and recomputes the log.
    await page.clock.fastForward(31_000);
    await expect(button).toBeFocused();
    expect(await button.evaluate((el) => (el as HTMLElement & { marker?: string }).marker)).toBe("same");
  } finally {
    await remove(page, ids);
  }
});
