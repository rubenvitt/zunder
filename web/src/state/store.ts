// Bridges the framework-agnostic sync engine into a Solid store. Engine change
// notifications update only the touched entities, so rows re-render fine-grained.

import { batch, createMemo, createRoot, createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { TaskIndex } from "../sync/derived";
import { SyncEngine, type SyncStatus } from "../sync/engine";
import { BrowserLocalStore, type LocalStore } from "../sync/local-store";
import type { EntityState } from "../sync/merge";
import { toArea, toProject, toTask } from "../sync/model";
import { HttpTransport, type ApiClient, type SyncTransport } from "../sync/transport";
import type { Area, Project, Rejection, Task, Warning } from "../sync/types";

interface DataState {
  areas: Record<string, Area>;
  projects: Record<string, Project>;
  tasks: Record<string, Task>;
}

export interface Services {
  transport: SyncTransport;
  api: ApiClient;
  localStore: LocalStore;
}

export interface Notifier {
  rejected(rejections: readonly Rejection[]): void;
  warnings(warnings: readonly Warning[]): void;
}

const httpTransport = new HttpTransport();
const defaultServices: Services = { transport: httpTransport, api: httpTransport, localStore: new BrowserLocalStore() };

const [data, setData] = createStore<DataState>({ areas: {}, projects: {}, tasks: {} });
const [syncStatus, setSyncStatus] = createSignal<SyncStatus>("loading");
const [pendingCount, setPendingCount] = createSignal(0);
const [ready, setReady] = createSignal(false);
const [now, setNow] = createSignal(Date.now());

let services = defaultServices;
let engine: SyncEngine | null = null;
let notifier: Notifier = { rejected: () => {}, warnings: () => {} };

export { data, now, pendingCount, ready, syncStatus };

export function api(): ApiClient {
  return services.api;
}

export function getEngine(): SyncEngine {
  if (!engine) throw new Error("Sync engine not started");
  return engine;
}

function writeEntity(e: EntityState): void {
  if (e.entity === "task") setData("tasks", e.id, reconcile(toTask(e)));
  else if (e.entity === "project") setData("projects", e.id, reconcile(toProject(e)));
  else setData("areas", e.id, reconcile(toArea(e)));
}

function writeAll(eng: SyncEngine): void {
  const next: DataState = { areas: {}, projects: {}, tasks: {} };
  for (const e of eng.view.values()) {
    if (e.entity === "task") next.tasks[e.id] = toTask(e);
    else if (e.entity === "project") next.projects[e.id] = toProject(e);
    else next.areas[e.id] = toArea(e);
  }
  setData(reconcile(next, { merge: true }));
}

export function setNotifier(n: Notifier): void {
  notifier = n;
}

/** For tests or the Tauri shell: swap transport, API and local store before start. */
export function configureServices(next: Partial<Services>): void {
  services = { ...services, ...next };
}

export async function startSync(): Promise<boolean> {
  engine?.stop();
  const eng = new SyncEngine(services.transport, services.localStore, {
    changed(keys) {
      batch(() => {
        if (keys === "all") {
          writeAll(eng);
          return;
        }
        for (const key of keys) {
          const e = eng.view.getByKey(key);
          if (e) writeEntity(e);
        }
      });
    },
    status: setSyncStatus,
    pending: setPendingCount,
    rejected: (r) => notifier.rejected(r),
    warnings: (w) => notifier.warnings(w),
  });
  engine = eng;
  const ok = await eng.start();
  setReady(ok);
  return ok;
}

// Relative views (Heute, 3-day hiding, claim expiry) follow the clock.
if (typeof window !== "undefined") {
  setInterval(() => setNow(Date.now()), 30_000);
  window.addEventListener("online", () => engine?.retryNow());
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") {
      setNow(Date.now());
      engine?.retryNow();
    }
  });
}

// ---- Derived, memoized views of the data -------------------------------

const derived = createRoot(() => {
  const index = createMemo(() => new TaskIndex(Object.values(data.tasks)));
  const areaList = createMemo(() =>
    Object.values(data.areas)
      .filter((a) => !a.deleted)
      .sort((a, b) => a.name.localeCompare(b.name, "de")),
  );
  const projectList = createMemo(() =>
    Object.values(data.projects)
      .filter((p) => !p.deleted && data.areas[p.area_id] && !data.areas[p.area_id]!.deleted)
      .sort((a, b) => a.name.localeCompare(b.name, "de")),
  );
  return { index, areaList, projectList };
});

export const taskIndex = derived.index;
export const areaList = derived.areaList;
export const projectList = derived.projectList;
