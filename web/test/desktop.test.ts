import { afterEach, describe, expect, it, vi } from "vitest";
import { EVENTS, type AppConfig, type TauriBackend } from "../src/platform/tauri";
import {
  appConfig,
  defaultDeviceName,
  desktopSync,
  initDesktop,
  isConfigured,
  loginState,
  resetDesktopForTests,
  saveConnection,
  startLogin,
  syncNow,
} from "../src/state/desktop";
import { api, pendingCount, setNotifier } from "../src/state/store";
import type { Rejection } from "../src/sync/types";

/** Minimal app double: config commands, sync_now and events. */
function fakeApp(initial: Partial<AppConfig> = {}) {
  const calls: { cmd: string; args?: Record<string, unknown> }[] = [];
  const listeners = new Map<string, Set<(e: { payload: unknown }) => void>>();
  const config: AppConfig = {
    server_url: null,
    has_api_token: false,
    node_id: "app-00000000abcd",
    cursor: 0,
    pending: 0,
    last_sync: null,
    ...initial,
  };
  const failures = new Map<string, unknown>();
  const emit = (event: string, payload: unknown) => {
    for (const h of listeners.get(event) ?? []) h({ payload });
  };
  const backend: TauriBackend = {
    async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
      calls.push({ cmd, args });
      if (failures.has(cmd)) {
        const failure = failures.get(cmd);
        failures.delete(cmd);
        throw failure;
      }
      switch (cmd) {
        case "get_config":
          return { ...config } as T;
        case "set_config": {
          const update = args!.update as { server_url?: string; api_token?: string };
          if (update.server_url !== undefined) config.server_url = update.server_url;
          if (update.api_token !== undefined) config.has_api_token = true;
          return { ...config } as T;
        }
        case "sync_now": {
          const report = { pushed: 0, rejected: [], warnings: [], pulled: 0, cursor: 0, pending: 0, changed: false };
          config.last_sync = { at_ms: 1_760_000_000_000, ok: true };
          emit(EVENTS.sync, { ok: true, report });
          return report as T;
        }
        case "task_history":
          return [] as T;
        case "login_start":
          return undefined as T;
        default:
          throw `command ${cmd} not found`;
      }
    },
    async listen<T>(event: string, handler: (e: { payload: T }) => void) {
      const set = listeners.get(event) ?? new Set();
      listeners.set(event, set);
      set.add(handler as (e: { payload: unknown }) => void);
      return () => set.delete(handler as (e: { payload: unknown }) => void);
    },
  };
  return { backend, calls, config, failures, emit };
}

afterEach(() => resetDesktopForTests());

describe("desktop state", () => {
  it("wires the Tauri services into the store", async () => {
    const fake = fakeApp({ server_url: "https://zunder.example.org", has_api_token: true, pending: 2 });
    const config = await initDesktop(fake.backend);
    expect(isConfigured(config)).toBe(true);
    expect(pendingCount()).toBe(2);
    await api().history("t1");
    expect(fake.calls.at(-1)).toEqual({ cmd: "task_history", args: { task_id: "t1" } });
  });

  it("saves URL and token, then syncs once to check them", async () => {
    const fake = fakeApp();
    await initDesktop(fake.backend);
    expect(isConfigured()).toBe(false);
    expect(desktopSync().state).toBe("unconfigured");

    const result = await saveConnection({ serverUrl: " https://zunder.example.org/ ", token: " zt_abc " });
    expect(result).toMatchObject({ ok: true });
    const set = fake.calls.find((c) => c.cmd === "set_config");
    expect(set?.args).toEqual({ update: { server_url: "https://zunder.example.org/", api_token: "zt_abc" } });
    expect(fake.calls.map((c) => c.cmd)).toContain("sync_now");
    expect(isConfigured()).toBe(true);
    expect(appConfig()?.last_sync?.ok).toBe(true);
    expect(desktopSync().state).toBe("ok");
  });

  it("keeps the stored token when the field stays empty", async () => {
    const fake = fakeApp({ server_url: "https://a.example.org", has_api_token: true });
    await initDesktop(fake.backend);
    await saveConnection({ serverUrl: "https://a.example.org", token: "  " });
    const set = fake.calls.find((c) => c.cmd === "set_config");
    expect(set?.args).toEqual({ update: { server_url: "https://a.example.org" } });
  });

  it("asks before discarding pending changes on a server switch", async () => {
    const fake = fakeApp({ server_url: "https://a.example.org", has_api_token: true, pending: 3 });
    await initDesktop(fake.backend);
    fake.failures.set("set_config", { code: "pending_ops", message: "3 lokale Änderungen sind noch nicht übertragen; ein Serverwechsel würde sie verwerfen" });
    const first = await saveConnection({ serverUrl: "https://b.example.org", token: "" });
    expect(first).toMatchObject({ ok: false, code: "pending_ops", saved: false });
    expect(fake.calls.map((c) => c.cmd)).not.toContain("sync_now");

    const second = await saveConnection({ serverUrl: "https://b.example.org", token: "", discardPending: true });
    expect(second.ok).toBe(true);
    const lastSet = fake.calls.filter((c) => c.cmd === "set_config").at(-1);
    expect(lastSet?.args).toEqual({ update: { server_url: "https://b.example.org", discard_pending: true } });
  });

  it("reports a failed check but keeps the settings", async () => {
    const fake = fakeApp();
    await initDesktop(fake.backend);
    fake.failures.set("sync_now", { code: "unauthorized", message: "Der Server lehnt den API-Token ab" });
    const result = await saveConnection({ serverUrl: "https://zunder.example.org", token: "zt_wrong" });
    expect(result).toMatchObject({ ok: false, code: "unauthorized", saved: true });
    expect(isConfigured()).toBe(true);
  });

  it("follows the app's sync events and shows its rejections", async () => {
    const rejected: Rejection[][] = [];
    setNotifier({ rejected: (r) => void rejected.push([...r]), warnings: () => {} });
    const fake = fakeApp({ server_url: "https://zunder.example.org", has_api_token: true });
    await initDesktop(fake.backend);

    fake.emit(EVENTS.sync, { ok: false, error: { code: "network", message: "Server nicht erreichbar: dns" } });
    expect(desktopSync()).toEqual({ state: "offline", message: "Server nicht erreichbar: dns" });
    fake.emit(EVENTS.sync, { ok: false, error: { code: "unauthorized", message: "Der Server lehnt den API-Token ab" } });
    expect(desktopSync().state).toBe("unauthorized");

    const rejection = { op_id: "o1", code: "invalid_reference", message: "Bereich gibt es nicht mehr." };
    fake.emit(EVENTS.sync, {
      ok: true,
      report: { pushed: 0, rejected: [rejection], warnings: [], pulled: 1, cursor: 4, pending: 1, changed: true },
    });
    expect(desktopSync().state).toBe("ok");
    expect(pendingCount()).toBe(1);
    expect(rejected).toEqual([[rejection]]);
  });

  it("shows rejections of a sync round that failed later on", async () => {
    const rejected: Rejection[][] = [];
    setNotifier({ rejected: (r) => void rejected.push([...r]), warnings: () => {} });
    const fake = fakeApp({ server_url: "https://zunder.example.org", has_api_token: true, pending: 3 });
    await initDesktop(fake.backend);

    const rejection = { op_id: "o1", code: "invalid_reference", message: "Bereich gibt es nicht mehr." };
    fake.emit(EVENTS.sync, {
      ok: false,
      error: { code: "network", message: "Server nicht erreichbar: timeout" },
      report: { pushed: 0, rejected: [rejection], warnings: [], pulled: 0, cursor: 4, pending: 0, changed: true, resynced: false },
    });
    expect(desktopSync().state).toBe("offline");
    expect(pendingCount()).toBe(0);
    expect(rejected).toEqual([[rejection]]);
  });

  it("syncNow reports errors with their code", async () => {
    const fake = fakeApp({ server_url: "https://zunder.example.org", has_api_token: true });
    await initDesktop(fake.backend);
    fake.failures.set("sync_now", { code: "network", message: "Server nicht erreichbar: timeout" });
    expect(await syncNow()).toMatchObject({ ok: false, code: "network", message: "Server nicht erreichbar: timeout" });
  });

  it("logs in through the browser: login_start, then the login event stores and syncs", async () => {
    const fake = fakeApp();
    await initDesktop(fake.backend);
    expect(await startLogin({ serverUrl: " https://zunder.example.org ", deviceName: " iPhone " })).toEqual({ ok: true });
    expect(fake.calls.at(-1)).toEqual({
      cmd: "login_start",
      args: { server_url: "https://zunder.example.org", device_name: "iPhone", discard_pending: false },
    });
    expect(loginState().state).toBe("waiting");

    // The app stored the device token before it reports the login.
    fake.config.server_url = "https://zunder.example.org";
    fake.config.has_api_token = true;
    fake.emit(EVENTS.login, { ok: true });
    await vi.waitFor(() => expect(fake.calls.some((c) => c.cmd === "sync_now")).toBe(true));
    expect(loginState().state).toBe("done");
    expect(isConfigured()).toBe(true);
  });

  it("reports a failed login and a refused start", async () => {
    const fake = fakeApp();
    await initDesktop(fake.backend);
    await startLogin({ serverUrl: "https://zunder.example.org", deviceName: "Mac" });
    fake.emit(EVENTS.login, { ok: false, error: { code: "login", message: "Anmeldung: Die Anmeldung wurde abgelehnt" } });
    expect(loginState()).toEqual({ state: "error", code: "login", message: "Anmeldung: Die Anmeldung wurde abgelehnt" });

    fake.failures.set("login_start", { code: "pending_ops", message: "2 lokale Änderungen sind noch nicht übertragen" });
    const refused = await startLogin({ serverUrl: "https://other.example.org", deviceName: "Mac" });
    expect(refused).toMatchObject({ ok: false, code: "pending_ops" });
    expect(loginState().state).toBe("idle");
    await startLogin({ serverUrl: "https://other.example.org", deviceName: "Mac", discardPending: true });
    expect(fake.calls.at(-1)?.args).toMatchObject({ discard_pending: true });
  });

  it("suggests a device name from the user agent", () => {
    expect(defaultDeviceName("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)")).toBe("iPhone");
    expect(defaultDeviceName("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe("Mac");
    expect(defaultDeviceName("Mozilla/5.0 (Linux; Android 15)")).toBe("Android");
    expect(defaultDeviceName("curl")).toBe("Zunder-App");
  });
});
