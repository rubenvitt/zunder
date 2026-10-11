// Shared test vectors (contract 2.6) produced by zunder-core. The Rust and the
// TypeScript merge must agree on every file in crates/zunder-core/tests/vectors.

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { compareHlc, isValidHlc } from "../src/sync/hlc";
import { mergeOps } from "../src/sync/merge";
import { validateOps } from "../src/sync/validate";
import type { JsonValue, Op } from "../src/sync/types";
import { permutations, shuffled } from "./helpers";

const here = dirname(fileURLToPath(import.meta.url));
const VECTOR_DIR = resolve(process.env.ZUNDER_VECTOR_DIR ?? resolve(here, "../../crates/zunder-core/tests/vectors"));

interface MergeVector {
  name: string;
  kind: "merge";
  ops: Op[];
  expected: Record<string, Record<string, JsonValue>>;
}

interface HlcOrderVector {
  name: string;
  kind: "hlc_order";
  sorted: string[];
}

interface ValidateVector {
  name: string;
  kind: "validate";
  now_ms: number;
  state: Op[];
  push: Op[];
  /** entity id → code of the cause; empty = everything accepted. */
  expected_rejected: Record<string, string>;
}

type Vector = MergeVector | HlcOrderVector | ValidateVector;

function orderings<T>(items: readonly T[]): T[][] {
  const result = [items.slice(), items.slice().reverse()];
  if (items.length <= 6) result.push(...permutations(items, 720));
  else for (let seed = 1; seed <= 25; seed++) result.push(shuffled(items, seed));
  return result;
}

function canonical(value: unknown): string {
  return JSON.stringify(value, (_key, v: unknown) =>
    v && typeof v === "object" && !Array.isArray(v)
      ? Object.fromEntries(Object.entries(v as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
      : v,
  );
}

const files = existsSync(VECTOR_DIR) ? readdirSync(VECTOR_DIR).filter((f) => f.endsWith(".json")).sort() : [];

describe("shared test vectors", () => {
  it(`finds the vectors in ${VECTOR_DIR}`, () => {
    // The vectors are part of the repository (zunder-core); a missing directory is an error, not a skip.
    expect(files.length).toBeGreaterThan(0);
  });

  for (const file of files) {
    const vector = JSON.parse(readFileSync(join(VECTOR_DIR, file), "utf8")) as Vector;
    it(`${file}: ${vector.name}`, () => {
      expect(file).toContain(vector.name);
      if (vector.kind === "hlc_order") {
        for (const hlc of vector.sorted) expect(isValidHlc(hlc), hlc).toBe(true);
        for (const order of orderings(vector.sorted)) expect([...order].sort(compareHlc)).toEqual(vector.sorted);
        return;
      }
      if (vector.kind === "validate") {
        // The client refuses a commit as a whole and names the cause per entity, which must match
        // the core's cause per rejected entity.
        const issues = validateOps(mergeOps(vector.state), vector.push, vector.now_ms);
        const got = Object.fromEntries(issues.map((i) => [i.entity_id, i.code]));
        expect(issues).toHaveLength(Object.keys(got).length);
        expect(got).toEqual(vector.expected_rejected);
        return;
      }
      expect(vector.kind).toBe("merge");
      const expected = canonical(vector.expected);
      for (const order of orderings(vector.ops)) {
        expect(canonical(mergeOps(order).toValues())).toBe(expected);
      }
    });
  }
});
