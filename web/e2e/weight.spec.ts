// Weight, for whom and "Heute nicht" (contract 5.1): the personal reading of a task outside
// dev projects, and the deferral counter that turns the third deferral into a decision.

import { expect, test, type Page } from "@playwright/test";
import { localDate } from "../src/sync/dates";
import { deferDay, offersNextWeek } from "../src/sync/defer";
import { entityFields, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const detail = (page: Page) => page.getByRole("complementary", { name: "Aufgabendetail" });
const today = () => localDate(new Date());
const yesterday = () => localDate(new Date(Date.now() - 86_400_000));

test("Gewicht und Für wen: im Detail wählen, als Chip sehen, erneut klicken entfernt", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Steuer ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, "#/lose");
  await row(page, `Steuer ${s}`).locator(".task-main").click();

  // The meaning is readable without hovering: the question first, then the picked option's hint.
  await expect(detail(page).getByText("Ist es dir wichtig, und drängt es wirklich?")).toBeVisible();
  await expect(detail(page).getByText("Wer wartet, wenn es sich verschiebt?")).toBeVisible();
  const weight = detail(page).getByRole("group", { name: "Gewicht" });
  await weight.getByRole("button", { name: "Glut" }).click();
  await expect(weight.getByRole("button", { name: "Glut" })).toHaveAttribute("aria-pressed", "true");
  await expect(row(page, `Steuer ${s}`).locator(".chip-weight-glut")).toHaveText("Glut");
  await detail(page).getByRole("group", { name: "Für wen" }).getByRole("button", { name: "andere" }).click();
  await expect(row(page, `Steuer ${s}`).locator(".chip-for-andere")).toHaveText("für andere");
  await expect(detail(page).getByText("Jemand wartet auf deine Zusage: wer verschiebt, sagt Bescheid")).toBeVisible();
  await expect.poll(async () => {
    const f = await entityFields(page, "task", id);
    return [f.weight, f.for_whom];
  }).toEqual(["glut", "andere"]);

  // Optional: the picked option clears on a second click.
  await weight.getByRole("button", { name: "Glut" }).click();
  await expect(row(page, `Steuer ${s}`).locator(".chip-weight")).toHaveCount(0);
  await expect.poll(async () => (await entityFields(page, "task", id)).weight).toBeNull();
});

test("Dev-Projekte bleiben ohne Gewicht, Für wen und Heute nicht", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`App ${s}`, area, { kind: "dev", repo: "example/app", view: "list" });
  seeder.task(`Login-Fehler ${s}`, { location: { kind: "project", id: project }, type: "bug", weight: "brand", for_whom: "andere" });
  await seeder.push(page);
  await openApp(page, `#/bereiche/project/${project}`);
  const r = row(page, `Login-Fehler ${s}`);
  await expect(r).toBeVisible();
  await expect(r.locator(".chip-weight, .chip-for")).toHaveCount(0);
  await r.locator(".task-main").click();
  // A value an agent set stays visible in the detail, the empty choices do not appear.
  await expect(detail(page).getByRole("group", { name: "Gewicht" })).toBeVisible();
  await expect(detail(page).getByRole("button", { name: "Nächste Woche" })).toHaveCount(0);
});

test("Heute nicht: zweimal verschieben, beim dritten Mal ruhen lassen", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Keller ${s}`, { location: { kind: "loose" }, due: yesterday(), weight: "asche" });
  seeder.task(`Zusage ${s}`, { location: { kind: "loose" }, due: yesterday(), weight: "brand" });
  await seeder.push(page);
  await openApp(page, "#/heute");

  // What matters comes first within a group.
  const overdue = page.getByRole("list", { name: "Überfällig" });
  await expect(overdue.locator(".task-title")).toHaveText([`Zusage ${s}`, `Keller ${s}`]);

  await row(page, `Keller ${s}`).getByRole("button", { name: "Heute nicht" }).click();
  await expect(row(page, `Keller ${s}`)).toHaveCount(0);
  await expect(page.locator(".toast")).toContainText(`„Keller ${s}“ auf morgen verschoben.`);
  await expect.poll(async () => {
    const f = await entityFields(page, "task", id);
    return [f.due, f.deferrals];
  }).toEqual([deferDay("tomorrow", today()), 1]);

  // The second time from the detail, on a task due already (the first one is tomorrow now).
  const again = new Seeder();
  const attic = again.task(`Dachboden ${s}`, { location: { kind: "loose" }, due: yesterday(), weight: "asche", deferrals: 1 });
  await again.push(page);
  await openApp(page, "#/lose");
  await row(page, `Dachboden ${s}`).locator(".task-main").click();
  const nextWeek = detail(page).getByRole("button", { name: "Nächste Woche" });
  if (offersNextWeek(today())) await nextWeek.click();
  else {
    // Sunday: next week is tomorrow, so only "Morgen" is offered.
    await expect(nextWeek).toHaveCount(0);
    await detail(page).getByRole("button", { name: "Morgen", exact: true }).click();
  }
  await expect(detail(page).locator(".deferral-count")).toHaveText("2-mal verschoben, beim nächsten Mal entscheiden");
  await expect(row(page, `Dachboden ${s}`).locator(".chip-deferred.chip-decision")).toHaveText("2");
  await expect.poll(async () => (await entityFields(page, "task", attic)).due).toBe(deferDay("next_week", today()));

  // The third time: a decision instead of a click.
  await detail(page).getByRole("button", { name: "Morgen", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Entscheiden statt verschieben" });
  await expect(dialog).toContainText(`„Dachboden ${s}“ ist schon 2-mal verschoben.`);
  await dialog.getByRole("button", { name: /Ruhen lassen/ }).click();
  await expect(dialog).toBeHidden();
  await expect.poll(async () => {
    const f = await entityFields(page, "task", attic);
    return [f.due, f.deferrals, typeof f.undated_ack];
  }).toEqual([null, null, "string"]);
  await expect(detail(page).locator(".undated-ack")).toBeVisible();
});

test("Heute nicht per Taste: s verschiebt, beim dritten Mal streichen oder trotzdem verschieben", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const drop = seeder.task(`Idee ${s}`, { location: { kind: "loose" }, due: today(), deferrals: 2 });
  const keep = seeder.task(`Termin ${s}`, { location: { kind: "loose" }, due: today(), deferrals: 2 });
  await seeder.push(page);
  await openApp(page, "#/heute");

  const dialog = page.getByRole("dialog", { name: "Entscheiden statt verschieben" });
  await row(page, `Idee ${s}`).locator(".task-main").focus();
  await page.keyboard.press("s");
  await dialog.getByRole("button", { name: /Streichen/ }).click();
  await expect(row(page, `Idee ${s}`)).toHaveCount(0);
  await expect.poll(async () => (await entityFields(page, "task", drop)).status).toBe("dropped");

  await row(page, `Termin ${s}`).locator(".task-main").focus();
  await page.keyboard.press("s");
  await dialog.getByRole("button", { name: "Trotzdem verschieben" }).click();
  await expect(row(page, `Termin ${s}`)).toHaveCount(0);
  await expect.poll(async () => {
    const f = await entityFields(page, "task", keep);
    return [f.due, f.deferrals];
  }).toEqual([deferDay("tomorrow", today()), 3]);
});
