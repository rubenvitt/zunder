// Last-Writer-Wins per (entity, field) by HLC, tie-break by op_id (greater wins).
// Commutative, associative and idempotent; validation is not part of the merge.

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
}

export function entityKey(entity: EntityKind, id: string): string {
  return `${entity}/${id}`;
}

/** True when `candidate` beats `current` under LWW. */
export function wins(candidate: { hlc: Hlc; op_id: string }, current: { hlc: Hlc; op_id: string }): boolean {
  if (candidate.hlc !== current.hlc) return candidate.hlc > current.hlc;
  return candidate.op_id > current.op_id;
}

export class EntityStore {
  private readonly map = new Map<string, EntityState>();

  /** Applies one op; returns true when the entity's visible state changed. */
  apply(op: Op): boolean {
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
      target = { entity, id, fields: {} };
      this.map.set(key, target);
    }
    const current = target.fields[field];
    if (current && !wins(state, current)) return false;
    target.fields[field] = state;
    return true;
  }

  loadSnapshot(snapshot: Snapshot): void {
    this.map.clear();
    for (const e of snapshot.entities) {
      for (const [field, f] of Object.entries(e.fields)) {
        this.applyField(e.entity, e.entity_id, field, { value: f.value, hlc: f.hlc, op_id: f.op_id ?? "" });
      }
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
    for (const [key, e] of this.map) copy.map.set(key, { entity: e.entity, id: e.id, fields: { ...e.fields } });
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
