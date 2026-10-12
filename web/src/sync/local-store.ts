// Pluggable local persistence. Today: pending queue, clock and node id in
// localStorage, confirmed state only in memory. Later: IndexedDB (offline web)
// or the Tauri app's local SQLite implement the same interface.
//
// Every open page (tab) has its own push queue and HLC node: two tabs writing
// one shared queue would overwrite each other's unpushed edits. The queue of a
// page that is gone (closed, reloaded) is adopted by the next page that looks.

import { upgradeOp } from "./ids";
import type { Hlc, Op, SnapshotEntity } from "./types";

export interface PersistedState {
  cursor: number;
  entities: SnapshotEntity[];
}

export interface LocalStore {
  /** Stable HLC node id / push client id of this device. */
  nodeId(): string;
  loadPending(): Promise<Op[][]>;
  savePending(groups: readonly Op[][]): Promise<void>;
  /**
   * Push queues left behind by pages that are gone. They are removed from storage,
   * the caller takes them over (and persists them with its next savePending).
   */
  adoptOrphans?(): Promise<Op[][]>;
  loadClock(): Promise<Hlc | null>;
  saveClock(hlc: Hlc): Promise<void>;
  /** Whether confirmed state survives restarts (enables offline start). */
  readonly persistsState: boolean;
  loadState(): Promise<PersistedState | null>;
  saveState(state: PersistedState): Promise<void>;
}

function randomHex(bytes: number): string {
  const buf = new Uint8Array(bytes);
  crypto.getRandomValues(buf);
  return [...buf].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function newWebNodeId(): string {
  return `web-${randomHex(6)}`;
}

/** In-memory store for tests and as a fallback when storage is unavailable. */
export class MemoryLocalStore implements LocalStore {
  readonly persistsState = false;
  pending: Op[][] = [];
  clock: Hlc | null = null;

  constructor(private readonly node: string = newWebNodeId()) {}

  nodeId(): string {
    return this.node;
  }
  async loadPending(): Promise<Op[][]> {
    return this.pending.map((g) => [...g]);
  }
  async savePending(groups: readonly Op[][]): Promise<void> {
    this.pending = groups.map((g) => [...g]);
  }
  async loadClock(): Promise<Hlc | null> {
    return this.clock;
  }
  async saveClock(hlc: Hlc): Promise<void> {
    this.clock = hlc;
  }
  async loadState(): Promise<PersistedState | null> {
    return null;
  }
  async saveState(): Promise<void> {}
}

const KEYS = {
  node: "zunder.node.v1",
  /** Queue of the pre-per-page version; adopted like an orphan. */
  legacyPending: "zunder.pending.v1",
  pendingPrefix: "zunder.pending.v1.",
  alivePrefix: "zunder.alive.v1.",
  clock: "zunder.clock.v1",
};

/** The subset of the Web Locks API used to tell live pages from gone ones. */
export interface PageLocks {
  request(name: string, callback: () => Promise<void>): Promise<unknown>;
  query(): Promise<{ held?: { name?: string }[] }>;
}

const LOCK_PREFIX = "zunder-page-";
/** Without Web Locks: heartbeat interval and the age after which a page counts as gone. */
const HEARTBEAT_MS = 20_000;
const HEARTBEAT_STALE_MS = 5 * 60_000;

function parseGroups(raw: string | null): Op[][] {
  if (!raw) return [];
  try {
    const data: unknown = JSON.parse(raw);
    if (!Array.isArray(data)) return [];
    // Queues from before CUID2 hold UUIDs; they are converted like on the server.
    return data
      .filter((g): g is Op[] => Array.isArray(g) && g.length > 0 && g.every((op) => op && typeof op.op_id === "string"))
      .map((g) => g.map(upgradeOp));
  } catch {
    return [];
  }
}

/** Concatenates queues, skipping groups whose ops are already queued. */
function mergeGroups(...queues: Op[][][]): Op[][] {
  const seen = new Set<string>();
  const out: Op[][] = [];
  for (const queue of queues) {
    for (const group of queue) {
      if (group.some((op) => seen.has(op.op_id))) continue;
      for (const op of group) seen.add(op.op_id);
      out.push(group);
    }
  }
  return out;
}

function defaultStorage(): Storage | undefined {
  try {
    return globalThis.localStorage ?? undefined;
  } catch {
    return undefined;
  }
}

function defaultLocks(): PageLocks | undefined {
  const locks = (globalThis.navigator as { locks?: PageLocks } | undefined)?.locks;
  return locks && typeof locks.request === "function" && typeof locks.query === "function" ? locks : undefined;
}

/** Browser store: pending queue (per page) and clock in localStorage, state in memory. */
export class BrowserLocalStore implements LocalStore {
  readonly persistsState = false;
  private readonly node: string;
  private readonly pageId = randomHex(4);
  private readonly storage: Storage | undefined;
  private readonly locks: PageLocks | undefined;

  constructor(options: { storage?: Storage; locks?: PageLocks | null } = {}) {
    this.storage = options.storage ?? defaultStorage();
    this.locks = options.locks === null ? undefined : (options.locks ?? defaultLocks());
    // The browser's id stays recognizable; the suffix keeps HLCs of two pages apart.
    let browser = this.read(KEYS.node);
    if (!browser || !/^[a-z0-9-]{1,36}$/.test(browser)) {
      browser = newWebNodeId();
      this.write(KEYS.node, browser);
    }
    this.node = `${browser.slice(0, 27)}-${this.pageId}`;
    this.markAlive();
  }

  nodeId(): string {
    return this.node;
  }

  private get ownKey(): string {
    return KEYS.pendingPrefix + this.pageId;
  }

  async loadPending(): Promise<Op[][]> {
    const own = parseGroups(this.read(this.ownKey));
    const merged = mergeGroups(own, await this.adoptOrphans());
    this.writePending(merged);
    return merged;
  }

  async savePending(groups: readonly Op[][]): Promise<void> {
    this.writePending(groups);
  }

  async adoptOrphans(): Promise<Op[][]> {
    if (!this.storage) return [];
    const alive = await this.livePages();
    const orphanKeys: string[] = [];
    for (const key of this.keys()) {
      if (key === KEYS.legacyPending) orphanKeys.push(key);
      else if (key.startsWith(KEYS.pendingPrefix)) {
        const page = key.slice(KEYS.pendingPrefix.length);
        if (page !== this.pageId && !alive.has(page)) orphanKeys.push(key);
      }
    }
    if (orphanKeys.length === 0) return [];
    const adopted = mergeGroups(...orphanKeys.map((key) => parseGroups(this.read(key))));
    // Store them under this page first, then remove the orphans: a crash in between
    // leaves duplicates (pushing them twice is harmless), never a loss.
    this.writePending(mergeGroups(parseGroups(this.read(this.ownKey)), adopted));
    for (const key of orphanKeys) {
      this.remove(key);
      if (key.startsWith(KEYS.pendingPrefix)) this.remove(KEYS.alivePrefix + key.slice(KEYS.pendingPrefix.length));
    }
    return adopted;
  }

  async loadClock(): Promise<Hlc | null> {
    return this.read(KEYS.clock);
  }

  async saveClock(hlc: Hlc): Promise<void> {
    this.write(KEYS.clock, hlc);
  }

  async loadState(): Promise<PersistedState | null> {
    return null;
  }

  async saveState(): Promise<void> {}

  /** Holds a Web Lock for the page's lifetime (released by the browser when it is gone), or beats. */
  private markAlive(): void {
    if (this.locks) {
      void this.locks.request(LOCK_PREFIX + this.pageId, () => new Promise<void>(() => {})).catch(() => {});
      return;
    }
    if (!this.storage) return;
    const beat = () => this.write(KEYS.alivePrefix + this.pageId, String(Date.now()));
    beat();
    if (typeof window !== "undefined") setInterval(beat, HEARTBEAT_MS);
  }

  private async livePages(): Promise<Set<string>> {
    const alive = new Set<string>();
    if (this.locks) {
      try {
        const { held = [] } = await this.locks.query();
        for (const lock of held) if (lock.name?.startsWith(LOCK_PREFIX)) alive.add(lock.name.slice(LOCK_PREFIX.length));
        return alive;
      } catch {
        // fall through to heartbeats
      }
    }
    const now = Date.now();
    for (const key of this.keys()) {
      if (!key.startsWith(KEYS.alivePrefix)) continue;
      const beat = Number(this.read(key));
      if (Number.isFinite(beat) && now - beat < HEARTBEAT_STALE_MS) alive.add(key.slice(KEYS.alivePrefix.length));
      else this.remove(key);
    }
    return alive;
  }

  private writePending(groups: readonly Op[][]): void {
    if (groups.length === 0) this.remove(this.ownKey);
    else this.write(this.ownKey, JSON.stringify(groups));
  }

  private keys(): string[] {
    const out: string[] = [];
    try {
      const storage = this.storage;
      if (!storage) return out;
      for (let i = 0; i < storage.length; i++) {
        const key = storage.key(i);
        if (key !== null) out.push(key);
      }
    } catch {
      // Storage blocked: nothing to adopt.
    }
    return out;
  }

  private read(key: string): string | null {
    try {
      return this.storage?.getItem(key) ?? null;
    } catch {
      return null;
    }
  }

  private write(key: string, value: string): void {
    try {
      this.storage?.setItem(key, value);
    } catch {
      // Storage full or blocked (private mode): the queue then only lives in memory.
    }
  }

  private remove(key: string): void {
    try {
      this.storage?.removeItem(key);
    } catch {
      // ignore
    }
  }
}
