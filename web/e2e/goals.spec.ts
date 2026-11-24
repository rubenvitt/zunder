// Ziele (design 6.2/6.3): active goals with this week's moves and minutes, quietly; a check mark
// only when the lead figure is reached, nothing otherwise. Closed goals are not listed, and the
// detail sets a goal whose sign then shows in every row. The real backend keeps data between
// tests, so seeded tasks and goals are deleted in `finally`.

import { expect, test, type Page } from "@playwright/test";
import { toRfc3339Local } from "../src/sync/dates";
import { isMock, newId, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const loose = { kind: "loose" };
const goalRow = (page: Page, name: string) => page.locator(".goal-row", { has: page.locator(".goal-name", { hasText: name }) });
const detail = (page: Page) => page.getByRole("complementary", { name: "Aufgabendetail" });

/** Start of a flame that has run for 40 minutes now (the server alone writes flame_log, a running flame counts until now). */
const fortyMinutesAgo = () => toRfc3339Local(new Date(Date.now() - 40 * 60_000));

async function remove(page: Page, tasks: string[], goals: string[]): Promise<void> {
  const seeder = new Seeder();
  for (const id of tasks) seeder.raw("task", id, { deleted: true });
  for (const id of goals) seeder.raw("goal", id, { deleted: true });
  await seeder.push(page);
}

test("zeigt Züge, Minuten und Leitkennzahl ruhig", async ({ page }) => {
  // The flame must have started on today's date (and so in this week).
  test.skip(new Date().getHours() === 0 && new Date().getMinutes() < 45, "Die 40-Minuten-Flamme begänne gestern.");
  const s = suffix();
  const seeder = new Seeder();
  const sport = seeder.raw("goal", newId(), { name: `Sport ${s}`, emoji: "🏃", state: "active", lead: "Dreimal die Woche laufen", lead_per_week: 3, rank: "a" });
  const tasks = [
    seeder.task(`Laufen ${s}`, { location: loose, goal_id: sport, status: "done" }),
    seeder.task(`Dehnen ${s}`, { location: loose, goal_id: sport, flame_started_at: fortyMinutesAgo() }),
    seeder.task(`Nichts ${s}`, { location: loose }),
  ];
  await seeder.push(page);
  try {
    await openApp(page, "#/ziele");
    const sportRow = goalRow(page, `Sport ${s}`);
    await expect(sportRow.locator(".goal-emoji")).toHaveText("🏃");
    await expect(sportRow).toContainText("1 Zug");
    await expect(sportRow).not.toContainText("1 Züge");
    await expect(sportRow).toContainText(/4[01] min/);
    await expect(sportRow).toContainText("(3×)");
    await expect(sportRow.locator(".goal-reached")).toHaveCount(0);

    // A click opens the lead and the week's tasks of the goal.
    await sportRow.getByRole("button").click();
    const open = page.locator(".goal-detail", { hasText: `Dreimal die Woche laufen` });
    await expect(open).toBeVisible();
    await expect(open).toContainText(`Laufen ${s}`);
    await expect(open).toContainText(`Dehnen ${s}`);
    await expect(open).not.toContainText(`Nichts ${s}`);
  } finally {
    await remove(page, tasks, [sport]);
  }
});

test("Häkchen nur bei erreicht, sonst nichts", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const erreicht = seeder.raw("goal", newId(), { name: `Erreicht ${s}`, state: "active", lead_per_week: 1, rank: "b" });
  const offen = seeder.raw("goal", newId(), { name: `Offen ${s}`, state: "active", lead_per_week: 2, rank: "c" });
  const frei = seeder.raw("goal", newId(), { name: `Frei ${s}`, state: "active", rank: "d" });
  const tasks = [
    seeder.task(`Fertig ${s}`, { location: loose, goal_id: erreicht, status: "done" }),
    seeder.task(`Fast ${s}`, { location: loose, goal_id: offen, status: "done" }),
  ];
  await seeder.push(page);
  try {
    await openApp(page, "#/ziele");
    await expect(goalRow(page, `Erreicht ${s}`).locator(".goal-reached")).toHaveCount(1);
    await expect(goalRow(page, `Offen ${s}`)).toContainText("1 Zug");
    await expect(goalRow(page, `Offen ${s}`).locator(".goal-reached")).toHaveCount(0);
    await expect(goalRow(page, `Frei ${s}`).locator(".goal-reached")).toHaveCount(0);
    await expect(goalRow(page, `Frei ${s}`)).toContainText("0 Züge");
    await expect(goalRow(page, `Frei ${s}`)).not.toContainText("min");
  } finally {
    await remove(page, tasks, [erreicht, offen, frei]);
  }
});

test("geschlossenes Ziel fehlt", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const aktiv = seeder.raw("goal", newId(), { name: `Aktiv ${s}`, state: "active" });
  const zu = seeder.raw("goal", newId(), { name: `Beendet ${s}`, state: "closed" });
  await seeder.push(page);
  try {
    await openApp(page, "#/ziele");
    await expect(goalRow(page, `Aktiv ${s}`)).toBeVisible();
    await expect(page.locator(".goal-name", { hasText: `Beendet ${s}` })).toHaveCount(0);
  } finally {
    await remove(page, [], [aktiv, zu]);
  }
});

test("Ziel im Detail setzen zeigt Zeichen in der Zeile", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const sport = seeder.raw("goal", newId(), { name: `Sport ${s}`, emoji: "🏃", state: "active" });
  const zu = seeder.raw("goal", newId(), { name: `Beendet ${s}`, state: "closed" });
  const task = seeder.task(`Aufgabe ${s}`, { location: loose });
  await seeder.push(page);
  try {
    await openApp(page, "#/lose");
    await expect(row(page, `Aufgabe ${s}`).locator(".goal-mark")).toHaveCount(0);
    await row(page, `Aufgabe ${s}`).locator(".task-main").click();
    const picker = detail(page).getByLabel("Ziel", { exact: true });
    await expect(picker).toBeVisible();
    // Only active goals and "Ohne Ziel" are offered.
    await expect(picker.locator("option", { hasText: `Sport ${s}` })).toHaveCount(1);
    await expect(picker.locator("option", { hasText: `Beendet ${s}` })).toHaveCount(0);
    await expect(picker.locator("option", { hasText: "Ohne Ziel" })).toHaveCount(1);
    await picker.selectOption({ label: `🏃 Sport ${s}` });
    await expect(row(page, `Aufgabe ${s}`).locator(".goal-mark")).toHaveText("🏃");
    await picker.selectOption({ label: "Ohne Ziel" });
    await expect(row(page, `Aufgabe ${s}`).locator(".goal-mark")).toHaveCount(0);
  } finally {
    await remove(page, [task], [sport, zu]);
  }
});

test("Zeilen bleiben beim Neuberechnen der Uhr stehen", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const sport = seeder.raw("goal", newId(), { name: `Sport ${s}`, emoji: "🏃", state: "active", rank: "a" });
  await seeder.push(page);
  try {
    await page.clock.install();
    await openApp(page, "#/ziele");
    const button = goalRow(page, `Sport ${s}`).locator(".goal-head");
    await button.focus();
    await expect(button).toBeFocused();
    await button.evaluate((el) => ((el as HTMLElement & { marker?: string }).marker = "same"));
    // The store's clock ticks every 30 s and recomputes the rows.
    await page.clock.fastForward(31_000);
    await expect(button).toBeFocused();
    expect(await button.evaluate((el) => (el as HTMLElement & { marker?: string }).marker)).toBe("same");
  } finally {
    await remove(page, [], [sport]);
  }
});

test("Timing-Zeit nur bei zugeordneten Projekten, sonst nichts", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const mit = seeder.raw("goal", newId(), { name: `Mit Timing ${s}`, state: "active", timing_projects: ["/projects/1"], rank: "e" });
  const ohne = seeder.raw("goal", newId(), { name: `Ohne Timing ${s}`, state: "active", rank: "f" });
  await seeder.push(page);
  try {
    await openApp(page, "#/ziele");
    await expect(goalRow(page, `Ohne Timing ${s}`)).toBeVisible();
    if (isMock) {
      // The mock answers 3.5 h for every goal with projects.
      await expect(goalRow(page, `Mit Timing ${s}`).locator(".goal-timing")).toHaveText("3,5 h Timing");
      await goalRow(page, `Mit Timing ${s}`).getByRole("button").click();
      await expect(page.locator(".goal-timing-text")).toHaveText("Timing: 3,5 h");
    } else {
      // The real test server has no Timing token: quietly nothing.
      await expect(goalRow(page, `Mit Timing ${s}`).locator(".goal-timing")).toHaveText("");
    }
    await expect(goalRow(page, `Ohne Timing ${s}`).locator(".goal-timing")).toHaveText("");
  } finally {
    await remove(page, [], [mit, ohne]);
  }
});
