// UUIDv7 (RFC 9562): 48 bit Unix milliseconds, version 7, 74 random bits.
// Within one millisecond the random part is incremented monotonically so that
// ids created by one client sort in creation order.

let lastMs = -1;
let lastRandom: Uint8Array | null = null;

function fillRandom(bytes: Uint8Array): void {
  crypto.getRandomValues(bytes);
}

function incrementBytes(bytes: Uint8Array): boolean {
  for (let i = bytes.length - 1; i >= 0; i--) {
    const value = bytes[i]!;
    if (value < 0xff) {
      bytes[i] = value + 1;
      return true;
    }
    bytes[i] = 0;
  }
  return false;
}

export function uuidv7(nowMs: number = Date.now()): string {
  let random: Uint8Array;
  let ms = nowMs;
  if (ms <= lastMs && lastRandom) {
    ms = lastMs;
    random = lastRandom;
    if (!incrementBytes(random)) {
      ms = lastMs + 1;
      fillRandom(random);
    }
  } else {
    random = new Uint8Array(10);
    fillRandom(random);
  }
  lastMs = ms;
  lastRandom = random;

  const bytes = new Uint8Array(16);
  let t = ms;
  for (let i = 5; i >= 0; i--) {
    bytes[i] = t % 256;
    t = Math.floor(t / 256);
  }
  bytes.set(random, 6);
  bytes[6] = 0x70 | (bytes[6]! & 0x0f);
  bytes[8] = 0x80 | (bytes[8]! & 0x3f);

  let hex = "";
  for (const b of bytes) hex += b.toString(16).padStart(2, "0");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export function isUuid(value: string): boolean {
  return UUID_RE.test(value);
}
