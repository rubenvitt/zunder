import { expect, test } from "@playwright/test";
import { entityFields, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

test("Schnellerfassung: nur Titel landet in der Inbox", async ({ page }) => {
  await openApp(page);
  const title = `Milch kaufen ${suffix()}`;
  await page.keyboard.press("n");
  await page.getByLabel("Titel der neuen Aufgabe").fill(title);
  await page.keyboard.press("Enter");
  await expect(page.locator(".toast", { hasText: "Erfasst in Inbox" })).toBeVisible();
  await page.keyboard.press("g");
  await page.keyboard.press("i");
  await expect(row(page, title)).toBeVisible();
});

test("Schnellerfassung wählt das offene Projekt vor, #tag und × übersteuern", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const areaId = seeder.area(`Haus ${s}`);
  const projectId = seeder.project(`Umzug ${s}`, areaId);
  await seeder.push(page);
  await openApp(page, `#/bereiche/project/${projectId}`);

  // Preselected: the open project.
  await page.keyboard.press("n");
  const input = page.getByLabel("Titel der neuen Aufgabe");
  await expect(page.locator(".capture-location")).toContainText(`Haus ${s} › Umzug ${s}`);
  await input.fill(`Kartons kaufen ${s}`);
  await page.keyboard.press("Enter");
  await expect(page.locator(".toast", { hasText: `Erfasst in Haus ${s} › Umzug ${s}` })).toBeVisible();
  await expect(row(page, `Kartons kaufen ${s}`)).toBeVisible();

  // A #tag wins over the preselection.
  await page.getByRole("button", { name: "Neue Aufgabe" }).first().click();
  await input.fill(`Keller ausmisten #haus-${s}`);
  await expect(page.locator(".capture-location")).toContainText(`Haus ${s}`);
  await expect(page.locator(".capture-location")).not.toContainText("Umzug");
  await page.keyboard.press("Enter");
  await expect(page.locator(".toast", { hasText: `Erfasst in Haus ${s}.` })).toBeVisible();

  // Backspace in the empty field (or ×) drops the preselection: back to the inbox.
  await page.keyboard.press("n");
  await page.keyboard.press("Backspace");
  await expect(page.locator(".capture-location")).toHaveText("Inbox");
  await input.fill(`Nachsendeauftrag ${s}`);
  await page.keyboard.press("Enter");
  await expect(page.locator(".toast", { hasText: "Erfasst in Inbox" })).toBeVisible();

  await page.keyboard.press("g");
  await page.keyboard.press("i");
  await expect(row(page, `Nachsendeauftrag ${s}`)).toBeVisible();
  await expect(row(page, `Kartons kaufen ${s}`)).toHaveCount(0);
  const id = await row(page, `Nachsendeauftrag ${s}`).getAttribute("data-task-id");
  await expect.poll(async () => (await entityFields(page, "task", id!)).location).toBeUndefined();
});

test("Schnellerfassung in Lose und im Bereich", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const areaId = seeder.area(`Garten ${s}`);
  await seeder.push(page);

  await openApp(page, "#/lose");
  await page.keyboard.press("n");
  await expect(page.locator(".capture-location")).toContainText("Lose");
  await page.getByLabel("Titel der neuen Aufgabe").fill(`Irgendwann ${s}`);
  await page.keyboard.press("Enter");
  await expect(row(page, `Irgendwann ${s}`)).toBeVisible();

  await page.goto(`/#/bereiche/area/${areaId}`);
  await page.keyboard.press("n");
  await expect(page.locator(".capture-location")).toContainText(`Garten ${s}`);
  await page.getByRole("button", { name: "Vorauswahl entfernen, in die Inbox" }).click();
  await expect(page.locator(".capture-location")).toHaveText("Inbox");
  await page.getByRole("button", { name: "Abbrechen" }).click();
  // The preselection is chosen afresh each time the capture opens.
  await page.keyboard.press("n");
  await expect(page.locator(".capture-location")).toContainText(`Garten ${s}`);
  await page.getByLabel("Titel der neuen Aufgabe").fill(`Rasen mähen ${s}`);
  await page.keyboard.press("Enter");
  await expect(row(page, `Rasen mähen ${s}`)).toBeVisible();
});

test("Schnellerfassung zeigt Beispiele der Kurzschreibweise, auch in den Tastenkürzeln", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("n");
  const input = page.getByLabel("Titel der neuen Aufgabe");
  const toggle = page.getByRole("button", { name: "Beispiele für die Kurzschreibweise" });
  const help = page.locator(".capture-help");
  await expect(toggle).toBeVisible();
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(help).toBeHidden();

  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  for (const example of ["morgen 9:00", "#bereich", "jeden Freitag", "alle 2 Wochen", "monatlich am 15."]) {
    await expect(help.getByText(example, { exact: true })).toBeVisible();
  }
  // The field keeps the focus: typing goes on.
  await expect(input).toBeFocused();

  // Remembered on this device: open again next time, until closed.
  await page.keyboard.press("Escape");
  await page.keyboard.press("n");
  await expect(help).toBeVisible();
  await toggle.click();
  await expect(help).toBeHidden();
  await page.keyboard.press("Escape");

  // The shortcut help lists them too.
  await page.keyboard.press("?");
  const dialog = page.getByRole("dialog", { name: "Tastenkürzel" });
  await expect(dialog.getByRole("heading", { name: "Kurzschreibweise beim Erfassen" })).toBeVisible();
  await expect(dialog.getByText("jeden Monatsletzten", { exact: true })).toBeVisible();
});

test("Schnellerfassung mit Kurzschreibweise setzt Fälligkeit, Erinnerung und Bereich", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const areaId = seeder.area(`Gesundheit ${s}`);
  await seeder.push(page);
  await openApp(page);

  await page.keyboard.press("n");
  const input = page.getByLabel("Titel der neuen Aufgabe");
  await input.fill(`Zahnarzt anrufen morgen 9:00 #gesundheit-${s}`);
  // Live preview of the parsed shorthand.
  await expect(page.locator(".capture-preview")).toContainText("Zahnarzt anrufen");
  await expect(page.locator(".capture-preview")).toContainText(`Gesundheit ${s}`);
  await expect(page.locator(".capture-preview")).toContainText("Morgen 09:00");
  await page.keyboard.press("Enter");

  await page.goto(`/#/bereiche/area/${areaId}`);
  const taskRow = row(page, "Zahnarzt anrufen");
  await expect(taskRow).toBeVisible();
  await expect(taskRow.locator(".chip", { hasText: "Morgen 09:00" })).toBeVisible();

  const id = await taskRow.getAttribute("data-task-id");
  await expect
    .poll(async () => entityFields(page, "task", id!))
    .toMatchObject({ title: "Zahnarzt anrufen", location: { kind: "area", id: areaId } });
  const fields = await entityFields(page, "task", id!);
  expect(fields.due).toMatch(/T09:00:00[+-]\d{2}:\d{2}$/);
  expect(fields.remind_at).toBe(fields.due);
});

test("Schnellerfassung mit wartet:Name legt eine blockierte Aufgabe an, die auf jemanden wartet", async ({ page }) => {
  const s = suffix();
  await openApp(page, "#/lose");
  await page.keyboard.press("n");
  const input = page.getByLabel("Titel der neuen Aufgabe");
  await input.fill(`Angebot ${s} wartet:Team_Einkauf @andere`);
  await expect(page.locator(".capture-preview .chip-waiting-on")).toHaveText("wartet auf Team Einkauf");
  await expect(page.locator(".capture-title")).toHaveText(`Angebot ${s}`);
  await input.press("Enter");

  // Handed back already: a plain confirmation, no commitment question.
  await expect(page.locator(".toast:not(.toast-ask)", { hasText: "Erfasst in Lose, wartet auf Team Einkauf." })).toBeVisible();
  await expect(page.locator(".toast-ask")).toHaveCount(0);
  const taskRow = row(page, `Angebot ${s}`);
  await expect(taskRow.locator(".chip-waiting-on")).toHaveText("wartet auf Team Einkauf");
  const id = await taskRow.getAttribute("data-task-id");
  await expect
    .poll(async () => {
      const f = await entityFields(page, "task", id!);
      return [f.waiting_on, f.status, f.for_whom];
    })
    .toEqual(["Team Einkauf", "blocked", "andere"]);
});

test("Schnellerfassung mit Rhythmus legt eine wiederkehrende Aufgabe an, × lässt die Wörter im Titel", async ({ page }) => {
  const s = suffix();
  await openApp(page);

  await page.keyboard.press("n");
  const input = page.getByLabel("Titel der neuen Aufgabe");
  // × removes the rhythm; its words stay in the title.
  await input.fill(`Gießen ${s} täglich`);
  const chip = page.locator(".capture-recurrence");
  await expect(chip).toContainText("Täglich");
  await chip.getByRole("button", { name: "Rhythmus entfernen" }).click();
  await expect(chip).toHaveCount(0);
  await expect(page.locator(".capture-title")).toHaveText(`Gießen ${s} täglich`);

  // Emptying the field brings rhythms back.
  await input.fill("");
  await input.fill(`Odoo absenden ${s} jeden Monatsletzten`);
  await expect(chip).toContainText("Monatlich am Monatsletzten");
  await expect(page.locator(".capture-title")).toHaveText(`Odoo absenden ${s}`);
  await page.keyboard.press("Enter");

  await page.keyboard.press("g");
  await page.keyboard.press("i");
  const taskRow = row(page, `Odoo absenden ${s}`);
  await expect(taskRow).toBeVisible();
  await expect(taskRow.locator(".chip-recurring")).toHaveAttribute("title", "Wiederkehrend: Monatlich am Monatsletzten");
  const id = await taskRow.getAttribute("data-task-id");
  await expect
    .poll(async () => entityFields(page, "task", id!))
    .toMatchObject({ title: `Odoo absenden ${s}`, recurrence: { every: 1, unit: "month", day: "last", from: "due" } });
  const due = String((await entityFields(page, "task", id!)).due);
  const [y, m, d] = due.split("-").map(Number);
  // The last day of the current month.
  expect(d).toBe(new Date(y!, m!, 0).getDate());
});

test("Abgelehnte Operation wird zurückgerollt und gemeldet", async ({ page }) => {
  // A genuine core rejection: the client still shows an area that another client has
  // deleted meanwhile (pulls are held back), so its own validation lets the move pass
  // and the server refuses it (invalid_reference).
  const s = suffix();
  const seeder = new Seeder();
  const areaId = seeder.area(`Bald weg ${s}`);
  const taskId = seeder.task(`Zuordnen ${s}`);
  await seeder.push(page);
  await openApp(page, "#/inbox");
  await expect(row(page, `Zuordnen ${s}`)).toBeVisible();

  await page.route("**/api/sync/pull**", (route) => route.abort());
  await page.route("**/api/sync/events", (route) => route.abort());
  const other = new Seeder();
  other.raw("area", areaId, { deleted: true });
  await other.push(page);

  await row(page, `Zuordnen ${s}`).locator(".task-main").focus();
  await page.keyboard.press("a");
  await page.keyboard.type(`Bald weg ${s}`);
  await page.keyboard.press("Enter");
  await expect(page.locator(".toast-error", { hasText: "Änderung abgelehnt" })).toBeVisible();
  // Rolled back: the task is back in the inbox, the server kept it there.
  await expect(row(page, `Zuordnen ${s}`)).toBeVisible();
  expect((await entityFields(page, "task", taskId)).location).toBeUndefined();
  await page.unrouteAll({ behavior: "ignoreErrors" });
});

test("Clientseitige Validierung verhindert leere Titel im Detail", async ({ page }) => {
  const seeder = new Seeder();
  const title = `Umbenennen ${suffix()}`;
  seeder.task(title);
  await seeder.push(page);
  await openApp(page, "#/inbox");
  await row(page, title).locator(".task-main").click();
  const field = page.getByLabel("Titel", { exact: true });
  await field.fill("");
  await field.press("Enter");
  await expect(page.locator(".toast-error", { hasText: "Eine Aufgabe braucht einen Titel." })).toBeVisible();
  await expect(row(page, title)).toBeVisible();
});
