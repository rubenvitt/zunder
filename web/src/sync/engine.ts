// Sync engine: snapshot at start, optimistic local changes with a persisted push
// queue, pull on live notifications (SSE) or polling, rollback of rejected ops by
// re-deriving the view from confirmed server state plus still-pending ops.

import { HlcClock } from "./hlc";
import type { LocalStore } from "./local-store";
import { entityKey, EntityStore } from "./merge";
import { isTransient, isUnauthorized, type SyncTransport } from "./transport";
import type { EntityKind, JsonValue, Op, Rejection, Warning } from "./types";
import { uuidv7 } from "./uuid";
import { validateOps, type ValidationIssue } from "./validate";

export type SyncStatus = "loading" | "online" | "offline" | "unauthorized";

export interface EngineListener {
  /** Entity keys ("task/<id>") whose view state changed, or "all" after a rebuild. */
  changed(keys: ReadonlySet<string> | "all"): void;
  status?(status: SyncStatus): void;
  pending?(count: number): void;
  rejected?(rejections: readonly Rejection[]): void;
  warnings?(warnings: readonly Warning[]): void;
}

export interface EngineOptions {
  pollIntervalMs?: number;
  maxBatchOps?: number;
  /** Rough JSON size limit of one push, well below the server's body limit (contract 3.1). */
  maxBatchBytes?: number;
  backoffMs?: readonly number[];
  physicalNow?: () => number;
}

export interface Change {
  entity: EntityKind;
  id: string;
  fields: Record<string, JsonValue>;
}

const DEFAULT_BACKOFF = [1_000, 2_000, 5_000, 10_000, 30_000];

/** Lone UTF-16 surrogates would be serialized as "\ud800", which the server refuses (400). */
const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g;

function wellFormed(value: JsonValue): JsonValue {
  if (typeof value === "string") return value.replace(LONE_SURROGATE, "\uFFFD");
  if (Array.isArray(value)) return value.map(wellFormed);
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, wellFormed(v)]));
  return value;
}

export class SyncEngine {
  readonly confirmed = new EntityStore();
  private viewStore = new EntityStore();
  private pending: Op[][] = [];
  readonly clock: HlcClock;
  readonly clientId: string;

  private cursorValue = 0;
  private statusValue: SyncStatus = "loading";
  private started = false;
  private stopped = false;
  private pushing = false;
  private pulling: Promise<void> | null = null;
  private pullAgain = false;
  private reloading: Promise<void> | null = null;
  private reloadAgain = false;
  /** Counts applied push results, so a reload can detect a stale snapshot. */
  private pushEpoch = 0;
  private live = false;
  private failures = 0;
  /** After a whole-request refusal: push one group at a time until the queue is empty. */
  private isolating = false;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private unsubscribe: (() => void) | null = null;

  private readonly pollIntervalMs: number;
  private readonly maxBatchOps: number;
  private readonly maxBatchBytes: number;
  private readonly backoffMs: readonly number[];

  constructor(
    private readonly transport: SyncTransport,
    private readonly store: LocalStore,
    private readonly listener: EngineListener,
    options: EngineOptions = {},
  ) {
    this.clientId = store.nodeId();
    this.clock = new HlcClock(this.clientId, options.physicalNow);
    this.pollIntervalMs = options.pollIntervalMs ?? 15_000;
    this.maxBatchOps = options.maxBatchOps ?? 1000;
    this.maxBatchBytes = options.maxBatchBytes ?? 4 * 1024 * 1024;
    this.backoffMs = options.backoffMs ?? DEFAULT_BACKOFF;
  }

  /** Optimistic state: confirmed server state merged with pending local ops. */
  get view(): EntityStore {
    return this.viewStore;
  }

  get cursor(): number {
    return this.cursorValue;
  }

  get status(): SyncStatus {
    return this.statusValue;
  }

  get pendingOps(): Op[] {
    return this.pending.flat();
  }

  /** Loads the initial state. Resolves true once data is shown, false when unauthorized. */
  async start(): Promise<boolean> {
    this.pending = await this.store.loadPending();
    const savedClock = await this.store.loadClock();
    if (savedClock) this.clock.receive(savedClock);
    for (const op of this.pending.flat()) this.clock.receive(op.hlc);
    this.listener.pending?.(this.pendingOps.length);

    const persisted = this.store.persistsState ? await this.store.loadState() : null;
    if (persisted) {
      this.confirmed.loadSnapshot(persisted);
      this.cursorValue = persisted.cursor;
      this.rebuild();
    }

    for (;;) {
      if (this.stopped) return false;
      try {
        const snapshot = await this.transport.snapshot();
        this.confirmed.loadSnapshot(snapshot);
        this.cursorValue = snapshot.cursor;
        for (const e of snapshot.entities) for (const f of Object.values(e.fields)) this.clock.receive(f.hlc);
        this.failures = 0;
        this.setStatus("online");
        break;
      } catch (err) {
        if (isUnauthorized(err)) {
          this.setStatus("unauthorized");
          return false;
        }
        this.setStatus("offline");
        if (persisted) break; // work offline on the persisted state, pull later
        await new Promise((resolve) => setTimeout(resolve, this.nextBackoff()));
      }
    }

    this.rebuild();
    this.started = true;
    this.unsubscribe = this.transport.subscribe({
      onCursor: (cursor) => {
        if (cursor > this.cursorValue) void this.pull();
      },
      onConnection: (connected) => {
        const wasLive = this.live;
        this.live = connected;
        if (connected && !wasLive) void this.pull();
      },
      onReset: () => void this.reload(),
    });
    this.pollTimer = setInterval(() => {
      if (!this.live) void this.pull();
      if (this.pending.length > 0 && !this.retryTimer) void this.flush();
    }, this.pollIntervalMs);
    void this.flush();
    return true;
  }

  stop(): void {
    this.stopped = true;
    this.unsubscribe?.();
    this.unsubscribe = null;
    if (this.pollTimer) clearInterval(this.pollTimer);
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.pollTimer = null;
    this.retryTimer = null;
  }

  /** Build ops for the given changes without applying them. Each op gets its own HLC. */
  buildOps(changes: readonly Change[]): Op[] {
    const ops: Op[] = [];
    for (const change of changes) {
      for (const [field, value] of Object.entries(change.fields)) {
        ops.push({ op_id: uuidv7(), entity: change.entity, entity_id: change.id, field, value: wellFormed(value), hlc: this.clock.now() });
      }
    }
    return ops;
  }

  /**
   * Applies changes optimistically as one push group (so creating an entity with
   * several fields always travels in one push). Returns client-side validation
   * issues; nothing is applied when there are any.
   */
  change(changes: readonly Change[]): ValidationIssue[] {
    const ops = this.buildOps(changes);
    if (ops.length === 0) return [];
    const issues = validateOps(this.viewStore, ops);
    if (issues.length > 0) return issues;
    this.commit(ops);
    return [];
  }

  /** Applies already-built ops optimistically and queues them for push. */
  commit(ops: Op[]): void {
    const changed = this.viewStore.applyAll(ops);
    this.pending.push(ops);
    void this.store.savePending(this.pending);
    void this.store.saveClock(this.clock.peek());
    this.listener.pending?.(this.pendingOps.length);
    if (changed.size > 0) this.listener.changed(changed);
    if (this.started) void this.flush();
  }

  /** Retry immediately (e.g. when the browser reports it is online again). */
  retryNow(): void {
    if (!this.started || this.stopped) return;
    if (this.retryTimer) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
    }
    void this.flush();
    void this.pull();
  }

  /** Fetch all ops since the cursor; concurrent calls coalesce into one more round. */
  pull(): Promise<void> {
    if (!this.started || this.stopped) return Promise.resolve();
    this.pullAgain = true;
    this.pulling ??= this.runPulls();
    return this.pulling;
  }

  private async runPulls(): Promise<void> {
    try {
      // The loop condition and the reset of `pulling` run in one synchronous step,
      // so a pull requested while the last round finishes is never lost.
      while (this.pullAgain && !this.stopped) {
        this.pullAgain = false;
        await this.pullOnce();
      }
      if (this.store.persistsState) {
        void this.store.saveState({ cursor: this.cursorValue, entities: this.snapshotEntities() });
      }
    } catch (err) {
      this.handleTransportError(err);
    } finally {
      this.pulling = null;
    }
  }

  /**
   * Replaces the confirmed state with a fresh snapshot and rebuilds the view
   * (pending ops stay on top). Used when the source changed in a way pulls
   * cannot express, e.g. the Tauri app's own sync rolled ops back.
   * Concurrent calls coalesce into one more round.
   */
  reload(): Promise<void> {
    if (!this.started || this.stopped) return Promise.resolve();
    this.reloadAgain = true;
    this.reloading ??= this.runReloads();
    return this.reloading;
  }

  private async runReloads(): Promise<void> {
    try {
      while (this.reloadAgain && !this.stopped) {
        this.reloadAgain = false;
        const epoch = this.pushEpoch;
        const snapshot = await this.transport.snapshot();
        if (this.stopped) return;
        if (epoch !== this.pushEpoch) {
          // A push result landed meanwhile; the snapshot may predate it.
          this.reloadAgain = true;
          continue;
        }
        this.confirmed.loadSnapshot(snapshot);
        this.cursorValue = snapshot.cursor;
        for (const e of snapshot.entities) for (const f of Object.values(e.fields)) this.clock.receive(f.hlc);
        this.markOnline();
        this.rebuild();
      }
    } catch (err) {
      this.handleTransportError(err);
    } finally {
      this.reloading = null;
    }
  }

  private async pullOnce(): Promise<void> {
    for (;;) {
      const res = await this.transport.pull(this.cursorValue);
      const changed = new Set<string>();
      for (const { op } of res.ops) {
        this.confirmed.apply(op);
        if (this.viewStore.apply(op)) changed.add(entityKey(op.entity, op.entity_id));
        this.clock.receive(op.hlc);
      }
      this.cursorValue = Math.max(this.cursorValue, res.cursor);
      this.markOnline();
      if (changed.size > 0) this.listener.changed(changed);
      if (!res.has_more || res.ops.length === 0) return;
    }
  }

  private async flush(): Promise<void> {
    if (this.pushing || this.stopped || !this.started || this.pending.length === 0) return;
    if (this.statusValue === "unauthorized") return;
    this.pushing = true;
    const { groupCount, ops } = this.nextBatch();
    let again = false;
    try {
      const res = await this.transport.push({ client_id: this.clientId, ops });
      this.markOnline();
      this.applyPushResult(groupCount, ops, res.rejected ?? []);
      if (res.warnings?.length) this.listener.warnings?.(res.warnings);
      if (res.cursor > this.cursorValue) void this.pull();
      again = this.pending.length > 0;
    } catch (err) {
      if (isUnauthorized(err) || isTransient(err)) {
        this.handleTransportError(err);
      } else if (groupCount > 1) {
        // The server refused the whole request (400, 413 …), possibly because of a single
        // group: drop nothing yet, retry group by group so only the culprit is rejected.
        this.isolating = true;
        again = true;
      } else {
        // The server refused this group on its own: it cannot succeed later.
        const message = err instanceof Error ? err.message : "Änderung abgelehnt.";
        const code = (err as { code?: string }).code ?? "push_failed";
        this.applyPushResult(
          groupCount,
          ops,
          ops.map((op) => ({ op_id: op.op_id, code, message })),
        );
        again = this.pending.length > 0;
      }
    } finally {
      this.pushing = false;
    }
    if (again) void this.flush();
  }

  /**
   * The next push: whole groups from the front of the queue. The server rejects all ops
   * of an entity in a push when one of them is invalid (contract 2.5), so a group that
   * touches an entity of an earlier group in the batch goes into a later push. Otherwise
   * a later invalid group would also take down earlier, valid edits of that entity.
   */
  private nextBatch(): { groupCount: number; ops: Op[] } {
    let groupCount = 0;
    let bytes = 0;
    const ops: Op[] = [];
    const touched = new Set<string>();
    for (const group of this.pending) {
      const keys = group.map((op) => entityKey(op.entity, op.entity_id));
      const size = group.reduce((sum, op) => sum + JSON.stringify(op).length, 0);
      if (groupCount > 0) {
        if (this.isolating) break;
        if (ops.length + group.length > this.maxBatchOps || bytes + size > this.maxBatchBytes) break;
        if (keys.some((k) => touched.has(k))) break;
      }
      ops.push(...group);
      for (const k of keys) touched.add(k);
      bytes += size;
      groupCount += 1;
    }
    return { groupCount, ops };
  }

  private applyPushResult(groupCount: number, ops: Op[], rejected: readonly Rejection[]): void {
    // Everything not rejected is stored by the server (accepted, or already known by op_id).
    const rejectedIds = new Set(rejected.map((r) => r.op_id));
    for (const op of ops) if (!rejectedIds.has(op.op_id)) this.confirmed.apply(op);
    this.pushEpoch += 1;
    this.pending = this.pending.slice(groupCount);
    if (this.pending.length === 0) this.isolating = false;
    void this.store.savePending(this.pending);
    this.listener.pending?.(this.pendingOps.length);
    if (rejected.length > 0) {
      this.rebuild();
      this.listener.rejected?.(rejected);
    }
  }

  /** View = confirmed ⊕ pending; used at start and to roll back rejected ops. */
  private rebuild(): void {
    const view = this.confirmed.clone();
    for (const group of this.pending) view.applyAll(group);
    this.viewStore = view;
    this.listener.changed("all");
  }

  private handleTransportError(err: unknown): void {
    if (isUnauthorized(err)) {
      this.setStatus("unauthorized");
      return;
    }
    if (!isTransient(err)) {
      console.error("Sync-Fehler", err);
    }
    this.setStatus("offline");
    this.scheduleRetry();
  }

  private scheduleRetry(): void {
    if (this.retryTimer || this.stopped) return;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      void this.flush();
      void this.pull();
    }, this.nextBackoff());
  }

  private nextBackoff(): number {
    const delay = this.backoffMs[Math.min(this.failures, this.backoffMs.length - 1)] ?? 30_000;
    this.failures += 1;
    return delay;
  }

  private markOnline(): void {
    this.failures = 0;
    if (this.statusValue !== "online") this.setStatus("online");
  }

  private setStatus(status: SyncStatus): void {
    this.statusValue = status;
    this.listener.status?.(status);
  }

  private snapshotEntities() {
    return [...this.confirmed.values()].map((e) => ({
      entity: e.entity,
      entity_id: e.id,
      created_hlc: e.created_hlc,
      fields: Object.fromEntries(Object.entries(e.fields).map(([k, f]) => [k, { value: f.value, hlc: f.hlc, op_id: f.op_id }])),
    }));
  }
}
