// State of the Tauri app (desktop and mobile): connection settings, the app's
// own background sync and the wiring of the Tauri services into the store.
// Only type imports from platform/tauri here; the module itself is loaded on
// demand in initDesktop(), so the browser bundle stays free of it.

import { createSignal } from "solid-js";
import type { AppConfig, ConfigUpdate, SyncEvent, TauriApp, TauriBackend } from "../platform/tauri";
import { configureServices, reloadData, reportRejected, setAppPending } from "./store";

const [config, setConfig] = createSignal<AppConfig | null>(null);
const [lastEvent, setLastEvent] = createSignal<SyncEvent | null>(null);
const [syncing, setSyncing] = createSignal(false);

let app: TauriApp | null = null;
let errorCode: (err: unknown) => string = () => "app";

export { config as appConfig, syncing as appSyncing };

export function isConfigured(c: AppConfig | null = config()): boolean {
  return !!c?.server_url && !!c.has_api_token;
}

function connected(): TauriApp {
  if (!app) throw new Error("Tauri app not connected");
  return app;
}

async function refreshConfig(): Promise<void> {
  try {
    const next = await connected().getConfig();
    setConfig(next);
    setAppPending(next.pending);
  } catch (err) {
    console.error("Einstellungen der App nicht lesbar", err);
  }
}

/**
 * Connects to the app, reads its configuration and hands TauriTransport and
 * the app's node id to the store. Call before startSync().
 */
export async function initDesktop(backend?: TauriBackend): Promise<AppConfig> {
  const mod = await import("../platform/tauri");
  errorCode = mod.appErrorCode;
  app?.close();
  app = await mod.TauriApp.connect(backend);
  app.on("sync", (event) => {
    setLastEvent(event);
    // A failed round also reports what it did before the error.
    if (event.report) {
      setAppPending(event.report.pending);
      // Rolled back by the app; the view follows via zunder://changed.
      reportRejected(event.report.rejected);
    }
    void refreshConfig();
  });
  const initial = await app.getConfig();
  setConfig(initial);
  setAppPending(initial.pending);
  const transport = new mod.TauriTransport(app, { pending: setAppPending });
  configureServices({ transport, api: transport, localStore: mod.tauriLocalStore(initial) });
  return initial;
}

export type DesktopSyncState = "ok" | "offline" | "unauthorized" | "unconfigured" | "error";

/** Health of the app's sync with the server, for the status indicator. */
export function desktopSync(): { state: DesktopSyncState; message?: string } {
  if (!isConfigured()) return { state: "unconfigured", message: "Kein Server eingerichtet." };
  const event = lastEvent();
  if (event) {
    if (event.ok) return { state: "ok" };
    const { code, message } = event.error;
    if (code === "network") return { state: "offline", message };
    if (code === "unauthorized") return { state: "unauthorized", message };
    if (code === "not_configured") return { state: "unconfigured", message };
    return { state: "error", message };
  }
  const last = config()?.last_sync;
  if (last && !last.ok) return { state: "error", message: last.error };
  return { state: "ok" };
}

export interface ActionResult {
  ok: boolean;
  /** Contract code on failure (pending_ops, invalid_config, network, unauthorized, …). */
  code?: string;
  message?: string;
  /** Whether the settings were stored (a failed first sync keeps them). */
  saved?: boolean;
}

function failure(err: unknown, saved: boolean): ActionResult {
  return { ok: false, code: errorCode(err), message: err instanceof Error ? err.message : String(err), saved };
}

/** Runs a sync right away and reports the outcome (also announced via zunder://sync). */
export async function syncNow(): Promise<ActionResult> {
  setSyncing(true);
  try {
    await connected().syncNow();
    return { ok: true, saved: true };
  } catch (err) {
    return failure(err, true);
  } finally {
    setSyncing(false);
    await refreshConfig();
  }
}

/**
 * Stores server URL and (if given) a new token, then syncs once to check
 * them. An empty token keeps the stored one. Switching servers with pending
 * changes fails with `pending_ops` unless `discardPending` is set.
 */
export async function saveConnection(input: { serverUrl: string; token: string; discardPending?: boolean }): Promise<ActionResult> {
  const update: ConfigUpdate = { server_url: input.serverUrl.trim() };
  const token = input.token.trim();
  if (token) update.api_token = token;
  if (input.discardPending) update.discard_pending = true;
  try {
    setConfig(await connected().setConfig(update));
  } catch (err) {
    return failure(err, false);
  }
  // A server switch cleared the local data: show that right away.
  // New data from the sync arrives via zunder://changed.
  await reloadData();
  return syncNow();
}

/** Test hook: forget the connection. */
export function resetDesktopForTests(): void {
  app?.close();
  app = null;
  setConfig(null);
  setLastEvent(null);
  setSyncing(false);
}
