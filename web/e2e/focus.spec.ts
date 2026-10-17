// "Erst für dich" at the top of Heute (contract 5.1): one Glut task for oneself, or a quiet line
// with a few Glut tasks to pick; clicking it away holds for the day (server setting).
// The real backend keeps data between tests: seeded tasks are closed and the setting is reset
// in `finally`; tasks that must win carry the earliest due dates.

import { expect, test, type Page } from "@playwright/test";
import { FOCUS_EMPTY } from "../src/labels";
import { localDate } from "../src/sync/dates";
import { entityFields, isMock, openApp, prepare, reloadApp, Seeder, suffix, WEB_HEADERS } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const section = (page: Page) => page.getByRole("region", { name: "Erst für dich" });
const focusTitle = (page: Page) => section(page).locator(".focus-task .focus-title");
const today = () => localDate(new Date());

interface FocusSettings {
  dismissed_on: string | null;
  chosen: { on: string; task_id: string } | null;
}

async function serverFocus(page: Page): Promise<FocusSettings> {
  const res = await page.request.get("/api/settings/focus");
  expect(res.ok(), await res.text()).toBe(true);
  return (await res.json()) as FocusSettings;
}

async function putFocus(page: Page, body: Partial<FocusSettings>): Promise<void> {
  const res = await page.request.put("/api/settings/focus", { data: body, headers: WEB_HEADERS });
  expect(res.ok(), await res.text()).toBe(true);
}

/** Closes seeded tasks and resets the setting (real backend keeps both for later tests). */
async function cleanUp(page: Page, ...ids: string[]): Promise<void> {
  const seeder = new Seeder();
  for (const id of ids) seeder.raw("task", id, { status: "done" });
  if (ids.length > 0) await seeder.push(page);
  await putFocus(page, { dismissed_on: null, chosen: null });
}

test("Erst für dich zeigt die Glut-Aufgabe für dich mit der frühesten Fälligkeit", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const loose = { kind: "loose" };
  const ids = [
    seeder.task(`Laufen ${s}`, { location: loose, weight: "glut", for_whom: "mich", due: "2001-01-02" }),
    seeder.task(`Später ${s}`, { location: loose, weight: "glut", for_whom: "mich", due: "2001-01-05" }),
    // Not at the top: for others, waiting on someone, other weights, dev projects.
    seeder.task(`Gutachten ${s}`, { location: loose, weight: "glut", for_whom: "andere", due: "2001-01-01" }),
    seeder.task(`Warten ${s}`, { location: loose, weight: "glut", for_whom: "mich", due: "2001-01-01", waiting_on: `Anna ${s}` }),
    seeder.task(`Brand ${s}`, { location: loose, weight: "brand", for_whom: "mich", due: "2001-01-01" }),
  ];
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`App ${s}`, area, { kind: "dev", repo: "example/app", view: "list" });
  ids.push(seeder.task(`Ticket ${s}`, { location: { kind: "project", id: project }, weight: "glut", for_whom: "mich", due: "2001-01-01" }));
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    await expect(section(page).getByRole("heading", { name: "Erst für dich" })).toBeVisible();
    await expect(focusTitle(page)).toHaveText(`Laufen ${s}`);
    // Above everything else in Heute.
    const first = await page.locator(".view > section, .view > .group").first().getAttribute("aria-labelledby");
    expect(first).toBe("focus-heading");

    await focusTitle(page).click();
    await expect(page.getByRole("complementary", { name: "Aufgabendetail" })).toBeVisible();

    // Done from the top: the next one moves up.
    await section(page).locator(".focus-task .check").click();
    await expect(focusTitle(page)).toHaveText(`Später ${s}`);
    await expect.poll(async () => (await entityFields(page, "task", ids[0]!)).status).toBe("done");
  } finally {
    await cleanUp(page, ...ids);
  }
});

test("Ohne Glut für dich: leise Zeile und eine Auswahl für heute, ohne Felder zu ändern", async ({ page }) => {
  const s = suffix();
  await openApp(page, "#/heute");
  // The empty state needs a Heute without open Glut tasks for oneself (other specs close theirs).
  await expect(section(page).locator(".focus-task"), "offene Glut-Aufgabe für mich aus einem anderen Test").toHaveCount(0);

  const seeder = new Seeder();
  const loose = { kind: "loose" };
  const tanz = seeder.task(`Tanzkurs ${s}`, { location: loose, weight: "glut", for_whom: "uns", due: "2001-01-02" });
  const steuer = seeder.task(`Steuer ${s}`, { location: loose, weight: "glut", due: "2001-01-03" });
  const ids = [
    tanz,
    steuer,
    seeder.task(`Gutachten ${s}`, { location: loose, weight: "glut", for_whom: "andere", due: "2001-01-01" }),
    seeder.task(`Rauch ${s}`, { location: loose, weight: "rauch", for_whom: "mich", due: "2001-01-01" }),
  ];
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    await expect(section(page).getByText(FOCUS_EMPTY)).toBeVisible();
    const choices = section(page).getByRole("group", { name: "Glut für heute wählen" }).getByRole("button");
    await expect(choices.first()).toHaveText(`Tanzkurs ${s}`);
    await expect(choices.nth(1)).toHaveText(`Steuer ${s}`);
    await expect(choices.filter({ hasText: `Gutachten ${s}` })).toHaveCount(0);
    await expect(choices.filter({ hasText: `Rauch ${s}` })).toHaveCount(0);

    await choices.nth(1).click();
    await expect(focusTitle(page)).toHaveText(`Steuer ${s}`);
    await expect(section(page).getByText("für heute gewählt")).toBeVisible();
    await expect.poll(() => serverFocus(page)).toMatchObject({ chosen: { on: today(), task_id: steuer } });
    // A day setting, not a task field: whom it is for stays unset.
    expect((await entityFields(page, "task", steuer)).for_whom ?? null).toBeNull();

    // Every browser sees the pick (server), also after a reload.
    await reloadApp(page);
    await expect(focusTitle(page)).toHaveText(`Steuer ${s}`);
  } finally {
    await cleanUp(page, ...ids);
  }
});

test("Wegklicken gilt für den Tag: bleibt nach Neuladen weg, morgen ist es wieder da", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Klavier ${s}`, { location: { kind: "loose" }, weight: "glut", for_whom: "mich", due: "2001-01-01" });
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    await expect(focusTitle(page)).toHaveText(`Klavier ${s}`);
    await section(page).getByRole("button", { name: "Für heute ausblenden" }).click();
    await expect(section(page)).toHaveCount(0);
    await expect(page.locator(".toast")).toContainText("„Erst für dich“ ist für heute ausgeblendet.");
    await expect.poll(async () => (await serverFocus(page)).dismissed_on).toBe(today());

    await reloadApp(page);
    await expect(page.getByRole("heading", { name: "Heute" })).toBeVisible();
    await expect(section(page)).toHaveCount(0);

    // The next day it is back by itself (the stored day is yesterday's then).
    const tomorrow = new Date();
    tomorrow.setDate(tomorrow.getDate() + 1);
    tomorrow.setHours(9, 0, 0, 0);
    await page.clock.setFixedTime(tomorrow);
    await reloadApp(page);
    await expect(focusTitle(page)).toHaveText(`Klavier ${s}`);
    expect((await serverFocus(page)).dismissed_on).toBe(today());
  } finally {
    await cleanUp(page, id);
  }
});

test("Rückgängig im Hinweis holt den Bereich zurück", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Malen ${s}`, { location: { kind: "loose" }, weight: "glut", for_whom: "mich", due: "2001-01-01" });
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    await section(page).getByRole("button", { name: "Für heute ausblenden" }).click();
    await expect(section(page)).toHaveCount(0);
    await page.locator(".toast").getByRole("button", { name: "Rückgängig" }).click();
    await expect(focusTitle(page)).toHaveText(`Malen ${s}`);
    await expect.poll(async () => (await serverFocus(page)).dismissed_on).toBeNull();
  } finally {
    await cleanUp(page, id);
  }
});

test("Offline weggeklickt: gilt sofort und geht online an den Server", async ({ page, context }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Lesen ${s}`, { location: { kind: "loose" }, weight: "glut", for_whom: "mich", due: "2001-01-01" });
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    await expect(focusTitle(page)).toHaveText(`Lesen ${s}`);
    await context.setOffline(true);
    await section(page).getByRole("button", { name: "Für heute ausblenden" }).click();
    await expect(section(page)).toHaveCount(0);
    // Remembered on the device until the server has it.
    const stored = await page.evaluate(() => JSON.parse(localStorage.getItem("zunder.focus") ?? "null"));
    expect(stored.pending).toEqual({ dismissed_on: today() });
    expect((await serverFocus(page)).dismissed_on).toBeNull();

    await context.setOffline(false);
    await expect.poll(async () => (await serverFocus(page)).dismissed_on).toBe(today());
    await expect.poll(() => page.evaluate(() => JSON.parse(localStorage.getItem("zunder.focus") ?? "null")?.pending ?? null)).toBeNull();
  } finally {
    await context.setOffline(false);
    await cleanUp(page, id);
  }
});

test("Weggeklickt auf einem anderen Gerät: Heute liest es beim Wechsel", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Atmen ${s}`, { location: { kind: "loose" }, weight: "glut", for_whom: "mich", due: "2001-01-01" });
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    await expect(focusTitle(page)).toHaveText(`Atmen ${s}`);
    await page.locator(".sidebar").getByRole("link", { name: "Lose" }).click();
    await expect(page.getByRole("heading", { name: "Lose" })).toBeVisible();
    // Another browser clicks it away.
    await putFocus(page, { dismissed_on: today() });
    await page.locator(".sidebar").getByRole("link", { name: "Heute" }).click();
    await expect(page.getByRole("heading", { name: "Heute" })).toBeVisible();
    await expect(section(page)).toHaveCount(0);
  } finally {
    await cleanUp(page, id);
  }
  if (isMock) expect(await serverFocus(page)).toEqual({ dismissed_on: null, chosen: null });
});

test("Erst für dich mobil @mobile", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Yoga ${s}`, { location: { kind: "loose" }, weight: "glut", for_whom: "mich", due: "2001-01-01" });
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    await expect(focusTitle(page)).toHaveText(`Yoga ${s}`);
    await expect(section(page).getByRole("button", { name: "Für heute ausblenden" })).toBeVisible();
  } finally {
    await cleanUp(page, id);
  }
});
