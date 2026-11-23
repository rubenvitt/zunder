// Heute as one order (design 5.1–5.3): what is planned for today by day_rank, then what was left
// over from earlier days, then what is due without a plan, as one list without red. The first
// open task stands in the Fokus box, the rest under "Danach". Without a plan, "Erst für dich"
// fills the box first. The real backend keeps data between tests: seeded tasks and goals are
// deleted in `finally`, so no plan for today is left behind for later tests.

import { expect, test, type Locator, type Page } from "@playwright/test";
import { addDays, localDate, toRfc3339Local } from "../src/sync/dates";
import { deferDay } from "../src/sync/defer";
import { entityFields, isMock, newId, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const today = () => localDate(new Date());
const yesterday = () => localDate(addDays(new Date(), -1));
const loose = { kind: "loose" };

const focusBox = (page: Page) => page.getByRole("region", { name: "Fokus" });
const focusTitle = (page: Page) => focusBox(page).locator(".task-title");
const rest = (page: Page) => page.getByRole("list", { name: "Danach" });
/** The titles under "Danach" that carry the test's suffix, in order (the real backend has more). */
const restOf = (page: Page, s: string) => rest(page).locator(".task-title").filter({ hasText: s });
const forYou = (page: Page) => page.getByRole("group", { name: "Erst für dich" });
const detail = (page: Page) => page.getByRole("complementary", { name: "Aufgabendetail" });

/** Deletes seeded tasks and goals (the real backend keeps them, and a plan for today with them). */
async function remove(page: Page, tasks: string[], goals: string[] = []): Promise<void> {
  const seeder = new Seeder();
  for (const id of tasks) seeder.raw("task", id, { deleted: true });
  for (const id of goals) seeder.raw("goal", id, { deleted: true });
  await seeder.push(page);
}

async function rightClick(target: Locator): Promise<void> {
  await target.scrollIntoViewIfNeeded();
  await target.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
  await target.click({ button: "right" });
}

test("geplante Aufgaben stehen in Reihenfolge, erste im Fokus", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const sport = seeder.raw("goal", newId(), { name: `Sport ${s}`, emoji: "🏃", state: "active" });
  const tasks = [
    seeder.task(`Gliederung ${s}`, { location: loose, planned_on: today(), day_rank: "b" }),
    seeder.task(`Krafttraining ${s}`, { location: loose, planned_on: today(), day_rank: "a", goal_id: sport }),
    seeder.task(`Funkgeräte ${s}`, { location: loose, planned_on: today(), day_rank: "c", goal_id: sport }),
    seeder.task(`Steuer ${s}`, { location: loose, planned_on: today(), day_rank: "d" }),
  ];
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    await expect(focusBox(page).getByRole("heading", { name: "Fokus" })).toBeVisible();
    await expect(focusTitle(page)).toHaveText(`Krafttraining ${s}`);
    await expect(page.getByRole("heading", { name: "Danach" })).toBeVisible();
    await expect(restOf(page, s)).toHaveText([`Gliederung ${s}`, `Funkgeräte ${s}`, `Steuer ${s}`]);
    // With a plan, "Erst für dich" is part of it.
    await expect(forYou(page)).toHaveCount(0);

    // Each row shows its goal's sign, quietly; without a goal nothing.
    await expect(row(page, `Krafttraining ${s}`).locator(".goal-mark")).toHaveText("🏃");
    await expect(row(page, `Funkgeräte ${s}`).locator(".goal-mark")).toHaveText("🏃");
    await expect(row(page, `Gliederung ${s}`).locator(".goal-mark")).toHaveCount(0);

    // Done: the next one moves into the box.
    await row(page, `Krafttraining ${s}`).locator(".check").click();
    await expect(focusTitle(page)).toHaveText(`Gliederung ${s}`);
    await expect(restOf(page, s)).toHaveText([`Funkgeräte ${s}`, `Steuer ${s}`]);
  } finally {
    await remove(page, tasks, [sport]);
  }
});

test("liegengebliebene und fällige stehen ohne Rot unter Danach", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const remind_at = toRfc3339Local(new Date(new Date().setHours(0, 5, 0, 0)));
  const tasks = [
    seeder.task(`Heute geplant ${s}`, { location: loose, planned_on: today(), remind_at }),
    seeder.task(`Fällig heute ${s}`, { location: loose, due: toRfc3339Local(new Date(new Date().setHours(0, 30, 0, 0))) }),
    seeder.task(`Überfällig ${s}`, { location: loose, due: toRfc3339Local(addDays(new Date(new Date().setHours(14, 0, 0, 0)), -1)), weight: "brand" }),
    seeder.task(`Liegen geblieben ${s}`, { location: loose, planned_on: yesterday() }),
    seeder.task(`Erinnerung ${s}`, { location: loose, remind_at }),
  ];
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    await expect(focusTitle(page)).toHaveText(`Heute geplant ${s}`);
    await expect(restOf(page, s)).toHaveText([`Liegen geblieben ${s}`, `Überfällig ${s}`, `Fällig heute ${s}`]);
    // One list: no red, no "Überfällig" or "Heute fällig" headings.
    await expect(page.locator(".view .tone-danger")).toHaveCount(0);
    await expect(page.locator(".view .chip-danger")).toHaveCount(0);
    // Due dates quietly, without the time of day; today's not at all.
    await expect(row(page, `Überfällig ${s}`).locator(".task-meta")).toContainText("Gestern");
    await expect(row(page, `Überfällig ${s}`).locator(".task-meta")).not.toContainText("14:00");
    await expect(row(page, `Fällig heute ${s}`).locator(".task-meta")).not.toContainText(/Heute|00:30/);
    await expect(page.getByRole("heading", { name: /Überfällig|Heute fällig/ })).toHaveCount(0);
    // Reminders stay, without what already stands in the order.
    const reminders = page.getByRole("list", { name: "Erinnerungen" });
    await expect(reminders.locator(".task-title").filter({ hasText: s })).toHaveText([`Erinnerung ${s}`]);
    // The sidebar counts the order and the reminders once each.
    if (isMock) await expect(page.locator(".sidebar").getByRole("link", { name: "Heute" }).locator(".nav-count")).toHaveText("5");
  } finally {
    await remove(page, tasks);
  }
});

test("ziehen ändert die Reihenfolge", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const [a, b, c, d] = [
    seeder.task(`Anna ${s}`, { location: loose, planned_on: today(), day_rank: "a" }),
    seeder.task(`Berta ${s}`, { location: loose, planned_on: today(), day_rank: "b" }),
    seeder.task(`Clara ${s}`, { location: loose, planned_on: today(), day_rank: "c" }),
    seeder.task(`Dora ${s}`, { location: loose, due: yesterday() }),
  ] as [string, string, string, string];
  await seeder.push(page);
  const field = async (id: string) => {
    const f = await entityFields(page, "task", id);
    return { planned_on: f.planned_on ?? null, day_rank: (f.day_rank as string | null) ?? null };
  };
  try {
    await openApp(page, "#/heute");
    await expect(focusTitle(page)).toHaveText(`Anna ${s}`);
    await expect(restOf(page, s)).toHaveText([`Berta ${s}`, `Clara ${s}`, `Dora ${s}`]);

    // Drag Clara above Berta.
    await row(page, `Clara ${s}`).dragTo(row(page, `Berta ${s}`), { targetPosition: { x: 40, y: 4 } });
    await expect(restOf(page, s)).toHaveText([`Clara ${s}`, `Berta ${s}`, `Dora ${s}`]);
    await expect.poll(async () => {
      const [cf, bf] = [await field(c), await field(b)];
      return cf.day_rank !== null && bf.day_rank !== null && cf.day_rank < bf.day_rank;
    }).toBe(true);

    // A due task without a plan dragged between planned ones is planned for today.
    await row(page, `Dora ${s}`).dragTo(row(page, `Berta ${s}`), { targetPosition: { x: 40, y: 4 } });
    await expect(restOf(page, s)).toHaveText([`Clara ${s}`, `Dora ${s}`, `Berta ${s}`]);
    await expect.poll(async () => (await field(d)).planned_on).toBe(today());

    // Shift+K moves up, past the box as well: the task is the focus then. Shift+J moves down.
    await row(page, `Dora ${s}`).locator(".task-main").focus();
    await page.keyboard.press("Shift+K");
    await expect(restOf(page, s)).toHaveText([`Dora ${s}`, `Clara ${s}`, `Berta ${s}`]);
    await page.keyboard.press("Shift+K");
    await expect(focusTitle(page)).toHaveText(`Dora ${s}`);
    await expect(restOf(page, s)).toHaveText([`Anna ${s}`, `Clara ${s}`, `Berta ${s}`]);
    await page.keyboard.press("Shift+J");
    await expect(focusTitle(page)).toHaveText(`Anna ${s}`);
    await expect(restOf(page, s)).toHaveText([`Dora ${s}`, `Clara ${s}`, `Berta ${s}`]);
    await expect.poll(async () => {
      const ranks = await Promise.all([a, d, c, b].map(async (id) => (await field(id)).day_rank ?? ""));
      return ranks.every((r, i) => i === 0 || ranks[i - 1]! < r);
    }).toBe(true);
  } finally {
    await remove(page, [a, b, c, d]);
  }
});

test("Plan erledigt zeigt ruhige Zeile ohne Vorschläge", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const tasks = [
    seeder.task(`Erledigt geplant ${s}`, { location: loose, planned_on: today(), status: "done" }),
    // Would be offered or stand in the box without a plan.
    seeder.task(`Tanzkurs ${s}`, { location: loose, weight: "glut", for_whom: "uns" }),
    seeder.task(`Laufen ${s}`, { location: loose, weight: "glut", for_whom: "mich", due: "2001-01-01" }),
  ];
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    await expect(focusBox(page).getByText("Plan für heute erledigt.")).toBeVisible();
    await expect(focusBox(page).locator(".task-row")).toHaveCount(0);
    await expect(forYou(page)).toHaveCount(0);
    await expect(page.getByRole("group", { name: "Glut für heute wählen" })).toHaveCount(0);
    // The rest of the list stays, without a focus.
    await expect(restOf(page, s)).toHaveText([`Laufen ${s}`]);
  } finally {
    await remove(page, tasks);
  }
});

test("ohne Plan steht Glut für mich im Fokus", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const tasks = [
    seeder.task(`Klavier ${s}`, { location: loose, weight: "glut", for_whom: "mich", due: "2001-01-01" }),
    seeder.task(`Zusage ${s}`, { location: loose, weight: "brand", due: "2001-01-01" }),
  ];
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    // Without a click, before the list's first task (Brand would come first in the list).
    await expect(forYou(page).locator(".task-title")).toHaveText(`Klavier ${s}`);
    await expect(focusTitle(page)).toHaveText(`Klavier ${s}`);
    await expect(restOf(page, s)).toHaveText([`Zusage ${s}`]);

    // Hidden for today: the focus falls to the list's first task.
    await forYou(page).getByRole("button", { name: "Für heute ausblenden" }).click();
    await expect(forYou(page)).toHaveCount(0);
    await expect(focusTitle(page)).toHaveText(`Zusage ${s}`);
    await expect(restOf(page, s)).toHaveText([`Klavier ${s}`]);
  } finally {
    const res = await page.request.put("/api/settings/focus", { data: { dismissed_on: null, chosen: null }, headers: { "X-Zunder-Client": "web" } });
    expect(res.ok()).toBe(true);
    await remove(page, tasks);
  }
});

test("Heute nicht nimmt eine geplante Aufgabe auf morgen mit", async ({ page }) => {
  const s = suffix();
  const inTwoWeeks = localDate(addDays(new Date(), 14));
  const seeder = new Seeder();
  const tasks = [
    seeder.task(`Zuerst ${s}`, { location: loose, planned_on: today(), day_rank: "a" }),
    seeder.task(`Später ${s}`, { location: loose, planned_on: today(), day_rank: "b" }),
    // Due later anyway: only the plan moves (and it counts).
    seeder.task(`Frist ${s}`, { location: loose, planned_on: today(), day_rank: "c", due: inTwoWeeks }),
  ];
  const [, later, deadline] = tasks as [string, string, string];
  await seeder.push(page);
  const tomorrow = deferDay("tomorrow", today());
  try {
    await openApp(page, "#/heute");
    await expect(restOf(page, s)).toHaveText([`Später ${s}`, `Frist ${s}`]);

    await row(page, `Später ${s}`).getByRole("button", { name: "Heute nicht" }).click();
    await expect(row(page, `Später ${s}`)).toHaveCount(0);
    await expect.poll(async () => {
      const f = await entityFields(page, "task", later);
      return [f.planned_on, f.due, f.deferrals];
    }).toEqual([tomorrow, tomorrow, 1]);

    await row(page, `Frist ${s}`).getByRole("button", { name: "Heute nicht" }).click();
    await expect(row(page, `Frist ${s}`)).toHaveCount(0);
    await expect.poll(async () => {
      const f = await entityFields(page, "task", deadline);
      return [f.planned_on, f.due, f.deferrals];
    }).toEqual([tomorrow, inTwoWeeks, 1]);
    await expect(focusTitle(page)).toHaveText(`Zuerst ${s}`);
  } finally {
    await remove(page, tasks);
  }
});

test("Aus Heute nehmen per Kontextmenü", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const tasks = [
    seeder.task(`Erstes ${s}`, { location: loose, planned_on: today(), day_rank: "a" }),
    seeder.task(`Zweites ${s}`, { location: loose, planned_on: today(), day_rank: "b" }),
    seeder.task(`Fälliges ${s}`, { location: loose, planned_on: today(), day_rank: "c", due: yesterday() }),
  ];
  const [, second, due] = tasks as [string, string, string];
  await seeder.push(page);
  const menu = page.getByRole("menu").first();
  try {
    await openApp(page, "#/heute");
    await expect(restOf(page, s)).toHaveText([`Zweites ${s}`, `Fälliges ${s}`]);

    await rightClick(row(page, `Zweites ${s}`).locator(".task-title"));
    await expect(menu.getByRole("menuitem", { name: "Für heute einplanen" })).toHaveCount(0);
    await menu.getByRole("menuitem", { name: "Aus Heute nehmen" }).click();
    await expect(row(page, `Zweites ${s}`)).toHaveCount(0);
    await expect.poll(async () => (await entityFields(page, "task", second)).planned_on ?? null).toBeNull();

    // A due task leaves the plan but stays in the list (due without a plan).
    await rightClick(row(page, `Fälliges ${s}`).locator(".task-title"));
    await menu.getByRole("menuitem", { name: "Aus Heute nehmen" }).click();
    await expect.poll(async () => (await entityFields(page, "task", due)).planned_on ?? null).toBeNull();
    await expect(restOf(page, s)).toHaveText([`Fälliges ${s}`]);

    // Back from elsewhere: planned at the end of today's plan.
    await openApp(page, "#/lose");
    await rightClick(row(page, `Zweites ${s}`).locator(".task-title"));
    await expect(menu.getByRole("menuitem", { name: "Aus Heute nehmen" })).toHaveCount(0);
    await menu.getByRole("menuitem", { name: "Für heute einplanen" }).click();
    await expect.poll(async () => (await entityFields(page, "task", second)).planned_on).toBe(today());

    // The detail offers the one that applies.
    await row(page, `Zweites ${s}`).locator(".task-main").click();
    await expect(detail(page).getByRole("button", { name: "Aus Heute nehmen" })).toBeVisible();
    await expect(detail(page).getByRole("button", { name: "Für heute einplanen" })).toHaveCount(0);

    await openApp(page, "#/heute");
    await expect(focusTitle(page)).toHaveText(`Erstes ${s}`);
    await expect(restOf(page, s)).toHaveText([`Zweites ${s}`, `Fälliges ${s}`]);
  } finally {
    await remove(page, tasks);
  }
});
