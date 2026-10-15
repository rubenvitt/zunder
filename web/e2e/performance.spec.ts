// Tempo-Budget (Spec 1/9): cold start < 1 s, interaction < 100 ms.

import { expect, test, type Page } from "@playwright/test";
import { prepare, row, Seeder, suffix } from "./fixtures";

const COLD_START_BUDGET_MS = 1000;
const INTERACTION_BUDGET_MS = 100;

/**
 * Presses a key and measures from the keydown event until the DOM satisfies
 * `doneExpr` and the following animation frame (i.e. the update is painted).
 */
async function measureKey(page: Page, key: string, doneExpr: string): Promise<number> {
  await page.evaluate((expr) => {
    const w = window as unknown as Record<string, unknown>;
    w.__t0 = undefined;
    w.__t1 = undefined;
    const check = new Function(`return (${expr});`) as () => boolean;
    window.addEventListener("keydown", () => (w.__t0 = performance.now()), { capture: true, once: true });
    const observer = new MutationObserver(() => {
      if (w.__t1 === undefined && check()) {
        observer.disconnect();
        requestAnimationFrame(() => (w.__t1 = performance.now()));
      }
    });
    observer.observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
  }, doneExpr);
  await page.keyboard.press(key);
  await page.waitForFunction(() => (window as unknown as Record<string, unknown>).__t1 !== undefined);
  return page.evaluate(() => {
    const w = window as unknown as Record<string, number>;
    return w.__t1! - w.__t0!;
  });
}

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

test("Kaltstart unter 1 s und Interaktionen unter 100 ms", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const areas = [seeder.area(`Bereich A ${s}`), seeder.area(`Bereich B ${s}`)];
  const project = seeder.project(`Projekt ${s}`, areas[0]!);
  for (let i = 0; i < 400; i++) {
    const location: Record<string, string> =
      i % 4 === 0 ? { kind: "inbox" } : i % 4 === 1 ? { kind: "loose" } : i % 4 === 2 ? { kind: "area", id: areas[i % 2]! } : { kind: "project", id: project };
    seeder.task(`Last ${s} ${String(i).padStart(3, "0")}`, { location, description: "Beschreibung ".repeat(10) });
  }
  const target = `Messziel ${s}`;
  seeder.task(target, { location: { kind: "loose" } });
  // A recurring task: checking it off also waits for the server's next instance.
  const recurring = `Rhythmusziel ${s}`;
  seeder.task(recurring, { location: { kind: "loose" }, due: `${new Date().getFullYear() + 1}-01-31`, recurrence: { every: 1, unit: "month", day: "last", from: "due" } });
  await seeder.push(page);

  // Cold start: navigation start → data rendered (performance mark set by the app).
  await page.goto("/#/lose");
  await expect(page.locator("html[data-ready]")).toBeAttached();
  const coldStart = await page.evaluate(() => performance.getEntriesByName("zunder-ready")[0]?.startTime ?? Number.POSITIVE_INFINITY);
  console.log(`Kaltstart: ${coldStart.toFixed(0)} ms`);
  expect(coldStart).toBeLessThan(COLD_START_BUDGET_MS);

  // Interaction: toggle done with x.
  await row(page, target).locator(".task-main").focus();
  const id = await row(page, target).getAttribute("data-task-id");
  const toggle = await measureKey(page, "x", `document.querySelector('[data-task-id="${id}"]')?.dataset.status === "done"`);
  console.log(`x (erledigt): ${toggle.toFixed(1)} ms`);
  expect(toggle).toBeLessThan(INTERACTION_BUDGET_MS);

  // Interaction: check off a recurring task with x (the next instance arrives later, by pull).
  await row(page, recurring).locator(".task-main").focus();
  const recurringId = await row(page, recurring).getAttribute("data-task-id");
  const toggleRecurring = await measureKey(page, "x", `document.querySelector('[data-task-id="${recurringId}"]')?.dataset.status === "done"`);
  console.log(`x (wiederkehrend erledigt): ${toggleRecurring.toFixed(1)} ms`);
  expect(toggleRecurring).toBeLessThan(INTERACTION_BUDGET_MS);
  await expect(page.locator(".toast", { hasText: "Nächste Instanz" })).toBeVisible();

  // Interaction: open quick capture.
  const openCapture = await measureKey(page, "n", `!!document.querySelector('.capture-input')`);
  console.log(`n (Erfassen öffnen): ${openCapture.toFixed(1)} ms`);
  expect(openCapture).toBeLessThan(INTERACTION_BUDGET_MS);

  // Interaction: capture a task until it shows up in the list (preselected: Lose, the open view).
  await page.getByLabel("Titel der neuen Aufgabe").fill(`Neu ${s}`);
  const capture = await measureKey(
    page,
    "Enter",
    `!document.querySelector('.capture-input') && !!document.querySelector('.toast') && [...document.querySelectorAll('.task-title')].some((el) => el.textContent === "Neu ${s}")`,
  );
  console.log(`Enter (Erfassen): ${capture.toFixed(1)} ms`);
  expect(capture).toBeLessThan(INTERACTION_BUDGET_MS);

  // Interaction: switch view with g i until the inbox tasks are shown.
  await page.keyboard.press("g");
  const switchView = await measureKey(
    page,
    "i",
    `document.querySelector('.view-head h1')?.textContent === "Inbox" && [...document.querySelectorAll('.task-title')].some((el) => el.textContent === "Last ${s} 000")`,
  );
  console.log(`g i (Sicht wechseln): ${switchView.toFixed(1)} ms`);
  expect(switchView).toBeLessThan(INTERACTION_BUDGET_MS);

  // Interaction: open the command palette and search.
  const palette = await measureKey(page, "Control+k", `!!document.querySelector('.palette-input')`);
  console.log(`Strg+K (Palette): ${palette.toFixed(1)} ms`);
  expect(palette).toBeLessThan(INTERACTION_BUDGET_MS);
});
