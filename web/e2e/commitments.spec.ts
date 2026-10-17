// Zusagen-Bremse (contract 5.1): "für andere" is a real promise. Setting it asks quietly whether
// someone was promised it, Heute counts the open ones and hints from eight on.
// The real backend keeps data between tests: counts are read first and checked relative to
// that, and every seeded commitment is closed again in `finally`.

import { expect, test, type Page } from "@playwright/test";
import { COMMITMENT_HINT, COMMITMENT_QUESTION, commitmentsLabel } from "../src/labels";
import { COMMITMENT_LIMIT } from "../src/sync/commitments";
import { entityFields, isMock, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const detail = (page: Page) => page.getByRole("complementary", { name: "Aufgabendetail" });
const ask = (page: Page) => detail(page).getByRole("group", { name: COMMITMENT_QUESTION });
const askToast = (page: Page) => page.locator(".toast", { hasText: COMMITMENT_QUESTION });
const count = (page: Page) => page.locator(".commitments-count");
const forWhom = async (page: Page, id: string) => (await entityFields(page, "task", id)).for_whom ?? null;

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

/** ID of the live task with this title (from the snapshot, e.g. after a quick capture). */
async function taskId(page: Page, title: string): Promise<string> {
  let id = "";
  await expect.poll(async () => {
    const res = await page.request.get("/api/snapshot");
    const body = (await res.json()) as { entities: { entity: string; entity_id: string; fields: Record<string, { value: unknown }> }[] };
    id = body.entities.find((x) => x.entity === "task" && x.fields.title?.value === title)?.entity_id ?? "";
    return id;
  }, { message: `Aufgabe „${title}“ auf dem Server` }).not.toBe("");
  return id;
}

test("Rückfrage im Detail: zugesagt, selbst verpflichtet oder einfach weiter", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Umzugshilfe ${s}`, { location: { kind: "loose" } });
  const given = seeder.task(`Geschenk ${s}`, { location: { kind: "loose" }, for_whom: "andere" });
  await seeder.push(page);
  try {
    await openApp(page, "#/lose");
    // Opening a task that already is for others does not ask.
    await row(page, `Geschenk ${s}`).locator(".task-main").click();
    await expect(detail(page).getByRole("group", { name: "Für wen" }).getByRole("button", { name: "andere" })).toHaveAttribute("aria-pressed", "true");
    await expect(ask(page)).toHaveCount(0);

    await row(page, `Umzugshilfe ${s}`).locator(".task-main").click();
    const choice = detail(page).getByRole("group", { name: "Für wen" });

    // Zugesagt: stays for others, the line goes.
    await choice.getByRole("button", { name: "andere" }).click();
    await expect(ask(page)).toBeVisible();
    await ask(page).getByRole("button", { name: "Zugesagt" }).click();
    await expect(ask(page)).toHaveCount(0);
    await expect.poll(() => forWhom(page, id)).toBe("andere");

    // Selbst verpflichtet: nobody waits, so it is for me.
    await choice.getByRole("button", { name: "mich" }).click();
    await expect(ask(page)).toHaveCount(0);
    await choice.getByRole("button", { name: "andere" }).click();
    await ask(page).getByRole("button", { name: "Selbst verpflichtet" }).click();
    await expect(ask(page)).toHaveCount(0);
    await expect(choice.getByRole("button", { name: "mich" })).toHaveAttribute("aria-pressed", "true");
    await expect(detail(page).getByText("Niemand wartet darauf: still verschiebbar")).toBeVisible();
    await expect.poll(() => forWhom(page, id)).toBe("mich");

    // Ignored: nothing blocks, the value stays, and the line is gone after leaving the task.
    await choice.getByRole("button", { name: "andere" }).click();
    await expect(ask(page)).toBeVisible();
    await expect.poll(() => forWhom(page, id)).toBe("andere");
    await row(page, `Geschenk ${s}`).locator(".task-main").click();
    await row(page, `Umzugshilfe ${s}`).locator(".task-main").click();
    await expect(choice.getByRole("button", { name: "andere" })).toHaveAttribute("aria-pressed", "true");
    await expect(ask(page)).toHaveCount(0);
  } finally {
    await close(page, id, given);
  }
});

test("Rückfrage nach der Schnellerfassung mit @andere, ohne zu blockieren", async ({ page }) => {
  const s = suffix();
  const ids: string[] = [];
  try {
    await openApp(page, "#/lose");
    const input = page.getByLabel("Titel der neuen Aufgabe");

    // Selbst verpflichtet turns it into a task for me.
    await page.keyboard.press("n");
    await input.fill(`Rückruf ${s} @andere`);
    await input.press("Enter");
    await expect(askToast(page)).toContainText(`Erfasst in Lose. ${COMMITMENT_QUESTION}`);
    ids.push(await taskId(page, `Rückruf ${s}`));
    await askToast(page).getByRole("button", { name: "Selbst verpflichtet" }).click();
    await expect(askToast(page)).toHaveCount(0);
    await expect(page.locator(".toast", { hasText: `„Rückruf ${s}“ ist für dich.` })).toContainText("Niemand wartet darauf: still verschiebbar.");
    await expect(row(page, `Rückruf ${s}`).locator(".chip-for-mich")).toHaveText("für mich");
    await expect.poll(() => forWhom(page, ids[0]!)).toBe("mich");

    // Zugesagt keeps it for others.
    await page.keyboard.press("n");
    await input.fill(`Abholung ${s} @andere`);
    await input.press("Enter");
    ids.push(await taskId(page, `Abholung ${s}`));
    await askToast(page).getByRole("button", { name: "Zugesagt" }).click();
    await expect(askToast(page)).toHaveCount(0);
    await expect(row(page, `Abholung ${s}`).locator(".chip-for-andere")).toHaveText("für andere");
    await expect.poll(() => forWhom(page, ids[1]!)).toBe("andere");

    // Ignored (closed or simply left): the task stays for others, the next capture works.
    await page.keyboard.press("n");
    await input.fill(`Brief ${s} @andere`);
    await input.press("Enter");
    ids.push(await taskId(page, `Brief ${s}`));
    await expect(askToast(page)).toBeVisible();
    await page.keyboard.press("n");
    await input.fill(`Einkauf ${s} @uns`);
    await input.press("Enter");
    // Only "andere" asks: a plain confirmation, and still just the one question.
    await expect(page.locator(".toast:not(.toast-ask)", { hasText: "Erfasst in Lose." })).toBeVisible();
    await expect(askToast(page)).toHaveCount(1);
    ids.push(await taskId(page, `Einkauf ${s}`));
    await expect(row(page, `Einkauf ${s}`)).toBeVisible();
    await askToast(page).getByRole("button", { name: "Meldung schließen" }).click();
    await expect(askToast(page)).toHaveCount(0);
    expect(await forWhom(page, ids[2]!)).toBe("andere");
  } finally {
    await close(page, ...ids);
  }
});

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
