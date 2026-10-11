import type { EntityKind, JsonValue, Op } from "../src/sync/types";

let counter = 0;

/** Deterministic op factory for tests. */
export function op(
  entity: EntityKind,
  entityId: string,
  field: string,
  value: JsonValue,
  wallMs: number,
  options: { counter?: number; node?: string; opId?: string } = {},
): Op {
  counter += 1;
  const hlc = `${String(wallMs).padStart(13, "0")}:${String(options.counter ?? 0).padStart(5, "0")}:${options.node ?? "test"}`;
  return {
    op_id: options.opId ?? `00000000-0000-7000-8000-${String(counter).padStart(12, "0")}`,
    entity,
    entity_id: entityId,
    field,
    value,
    hlc,
  };
}

export function permutations<T>(items: readonly T[], limit = 24): T[][] {
  const out: T[][] = [];
  const rec = (prefix: T[], rest: T[]) => {
    if (out.length >= limit) return;
    if (rest.length === 0) {
      out.push(prefix);
      return;
    }
    for (let i = 0; i < rest.length; i++) rec([...prefix, rest[i]!], [...rest.slice(0, i), ...rest.slice(i + 1)]);
  };
  rec([], [...items]);
  return out;
}

/** Seeded pseudo-random shuffles (mulberry32). */
export function shuffled<T>(items: readonly T[], seed: number): T[] {
  let s = seed >>> 0;
  const rand = () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const arr = [...items];
  for (let i = arr.length - 1; i > 0; i--) {
    const j = Math.floor(rand() * (i + 1));
    [arr[i], arr[j]] = [arr[j]!, arr[i]!];
  }
  return arr;
}
