// Shared e2e helpers. Data is seeded through the contract API (POST /api/sync/push),
// so the same tests run against the mock and the real server.

import { randomUUID } from "node:crypto";
import { expect, type Page } from "@playwright/test";

export const backend = process.env.ZUNDER_E2E_BACKEND ?? "mock";
export const isMock = backend === "mock";

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

let wall = 0;
let counter = 0;
function hlc(): string {
  const now = Date.now();
  if (now > wall) {
    wall = now;
    counter = 0;
  } else counter += 1;
  return `${String(wall).padStart(13, "0")}:${String(counter).padStart(5, "0")}:e2e`;
}

/** UUIDv7-shaped id (time prefix + random), good enough for test data. */
export function newId(): string {
  const hex = Date.now().toString(16).padStart(12, "0");
  const rand = randomUUID().replace(/-/g, "");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-7${rand.slice(1, 4)}-8${rand.slice(5, 8)}-${rand.slice(8, 20)}`;
}

export function suffix(): string {
  return Math.random().toString(36).slice(2, 7);
}

export class Seeder {
  private ops: { op_id: string; entity: string; entity_id: string; field: string; value: Json; hlc: string }[] = [];

  private add(entity: string, id: string, fields: Record<string, Json>): string {
    for (const [field, value] of Object.entries(fields)) {
      this.ops.push({ op_id: newId(), entity, entity_id: id, field, value, hlc: hlc() });
    }
    return id;
  }

  area(name: string): string {
    return this.add("area", newId(), { name });
  }

  project(name: string, areaId: string, extra: Record<string, Json> = {}): string {
    return this.add("project", newId(), { name, area_id: areaId, ...extra });
  }

  task(title: string, extra: Record<string, Json> = {}): string {
    return this.add("task", newId(), { title, ...extra });
  }

  async push(page: Page): Promise<void> {
    const res = await page.request.post("/api/sync/push", { data: { client_id: "e2e", ops: this.ops } });
    expect(res.ok(), await res.text()).toBe(true);
    const body = (await res.json()) as { rejected: unknown[] };
    expect(body.rejected).toEqual([]);
    this.ops = [];
  }
}

/** Fresh state (mock) or a logged-in session (real dev auth). */
export async function prepare(page: Page): Promise<void> {
  if (isMock) {
    const res = await page.request.post("/__test/reset", { data: { seed: "empty", auth: true } });
    expect(res.ok()).toBe(true);
  } else {
    await page.goto("/auth/login");
  }
}

export async function openApp(page: Page, hash = "#/heute"): Promise<void> {
  await page.goto(`/${hash}`);
  await expect(page.locator("html[data-ready]")).toBeAttached();
  await expect(page.locator(".app")).toBeVisible();
}

export async function entityFields(page: Page, entity: string, id: string): Promise<Record<string, Json>> {
  const res = await page.request.get("/api/snapshot");
  const body = (await res.json()) as { entities: { entity: string; entity_id: string; fields: Record<string, { value: Json }> }[] };
  const e = body.entities.find((x) => x.entity === entity && x.entity_id === id);
  return e ? Object.fromEntries(Object.entries(e.fields).map(([k, v]) => [k, v.value])) : {};
}

export function row(page: Page, title: string) {
  return page.locator(".task-row", { has: page.locator(".task-title", { hasText: title }) });
}

export function card(page: Page, title: string) {
  return page.locator(".card", { has: page.locator(".task-title", { hasText: title }) });
}
