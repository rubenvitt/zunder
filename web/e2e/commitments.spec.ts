// Zusagen-Bremse (contract 5.1): "für andere" is a real promise. Heute counts the open ones
// and hints from eight on.
// The real backend keeps data between tests: counts are read first and checked relative to
// that, and every seeded commitment is closed again in `finally`.

import { expect, test, type Page } from "@playwright/test";
import { COMMITMENT_HINT, commitmentsLabel } from "../src/labels";
import { COMMITMENT_LIMIT } from "../src/sync/commitments";
import { entityFields, isMock, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const count = (page: Page) => page.locator(".commitments-count");

/** Open commitments Heute shows right now (0 while the count is hidden). */
async function openCount(page: Page): Promise<number> {
  await openApp(page, "#/heute");
  if ((await count(page).count()) === 0) return 0;
  return Number.parseInt((await count(page).textContent()) ?? "", 10);
}

/** Closes seeded tasks again, so they leave the count for later tests (real backend keeps data). */
async function close(page: Page, ...ids: string[]): Promise<void> {
  const seeder = new Seeder();
  for (const id of ids) seeder.raw("task", id, { status: "done" });
  await seeder.push(page);
}

test("Heute zählt offene Zusagen, ein Klick zeigt sie, Rauch lässt sich dort zurückgeben", async ({ page }) => {
  const s = suffix();
  const base = await openCount(page);
  // Hidden while there are none.
  if (base === 0) await expect(page.locator(".commitments")).toHaveCount(0);

  const seeder = new Seeder();
  const loose = { kind: "loose" };
  const rauch = seeder.task(`Bewerbung lesen ${s}`, { location: loose, for_whom: "andere", weight: "rauch" });
  await seeder.push(page);
  const ids = [rauch];
  try {
    await openApp(page, "#/heute");
    await expect(count(page)).toHaveText(commitmentsLabel(base + 1));

    // Counted: waiting on someone (blocked), in progress, subtasks. Not: done, dropped, for me, dev projects.
    const waiting = seeder.task(`Gutachten ${s}`, { location: loose, for_whom: "andere", waiting_on: `Anna ${s}` });
    const parent = seeder.task(`Umzug ${s}`, { location: loose });
    const sub = seeder.task(`Kisten ${s}`, { parent_id: parent, for_whom: "andere", status: "in_progress" });
    ids.push(waiting, parent, sub);
    ids.push(seeder.task(`Erledigt ${s}`, { location: loose, for_whom: "andere", status: "done" }));
    ids.push(seeder.task(`Verworfen ${s}`, { location: loose, for_whom: "andere", status: "dropped" }));
    ids.push(seeder.task(`Für mich ${s}`, { location: loose, for_whom: "mich", weight: "rauch" }));
    const area = seeder.area(`Arbeit ${s}`);
    const project = seeder.project(`App ${s}`, area, { kind: "dev", repo: "example/app", view: "list" });
    ids.push(seeder.task(`Ticket ${s}`, { location: { kind: "project", id: project }, for_whom: "andere", weight: "rauch" }));
    await seeder.push(page);
    await openApp(page, "#/heute");
    await expect(count(page)).toHaveText(commitmentsLabel(base + 3));
    await expect.poll(async () => (await entityFields(page, "task", waiting)).status).toBe("blocked");

    await count(page).click();
    await expect(page).toHaveURL(/#\/zusagen$/);
    await expect(page.getByRole("heading", { name: "Offene Zusagen" })).toBeVisible();
    const list = page.getByRole("list", { name: "Offene Zusagen" });
    for (const title of [`Bewerbung lesen ${s}`, `Gutachten ${s}`, `Kisten ${s}`]) await expect(row(page, title)).toBeVisible();
    for (const title of [`Umzug ${s}`, `Erledigt ${s}`, `Verworfen ${s}`, `Für mich ${s}`, `Ticket ${s}`]) await expect(list.locator(".task-title", { hasText: title })).toHaveCount(0);
    await expect(row(page, `Gutachten ${s}`).locator(".chip-waiting-on")).toHaveText(`wartet auf Anna ${s}`);

    // "Zurückgeben" only on Rauch rows, with the dialog from the detail.
    await expect(row(page, `Gutachten ${s}`).getByRole("button", { name: "Zurückgeben" })).toHaveCount(0);
    await expect(row(page, `Kisten ${s}`).getByRole("button", { name: "Zurückgeben" })).toHaveCount(0);
    await row(page, `Bewerbung lesen ${s}`).getByRole("button", { name: "Zurückgeben" }).click();
    const dialog = page.getByRole("dialog", { name: "Zurückgeben" });
    await expect(dialog.getByLabel("Satz zum Verschicken")).toHaveValue(new RegExp(`„Bewerbung lesen ${s}“`));
    await dialog.getByLabel("Wer entscheidet?").fill(`Ben ${s}`);
    await dialog.getByRole("button", { name: "Zurückgeben" }).click();
    await expect(dialog).toBeHidden();
    await expect.poll(async () => {
      const f = await entityFields(page, "task", rauch);
      return [f.waiting_on, f.status, f.for_whom];
    }).toEqual([`Ben ${s}`, "blocked", "andere"]);
    // The promise stays open while the other person decides.
    await expect(row(page, `Bewerbung lesen ${s}`).locator(".chip-waiting-on")).toHaveText(`wartet auf Ben ${s}`);
  } finally {
    await close(page, ...ids);
  }
  if (isMock) expect(await openCount(page)).toBe(0);
});

test("Ab acht offenen Zusagen ein leiser Hinweis, der zur Liste führt", async ({ page }) => {
  const s = suffix();
  const base = await openCount(page);
  const seeder = new Seeder();
  const ids: string[] = [];
  const commitment = (i: number) => seeder.task(`Zusage ${s} ${i}`, { location: { kind: "loose" }, for_whom: "andere" });
  for (let i = base; i < COMMITMENT_LIMIT - 1; i++) ids.push(commitment(i));
  try {
    if (ids.length > 0) await seeder.push(page);
    const hint = page.locator(".commitments-hint");
    await openApp(page, "#/heute");
    // One below the threshold (unless earlier tests left more open ones on the real backend).
    if (base < COMMITMENT_LIMIT) {
      await expect(count(page)).toHaveText(commitmentsLabel(COMMITMENT_LIMIT - 1));
      await expect(hint).toHaveCount(0);
    }

    ids.push(commitment(COMMITMENT_LIMIT));
    await seeder.push(page);
    await openApp(page, "#/heute");
    await expect(count(page)).toHaveText(commitmentsLabel(Math.max(base, COMMITMENT_LIMIT - 1) + 1));
    await expect(hint).toHaveText(COMMITMENT_HINT);
    await hint.click();
    await expect(page.getByRole("heading", { name: "Offene Zusagen" })).toBeVisible();
    await expect(page.getByText("Rauch kannst du hier zurückgeben.")).toBeVisible();
  } finally {
    await close(page, ...ids);
  }
});

test("Offene Zusagen mobil: Zahl in Heute, Zurückgeben als Knopf @mobile", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Rede ${s}`, { location: { kind: "loose" }, for_whom: "andere", weight: "rauch" });
  await seeder.push(page);
  try {
    await openApp(page, "#/heute");
    await count(page).click();
    await expect(page.getByRole("heading", { name: "Offene Zusagen" })).toBeVisible();
    await expect(row(page, `Rede ${s}`).getByRole("button", { name: "Zurückgeben" })).toBeVisible();
  } finally {
    await close(page, id);
  }
});
