// Links between tasks: mentions like "ZUN-42" in Markdown with a hover card,
// and "Blockiert durch" with its reverse "Blockiert".

import { expect, test, type Page } from "@playwright/test";
import { entityFields, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const detail = (page: Page) => page.getByRole("complementary", { name: "Aufgabendetail" });
const title = (page: Page) => detail(page).getByLabel("Titel", { exact: true });

function seed(s: string) {
  const key = `L${s.toUpperCase().replace(/[^A-Z0-9]/g, "").slice(0, 5)}`;
  const seeder = new Seeder();
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`Umzug ${s}`, area, { key });
  // Numbered in creation order: KEY-1, KEY-2.
  const offer = seeder.task(`Angebot einholen ${s}`, { location: { kind: "project", id: project } });
  const contract = seeder.task(`Vertrag ${s}`, {
    location: { kind: "project", id: project },
    description: `Erst ${key}-1 klären, ${key}-999 gibt es nicht.`,
  });
  return { seeder, key, project, offer, contract };
}

test("Erwähnte Kennungen sind Links mit Hover-Karte", async ({ page }) => {
  const s = suffix();
  const { seeder, key, project } = seed(s);
  await seeder.push(page);
  await openApp(page, `#/bereiche/project/${project}`);
  await row(page, `Vertrag ${s}`).locator(".task-main").click();

  const description = detail(page).getByRole("textbox", { name: "Beschreibung (Markdown)" });
  const mention = description.locator("a.task-mention");
  await expect(mention).toHaveCount(1);
  await expect(mention).toHaveText(`${key}-1`);
  await expect(mention).toHaveAttribute("href", `#/aufgabe/${key}-1`);
  await expect(description).toContainText(`${key}-999`);

  // The panel may still slide in: hover again until the card holds.
  const card = page.getByRole("tooltip");
  await expect(async () => {
    await page.mouse.move(0, 0);
    await mention.hover();
    await expect(card).toBeVisible({ timeout: 1500 });
  }).toPass();
  await expect(card).toContainText(`Angebot einholen ${s}`);
  await expect(card).toContainText("Offen");
  await expect(card).toContainText(`Umzug ${s}`);

  // A click opens the mentioned task; the stored text stays plain Markdown.
  await mention.click();
  await expect(title(page)).toHaveValue(`Angebot einholen ${s}`);
  await expect(card).toBeHidden();
});

test("Blockiert durch mit Rückverweis, in beide Richtungen bearbeitbar", async ({ page }) => {
  const s = suffix();
  const { seeder, key, project, offer, contract } = seed(s);
  await seeder.push(page);
  await openApp(page, `#/bereiche/project/${project}`);
  await row(page, `Vertrag ${s}`).locator(".task-main").click();

  // Add the blocker through the task picker.
  await detail(page).getByRole("button", { name: "Blockierende Aufgabe" }).click();
  await expect(page.locator(".palette-input")).toBeFocused();
  await page.keyboard.type(`Angebot einholen ${s}`);
  await expect(page.getByRole("dialog").getByRole("option").first()).toContainText(`Angebot einholen ${s}`);
  await page.keyboard.press("Enter");
  const blockedBy = detail(page).getByRole("list", { name: "Blockiert durch" });
  await expect(blockedBy.getByRole("link")).toHaveText(`${key}-1Angebot einholen ${s}`);
  await expect.poll(async () => (await entityFields(page, "task", contract)).blocked_by).toEqual([offer]);
  await expect(detail(page).locator("h3", { hasText: "Abhängigkeiten" })).toContainText("wartet auf 1");
  await expect(row(page, `Vertrag ${s}`).locator(".chip-waiting")).toBeVisible();

  // The reverse side shows it; removing it there writes the waiting task.
  await blockedBy.getByRole("link").click();
  await expect(title(page)).toHaveValue(`Angebot einholen ${s}`);
  const blocks = detail(page).getByRole("list", { name: "Blockiert", exact: true });
  await expect(blocks.getByRole("link")).toContainText(`Vertrag ${s}`);
  await blocks.getByRole("button", { name: `„Vertrag ${s}“ wartet nicht mehr auf diese Aufgabe` }).click();
  await expect(blocks.getByRole("link")).toHaveCount(0);
  await expect.poll(async () => (await entityFields(page, "task", contract)).blocked_by ?? null).toBeNull();
  await expect(row(page, `Vertrag ${s}`).locator(".chip-waiting")).toHaveCount(0);

  // Done blockers no longer count as waiting.
  seeder.raw("task", contract, { blocked_by: [offer] });
  seeder.raw("task", offer, { status: "done" });
  await seeder.push(page);
  await expect(row(page, `Vertrag ${s}`)).toBeVisible();
  await expect(row(page, `Vertrag ${s}`).locator(".chip-waiting")).toHaveCount(0);
});

test("Gegenseitiges Blockieren wird abgelehnt", async ({ page }) => {
  const s = suffix();
  const { seeder, offer, contract } = seed(s);
  seeder.raw("task", contract, { blocked_by: [offer] });
  await seeder.push(page);
  await openApp(page, `#/aufgabe/${offer}`);
  await expect(title(page)).toHaveValue(`Angebot einholen ${s}`);
  // The picker leaves out tasks that would close a circle.
  await detail(page).getByRole("button", { name: "Blockierende Aufgabe" }).click();
  await expect(page.locator(".palette-input")).toBeFocused();
  await page.keyboard.type(`Vertrag ${s}`);
  await expect(page.getByRole("dialog").getByRole("option")).toHaveCount(0);
});
