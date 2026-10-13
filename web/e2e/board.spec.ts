import { expect, test } from "@playwright/test";
import { card, entityFields, openApp, prepare, Seeder, suffix } from "./fixtures";

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
