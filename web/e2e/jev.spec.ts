import { expect, test } from "@playwright/test";
import { entityFields, isMock, openApp, prepare, row, Seeder, suffix, WEB_HEADERS } from "./fixtures";

test.beforeEach(async ({ page }) => {
  await prepare(page);
});

test("Jev einrichten; Vorschläge kommen von selbst und gelten mit einem Tipp", async ({ page }) => {
  test.skip(!isMock, "Der echte Server würde Jev wirklich fragen; nur der Mock antwortet fest");
  const s = suffix();
  const seeder = new Seeder();
  const privateArea = seeder.area(`Privat ${s}`);
  seeder.project(`Tagebuch ${s}`, privateArea);
  const work = seeder.area(`Arbeit ${s}`);
  const site = seeder.project(`Website ${s}`, work);
  const task = seeder.task(`Impressum prüfen ${s}`);
  seeder.task(`Rechnung bezahlen ${s}`, { weight: "rauch", for_whom: "andere", location: { kind: "project", id: site } });
  const sure = seeder.task(`Domain eindeutig verlängern ${s}`, { location: { kind: "project", id: site } });
  const hidden = seeder.task(`Brief schreiben ${s}`, { location: { kind: "area", id: privateArea } });
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

    // Eval run against sorted tasks: the mock answers glut / für mich, so the sorted task is a miss.
    page.once("dialog", (d) => void d.accept());
    await section.getByLabel("Aufgaben").fill("100");
    await section.getByRole("button", { name: "Probelauf starten" }).click();
    const report = section.getByTestId("jev-eval");
    await expect(report.getByTestId("jev-eval-table")).toContainText("Gewicht");
    await report.getByText("Aufgaben mit Abweichungen").click();
    await expect(report).toContainText(`Rechnung bezahlen ${s}`);
    await expect(report).toContainText("Gewicht rauch → glut (70 %)");

    await openApp(page, "#/inbox");
    await row(page, `Impressum prüfen ${s}`).locator(".task-main").click();
    const detail = page.getByRole("complementary", { name: "Aufgabendetail" });
    // No click: the proposals come by themselves, as chips without numbers.
    const panel = detail.getByTestId("jev-suggestion");
    await expect(panel.getByRole("button", { name: "Glut" })).toBeVisible();
    await expect(panel.getByRole("button", { name: "für mich" })).toBeVisible();
    // The private area's project is never proposed; the unsure place says "?".
    await expect(panel.getByRole("button", { name: `Website ${s}` })).toContainText("?");
    await expect(panel).not.toContainText(`Tagebuch ${s}`);
    await expect(panel).not.toContainText("%");
    // 0.7 and 0.8 are not sure enough to be set without a tap.
    expect((await entityFields(page, "task", task)).weight ?? null).toBeNull();

    await panel.getByRole("button", { name: "Alle übernehmen" }).click();
    await expect.poll(async () => (await entityFields(page, "task", task)).weight).toBe("glut");
    await expect.poll(async () => (await entityFields(page, "task", task)).for_whom).toBe("mich");
    await expect.poll(async () => (await entityFields(page, "task", task)).location).toEqual({ kind: "project", id: site });
    // Everything taken: nothing left to propose.
    await expect(detail.getByTestId("jev-suggestion")).toHaveCount(0);

    // Outside the inbox too; what Jev is sure about is set right away and can be undone.
    await openApp(page, `#/aufgabe/${sure}`);
    const surePanel = detail.getByTestId("jev-suggestion");
    await expect(surePanel.getByTestId("jev-auto")).toContainText("Glut");
    await expect.poll(async () => (await entityFields(page, "task", sure)).weight).toBe("glut");
    await expect(surePanel).not.toContainText(`Website ${s}`);
    await surePanel.getByRole("button", { name: "Rückgängig" }).click();
    await expect.poll(async () => (await entityFields(page, "task", sure)).weight ?? null).toBeNull();
    await surePanel.getByRole("button", { name: "für mich" }).click();
    await expect.poll(async () => (await entityFields(page, "task", sure)).for_whom).toBe("mich");

    // Private areas never go to Jev: no panel.
    await openApp(page, `#/aufgabe/${hidden}`);
    await expect(detail.getByRole("group", { name: "Gewicht" })).toBeVisible();
    await page.waitForTimeout(500);
    await expect(detail.getByTestId("jev-suggestion")).toHaveCount(0);
  } finally {
    await page.request.delete("/api/settings/jev", { headers: WEB_HEADERS });
  }
});
