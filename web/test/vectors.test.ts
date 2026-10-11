// Shared test vectors (contract 2.6) produced by zunder-core. The Rust and the
// TypeScript merge must agree on every file in crates/zunder-core/tests/vectors.

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { compareHlc, isValidHlc } from "../src/sync/hlc";
import { mergeOps } from "../src/sync/merge";
import type { JsonValue, Op } from "../src/sync/types";
import { permutations, shuffled } from "./helpers";

const here = dirname(fileURLToPath(import.meta.url));
const VECTOR_DIR = resolve(here, "../../crates/zunder-core/tests/vectors");

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

type Vector = MergeVector | HlcOrderVector;

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

if (files.length === 0) {
  describe("shared test vectors", () => {
    it.skip(`no vectors found in ${VECTOR_DIR} (created by zunder-core); skipped`, () => {});
  });
} else {
  describe("shared test vectors", () => {
    for (const file of files) {
      const vector = JSON.parse(readFileSync(join(VECTOR_DIR, file), "utf8")) as Vector;
      const known = vector.kind === "merge" || vector.kind === "hlc_order";
      // Other vector kinds (e.g. core-only validation vectors) are not covered by the web client.
      (known ? it : it.skip)(`${file}: ${vector.name}`, () => {
        if (vector.kind === "hlc_order") {
          for (const hlc of vector.sorted) expect(isValidHlc(hlc), hlc).toBe(true);
          for (const order of orderings(vector.sorted)) expect([...order].sort(compareHlc)).toEqual(vector.sorted);
          return;
        }
        const expected = canonical(vector.expected);
        for (const order of orderings(vector.ops)) {
          expect(canonical(mergeOps(order).toValues())).toBe(expected);
        }
      });
    }
  });
}
