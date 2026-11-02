// Tagesvorschau in Heute (contract 5.1): the next seven days with their due tasks, a crowded
// day marked and named, and a hint when a task's day is crowded already (detail, quick capture).
// The real backend keeps data between tests: the day's count is read first and checked
// relative to that, and every seeded task is closed again in `finally`.

import { expect, test, type Page } from "@playwright/test";
import { crowdedDayLine, crowdedPickHint } from "../src/labels";
import { CROWDED_DAY } from "../src/sync/dayload";
import { openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const preview = (page: Page) => page.getByRole("region", { name: "Nächste Tage" });
const cell = (page: Page, date: string) => preview(page).locator(`.day-cell[data-date="${date}"]`);
const detail = (page: Page) => page.getByRole("complementary", { name: "Aufgabendetail" });

const pad = (n: number) => String(n).padStart(2, "0");

/** Local day `days` from today (tests run in Europe/Berlin), as "YYYY-MM-DD" and "DD.MM.". */
function dayAhead(days: number): { iso: string; short: string } {
  const now = new Date();
  const d = new Date(now.getFullYear(), now.getMonth(), now.getDate() + days);
  return { iso: `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`, short: `${pad(d.getDate())}.${pad(d.getMonth() + 1)}.` };
}

test("Tagesvorschau zeigt die nächsten Tage, markiert einen Stau und warnt beim Datum", async ({ page }) => {
  const s = suffix();
  const day = dayAhead(3);
  await openApp(page, "#/heute");
  await expect(preview(page).locator(".day-cell")).toHaveCount(7);
  // Tomorrow first, a week from today last; today is not part of the strip.
  await expect(preview(page).locator(".day-cell").first()).toHaveAttribute("data-date", dayAhead(1).iso);
  await expect(preview(page).locator(".day-cell").last()).toHaveAttribute("data-date", dayAhead(7).iso);
  const before = Number(await cell(page, day.iso).getAttribute("data-count"));

  const seeder = new Seeder();
  const loose = { kind: "loose" };
  const ids: string[] = [];
  for (let i = 1; i <= CROWDED_DAY; i++) ids.push(seeder.task(`Kleinkram ${i} ${s}`, { location: loose, due: i % 2 ? day.iso : `${day.iso}T10:00:00+01:00` }));
  // Not counted: done, dev projects.
  ids.push(seeder.task(`Erledigt ${s}`, { location: loose, due: day.iso, status: "done" }));
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`App ${s}`, area, { kind: "dev", repo: "example/app", view: "list" });
  ids.push(seeder.task(`Ticket ${s}`, { location: { kind: "project", id: project }, due: day.iso }));
  await seeder.push(page);
  try {
    const total = before + CROWDED_DAY;
    await openApp(page, "#/heute");
    await expect(cell(page, day.iso)).toHaveAttribute("data-count", String(total));
    await expect(cell(page, day.iso)).toHaveClass(/day-crowded/);
    const line = preview(page).locator(`.day-crowded-line[data-date="${day.iso}"]`);
    await expect(line).toContainText(crowdedDayLine(day.iso, total));

    // The line opens the day's tasks right here, the cell closes them again.
    await line.getByRole("button").click();
    await expect(cell(page, day.iso)).toHaveAttribute("aria-pressed", "true");
    const list = preview(page).locator(".task-list");
    await expect(list.locator(".task-row", { hasText: s })).toHaveCount(CROWDED_DAY);
    await expect(row(page, `Ticket ${s}`)).toHaveCount(0);

    // The detail says the day is crowded already.
    await row(page, `Kleinkram 1 ${s}`).locator(".task-main").click();
    await expect(detail(page).locator(".due-crowded")).toHaveText(crowdedPickHint(total - 1));
    await page.keyboard.press("Escape");

    await cell(page, day.iso).click();
    await expect(cell(page, day.iso)).toHaveAttribute("aria-pressed", "false");
    await expect(preview(page).locator(".task-list")).toHaveCount(0);

    // Quick capture: picking that day shows how many are due there already.
    await page.keyboard.press("n");
    await page.getByLabel("Titel der neuen Aufgabe").fill(`Noch was ${s} ${day.short}`);
    await expect(page.locator(".capture-crowded")).toHaveText(`dort schon ${total} fällig`);
    // A quiet day has no hint.
    await page.getByLabel("Titel der neuen Aufgabe").fill(`Noch was ${s} ${dayAhead(30).short}`);
    await expect(page.locator(".capture-preview .chip-accent").first()).toBeVisible();
    await expect(page.locator(".capture-crowded")).toHaveCount(0);
  } finally {
    const close = new Seeder();
    for (const id of ids) close.raw("task", id, { status: "dropped" });
    await close.push(page);
  }
});
