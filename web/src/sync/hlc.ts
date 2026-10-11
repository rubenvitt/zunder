// Hybrid Logical Clock, string format per contract 2.2:
// "<wall_ms 13 digits>:<counter 5 digits>:<node>", ordered lexicographically.

import type { Hlc } from "./types";

export const MAX_COUNTER = 99_999;
const NODE_RE = /^[a-z0-9-]{1,36}$/;
const HLC_RE = /^(\d{13}):(\d{5}):([a-z0-9-]{1,36})$/;

export interface HlcParts {
  wallMs: number;
  counter: number;
  node: string;
}

export function isValidNode(node: string): boolean {
  return NODE_RE.test(node);
}

export function formatHlc(parts: HlcParts): Hlc {
  if (!Number.isInteger(parts.wallMs) || parts.wallMs < 0 || parts.wallMs > 9_999_999_999_999) {
    throw new RangeError(`HLC wall_ms out of range: ${parts.wallMs}`);
  }
  if (!Number.isInteger(parts.counter) || parts.counter < 0 || parts.counter > MAX_COUNTER) {
    throw new RangeError(`HLC counter out of range: ${parts.counter}`);
  }
  if (!isValidNode(parts.node)) throw new RangeError(`Invalid HLC node: ${parts.node}`);
  return `${String(parts.wallMs).padStart(13, "0")}:${String(parts.counter).padStart(5, "0")}:${parts.node}`;
}

export function parseHlc(hlc: Hlc): HlcParts | null {
  const m = HLC_RE.exec(hlc);
  if (!m) return null;
  return { wallMs: Number(m[1]), counter: Number(m[2]), node: m[3]! };
}

export function isValidHlc(hlc: string): boolean {
  return HLC_RE.test(hlc);
}

/** Total order of HLC strings (lexicographic, identical to tuple order thanks to fixed widths). */
export function compareHlc(a: Hlc, b: Hlc): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Wall-clock milliseconds of an HLC, or NaN when unparsable. */
export function hlcWallMs(hlc: Hlc): number {
  const parts = parseHlc(hlc);
  return parts ? parts.wallMs : Number.NaN;
}

export class HlcClock {
  private wallMs = 0;
  private counter = 0;

  constructor(
    readonly node: string,
    private readonly physicalNow: () => number = () => Date.now(),
  ) {
    if (!isValidNode(node)) throw new RangeError(`Invalid HLC node: ${node}`);
  }

  /** Next timestamp: max(physical, last); same wall_ms increments the counter. */
  now(): Hlc {
    const physical = Math.floor(this.physicalNow());
    if (physical > this.wallMs) {
      this.wallMs = physical;
      this.counter = 0;
    } else {
      this.bumpCounter();
    }
    return formatHlc({ wallMs: this.wallMs, counter: this.counter, node: this.node });
  }

  /** Raise the local clock to max(local, remote) so later local stamps sort after the remote one. */
  receive(remote: Hlc): void {
    const parts = parseHlc(remote);
    if (!parts) return;
    if (parts.wallMs > this.wallMs || (parts.wallMs === this.wallMs && parts.counter > this.counter)) {
      this.wallMs = parts.wallMs;
      this.counter = parts.counter;
    }
  }

  /** The last issued or received timestamp (for persistence). */
  peek(): Hlc {
    return formatHlc({ wallMs: this.wallMs, counter: this.counter, node: this.node });
  }

  private bumpCounter(): void {
    if (this.counter >= MAX_COUNTER) {
      // Counter exhausted within one millisecond: borrow the next millisecond.
      this.wallMs += 1;
      this.counter = 0;
    } else {
      this.counter += 1;
    }
  }
}
