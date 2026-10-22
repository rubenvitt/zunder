import { expect, test } from "@playwright/test";
import { card, entityFields, openApp, prepare, reloadApp, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

test("Board verschieben per Drag & Drop und Tastatur", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`App ${s}`, area, { kind: "dev", repo: "example/app" });
  const a = seeder.task(`Karte A ${s}`, { location: { kind: "project", id: project }, type: "feature" });
  const b = seeder.task(`Karte B ${s}`, { location: { kind: "project", id: project } });
  await seeder.push(page);

  await openApp(page, `#/bereiche/project/${project}`);
  // Dev projects default to the board.
  const board = page.getByRole("region", { name: `Board App ${s}` });
  await expect(board).toBeVisible();
  const open = board.locator('.board-column[data-status="open"]');
  const inProgress = board.locator('.board-column[data-status="in_progress"]');
  await expect(open.locator(".card")).toHaveCount(2);

  await card(page, `Karte A ${s}`).dragTo(inProgress.locator(".board-cards"));
  await expect(inProgress.locator(".card", { hasText: `Karte A ${s}` })).toBeVisible();
  await expect.poll(async () => (await entityFields(page, "task", a)).status).toBe("in_progress");

  // Keyboard: select card B and move it two columns to the right (open → in_progress → review).
  await card(page, `Karte B ${s}`).click();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Shift+L");
  await expect(inProgress.locator(".card", { hasText: `Karte B ${s}` })).toBeVisible();
  await page.keyboard.press("Shift+L");
  await expect(board.locator('.board-column[data-status="review"] .card', { hasText: `Karte B ${s}` })).toBeVisible();
  await expect.poll(async () => (await entityFields(page, "task", b)).status).toBe("review");

  // Toggle to the list view and back.
  await page.keyboard.press("v");
  await expect(page.locator(".task-row", { hasText: `Karte A ${s}` })).toBeVisible();
  await page.keyboard.press("v");
  await expect(board).toBeVisible();
});

test("Dev-Sicht zeigt Reservierung, Branch und PR", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`Dienst ${s}`, area, { kind: "dev" });
  seeder.task(`Agentenarbeit ${s}`, {
    location: { kind: "project", id: project },
    status: "in_progress",
    claimed_by: "agent-7",
    claim_expires_at: new Date(Date.now() + 3600_000).toISOString(),
    branch: "feature/agent",
    pr_url: "https://github.com/example/app/pull/7",
  });
  await seeder.push(page);
  await openApp(page, "#/dev");
  const c = card(page, `Agentenarbeit ${s}`);
  await expect(c).toContainText(`Dienst ${s}`);
  await expect(c).toContainText("agent-7");
  await expect(c).toContainText("feature/agent");
  await expect(c.getByRole("link", { name: "PR" })).toHaveAttribute("href", "https://github.com/example/app/pull/7");
});

test("Umsortieren zwischen Rängen des ClickUp-Imports", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Import ${s}`);
  const project = seeder.project(`Altbestand ${s}`, area, { view: "board" });
  const loc = { kind: "project", id: project };
  const a = seeder.task(`Eins ${s}`, { location: loc, rank: "0000000000001.000000" });
  const b = seeder.task(`Zwei ${s}`, { location: loc, rank: "0000000000001.500000" });
  const c = seeder.task(`Drei ${s}`, { location: loc, rank: "0000000000002.000000" });
  await seeder.push(page);

  await openApp(page, `#/bereiche/project/${project}`);
  const open = page.locator('.board-column[data-status="open"] .card .task-title');
  await expect(open).toHaveText([`Eins ${s}`, `Zwei ${s}`, `Drei ${s}`]);

  await card(page, `Drei ${s}`).click();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Shift+K");
  await expect(open).toHaveText([`Eins ${s}`, `Drei ${s}`, `Zwei ${s}`]);
  await page.keyboard.press("Shift+K");
  await expect(open).toHaveText([`Drei ${s}`, `Eins ${s}`, `Zwei ${s}`]);

  await expect
    .poll(async () => {
      const ranks = await Promise.all([c, a, b].map(async (id) => String((await entityFields(page, "task", id)).rank)));
      return [...ranks].sort().join() === ranks.join() && new Set(ranks).size === 3;
    })
    .toBe(true);
});

test("Dev-Sicht filtert nach Projekten per Chip", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Arbeit ${s}`);
  const app = seeder.project(`App ${s}`, area, { kind: "dev" });
  const api = seeder.project(`Api ${s}`, area, { kind: "dev" });
  seeder.task(`App-Karte ${s}`, { location: { kind: "project", id: app } });
  seeder.task(`Api-Karte ${s}`, { location: { kind: "project", id: api } });
  await seeder.push(page);
  await openApp(page, "#/dev");

  const chips = page.getByRole("group", { name: "Projekte filtern" });
  const all = chips.getByRole("button", { name: "Alle" });
  const appChip = chips.locator(`[data-project-id="${app}"]`);
  await expect(all).toHaveAttribute("aria-pressed", "true");
  await expect(card(page, `App-Karte ${s}`)).toBeVisible();
  await expect(card(page, `Api-Karte ${s}`)).toBeVisible();

  await appChip.click();
  await expect(appChip).toHaveAttribute("aria-pressed", "true");
  await expect(all).toHaveAttribute("aria-pressed", "false");
  await expect(card(page, `App-Karte ${s}`)).toBeVisible();
  await expect(card(page, `Api-Karte ${s}`)).toHaveCount(0);

  // Remembered across reloads.
  await page.reload();
  await expect(card(page, `App-Karte ${s}`)).toBeVisible();
  await expect(card(page, `Api-Karte ${s}`)).toHaveCount(0);

  await chips.getByRole("button", { name: "Alle" }).click();
  await expect(card(page, `Api-Karte ${s}`)).toBeVisible();
});

test("Board-Karte: verschachtelte Unteraufgaben, erledigte unten", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`Epics ${s}`, area, { view: "board" });
  const epic = seeder.task(`Epic ${s}`, { location: { kind: "project", id: project } });
  seeder.task(`Erledigt zuerst ${s}`, { parent_id: epic, status: "done" });
  const open = seeder.task(`Offen danach ${s}`, { parent_id: epic });
  seeder.task(`Enkel offen ${s}`, { parent_id: open });
  const finished = seeder.task(`Fertiger Zweig ${s}`, { parent_id: epic, status: "done" });
  seeder.task(`Enkel fertig ${s}`, { parent_id: finished, status: "done" });
  await seeder.push(page);
  await openApp(page, `#/bereiche/project/${project}`);

  const c = card(page, `Epic ${s}`);
  await c.locator(".subtask-chip").click();
  const top = c.locator(".card-subtasks > .subtree > .subtask > .subtask-row .subtask-title");
  await expect(top).toHaveText([`Offen danach ${s}`, `Erledigt zuerst ${s}`, `Fertiger Zweig ${s}`]);

  // A branch with open work is unfolded, a finished one stays folded until asked.
  const title = (text: string) => c.locator(".subtask-title", { hasText: text });
  await expect(title(`Enkel offen ${s}`)).toBeVisible();
  await expect(title(`Enkel fertig ${s}`)).toHaveCount(0);
  const branch = c.locator(".subtask-row", { hasText: `Fertiger Zweig ${s}` });
  await branch.getByRole("button", { name: "Unteraufgaben aufklappen" }).click();
  await expect(title(`Enkel fertig ${s}`)).toBeVisible();
});

test("Board-Karte: mit erledigten Unteraufgaben direkt abschließen", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`Abschluss ${s}`, area, { kind: "dev", repo: "example/app" });
  const epic = seeder.task(`Sammelaufgabe ${s}`, { location: { kind: "project", id: project }, status: "review" });
  seeder.task(`Schon fertig ${s}`, { parent_id: epic, status: "done" });
  seeder.task(`Verworfen ${s}`, { parent_id: epic, status: "dropped" });
  seeder.task(`Noch offen ${s}`, { parent_id: epic });
  await seeder.push(page);
  await openApp(page, `#/bereiche/project/${project}`);

  const c = card(page, `Sammelaufgabe ${s}`);
  const close = c.getByRole("button", { name: "Abschließen" });
  await expect(close).toHaveCount(0);

  // Ticking off the last open subtask on the card offers closing the task itself.
  await c.locator(".subtask-chip").click();
  await c.locator(".subtask-row", { hasText: `Noch offen ${s}` }).getByRole("button", { name: "Erledigt" }).click();
  await expect(close).toBeVisible();
  await close.click();

  const done = page.locator('.board-column[data-status="done"]');
  await expect(done.locator(".card", { hasText: `Sammelaufgabe ${s}` })).toBeVisible();
  await expect.poll(async () => (await entityFields(page, "task", epic)).status).toBe("done");
  await expect(page.locator(".toast", { hasText: "noch offen" })).toHaveCount(0);
  await expect(close).toHaveCount(0);
});

test("Erledigt-Spalte zeigt die letzten fünf, Ältere im Archiv", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Fluss ${s}`);
  const project = seeder.project(`Werk ${s}`, area, { kind: "dev" });
  const loc = { kind: "project", id: project };
  // Seeded in order, so "Fertig 7" is the most recently closed.
  for (let i = 1; i <= 7; i++) seeder.task(`Fertig ${i} ${s}`, { location: loc, status: "done" });
  await seeder.push(page);

  await openApp(page, `#/bereiche/project/${project}`);
  const done = page.locator('.board-column[data-status="done"]');
  await expect(done.locator(".card .task-title")).toHaveText([7, 6, 5, 4, 3].map((i) => `Fertig ${i} ${s}`));
  await expect(done.locator(".board-column-head .count")).toHaveText("7");
  await done.getByRole("button", { name: "2 weitere zeigen" }).click();
  await expect(done.locator(".card")).toHaveCount(7);
  await done.getByRole("button", { name: "Weniger zeigen" }).click();
  await expect(done.locator(".card")).toHaveCount(5);

  // Four days later they have left the board and wait in the archive, newest first.
  await page.clock.install({ time: Date.now() + 4 * 24 * 60 * 60 * 1000 });
  await reloadApp(page);
  await expect(done.locator(".card")).toHaveCount(0);
  await done.getByRole("button", { name: "Ältere erledigte" }).click();
  const archive = page.getByRole("dialog", { name: "Ältere erledigte" });
  await expect(archive.locator(".task-row .task-title")).toHaveText([7, 6, 5, 4, 3, 2, 1].map((i) => `Fertig ${i} ${s}`));
  await archive.getByRole("button", { name: "Schließen" }).click();
  await expect(archive).toBeHidden();
});

test("Archiv mobil: Schließen bleibt beim Scrollen sichtbar @mobile", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Fluss ${s}`);
  const project = seeder.project(`Werk ${s}`, area, { kind: "dev" });
  for (let i = 1; i <= 30; i++) seeder.task(`Alt ${i} ${s}`, { location: { kind: "project", id: project }, status: "done" });
  await seeder.push(page);

  await openApp(page, `#/bereiche/project/${project}`);
  await page.clock.install({ time: Date.now() + 4 * 24 * 60 * 60 * 1000 });
  await reloadApp(page);
  await page.locator('.board-column[data-status="done"]').getByRole("button", { name: "Ältere erledigte" }).click();
  const archive = page.getByRole("dialog", { name: "Ältere erledigte" });
  const close = archive.getByRole("button", { name: "Schließen" });
  // Right after opening, at the top of a long list …
  await expect(archive.locator(".task-row").first()).toBeInViewport();
  await expect(close).toBeInViewport();
  // … and after scrolling to its end.
  const last = archive.locator(".task-row", { hasText: `Alt 1 ${s}` });
  await last.scrollIntoViewIfNeeded();
  await expect(last).toBeInViewport();
  await expect(close).toBeInViewport();
  await close.click();
  await expect(archive).toBeHidden();
});
