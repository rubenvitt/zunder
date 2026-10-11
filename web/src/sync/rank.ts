// Lexicographically sortable rank strings (fractional indexing). Generated ranks use
// [0-9a-z] and never end in "0", so there is always room for another rank between
// two of them. Tasks without an explicit rank sort by a default rank derived from
// their creation time, so explicit and implicit ranks live in one key space.
//
// Other clients may write ranks with other characters, e.g. the ClickUp import
// writes fixed-width decimals like "0000000000001.500000" (contract 6). rankBetween
// only relies on plain string order (UTF-16 code units, which for ASCII equals the
// byte order the server and core use), so it also inserts between such ranks.

const DIGITS = "0123456789abcdefghijklmnopqrstuvwxyz";
/** Characters allowed at the end of a generated rank. */
const FINAL = DIGITS.slice(1);

/** Default rank for a creation time: fixed width base 36 plus a non-zero suffix. */
export function defaultRank(createdMs: number): string {
  const ms = Number.isFinite(createdMs) && createdMs > 0 ? Math.floor(createdMs) : 0;
  return `${ms.toString(36).padStart(9, "0")}i`;
}

/**
 * A rank strictly between `a` and `b` (null = open end). When `b` does not sort after
 * `a`, it is ignored (open end), so the result is always greater than `a`.
 */
export function rankBetween(a: string | null, b: string | null): string {
  const lower = a ?? "";
  let upper = b !== null && b > lower ? b : null;
  let result = "";
  for (let i = 0; i < 4096; i++) {
    if (upper !== null && i >= upper.length) {
      // Degenerate: the rest of `upper` only had characters up to "0" (e.g. the import
      // rank "0000000000000.000000"), so `result` caught up with it. A proper prefix of
      // `upper` that is still longer than `lower` sorts between both.
      if (result.length - 1 > lower.length) return result.slice(0, -1);
      const last = result.charCodeAt(result.length - 1);
      return `${lower}${String.fromCharCode(Math.max(last - 1, 1))}i`;
    }
    const lo = i < lower.length ? lower[i]! : null;
    // `upper` stays set only while `result` equals its prefix.
    const hi = upper !== null && i < upper.length ? upper[i]! : null;
    const fits = [...FINAL].filter((c) => (lo === null || c > lo) && (hi === null || c < hi));
    if (fits.length > 0) return result + fits[Math.floor(fits.length / 2)]!;
    if (lo !== null) {
      // Follow the lower bound; once it is below the upper bound, the upper bound is out of reach.
      result += lo;
      if (hi !== null && lo < hi) upper = null;
    } else if (hi !== null && "0" < hi) {
      result += "0";
      upper = null;
    } else if (hi !== null) {
      // Only characters below "0" (like ".") remain: follow the upper bound.
      result += hi;
    } else {
      return result + "i";
    }
  }
  return `${lower}i`;
}
