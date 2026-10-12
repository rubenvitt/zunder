// Bridge to the Tauri app (contract 5.1, apps/zunder-app/README.md). Loaded
// only inside the app via dynamic import, so the browser bundle never carries
// it or @tauri-apps/api.
//
// In the app the web UI never talks to the server itself (the CSP only allows
// IPC). The app keeps the local SQLite store (confirmed state + pending ops)
// and syncs on its own; the web engine sees the app's local view:
//   snapshot  → local_snapshot
//   push      → local_apply (validated locally, queued for the app's push)
//   pull      → nothing; after each app sync with changes the app emits
//               zunder://changed and the engine reloads the snapshot (onReset)
// History and flames go through app commands that call the server with the
// stored API token. Token management and server settings need a browser
// session and are not available in the app.

import { MemoryLocalStore } from "../sync/local-store";
import { asFlameResult, HttpError, NetworkError, type ApiClient, type SubscriptionHandlers, type SyncTransport } from "../sync/transport";
import type {
  ApiToken,
  CalendarSettings,
  CreatedCalendarFeed,
  CreatedToken,
  Device,
  FlameResult,
  HistoryEntry,
  Me,
  OAuthClient,
  Op,
  PullResponse,
  PushRequest,
  PushResponse,
  Rejection,
  Snapshot,
  TimingCheck,
  TimingProject,
  TimingSettings,
  TimingSettingsUpdate,
  Warning,
} from "../sync/types";

// ---- Command and event contract ------------------------------------------

export interface LastSync {
  at_ms: number;
  ok: boolean;
  error?: string;
}

/** Result of get_config / set_config. The token itself is never returned. */
export interface AppConfig {
  server_url: string | null;
  has_api_token: boolean;
  /** HLC node of this device (app-<12 hex>); the web engine uses it. */
  node_id: string;
  cursor: number;
  pending: number;
  last_sync: LastSync | null;
}

/** Argument of set_config: omitted = unchanged, null = clear. */
export interface ConfigUpdate {
  server_url?: string | null;
  api_token?: string | null;
  discard_pending?: boolean;
}

export interface LocalSnapshot extends Snapshot {
  pending: number;
}

export interface ApplyResult {
  accepted: string[];
  rejected: Rejection[];
  warnings: Warning[];
  pending: number;
}

export interface SyncReport {
  pushed: number;
  rejected: Rejection[];
  warnings: Warning[];
  pulled: number;
  cursor: number;
  pending: number;
  changed: boolean;
  /** The server's log no longer matched (restored backup); confirmed data was reloaded. */
  resynced?: boolean;
}

export interface AppErrorBody {
  code: string;
  message: string;
}

/** A failed round carries what it committed before the error in `report`. */
export type SyncEvent = { ok: true; report: SyncReport } | { ok: false; error: AppErrorBody; report?: SyncReport };

export interface ChangedEvent {
  cursor: number;
  pending: number;
}

/** A login in the system browser finished (the browser came back to the app). */
export type LoginEvent = { ok: true } | { ok: false; error: AppErrorBody };

/** Self-update of the desktop app (check_update, zunder://update). */
export interface UpdateStatus {
  /** Version installed and waiting for a restart. */
  installed: string | null;
  /** The last check since the app started. */
  last_check: LastSync | null;
}

/** Result of app_info: the app itself (not the server). */
export interface AppInfo {
  version: string;
  /** Desktop app: menu bar, start at login, self-update. */
  desktop: boolean;
  /** Starts at login; null on mobile or when unknown. */
  autostart: boolean | null;
  /** null on mobile. */
  update: UpdateStatus | null;
}

export const EVENTS = {
  changed: "zunder://changed",
  sync: "zunder://sync",
  login: "zunder://login",
  update: "zunder://update",
} as const;

// ---- Backend: invoke + listen (real Tauri API or a test double) ----------

export type UnlistenFn = () => void;

export interface TauriBackend {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<UnlistenFn>;
}

/** Loads @tauri-apps/api on demand (separate chunk). */
export async function loadTauriBackend(): Promise<TauriBackend> {
  const [core, event] = await Promise.all([import("@tauri-apps/api/core"), import("@tauri-apps/api/event")]);
  return {
    invoke: (cmd, args) => core.invoke(cmd, args),
    listen: (name, handler) => event.listen(name, handler),
  };
}

// ---- Errors ----------------------------------------------------------------

/** A command failed with an app error code (contract 5.1) other than network/unauthorized. */
export class AppCommandError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "AppCommandError";
  }
}

function errorBody(raw: unknown): AppErrorBody {
  if (raw && typeof raw === "object") {
    const { code, message } = raw as { code?: unknown; message?: unknown };
    if (typeof code === "string" && typeof message === "string") return { code, message };
  }
  // Tauri's own failures (unknown command, bad arguments) arrive as plain strings.
  if (typeof raw === "string") return { code: "app", message: raw };
  return { code: "app", message: raw instanceof Error ? raw.message : "Unbekannter Fehler der App." };
}

/**
 * Maps a rejected command to the error types the sync engine understands:
 * `network` → NetworkError (transient), `unauthorized` → HttpError 401.
 */
export function toAppError(raw: unknown): Error {
  const { code, message } = errorBody(raw);
  if (code === "network") return new NetworkError(message);
  if (code === "unauthorized") return new HttpError(401, code, message);
  return new AppCommandError(code, message);
}

/** The contract code of an error from this module. */
export function appErrorCode(err: unknown): string {
  if (err instanceof NetworkError) return "network";
  if (err instanceof HttpError || err instanceof AppCommandError) return err.code;
  return "app";
}

export const TOKENS_UNSUPPORTED =
  "API-Tokens, verbundene Apps und Geräte lassen sich nur in der Web-Oberfläche mit Browser-Anmeldung verwalten.";

export const SETTINGS_UNSUPPORTED = "Einstellungen des Servers lassen sich nur in der Web-Oberfläche mit Browser-Anmeldung ändern.";

// ---- The app's commands ----------------------------------------------------

type Handler<T> = (payload: T) => void;
type AppEventName = keyof typeof EVENTS;

export class TauriApp {
  private readonly handlers = {
    sync: new Set<Handler<SyncEvent>>(),
    changed: new Set<Handler<ChangedEvent>>(),
    login: new Set<Handler<LoginEvent>>(),
    update: new Set<Handler<UpdateStatus>>(),
  };
  private unlisten: UnlistenFn[] = [];

  private constructor(private readonly backend: TauriBackend) {}

  /** Connects to the app and starts listening to its events. */
  static async connect(backend?: TauriBackend): Promise<TauriApp> {
    const app = new TauriApp(backend ?? (await loadTauriBackend()));
    app.unlisten = await Promise.all([
      app.backend.listen<SyncEvent>(EVENTS.sync, (e) => app.dispatch("sync", e.payload)),
      app.backend.listen<ChangedEvent>(EVENTS.changed, (e) => app.dispatch("changed", e.payload)),
      app.backend.listen<LoginEvent>(EVENTS.login, (e) => app.dispatch("login", e.payload)),
      app.backend.listen<UpdateStatus>(EVENTS.update, (e) => app.dispatch("update", e.payload)),
    ]);
    return app;
  }

  on(event: "sync", handler: Handler<SyncEvent>): () => void;
  on(event: "changed", handler: Handler<ChangedEvent>): () => void;
  on(event: "login", handler: Handler<LoginEvent>): () => void;
  on(event: "update", handler: Handler<UpdateStatus>): () => void;
  on(event: AppEventName, handler: Handler<never>): () => void {
    const set = this.handlers[event] as Set<Handler<never>>;
    set.add(handler);
    return () => set.delete(handler);
  }

  close(): void {
    for (const off of this.unlisten) off();
    this.unlisten = [];
  }

  private dispatch(event: AppEventName, payload: unknown): void {
    for (const handler of this.handlers[event] as Set<Handler<unknown>>) {
      try {
        handler(payload);
      } catch (err) {
        console.error(`Fehler in einem Handler für ${event}`, err);
      }
    }
  }

  private async call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    try {
      return await this.backend.invoke<T>(cmd, args);
    } catch (err) {
      throw toAppError(err);
    }
  }

  getConfig(): Promise<AppConfig> {
    return this.call("get_config");
  }

  setConfig(update: ConfigUpdate): Promise<AppConfig> {
    return this.call("set_config", { update });
  }

  syncNow(): Promise<SyncReport> {
    return this.call("sync_now");
  }

  /** Opens the server's login in the system browser; the outcome arrives as `login` event. */
  loginStart(serverUrl: string, deviceName: string, discardPending: boolean): Promise<void> {
    return this.call("login_start", { server_url: serverUrl, device_name: deviceName, discard_pending: discardPending });
  }

  localSnapshot(): Promise<LocalSnapshot> {
    return this.call("local_snapshot");
  }

  localApply(ops: Op[]): Promise<ApplyResult> {
    return this.call("local_apply", { ops });
  }

  taskHistory(taskId: string): Promise<HistoryEntry[]> {
    return this.call("task_history", { task_id: taskId });
  }

  flame(taskId: string, action: "start" | "stop"): Promise<unknown> {
    return this.call(action === "start" ? "flame_start" : "flame_stop", { task_id: taskId });
  }

  appInfo(): Promise<AppInfo> {
    return this.call("app_info");
  }

  /** Starts the app at login (hidden in the menu bar); returns the new state. */
  setAutostart(enabled: boolean): Promise<boolean> {
    return this.call("set_autostart", { enabled });
  }

  /** Looks for a newer version now and installs it (active after restartApp). */
  checkUpdate(): Promise<UpdateStatus> {
    return this.call("check_update");
  }

  restartApp(): Promise<void> {
    return this.call("restart_app");
  }

  testNotification(): Promise<void> {
    return this.call("test_notification");
  }
}

// ---- Transport and API client for the engine and the UI -------------------

export interface TauriHooks {
  /** The app's pending count (ops not yet pushed to the server). */
  pending?(count: number): void;
}

export class TauriTransport implements SyncTransport, ApiClient {
  private cursor = 0;

  constructor(
    private readonly app: TauriApp,
    private readonly hooks: TauriHooks = {},
  ) {}

  async snapshot(): Promise<Snapshot> {
    const local = await this.app.localSnapshot();
    this.cursor = local.cursor;
    this.hooks.pending?.(local.pending);
    return { cursor: local.cursor, entities: local.entities };
  }

  async push(request: PushRequest): Promise<PushResponse> {
    const result = await this.app.localApply(request.ops);
    this.hooks.pending?.(result.pending);
    // Accepted = stored locally; the app pushes them to the server itself.
    return { accepted: result.accepted, rejected: result.rejected, warnings: result.warnings, cursor: this.cursor };
  }

  async pull(since: number): Promise<PullResponse> {
    // The local view has no op feed; changes arrive as zunder://changed → onReset.
    return { ops: [], cursor: since, has_more: false };
  }

  subscribe(handlers: SubscriptionHandlers): () => void {
    // The app is always "connected"; there is nothing to poll.
    handlers.onConnection(true);
    return this.app.on("changed", (event) => {
      this.hooks.pending?.(event.pending);
      handlers.onReset?.();
    });
  }

  async me(): Promise<Me> {
    return { user_id: "app", name: "Zunder-App", email: "" };
  }

  history(taskId: string): Promise<HistoryEntry[]> {
    return this.app.taskHistory(taskId);
  }

  async flame(taskId: string, action: "start" | "stop"): Promise<FlameResult | null> {
    // The app hands the server's answer through unchanged (contract 5.1, flame_start/flame_stop).
    return asFlameResult(await this.app.flame(taskId, action));
  }

  listTokens(): Promise<ApiToken[]> {
    return Promise.reject(new AppCommandError("unsupported", TOKENS_UNSUPPORTED));
  }

  createToken(_name: string): Promise<CreatedToken> {
    return Promise.reject(new AppCommandError("unsupported", TOKENS_UNSUPPORTED));
  }

  revokeToken(_id: string): Promise<void> {
    return Promise.reject(new AppCommandError("unsupported", TOKENS_UNSUPPORTED));
  }

  listOAuthClients(): Promise<OAuthClient[]> {
    return Promise.reject(new AppCommandError("unsupported", TOKENS_UNSUPPORTED));
  }

  revokeOAuthClient(_id: string): Promise<void> {
    return Promise.reject(new AppCommandError("unsupported", TOKENS_UNSUPPORTED));
  }

  timingSettings(): Promise<TimingSettings> {
    return Promise.reject(new AppCommandError("unsupported", SETTINGS_UNSUPPORTED));
  }

  updateTimingSettings(_update: TimingSettingsUpdate): Promise<TimingSettings> {
    return Promise.reject(new AppCommandError("unsupported", SETTINGS_UNSUPPORTED));
  }

  resetTimingSettings(): Promise<TimingSettings> {
    return Promise.reject(new AppCommandError("unsupported", SETTINGS_UNSUPPORTED));
  }

  checkTiming(): Promise<TimingCheck> {
    return Promise.reject(new AppCommandError("unsupported", SETTINGS_UNSUPPORTED));
  }

  timingProjects(): Promise<TimingProject[]> {
    return Promise.reject(new AppCommandError("unsupported", SETTINGS_UNSUPPORTED));
  }

  calendarSettings(): Promise<CalendarSettings> {
    return Promise.reject(new AppCommandError("unsupported", SETTINGS_UNSUPPORTED));
  }

  createCalendarFeed(): Promise<CreatedCalendarFeed> {
    return Promise.reject(new AppCommandError("unsupported", SETTINGS_UNSUPPORTED));
  }

  disableCalendarFeed(): Promise<CalendarSettings> {
    return Promise.reject(new AppCommandError("unsupported", SETTINGS_UNSUPPORTED));
  }

  listDevices(): Promise<Device[]> {
    return Promise.reject(new AppCommandError("unsupported", TOKENS_UNSUPPORTED));
  }

  revokeDevice(_id: string): Promise<void> {
    return Promise.reject(new AppCommandError("unsupported", TOKENS_UNSUPPORTED));
  }

  async logout(): Promise<void> {
    // No session in the app; the connection is managed in the settings view.
  }
}

/**
 * Engine store for the app: the app persists pending ops itself (local_apply
 * returns after the SQLite write), so the web side only holds the ops of a
 * local_apply in flight, in memory. The HLC node is the app's node_id; the
 * clock catches up with every snapshot, so it needs no persistence either.
 */
export function tauriLocalStore(config: AppConfig): MemoryLocalStore {
  return new MemoryLocalStore(config.node_id);
}
