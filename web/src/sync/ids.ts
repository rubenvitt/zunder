// Identifiers (contract 2.1): CUID2 strings, 24 characters, a lowercase letter
// followed by lowercase letters and digits. Clients generate them themselves.
//
// IDs from before CUID2 (UUIDs) are converted with the same fixed hash as
// zunder_core::ids::Id::from_legacy, so an old pending queue, link or vault
// zunder_id lands on the same entity as on the server. Shared vector 15 keeps
// both sides in step.

import { sha256 } from "@noble/hashes/sha2.js";
import { createId } from "@paralleldrive/cuid2";
import type { EntityKind, JsonValue, Op } from "./types";

export const ID_LEN = 24;

const ID_RE = /^[a-z][a-z0-9]{23}$/;
const LEGACY_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const BASE36 = "0123456789abcdefghijklmnopqrstuvwxyz";
const encoder = new TextEncoder();

/** A new random ID (CUID2). */
export function newId(): string {
  return createId();
}

export function isCanonicalId(v: unknown): v is string {
  return typeof v === "string" && ID_RE.test(v);
}

/** A UUID in its only legacy wire form (lowercase, hyphenated). */
export function isLegacyUuid(v: unknown): v is string {
  return typeof v === "string" && LEGACY_RE.test(v);
}

/** The ID a legacy UUID became: SHA-256 of "uuid:<uuid>", first byte mod 26 as a letter, bytes 1–23 mod 36. */
export function fromLegacy(uuid: string): string {
  const hash = sha256(encoder.encode(`uuid:${uuid}`));
  let out = String.fromCharCode(97 + (hash[0]! % 26));
  for (let i = 1; i < ID_LEN; i++) out += BASE36[hash[i]! % 36];
  return out;
}

/** A legacy UUID converted, anything else unchanged. */
export function upgradeId(s: string): string {
  return isLegacyUuid(s) ? fromLegacy(s) : s;
}

/** Converts legacy UUIDs in the ID-valued fields (core: op::upgrade_legacy_value). */
export function upgradeValue(entity: EntityKind, field: string, value: JsonValue): JsonValue {
  if ((entity === "project" && field === "area_id") || (entity === "task" && field === "parent_id")) {
    return typeof value === "string" ? upgradeId(value) : value;
  }
  if (entity === "task" && field === "location" && value && typeof value === "object" && !Array.isArray(value) && typeof value.id === "string") {
    return isLegacyUuid(value.id) ? { ...value, id: fromLegacy(value.id) } : value;
  }
  return value;
}

/** The op with every legacy UUID converted (the same object when there is none). */
export function upgradeOp(op: Op): Op {
  const op_id = upgradeId(op.op_id);
  const entity_id = upgradeId(op.entity_id);
  const value = upgradeValue(op.entity, op.field, op.value);
  if (op_id === op.op_id && entity_id === op.entity_id && value === op.value) return op;
  return { ...op, op_id, entity_id, value };
}
