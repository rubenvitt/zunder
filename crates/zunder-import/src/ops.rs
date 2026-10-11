//! From the desired state to operations: diff against the current Zunder
//! state, HLC assignment and push batches.

use serde_json::Value;
use uuid::Uuid;
use zunder_core::hlc::{Clock, NodeId};
use zunder_core::{EntityKind, Op, OpBatch, State};

use crate::convert::{Desired, IMPORT_NODE};

/// The operations of one entity. Batches never split an entity, so a new
/// entity always arrives together with its required fields.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityOps {
    pub kind: EntityKind,
    pub id: Uuid,
    pub label: String,
    pub ops: Vec<Op>,
}

/// Statistics of [`plan_ops`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffStats {
    /// Entities that do not exist in Zunder yet.
    pub created: usize,
    /// Existing entities with at least one changed field.
    pub updated: usize,
    /// Existing entities that are already up to date.
    pub unchanged: usize,
    /// Entities tombstoned in Zunder; left alone.
    pub deleted_in_zunder: usize,
}

/// A clock for the import node that is ahead of everything in `state`, so
/// import operations win against older values regardless of clock skew.
pub fn import_clock(state: Option<&State>) -> Clock {
    let mut clock = Clock::new(NodeId::new(IMPORT_NODE).expect("valid node"));
    if let Some(state) = state {
        for entity in state.entities() {
            for entry in entity.fields().values() {
                clock.receive(&entry.hlc);
            }
        }
    }
    clock
}

/// Builds the operations needed to reach the desired state.
///
/// Without `state` every non-null field of every entity is written (first
/// import or dry run without server). With `state`:
///
/// - new entities get all their non-null fields,
/// - existing entities only get fields whose value differs (`null` resets
///   a field the import owns),
/// - entities tombstoned in Zunder are skipped (deleted on purpose).
pub fn plan_ops<'a>(
    desired: impl IntoIterator<Item = &'a Desired>,
    state: Option<&State>,
    clock: &mut Clock,
    physical_ms: u64,
) -> (Vec<EntityOps>, DiffStats) {
    let mut stats = DiffStats::default();
    let mut out = Vec::new();
    let mut batch = OpBatch::new(clock, physical_ms);
    for entity in desired {
        let current = state.and_then(|s| s.entity(entity.kind, entity.id));
        if current.is_some_and(|e| e.is_deleted()) {
            stats.deleted_in_zunder += 1;
            continue;
        }
        let start = batch.ops().len();
        for (field, value) in &entity.fields {
            let have = current.and_then(|e| e.value(field));
            let write = match (current, value) {
                (None, Value::Null) => false,
                (None, _) => true,
                (Some(_), Value::Null) => have.is_some(),
                (Some(_), v) => have != Some(v),
            };
            if write {
                batch.set(entity.kind, entity.id, field, value.clone());
            }
        }
        let count = batch.ops().len() - start;
        match (current, count) {
            (None, _) => stats.created += 1,
            (Some(_), 0) => stats.unchanged += 1,
            (Some(_), _) => stats.updated += 1,
        }
        if count > 0 {
            out.push(EntityOps {
                kind: entity.kind,
                id: entity.id,
                label: entity.label.clone(),
                ops: batch.ops()[start..].to_vec(),
            });
        }
    }
    (out, stats)
}

/// Splits entity groups into push batches of at most `max_ops` operations
/// (a single entity larger than `max_ops` gets its own batch). Order is
/// preserved, so areas and projects precede tasks and parents precede
/// subtasks.
pub fn batches(groups: &[EntityOps], max_ops: usize) -> Vec<Vec<Op>> {
    let max_ops = max_ops.max(1);
    let mut out: Vec<Vec<Op>> = Vec::new();
    let mut current: Vec<Op> = Vec::new();
    for group in groups {
        if !current.is_empty() && current.len() + group.ops.len() > max_ops {
            out.push(std::mem::take(&mut current));
        }
        current.extend(group.ops.iter().cloned());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn desired(id: u128, fields: &[(&str, Value)]) -> Desired {
        Desired {
            kind: EntityKind::Task,
            id: Uuid::from_u128(id),
            label: format!("t{id}"),
            fields: fields
                .iter()
                .map(|(f, v)| ((*f).to_owned(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn diff_skips_unchanged_and_resets_owned_fields() {
        let d1 = desired(
            1,
            &[
                ("title", json!("A")),
                ("due", json!(null)),
                ("rank", json!("1")),
            ],
        );
        let mut clock = import_clock(None);
        let (groups, stats) = plan_ops([&d1], None, &mut clock, 1_000);
        assert_eq!(stats.created, 1);
        assert_eq!(
            groups[0].ops.len(),
            2,
            "nulls are not written for new entities"
        );

        // Apply, then change Zunder-side due and ClickUp-side title.
        let mut state = State::from_ops(&groups[0].ops);
        let mut other = Clock::new(NodeId::new("web").unwrap());
        let mut b = OpBatch::new(&mut other, 2_000);
        b.set_task(d1.id, "due", "2026-10-12");
        for op in b.ops() {
            state.apply(op);
        }
        let d1b = desired(
            1,
            &[
                ("title", json!("B")),
                ("due", json!(null)),
                ("rank", json!("1")),
            ],
        );
        let mut clock = import_clock(Some(&state));
        let (groups, stats) = plan_ops([&d1b], Some(&state), &mut clock, 1_500);
        assert_eq!(stats.updated, 1);
        let fields: Vec<&str> = groups[0].ops.iter().map(|o| o.field.as_str()).collect();
        assert_eq!(fields, ["title", "due"]);
        assert!(
            groups[0].ops.iter().all(|o| o.hlc.wall_ms() >= 2_000),
            "the import clock is ahead of the state"
        );
        for op in &groups[0].ops {
            state.apply(op);
        }
        let (groups, stats) = plan_ops([&d1b], Some(&state), &mut clock, 1_500);
        assert!(groups.is_empty());
        assert_eq!(stats.unchanged, 1);
    }

    #[test]
    fn batches_never_split_an_entity() {
        let mut clock = import_clock(None);
        let ds: Vec<Desired> = (0..5)
            .map(|i| desired(i, &[("title", json!("x")), ("status", json!("open"))]))
            .collect();
        let (groups, _) = plan_ops(&ds, None, &mut clock, 1);
        let b = batches(&groups, 3);
        assert_eq!(b.iter().map(Vec::len).collect::<Vec<_>>(), [2, 2, 2, 2, 2]);
        let b = batches(&groups, 4);
        assert_eq!(b.iter().map(Vec::len).collect::<Vec<_>>(), [4, 4, 2]);
        let flat: Vec<Op> = b.concat();
        assert_eq!(
            flat,
            groups
                .iter()
                .flat_map(|g| g.ops.clone())
                .collect::<Vec<_>>()
        );
    }
}
