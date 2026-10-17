// Wochenspiegel in Heute (contract 5.1): this ISO week's done tasks by weight and for whom.
// The real backend keeps data between tests: counts are read first and checked relative to
// that (only tasks done now, i.e. in this week, are seeded).

import { expect, test, type Page } from "@playwright/test";
import { openApp, prepare, reloadApp, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const mirror = (page: Page) => page.getByRole("region", { name: "Wochenspiegel" });

interface Counts {
  total: number;
  weight: Record<string, number>;
  forWhom: Record<string, number>;
}

/** What the open mirror shows right now. */
async function counts(page: Page): Promise<Counts> {
  const total = Number.parseInt((await mirror(page).locator(".week-mirror-total").textContent())!.split("·")[1]!, 10);
  const read = async (kind: string) => {
    const out: Record<string, number> = {};
    for (const seg of await mirror(page).locator(`.balance-bar-${kind} .balance-seg`).all()) {
      out[(await seg.getAttribute("data-key"))!] = Number(await seg.getAttribute("data-count"));
    }
    return out;
  };
  return { total, weight: await read("weight"), forWhom: await read("for") };
}

const n = (counts: Record<string, number>, key: string) => counts[key] ?? 0;

test("Wochenspiegel zählt Erledigtes dieser Woche nach Gewicht und Für wen, einklappbar", async ({ page }) => {
  const s = suffix();
  await openApp(page, "#/heute");
  await expect(mirror(page)).toBeVisible();
  const before = await counts(page);
  if (before.total === 0) await expect(mirror(page).locator('[data-line="weight"]')).toHaveText("Diese Woche ist noch nichts erledigt.");

  const seeder = new Seeder();
  const loose = { kind: "loose" };
  const parent = seeder.task(`Laufen ${s}`, { location: loose, weight: "glut", for_whom: "mich", status: "done" });
  seeder.task(`Schuhe ${s}`, { parent_id: parent, weight: "glut", status: "done" });
  seeder.task(`Formular ${s}`, { location: loose, weight: "rauch", for_whom: "andere", status: "done" });
  seeder.task(`Ohne ${s}`, { location: loose, status: "done" });
  // Not counted: dropped, open, dev projects.
  seeder.task(`Verworfen ${s}`, { location: loose, weight: "brand", status: "dropped" });
  // Open Glut for oneself would stand in "Erst für dich" for later tests: closed again below.
  const open = seeder.task(`Offen ${s}`, { location: loose, weight: "glut", for_whom: "mich" });
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`App ${s}`, area, { kind: "dev", repo: "example/app", view: "list" });
  seeder.task(`Ticket ${s}`, { location: { kind: "project", id: project }, weight: "rauch", status: "done" });
  await seeder.push(page);
  try {
    await checkMirror(page, before);
  } finally {
    const close = new Seeder();
    close.raw("task", open, { status: "dropped" });
    await close.push(page);
  }
});

async function checkMirror(page: Page, before: Counts): Promise<void> {
  await openApp(page, "#/heute");
  await expect(mirror(page).locator(".week-mirror-total")).toContainText(`${before.total + 4} erledigt`);
  const after = await counts(page);
  expect(after.total).toBe(before.total + 4);
  for (const [key, plus] of [["brand", 0], ["glut", 2], ["rauch", 1], ["asche", 0], ["none", 1]] as const) {
    expect(n(after.weight, key), key).toBe(n(before.weight, key) + plus);
  }
  for (const [key, plus] of [["mich", 1], ["uns", 0], ["andere", 1], ["none", 2]] as const) {
    expect(n(after.forWhom, key), key).toBe(n(before.forWhom, key) + plus);
  }
  // In words, only the groups that have something.
  const weightLine = mirror(page).locator('[data-line="weight"]');
  await expect(weightLine).toContainText(`${n(after.weight, "glut")} Glut`);
  await expect(weightLine).toContainText(`${n(after.weight, "rauch")} Rauch`);
  if (n(after.weight, "asche") === 0) await expect(weightLine).not.toContainText("Asche");
  await expect(mirror(page).locator('[data-line="for-whom"]')).toContainText(`${n(after.forWhom, "andere")} für andere`);

  // Collapsed per device: the summary stays, the bars go, and a reload keeps it.
  const summary = mirror(page).locator("summary");
  await summary.click();
  await expect(weightLine).toBeHidden();
  await expect(mirror(page).locator(".week-mirror-total")).toBeVisible();
  // The toggle event (which remembers the state) fires after the click: wait for it before reloading.
  await expect.poll(() => storedOpen(page)).toBe("0");
  await reloadApp(page);
  await expect(mirror(page).locator("details")).not.toHaveAttribute("open");
  await expect(weightLine).toBeHidden();
  await summary.click();
  await expect(weightLine).toBeVisible();
  await expect.poll(() => storedOpen(page)).toBe("1");
  await reloadApp(page);
  await expect(weightLine).toBeVisible();
}

const storedOpen = (page: Page) => page.evaluate(() => localStorage.getItem("zunder.weekMirrorOpen"));
