// Pluggable local persistence. Today: pending queue, clock and node id in
// localStorage, confirmed state only in memory. Later: IndexedDB (offline web)
// or the Tauri app's local SQLite implement the same interface.

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
  pending: "zunder.pending.v1",
  clock: "zunder.clock.v1",
};

function readStorage(key: string): string | null {
  try {
    return globalThis.localStorage?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function writeStorage(key: string, value: string): void {
  try {
    globalThis.localStorage?.setItem(key, value);
  } catch {
    // Storage full or blocked (private mode): the queue then only lives in memory.
  }
}

/** Browser store: pending queue and clock in localStorage, state in memory. */
export class BrowserLocalStore implements LocalStore {
  readonly persistsState = false;
  private readonly node: string;

  constructor() {
    const existing = readStorage(KEYS.node);
    if (existing && /^[a-z0-9-]{1,36}$/.test(existing)) {
      this.node = existing;
    } else {
      this.node = newWebNodeId();
      writeStorage(KEYS.node, this.node);
    }
  }

  nodeId(): string {
    return this.node;
  }

  async loadPending(): Promise<Op[][]> {
    const raw = readStorage(KEYS.pending);
    if (!raw) return [];
    try {
      const data: unknown = JSON.parse(raw);
      if (!Array.isArray(data)) return [];
      return data.filter((g): g is Op[] => Array.isArray(g) && g.every((op) => op && typeof op.op_id === "string"));
    } catch {
      return [];
    }
  }

  async savePending(groups: readonly Op[][]): Promise<void> {
    writeStorage(KEYS.pending, JSON.stringify(groups));
  }

  async loadClock(): Promise<Hlc | null> {
    return readStorage(KEYS.clock);
  }

  async saveClock(hlc: Hlc): Promise<void> {
    writeStorage(KEYS.clock, hlc);
  }

  async loadState(): Promise<PersistedState | null> {
    return null;
  }

  async saveState(): Promise<void> {}
}
