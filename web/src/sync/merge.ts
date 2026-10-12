// Last-Writer-Wins per (entity, field) by HLC, tie-break by op_id (greater wins),
// then by a fixed order on the JSON value (like zunder_core::state::cmp_json), so
// even faulty duplicates converge. Commutative, associative and idempotent;
// validation is not part of the merge.

import { upgradeId, upgradeOp, upgradeValue } from "./ids";
import type { EntityKind, Hlc, JsonValue, Op, Snapshot } from "./types";

export interface FieldState {
  value: JsonValue;
  hlc: Hlc;
  /** Empty when unknown (e.g. a snapshot without op ids); any real op id beats it on an HLC tie. */
  op_id: string;
}

export interface EntityState {
  entity: EntityKind;
  id: string;
  fields: Record<string, FieldState>;
  /** Smallest HLC of any op ever applied to the entity (derived created_at). */
  created_hlc: Hlc;
}

export function entityKey(entity: EntityKind, id: string): string {
  return `${entity}/${id}`;
}

/** Compares strings by Unicode code points (= UTF-8 byte order, as in Rust). */
function cmpStr(a: string, b: string): number {
  if (a === b) return 0;
  const x = [...a];
  const y = [...b];
  for (let i = 0; i < Math.min(x.length, y.length); i++) {
    const d = x[i]!.codePointAt(0)! - y[i]!.codePointAt(0)!;
    if (d !== 0) return d < 0 ? -1 : 1;
  }
  return x.length - y.length < 0 ? -1 : x.length === y.length ? 0 : 1;
}

function jsonRank(v: JsonValue): number {
  if (v === null) return 0;
  if (typeof v === "boolean") return 1;
  if (typeof v === "number") return 2;
  if (typeof v === "string") return 3;
  if (Array.isArray(v)) return 4;
  return 5;
}

/** Fixed total order on JSON values (mirror of zunder_core::state::cmp_json). */
export function cmpJson(a: JsonValue, b: JsonValue): number {
  const ra = jsonRank(a);
  const rb = jsonRank(b);
  if (ra !== rb) return ra < rb ? -1 : 1;
  if (typeof a === "boolean") return a === b ? 0 : a ? 1 : -1;
  if (typeof a === "number") return cmpStr(String(a), String(b));
  if (typeof a === "string") return cmpStr(a, b as string);
  if (Array.isArray(a)) {
    const y = b as JsonValue[];
    for (let i = 0; i < Math.min(a.length, y.length); i++) {
      const o = cmpJson(a[i]!, y[i]!);
      if (o !== 0) return o;
    }
    return Math.sign(a.length - y.length);
  }
  if (a === null) return 0;
  const xs = Object.entries(a).sort(([p], [q]) => cmpStr(p, q));
  const ys = Object.entries(b as { [key: string]: JsonValue }).sort(([p], [q]) => cmpStr(p, q));
  for (let i = 0; i < Math.min(xs.length, ys.length); i++) {
    const o = cmpStr(xs[i]![0], ys[i]![0]) || cmpJson(xs[i]![1], ys[i]![1]);
    if (o !== 0) return o;
  }
  return Math.sign(xs.length - ys.length);
}

/** True when `candidate` beats `current` under LWW. */
export function wins(candidate: FieldState, current: FieldState): boolean {
  if (candidate.hlc !== current.hlc) return candidate.hlc > current.hlc;
  if (candidate.op_id !== current.op_id) return candidate.op_id > current.op_id;
  return cmpJson(candidate.value, current.value) > 0;
}

export class EntityStore {
  private readonly map = new Map<string, EntityState>();

  /** Applies one op (legacy UUIDs converted); returns true when the entity's visible state changed. */
  apply(raw: Op): boolean {
    const op = upgradeOp(raw);
    return this.applyField(op.entity, op.entity_id, op.field, { value: op.value, hlc: op.hlc, op_id: op.op_id });
  }

  applyAll(ops: Iterable<Op>): Set<string> {
    const changed = new Set<string>();
    for (const op of ops) if (this.apply(op)) changed.add(entityKey(op.entity, op.entity_id));
    return changed;
  }

  applyField(entity: EntityKind, id: string, field: string, state: FieldState): boolean {
    const key = entityKey(entity, id);
    let target = this.map.get(key);
    if (!target) {
      target = { entity, id, fields: {}, created_hlc: state.hlc };
      this.map.set(key, target);
    } else if (state.hlc < target.created_hlc) {
      target.created_hlc = state.hlc;
    }
    const current = target.fields[field];
    if (current && !wins(state, current)) return false;
    target.fields[field] = state;
    return true;
  }

  loadSnapshot(snapshot: Snapshot): void {
    this.map.clear();
    for (const e of snapshot.entities) {
      const id = upgradeId(e.entity_id);
      for (const [field, f] of Object.entries(e.fields)) {
        const value = upgradeValue(e.entity, field, f.value);
        this.applyField(e.entity, id, field, { value, hlc: f.hlc, op_id: upgradeId(f.op_id ?? "") });
      }
      // created_hlc (contract 3) also covers ops whose values were overwritten since.
      const target = this.map.get(entityKey(e.entity, id));
      if (target && e.created_hlc && e.created_hlc < target.created_hlc) target.created_hlc = e.created_hlc;
    }
  }

  get(entity: EntityKind, id: string): EntityState | undefined {
    return this.map.get(entityKey(entity, id));
  }

  getByKey(key: string): EntityState | undefined {
    return this.map.get(key);
  }

  has(entity: EntityKind, id: string): boolean {
    return this.map.has(entityKey(entity, id));
  }

  values(): IterableIterator<EntityState> {
    return this.map.values();
  }

  keys(): IterableIterator<string> {
    return this.map.keys();
  }

  get size(): number {
    return this.map.size;
  }

  /** Deep enough copy: field states are immutable and shared. */
  clone(): EntityStore {
    const copy = new EntityStore();
    for (const [key, e] of this.map) copy.map.set(key, { entity: e.entity, id: e.id, fields: { ...e.fields }, created_hlc: e.created_hlc });
    return copy;
  }

  /** Winning values only, e.g. for test-vector comparison: {"task/<id>": {field: value}}. */
  toValues(): Record<string, Record<string, JsonValue>> {
    const out: Record<string, Record<string, JsonValue>> = {};
    for (const [key, e] of this.map) {
      const fields: Record<string, JsonValue> = {};
      for (const [name, f] of Object.entries(e.fields)) fields[name] = f.value;
      out[key] = fields;
    }
    return out;
  }
}

/** Convenience: merge a list of ops from scratch. */
export function mergeOps(ops: Iterable<Op>): EntityStore {
  const store = new EntityStore();
  store.applyAll(ops);
  return store;
}

export function maxHlcOf(ops: Iterable<{ hlc: Hlc }>): Hlc | null {
  let max: Hlc | null = null;
  for (const op of ops) if (max === null || op.hlc > max) max = op.hlc;
  return max;
}
