// Task detail: inline fields commit only real edits, drafts stay with their task,
// unsafe links are not clickable, flame/Timing feedback, links to single tasks.

import { expect, test, type Page } from "@playwright/test";
import { fromLegacy } from "../src/sync/ids";
import { entityFields, newId, openApp, prepare, row, Seeder, suffix } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

const detail = (page: Page) => page.getByRole("complementary", { name: "Aufgabendetail" });

async function openLoose(page: Page, title: string) {
  await openApp(page, "#/lose");
  await row(page, title).locator(".task-main").click();
  await expect(detail(page)).toBeVisible();
}

test("Fokus ohne Tippen überschreibt keine Änderung von außen", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Agentenziel ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openLoose(page, `Agentenziel ${s}`);

  const description = detail(page).getByRole("textbox", { name: "Beschreibung (Markdown)" });
  await description.click();
  // An agent writes the description while the field has the focus.
  seeder.raw("task", id, { description: "Plan vom Agenten" });
  await seeder.push(page);
  await expect(description).toHaveText("Plan vom Agenten");
  await detail(page).locator("h3", { hasText: "Links" }).click();
  await page.waitForTimeout(300);
  expect((await entityFields(page, "task", id)).description).toBe("Plan vom Agenten");

  // A real edit is still committed on blur.
  await description.fill("Eigener Text");
  await detail(page).locator("h3", { hasText: "Links" }).click();
  await expect.poll(async () => (await entityFields(page, "task", id)).description).toBe("Eigener Text");
});

test("Beschreibung: Markdown wird formatiert angezeigt und als Markdown gespeichert", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Markdown ${s}`, { location: { kind: "loose" }, description: "## Ziel\n\n- **fett** und `code`\n- [Beispiel](https://example.org/)" });
  await seeder.push(page);
  await openLoose(page, `Markdown ${s}`);

  const description = detail(page).getByRole("textbox", { name: "Beschreibung (Markdown)" });
  await expect(description.locator("h2")).toHaveText("Ziel");
  await expect(description.locator("strong")).toHaveText("fett");
  await expect(description.locator("code")).toHaveText("code");
  await expect(description.getByRole("link", { name: "Beispiel" })).toHaveAttribute("href", "https://example.org/");

  // Typing Markdown syntax formats in place; the stored value stays Markdown.
  await description.locator("li").last().click();
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Enter");
  await page.keyboard.type("Neuer Absatz mit **Nachdruck** ");
  await expect(description.locator("strong", { hasText: "Nachdruck" })).toBeVisible();
  await page.keyboard.press("Control+Enter");
  await expect.poll(async () => String((await entityFields(page, "task", id)).description)).toContain("Neuer Absatz mit **Nachdruck**");
  expect(String((await entityFields(page, "task", id)).description)).toContain("## Ziel");

  // Escape discards an unsaved edit.
  await description.click();
  await page.keyboard.type("Verworfen");
  await page.keyboard.press("Escape");
  await expect(description).not.toContainText("Verworfen");
  await expect(detail(page)).toBeVisible();
});

test("Beschreibung: Links öffnen sich per Klick, beim Schreiben per Cmd/Ctrl+Klick", async ({ page, context }) => {
  await context.route("https://example.org/**", (route) => route.fulfill({ body: "Beispiel" }));
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Linkziel ${s}`, { location: { kind: "loose" }, description: "Siehe [Beispiel](https://example.org/doku) hier" });
  await seeder.push(page);
  await openLoose(page, `Linkziel ${s}`);

  const description = detail(page).getByRole("textbox", { name: "Beschreibung (Markdown)" });
  const link = description.getByRole("link", { name: "Beispiel" });

  // Reading: a plain click opens the link and does not start editing.
  const [reading] = await Promise.all([page.waitForEvent("popup"), link.click()]);
  await expect(reading).toHaveURL("https://example.org/doku");
  await reading.close();
  await expect(description).not.toBeFocused();

  // Writing: a plain click places the cursor, Cmd/Ctrl+click opens.
  await description.locator("p").click({ position: { x: 2, y: 2 } });
  await expect(description).toBeFocused();
  let opened = false;
  page.once("popup", () => (opened = true));
  await link.click();
  await page.waitForTimeout(300);
  expect(opened).toBe(false);
  const [writing] = await Promise.all([page.waitForEvent("popup"), link.click({ modifiers: ["ControlOrMeta"] })]);
  await expect(writing).toHaveURL("https://example.org/doku");
  await writing.close();
  expect((await entityFields(page, "task", id)).description).toBe("Siehe [Beispiel](https://example.org/doku) hier");
});

test("Ein abgelehnter Wert verschwindet aus dem Feld", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Behält Titel ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openLoose(page, `Behält Titel ${s}`);
  const title = detail(page).getByLabel("Titel", { exact: true });
  await title.fill("");
  await title.press("Enter");
  await expect(page.locator(".toast-error", { hasText: "Eine Aufgabe braucht einen Titel." })).toBeVisible();
  await expect(title).toHaveValue(`Behält Titel ${s}`);
});

test("Entwürfe bleiben nicht im Detail einer anderen Aufgabe stehen", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const parent = seeder.task(`Eltern ${s}`, { location: { kind: "loose" } });
  seeder.task(`Kind A ${s}`, { parent_id: parent });
  await seeder.push(page);
  await openLoose(page, `Eltern ${s}`);

  await detail(page).getByLabel("Neue Unteraufgabe").fill("Gehört zu Eltern");
  await detail(page).getByLabel("Link-URL").fill("https://example.org/eltern");
  await detail(page).locator(".subtask .link-button", { hasText: `Kind A ${s}` }).click();
  await expect(detail(page).getByLabel("Titel", { exact: true })).toHaveValue(`Kind A ${s}`);
  await expect(detail(page).getByLabel("Neue Unteraufgabe")).toHaveValue("");
  await expect(detail(page).getByLabel("Link-URL")).toHaveValue("");
});

test("Links mit anderen Schemata als http(s)/mailto sind nicht klickbar", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Links ${s}`, {
    location: { kind: "loose" },
    links: [{ url: "javascript:alert(1)", title: "Böse" }, { url: "https://example.org/", title: "Beispiel" }],
  });
  await seeder.push(page);
  await openLoose(page, `Links ${s}`);
  const list = detail(page).locator(".link-list").first();
  await expect(list).toContainText("Böse");
  await expect(list.locator('a[href^="javascript:"]')).toHaveCount(0);
  await expect(list.getByRole("link", { name: "Beispiel" })).toHaveAttribute("href", "https://example.org/");
});

test("Flamme: ein Timing-Fehler wird gemeldet statt „Flamme gestartet“", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Flamme ${s}`, { location: { kind: "loose" }, phoenix: { role: "flamme", fire: "2026-W42" } });
  await seeder.push(page);
  await page.route("**/api/tasks/*/flame/start", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        running: true,
        unchanged: false,
        stopped_task_ids: [],
        timing: { configured: true, used: false, entry: null, error: "Timing-Token abgelaufen." },
        warnings: [],
      }),
    }),
  );
  await openLoose(page, `Flamme ${s}`);
  await detail(page).getByRole("button", { name: "Flamme starten" }).click();
  await expect(page.locator(".toast-warning")).toHaveText(/Flamme läuft, aber Timing: Timing-Token abgelaufen\./);
  await expect(page.locator(".toast", { hasText: "Flamme gestartet." })).toHaveCount(0);
});

test("Erledigt mit offenen Unteraufgaben zeigt genau einen Hinweis", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const parent = seeder.task(`Eltern ${s}`, { location: { kind: "loose" } });
  seeder.task(`Kind ${s}`, { parent_id: parent });
  await seeder.push(page);
  await openApp(page, "#/lose");
  const pushed = page.waitForResponse((r) => r.url().endsWith("/api/sync/push") && r.request().method() === "POST");
  await row(page, `Eltern ${s}`).locator(".check").click();
  const body = (await (await pushed).json()) as { warnings: { code: string }[] };
  expect(body.warnings.map((w) => w.code)).toContain("open_subtasks");
  await page.waitForTimeout(200);
  await expect(page.locator(".toast-warning")).toHaveCount(1);
});

test("#/aufgabe/<id> öffnet die Aufgabe über ihrer Sicht", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Verlinkt ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, `#/aufgabe/${id}`);
  await expect(page.locator(".view-head h1")).toHaveText("Lose");
  await expect(detail(page).getByLabel("Titel", { exact: true })).toHaveValue(`Verlinkt ${s}`);
  await detail(page).getByRole("button", { name: "Link kopieren" }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(new URL(`/#/aufgabe/${id}`, page.url()).href);

  await page.goto("/#/aufgabe/01900000-0000-7000-8000-00000000dead");
  await expect(page.locator(".toast-error", { hasText: "Aufgabe nicht gefunden" })).toBeVisible();
  await expect(page.locator(".view-head h1")).toHaveText("Heute");
});

test("Heute: das Datum im Kopf folgt der Uhr über Mitternacht", async ({ page }) => {
  // A fresh document (with the real backend, the login already loaded the app at "/").
  await page.goto("about:blank");
  await page.clock.install({ time: new Date("2026-10-11T23:59:00+02:00") });
  await openApp(page, "#/heute");
  await expect(page.locator(".view-head .muted")).toHaveText("Sonntag, 11. Oktober");
  await page.clock.fastForward("02:00");
  await expect(page.locator(".view-head .muted")).toHaveText("Montag, 12. Oktober");
});

test("Dunkles Design: Text auf Fehler-Toasts ist dunkel", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Dunkel ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openLoose(page, `Dunkel ${s}`);
  const title = detail(page).getByLabel("Titel", { exact: true });
  await title.fill("");
  await title.press("Enter");
  const toast = page.locator(".toast-error").first();
  await expect(toast).toBeVisible();
  expect(await toast.evaluate((el) => getComputedStyle(el).color)).toBe("rgb(22, 19, 15)");
});

test("Dev-Aufgabe: ID per Klick und mit y kopieren", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const clipboard = () => page.evaluate(() => navigator.clipboard.readText());
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`App ${s}`, area, { kind: "dev" });
  const a = seeder.task(`Erste ${s}`, { location: { kind: "project", id: project } });
  const b = seeder.task(`Zweite ${s}`, { location: { kind: "project", id: project } });
  await seeder.push(page);
  await openApp(page, `#/bereiche/project/${project}`);

  // Detail panel: the id is shown right at the top and copies on click.
  const card = (title: string) => page.locator(".card", { hasText: title });
  await card(`Erste ${s}`).click();
  await detail(page).getByRole("button", { name: "ID kopieren" }).click();
  await expect(page.locator(".toast")).toContainText(a);
  await expect.poll(clipboard).toBe(a);

  // Keyboard: y copies the selected task, with or without the panel.
  await page.keyboard.press("Escape");
  await expect(detail(page)).toBeHidden();
  await card(`Zweite ${s}`).click();
  await page.keyboard.press("Escape");
  await page.keyboard.press("y");
  await expect.poll(clipboard).toBe(b);
});

test("Projektschlüssel: Aufgaben bekommen Kennungen wie ZUN-1", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const clipboard = () => page.evaluate(() => navigator.clipboard.readText());
  const s = suffix();
  const key = `Z${s.toUpperCase().replace(/[^A-Z0-9]/g, "").slice(0, 5)}`;
  const seeder = new Seeder();
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`App ${s}`, area, { kind: "dev" });
  seeder.task(`Erste ${s}`, { location: { kind: "project", id: project } });
  seeder.task(`Zweite ${s}`, { location: { kind: "project", id: project } });
  await seeder.push(page);
  await openApp(page, `#/bereiche/project/${project}`);

  // The key is set in the project form; the server numbers the tasks.
  await page.getByRole("button", { name: "Bearbeiten" }).click();
  await page.getByRole("dialog").locator('input[name="key"]').fill(key.toLowerCase());
  await page.getByRole("dialog").getByRole("button", { name: "Speichern" }).click();
  const card = (title: string) => page.locator(".card", { hasText: title });
  await expect(card(`Erste ${s}`).locator(".chip-ref")).toHaveText(`${key}-1`);
  await expect(card(`Zweite ${s}`).locator(".chip-ref")).toHaveText(`${key}-2`);

  // Detail and y copy the reference.
  await card(`Zweite ${s}`).click();
  await detail(page).getByRole("button", { name: "Kennung kopieren" }).click();
  await expect.poll(clipboard).toBe(`${key}-2`);
  await page.keyboard.press("Escape");
  await card(`Erste ${s}`).click();
  await page.keyboard.press("Escape");
  await page.keyboard.press("y");
  await expect.poll(clipboard).toBe(`${key}-1`);

  // The palette finds a reference, and #/aufgabe/<ref> opens the task.
  await page.keyboard.press("Control+k");
  await page.keyboard.type(`${key}-2`);
  await expect(page.getByRole("dialog").getByRole("option").first()).toContainText(`Zweite ${s}`);
  await page.keyboard.press("Escape");
  await openApp(page, `#/aufgabe/${key.toLowerCase()}-1`);
  await expect(detail(page).getByLabel("Titel", { exact: true })).toHaveValue(`Erste ${s}`);
});

test("Alte Links mit UUID (vor CUID2) öffnen dieselbe Aufgabe", async ({ page }) => {
  const s = suffix();
  const legacy = "0192a000-0000-7000-8000-0c00000000e2";
  const seeder = new Seeder();
  seeder.raw("task", fromLegacy(legacy), { title: `Alter Link ${s}`, location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page, `#/aufgabe/${legacy}`);
  await expect(detail(page).getByLabel("Titel", { exact: true })).toHaveValue(`Alter Link ${s}`);
});

test("Dev-Board: Unteraufgaben aufklappen und ihre ID mit einem Klick kopieren", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const clipboard = () => page.evaluate(() => navigator.clipboard.readText());
  const s = suffix();
  const seeder = new Seeder();
  const area = seeder.area(`Arbeit ${s}`);
  const project = seeder.project(`App ${s}`, area, { kind: "dev" });
  const parent = seeder.task(`Epic ${s}`, { location: { kind: "project", id: project } });
  const child = seeder.task(`Teilschritt ${s}`, { parent_id: parent });
  await seeder.push(page);
  await openApp(page, "#/dev");

  const epic = page.locator(".card", { hasText: `Epic ${s}` });
  await epic.locator(".subtask-chip").click();
  const sub = epic.locator(".subtask", { hasText: `Teilschritt ${s}` });
  await expect(sub).toBeVisible();
  await sub.getByRole("button", { name: "ID kopieren" }).click();
  await expect.poll(clipboard).toBe(child);
  await expect(detail(page)).toBeHidden();

  // The card's own ID is one click away as well; a click on the subtask opens it.
  await epic.locator(".card-top").getByRole("button", { name: "ID kopieren" }).click();
  await expect.poll(clipboard).toBe(parent);
  await sub.locator(".subtask-title").click();
  await expect(detail(page).getByLabel("Titel", { exact: true })).toHaveValue(`Teilschritt ${s}`);
});

test("Kommentare: Agenten-Stand erscheint, eigener Kommentar wird gespeichert und gelöscht", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Kommentiert ${s}`, { location: { kind: "loose" }, description: "Ziel" });
  const agentComment = seeder.raw("comment", newId(), { task_id: id, body: "Stand: **Backup** läuft.", author: "claude-code" });
  await seeder.push(page);
  await openLoose(page, `Kommentiert ${s}`);

  const comments = detail(page).locator("section.comments");
  const first = comments.locator(`[data-comment-id="${agentComment}"]`);
  await expect(first.locator(".comment-author")).toHaveText("claude-code");
  await expect(first.locator("strong")).toHaveText("Backup");

  // An agent posts while the panel is open: it shows up live.
  seeder.raw("comment", newId(), { task_id: id, body: "Fertig.", author: "claude-code" });
  await seeder.push(page);
  await expect(comments.locator(".comment")).toHaveCount(2);

  const input = comments.getByRole("textbox", { name: "Neuer Kommentar" });
  await input.fill("Danke, sieht gut aus.");
  await input.press("Control+Enter");
  await expect(input).toHaveValue("");
  const own = comments.locator(".comment").last();
  await expect(own.locator(".comment-author")).toHaveText("Du");
  const ownId = (await own.getAttribute("data-comment-id"))!;
  await expect.poll(async () => (await entityFields(page, "comment", ownId)).body).toBe("Danke, sieht gut aus.");
  expect((await entityFields(page, "task", id)).description).toBe("Ziel");

  page.once("dialog", (d) => void d.accept());
  await own.hover();
  await own.getByRole("button", { name: "Kommentar löschen" }).click();
  await expect(comments.locator(".comment")).toHaveCount(2);
  await expect.poll(async () => (await entityFields(page, "comment", ownId)).deleted).toBe(true);
});

test("Bewusst ohne Fälligkeit: Markierung setzen, entfernen und durch ein Datum ersetzen", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  const id = seeder.task(`Irgendwann ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await openLoose(page, `Irgendwann ${s}`);

  const remove = detail(page).getByRole("button", { name: "Markierung „bewusst ohne Fälligkeit“ entfernen" });
  await detail(page).getByRole("button", { name: "Bewusst ohne", exact: true }).click();
  await expect(remove).toBeVisible();
  await expect(detail(page).locator(".undated-ack")).toContainText("Heute");
  await expect.poll(async () => (await entityFields(page, "task", id)).undated_ack).toMatch(/^\d{4}-\d{2}-\d{2}T/);

  await remove.click();
  await expect(remove).toBeHidden();
  await expect.poll(async () => (await entityFields(page, "task", id)).undated_ack).toBeNull();

  // A due date supersedes the decision.
  await detail(page).getByRole("button", { name: "Bewusst ohne", exact: true }).click();
  await expect(remove).toBeVisible();
  await detail(page).getByRole("button", { name: "Fälligkeit setzen", exact: true }).click();
  await detail(page).getByLabel("Fälligkeitsdatum").fill("2030-01-15");
  await expect(remove).toBeHidden();
  await expect(detail(page).getByRole("button", { name: "Bewusst ohne", exact: true })).toBeHidden();
  await expect.poll(async () => {
    const fields = await entityFields(page, "task", id);
    return [fields.due, fields.undated_ack];
  }).toEqual(["2030-01-15", null]);
});
