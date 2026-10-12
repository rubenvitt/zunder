import { createHash, randomBytes } from "node:crypto";
import { expect, test, type Page } from "@playwright/test";
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

test("Timing in den Einstellungen eintragen, ändern und zurücksetzen", async ({ page }) => {
  // The real server would call Timing for flames afterwards: always reset.
  try {
    await openApp(page, "#/einstellungen");
    await expect(page.getByRole("heading", { name: "Einstellungen" })).toBeVisible();
    const status = page.getByTestId("timing-status");
    await expect(status).toContainText("Nicht eingerichtet");
    const save = page.getByRole("button", { name: "Speichern" });
    await expect(save).toBeDisabled();

    await page.getByLabel("Timing-Token").fill("e2e-timing-token-1234");
    await page.getByLabel("Timing-Projekt").fill("/projects/7");
    await save.click();
    await expect(status).toContainText("…1234");
    await expect(status).toContainText("/projects/7");
    await expect(status).toContainText("aus den Einstellungen");
    // The token is never shown again; the field is empty after saving.
    await expect(page.getByLabel("Timing-Token")).toHaveValue("");
    await expect(page.getByLabel("Timing-Token")).toHaveAttribute("placeholder", /…1234/);
    await expect(page.getByRole("button", { name: "Verbindung prüfen" })).toBeVisible();

    // Only the project: the token stays.
    await page.getByLabel("Timing-Projekt").fill("");
    // With a token the field lists Timing's projects; Escape closes the list.
    await page.getByLabel("Timing-Projekt").press("Escape");
    await save.click();
    await expect(status).toContainText("ohne Projekt");
    await expect(status).toContainText("…1234");

    page.once("dialog", (d) => void d.accept());
    await page.getByRole("button", { name: "Zurücksetzen" }).click();
    await expect(status).toContainText("Nicht eingerichtet");
    await expect(page.getByRole("button", { name: "Zurücksetzen" })).toHaveCount(0);
  } finally {
    await page.request.delete("/api/settings/timing", { headers: WEB_HEADERS });
  }
});

test("Timing-Projekt aus der Liste wählen", async ({ page }) => {
  test.skip(!isMock, "Die Projektliste kommt aus Timing; nur der Mock hat ein Timing-Konto");
  try {
    await openApp(page, "#/einstellungen");
    const status = page.getByTestId("timing-status");
    await page.getByLabel("Timing-Token").fill("e2e-timing-token-1234");
    await page.getByRole("button", { name: "Speichern" }).click();
    await expect(status).toContainText("ohne Projekt");

    // Typing filters the projects; choosing one stores its self link.
    const field = page.getByRole("combobox", { name: "Timing-Projekt" });
    await field.fill("zund");
    const list = page.getByRole("listbox", { name: "Projekte aus Timing" });
    await expect(list.getByRole("option")).toHaveText(["Arbeit › Zunder"]);
    await list.getByRole("option", { name: "Arbeit › Zunder" }).click();
    await expect(field).toHaveValue("Arbeit › Zunder");
    await expect(page.getByTestId("timing-project-hint")).toContainText("/projects/2");
    await page.getByRole("button", { name: "Speichern" }).click();
    await expect(status).toContainText("Arbeit › Zunder");
    expect((await (await page.request.get("/api/settings/timing", { headers: WEB_HEADERS })).json()).project).toBe("/projects/2");

    // Keyboard: open the list, pick the first project with Enter.
    await field.fill("");
    await field.press("ArrowDown");
    await expect(list.getByRole("option")).toHaveCount(3);
    await field.press("Enter");
    await expect(field).toHaveValue("Arbeit");

    // Free text that matches no project is kept as a title.
    await field.fill("Neues Projekt");
    await expect(list).toContainText("als Projekttitel gespeichert");
    await field.press("Escape");
    await expect(list).toHaveCount(0);
    await page.getByRole("button", { name: "Speichern" }).click();
    await expect(status).toContainText("Neues Projekt");
  } finally {
    await page.request.delete("/api/settings/timing", { headers: WEB_HEADERS });
  }
});

test("Einstellungen über die Befehlspalette", async ({ page }) => {
  await openApp(page);
  await page.keyboard.press("Control+k");
  await page.keyboard.type("einstellungen");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/#\/einstellungen$/);
  await expect(page.getByRole("heading", { name: "Timing" })).toBeVisible();
});

/**
 * Connects an OAuth app. Real backend: the actual flow of a claude.ai connector
 * (registration, consent page in the browser, code with PKCE, token exchange).
 * Mock: a seeded client.
 */
async function connectOAuthApp(page: Page, name: string): Promise<void> {
  if (isMock) {
    const res = await page.request.post("/__test/oauth-client", { data: { name } });
    expect(res.ok()).toBe(true);
    return;
  }
  const callback = "https://claude.ai/api/mcp/auth_callback";
  const reg = await page.request.post("/oauth/register", { data: { client_name: name, redirect_uris: [callback] } });
  expect(reg.status(), await reg.text()).toBe(201);
  const clientId = ((await reg.json()) as { client_id: string }).client_id;
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("base64url");
  await page.goto("/healthz");
  const origin = new URL(page.url()).origin;
  const params = new URLSearchParams({
    response_type: "code",
    client_id: clientId,
    redirect_uri: callback,
    code_challenge: challenge,
    code_challenge_method: "S256",
    state: "e2e",
    resource: `${origin}/mcp`,
  });
  // The redirect to claude.ai carries the code; it never leaves the test.
  const toCallback = (url: URL) => url.href.startsWith(callback);
  await page.route(toCallback, (route) => route.fulfill({ status: 200, contentType: "text/plain", body: "ok" }));
  await page.goto(`/oauth/authorize?${params.toString()}`);
  await expect(page.getByRole("heading", { name: "Zugriff erlauben?" })).toBeVisible();
  await expect(page.locator("code", { hasText: "claude.ai" })).toBeVisible();
  const redirected = page.waitForRequest((req) => req.url().startsWith(callback));
  await page.getByRole("button", { name: "Erlauben" }).click();
  const code = new URL((await redirected).url()).searchParams.get("code") ?? "";
  expect(code).toMatch(/^zoc_/);
  await page.unroute(toCallback);
  const token = await page.request.post("/oauth/token", {
    form: { grant_type: "authorization_code", code, redirect_uri: callback, client_id: clientId, code_verifier: verifier },
  });
  expect(token.ok(), await token.text()).toBe(true);
  const { access_token } = (await token.json()) as { access_token: string };
  const mcp = await page.request.post("/mcp", {
    headers: { Authorization: `Bearer ${access_token}` },
    data: { jsonrpc: "2.0", id: 1, method: "ping" },
  });
  expect(mcp.ok()).toBe(true);
}

test("Verbundene App (OAuth) auf der Token-Seite trennen", async ({ page }) => {
  const name = `Claude ${suffix()}`;
  await connectOAuthApp(page, name);
  await openApp(page, "#/tokens");
  const clientRow = page.locator("tr", { hasText: name });
  await expect(clientRow).toBeVisible();
  await expect(clientRow).toContainText("claude.ai");
  page.once("dialog", (d) => void d.accept());
  await clientRow.getByRole("button", { name: "Trennen" }).click();
  await expect(clientRow).toHaveCount(0);
});

/**
 * Logs in a device like the Zunder app. Real backend: registration with the app scheme,
 * consent with the browser session (the redirect to the app is read, not followed),
 * code with PKCE for the sync API, then a sync call with the device token. Mock: a seeded
 * device.
 */
async function loginDevice(page: Page, name: string): Promise<void> {
  if (isMock) {
    const res = await page.request.post("/__test/device", { data: { name } });
    expect(res.ok()).toBe(true);
    return;
  }
  const callback = "dev.rubeen.zunder:/oauth";
  const reg = await page.request.post("/oauth/register", { data: { client_name: name, redirect_uris: [callback] } });
  expect(reg.status(), await reg.text()).toBe(201);
  const clientId = ((await reg.json()) as { client_id: string }).client_id;
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("base64url");
  await page.goto("/healthz");
  const origin = new URL(page.url()).origin;
  const params = new URLSearchParams({
    response_type: "code",
    client_id: clientId,
    redirect_uri: callback,
    code_challenge: challenge,
    code_challenge_method: "S256",
    state: "e2e",
    resource: `${origin}/api`,
  });
  const consent = await page.request.get(`/oauth/authorize?${params.toString()}`);
  const html = await consent.text();
  expect(html).toContain("als Gerät");
  const requestId = /name="request_id" value="([^"]+)"/.exec(html)?.[1] ?? "";
  const allowed = await page.request.post("/oauth/authorize", { form: { request_id: requestId, decision: "allow" }, maxRedirects: 0 });
  expect(allowed.status()).toBe(303);
  const code = new URL(allowed.headers()["location"] ?? "").searchParams.get("code") ?? "";
  const token = await page.request.post("/oauth/token", {
    form: { grant_type: "authorization_code", code, redirect_uri: callback, client_id: clientId, code_verifier: verifier, resource: `${origin}/api` },
  });
  expect(token.ok(), await token.text()).toBe(true);
  const { access_token } = (await token.json()) as { access_token: string };
  expect(access_token).toMatch(/^zd_/);
  const snapshot = await page.request.get("/api/snapshot", { headers: { Authorization: `Bearer ${access_token}` } });
  expect(snapshot.ok()).toBe(true);
}

test("Gerät (Zunder-App) auf der Token-Seite abmelden", async ({ page }) => {
  const name = `iPhone ${suffix()}`;
  await loginDevice(page, name);
  await openApp(page, "#/tokens");
  const deviceRow = page.locator("tr[data-device-id]", { hasText: name });
  await expect(deviceRow).toBeVisible();
  page.once("dialog", (d) => void d.accept());
  await deviceRow.getByRole("button", { name: "Abmelden" }).click();
  await expect(deviceRow).toHaveCount(0);
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
