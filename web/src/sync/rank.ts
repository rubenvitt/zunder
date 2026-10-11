// Lexicographically sortable rank strings (fractional indexing over base 36).
// Tasks without an explicit rank sort by a default rank derived from their
// creation time, so explicit and implicit ranks live in one key space.

const DIGITS = "0123456789abcdefghijklmnopqrstuvwxyz";
const BASE = DIGITS.length;

function digit(c: string | undefined): number {
  if (c === undefined) return 0;
  const i = DIGITS.indexOf(c);
  return i < 0 ? 0 : i;
}

/** Default rank for a creation time: fixed width base 36 plus a non-zero suffix. */
export function defaultRank(createdMs: number): string {
  const ms = Number.isFinite(createdMs) && createdMs > 0 ? Math.floor(createdMs) : 0;
  return `${ms.toString(36).padStart(9, "0")}i`;
}

/**
 * A rank strictly between `a` and `b` (null = open end). Results never end in "0",
 * so there is always room for another rank between two generated ones.
 */
export function rankBetween(a: string | null, b: string | null): string {
  const lower = a ?? "";
  let upper = b;
  if (upper !== null && upper <= lower) upper = null;
  let result = "";
  for (let i = 0; ; i++) {
    const lo = digit(lower[i]);
    const hi = upper === null ? BASE : i < upper.length ? digit(upper[i]) : 0;
    if (hi - lo > 1) {
      return result + DIGITS[Math.floor((lo + hi) / 2)];
    }
    result += DIGITS[lo];
    if (hi - lo === 1) upper = null;
    // Guard against pathological inputs (e.g. upper ending in "0").
    if (i > 64) return lower + "i";
  }
}
