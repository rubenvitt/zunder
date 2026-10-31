import { expect, test } from "@playwright/test";
import { entityFields, isMock, openApp, prepare, row, Seeder, suffix, WEB_HEADERS } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

test("Jev einrichten und einen Vorschlag für eine Inbox-Aufgabe übernehmen", async ({ page }) => {
  test.skip(!isMock, "Der echte Server würde Jev wirklich fragen; nur der Mock antwortet fest");
  const s = suffix();
  const seeder = new Seeder();
  const privateArea = seeder.area(`Privat ${s}`);
  seeder.project(`Tagebuch ${s}`, privateArea);
  const work = seeder.area(`Arbeit ${s}`);
  const site = seeder.project(`Website ${s}`, work);
  const task = seeder.task(`Impressum prüfen ${s}`);
  await seeder.push(page);

  try {
    await openApp(page, "#/einstellungen");
    const section = page.getByRole("region", { name: "Jev" });
    await expect(section.getByTestId("jev-status")).toContainText("Nicht eingerichtet");
    await section.getByLabel("Jev-Key").fill("e2e-jev-key-abcd");
    await section.getByLabel(`Privat ${s}`).check();
    await section.getByRole("button", { name: "Speichern" }).click();
    await expect(section.getByTestId("jev-status")).toContainText("…abcd");
    await expect(section.getByTestId("jev-status")).toContainText("jev-1.14.0");

    // An alias is refused with the server's reason.
    await section.getByLabel("Version").fill("jev-latest");
    await section.getByRole("button", { name: "Speichern" }).click();
    await expect(page.getByText("Bitte eine feste Version")).toBeVisible();
    await section.getByLabel("Version").fill("");

    await openApp(page, "#/inbox");
    await row(page, `Impressum prüfen ${s}`).locator(".task-main").click();
    const detail = page.getByRole("complementary", { name: "Aufgabendetail" });
    const panel = detail.getByTestId("jev-suggestion");
    await panel.getByRole("button", { name: "Vorschlag holen" }).click();
    await expect(panel).toContainText("Vorschlag von Jev");
    await expect(panel).toContainText("70 %");
    // The private area's project is never proposed.
    await expect(panel).toContainText(`Website ${s}`);
    await expect(panel).not.toContainText(`Tagebuch ${s}`);

    await panel.getByRole("button", { name: "Alles übernehmen" }).click();
    await expect.poll(async () => (await entityFields(page, "task", task)).weight).toBe("glut");
    await expect.poll(async () => (await entityFields(page, "task", task)).for_whom).toBe("mich");
    await expect.poll(async () => (await entityFields(page, "task", task)).location).toEqual({ kind: "project", id: site });
    // Out of the inbox: no proposal panel any more.
    await expect(detail.getByTestId("jev-suggestion")).toHaveCount(0);
  } finally {
    await page.request.delete("/api/settings/jev", { headers: WEB_HEADERS });
  }
});
