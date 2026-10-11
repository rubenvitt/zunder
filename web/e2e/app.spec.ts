import { expect, test } from "@playwright/test";
import { isMock, newId, openApp, prepare, row, Seeder, suffix, WEB_HEADERS } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

test("Login-Seite bei fehlender Sitzung", async ({ page }) => {
  test.skip(!isMock, "Sitzung wird nur im Mock gezielt entfernt");
  await page.request.post("/__test/reset", { data: { seed: "empty", auth: false } });
  await page.goto("/");
  const login = page.getByRole("link", { name: "Anmelden" });
  await expect(login).toHaveAttribute("href", "/auth/login");
  await login.click();
  await expect(page.locator(".app")).toBeVisible();
});

test("Abmelden über die Befehlspalette", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Control+k");
  await page.keyboard.type("abmelden");
  const logout = page.waitForResponse((r) => r.url().endsWith("/auth/logout") && r.request().method() === "POST");
  await page.keyboard.press("Enter");
  expect((await logout).status()).toBe(204);
  if (isMock) {
    // Without a session the login page appears.
    await expect(page.getByRole("link", { name: "Anmelden" })).toHaveAttribute("href", "/auth/login");
  } else {
    // Dev auth (contract 3.1) treats requests without credentials as the owner again.
    await expect(page.locator(".app")).toBeVisible();
  }
});

test("Zustandsändernde Anfragen brauchen X-Zunder-Client: web", async ({ page }) => {
  await openApp(page);
  const op = { op_id: newId(), entity: "area", entity_id: newId(), field: "name", value: "CSRF", hlc: `${String(Date.now()).padStart(13, "0")}:00000:e2e` };
  const denied = await page.request.post("/api/sync/push", { data: { client_id: "e2e", ops: [op] } });
  expect(denied.status()).toBe(403);
  expect(((await denied.json()) as { error: { code: string } }).error.code).toBe("forbidden");
  expect((await page.request.post("/auth/logout")).status()).toBe(403);
  const ok = await page.request.post("/api/sync/push", { data: { client_id: "e2e", ops: [op] }, headers: WEB_HEADERS });
  expect(ok.status()).toBe(200);
  const missing = await page.request.get("/api/gibt-es-nicht");
  expect(missing.status()).toBe(404);
  expect(((await missing.json()) as { error: { code: string } }).error.code).toBe("not_found");
});

test("Befehlspalette findet Aufgaben, auch erledigte, und Aktionen", async ({ page }) => {
  const s = suffix();
  const seeder = new Seeder();
  seeder.task(`Rechnung bezahlen ${s}`, { status: "done", location: { kind: "loose" } });
  await seeder.push(page);
  await openApp(page);

  await page.keyboard.press("Control+k");
  await page.keyboard.type(`rechnung ${s}`);
  const item = page.locator(".palette-item", { hasText: `Rechnung bezahlen ${s}` });
  await expect(item).toBeVisible();
  await expect(item).toContainText("Erledigt");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("complementary", { name: "Aufgabendetail" })).toBeVisible();

  await page.keyboard.press("Escape");
  await page.keyboard.press("Control+k");
  await page.keyboard.type("gehe zu dev");
  await page.keyboard.press("Enter");
  await expect(page.locator(".view-head h1")).toHaveText("Dev");
});

test("Bereich und Projekt anlegen, bearbeiten und archivieren", async ({ page }) => {
  const s = suffix();
  await openApp(page, "#/bereiche");
  await page.getByRole("button", { name: "Neuer Bereich" }).click();
  await page.getByRole("dialog").getByLabel("Name", { exact: true }).fill(`Garten ${s}`);
  await page.getByRole("dialog").getByRole("button", { name: "Speichern" }).click();
  await expect(page.locator(".split-main h1")).toHaveText(`Garten ${s}`);

  await page.getByRole("button", { name: "Projekt" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Name", { exact: true }).fill(`Hochbeet ${s}`);
  await dialog.locator('select[name="kind"]').selectOption("dev");
  await dialog.getByLabel("GitHub-Repository").fill("example/hochbeet");
  await dialog.getByRole("button", { name: "Speichern" }).click();
  await expect(page.locator(".split-main h1")).toHaveText(`Hochbeet ${s}`);
  await expect(page.getByRole("region", { name: `Board Hochbeet ${s}` })).toBeVisible();

  await page.getByLabel("Aufgabe im Projekt anlegen …").fill(`Erde kaufen ${s}`);
  await page.keyboard.press("Enter");
  await expect(page.locator(".card", { hasText: `Erde kaufen ${s}` })).toBeVisible();

  await page.getByRole("button", { name: "Bearbeiten" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Abschließen" }).click();
  await expect(page.locator(".split-main .chip", { hasText: "Abgeschlossen" })).toBeVisible();
  await expect(page.locator(".tree-item", { hasText: `Hochbeet ${s}` })).toHaveCount(0);
});

test("API-Tokens anlegen (einmal sichtbar) und widerrufen", async ({ page }) => {
  const name = `agent-${suffix()}`;
  await openApp(page, "#/tokens");
  await page.getByLabel("Token-Name").fill(name);
  await page.getByRole("button", { name: "Token erstellen" }).click();
  await expect(page.getByTestId("new-token")).toHaveText(/^zt_/);
  const tokenRow = page.locator("tr", { hasText: name });
  await expect(tokenRow).toBeVisible();
  page.once("dialog", (d) => void d.accept());
  await tokenRow.getByRole("button", { name: "Widerrufen" }).click();
  await expect(tokenRow).toHaveCount(0);
});

test("Tastenkürzel-Übersicht mit ?", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("?");
  await expect(page.getByRole("dialog", { name: "Tastenkürzel" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("Änderungen anderer Clients erscheinen live", async ({ page }) => {
  const s = suffix();
  await openApp(page, "#/lose");
  const seeder = new Seeder();
  seeder.task(`Von außen ${s}`, { location: { kind: "loose" } });
  await seeder.push(page);
  await expect(row(page, `Von außen ${s}`)).toBeVisible();
});

test("Mobil: Tabs unten und großes Plus @mobile", async ({ page }) => {
  await openApp(page);
  const tabbar = page.locator(".tabbar");
  await expect(tabbar).toBeVisible();
  await expect(page.locator(".sidebar")).toBeHidden();
  await tabbar.getByRole("button", { name: /Inbox/ }).click();
  await expect(page.locator(".view-head h1")).toHaveText("Inbox");
  await page.getByRole("button", { name: "Neue Aufgabe" }).click();
  await page.getByLabel("Titel der neuen Aufgabe").fill(`Mobil erfasst ${suffix()}`);
  await page.getByRole("button", { name: "Erfassen" }).click();
  await expect(page.locator(".task-row", { hasText: "Mobil erfasst" })).toBeVisible();
});
