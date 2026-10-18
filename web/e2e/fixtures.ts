// Shared e2e helpers. Data is seeded through the contract API (POST /api/sync/push),
// so the same tests run against the mock and the real server.

import { createId } from "@paralleldrive/cuid2";
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

/** A new ID in the wire form (CUID2). */
export function newId(): string {
  return createId();
}

/** CSRF header the real server requires for cookie/dev-auth requests that change state. */
export const WEB_HEADERS = { "X-Zunder-Client": "web" };

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

  /** Arbitrary field ops for an existing entity (e.g. a tombstone from "another client"). */
  raw(entity: string, id: string, fields: Record<string, Json>): string {
    return this.add(entity, id, fields);
  }

  /** Op IDs of the collected ops for `field` of entity `id`, in order. */
  opIds(id: string, field: string): string[] {
    return this.ops.filter((op) => op.entity_id === id && op.field === field).map((op) => op.op_id);
  }

  async push(page: Page): Promise<void> {
    const body = await this.send(page);
    expect(body.rejected).toEqual([]);
  }

  /** Pushes the collected ops like a client and returns the server's verdict (rejections allowed). */
  async send(page: Page): Promise<PushResult> {
    const res = await page.request.post("/api/sync/push", { data: { client_id: "e2e", ops: this.ops }, headers: WEB_HEADERS });
    expect(res.ok(), await res.text()).toBe(true);
    this.ops = [];
    return (await res.json()) as PushResult;
  }
}

export interface PushResult {
  accepted: string[];
  rejected: { op_id: string; code: string; message: string }[];
  warnings: { entity_id: string; code: string; message: string }[];
  cursor: number;
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

/** A real reload (openApp with the same hash only moves within the page). */
export async function reloadApp(page: Page): Promise<void> {
  await page.reload();
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
