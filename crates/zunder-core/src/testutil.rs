//! Helpers for unit tests.

use crate::ids::Id;
use serde_json::Value;

use crate::hlc::Hlc;
use crate::op::{EntityKind, Op};

/// Deterministic test ID, ordered like `n`.
pub fn uid(n: u128) -> Id {
    Id::parse(&format!("t{n:023}")).expect("valid test ID")
}

pub fn hlc(s: &str) -> Hlc {
    Hlc::parse(s).unwrap()
}

/// HLC `wall_ms:counter:node` with padding applied.
pub fn h(wall_ms: u64, counter: u32, node: &str) -> Hlc {
    hlc(&format!("{wall_ms:013}:{counter:05}:{node}"))
}

pub fn op(
    op_id: u128,
    entity: EntityKind,
    entity_id: u128,
    field: &str,
    value: Value,
    hlc: Hlc,
) -> Op {
    Op {
        op_id: uid(op_id),
        entity,
        entity_id: uid(entity_id),
        field: field.to_owned(),
        value,
        hlc,
    }
}
