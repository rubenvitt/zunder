//! From the desired state to operations: diff against the current Zunder
//! state, HLC assignment and push batches.

use serde_json::Value;
use uuid::Uuid;
use zunder_core::hlc::{Clock, NodeId};
use zunder_core::model::fields;
use zunder_core::{EntityKind, Op, OpBatch, State, TaskStatus, TaskType};

use crate::convert::{Desired, IMPORT_NODE};

const STATUS_FIELD: &str = fields::task::STATUS;
const TYPE_FIELD: &str = fields::task::TYPE;

/// Upper bound for the serialized operations of one push. The server accepts
/// request bodies up to 8 MiB; the rest is headroom for the envelope.
pub const MAX_PUSH_BYTES: usize = 7 * 1024 * 1024;

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
///
/// Push order: first the operations that only relax the dev rules on
/// existing tasks (a `type` or `status` that is allowed everywhere, e.g.
/// `bug` → `aufgabe`), then all other operations in the order of `desired`
/// (areas, projects, tasks with parents first). Rules that span entities
/// (a project becoming standard, a parent moving out of a dev project) only
/// hold once those tasks are downgraded, and the server checks every push
/// batch on its own.
///
/// Operations on existing entities use `clock` at `physical_ms`, so they win
/// against everything in `state`. A new entity with source timestamps
/// ([`Desired::created_ms`], [`Desired::closed_ms`]) is dated back instead:
/// its fields to the creation time and its status to the closing time
/// (never later than `physical_ms`). Nothing else has written to the ID of a
/// new entity, so the older HLCs cannot lose; `created_at` keeps the ClickUp
/// history and a task closed long ago is not shown as just closed.
pub fn plan_ops<'a>(
    desired: impl IntoIterator<Item = &'a Desired>,
    state: Option<&State>,
    clock: &mut Clock,
    physical_ms: u64,
) -> (Vec<EntityOps>, DiffStats) {
    let mut stats = DiffStats::default();
    let mut relaxing = Vec::new();
    let mut out = Vec::new();
    for entity in desired {
        let current = state.and_then(|s| s.entity(entity.kind, entity.id));
        if current.is_some_and(|e| e.is_deleted()) {
            stats.deleted_in_zunder += 1;
            continue;
        }
        let writes: Vec<(&str, &Value)> = entity
            .fields
            .iter()
            .filter(|(field, value)| {
                let have = current.and_then(|e| e.value(field));
                match (current, value) {
                    (None, Value::Null) => false,
                    (None, _) => true,
                    (Some(_), Value::Null) => have.is_some(),
                    (Some(_), v) => have != Some(v),
                }
            })
            .map(|(field, value)| (field.as_str(), value))
            .collect();
        match (current, writes.len()) {
            (None, _) => stats.created += 1,
            (Some(_), 0) => stats.unchanged += 1,
            (Some(_), _) => stats.updated += 1,
        }
        if writes.is_empty() {
            continue;
        }
        let created = entity.created_ms.or(entity.closed_ms);
        let ops = match created {
            Some(created_ms) if current.is_none() => {
                backdated_ops(entity, &writes, created_ms, physical_ms)
            }
            _ => {
                let mut batch = OpBatch::new(clock, physical_ms);
                for (field, value) in &writes {
                    batch.set(entity.kind, entity.id, field, (*value).clone());
                }
                batch.into_ops()
            }
        };
        let (early, ops): (Vec<Op>, Vec<Op>) = if current.is_some() {
            ops.into_iter().partition(relaxes_dev_rules)
        } else {
            (Vec::new(), ops)
        };
        for (target, ops) in [(&mut relaxing, early), (&mut out, ops)] {
            if !ops.is_empty() {
                target.push(EntityOps {
                    kind: entity.kind,
                    id: entity.id,
                    label: entity.label.clone(),
                    ops,
                });
            }
        }
    }
    relaxing.extend(out);
    (relaxing, stats)
}

/// A task `type` or `status` that is valid outside dev projects. Such an
/// operation never breaks a rule on its own, but may be what makes another
/// entity's change legal.
fn relaxes_dev_rules(op: &Op) -> bool {
    if op.entity != EntityKind::Task {
        return false;
    }
    match op.field.as_str() {
        TYPE_FIELD => {
            serde_json::from_value::<TaskType>(op.value.clone()).is_ok_and(|t| !t.is_dev_only())
        }
        STATUS_FIELD => {
            serde_json::from_value::<TaskStatus>(op.value.clone()).is_ok_and(|s| !s.is_dev_only())
        }
        _ => false,
    }
}

/// Operations of a new entity dated to its source timestamps: the status
/// at `closed_ms` (if any), everything else at `created_ms`, both capped at
/// `now_ms`. Uses a clock of its own, so the import clock stays ahead.
fn backdated_ops(
    entity: &Desired,
    writes: &[(&str, &Value)],
    created_ms: u64,
    now_ms: u64,
) -> Vec<Op> {
    let mut own = Clock::new(NodeId::new(IMPORT_NODE).expect("valid node"));
    let (status, rest): (Vec<_>, Vec<_>) = writes
        .iter()
        .partition(|(field, _)| entity.closed_ms.is_some() && *field == STATUS_FIELD);
    let mut ops = Vec::with_capacity(writes.len());
    for (fields, at) in [
        (rest, created_ms),
        (status, entity.closed_ms.unwrap_or(created_ms)),
    ] {
        let mut batch = OpBatch::new(&mut own, at.min(now_ms));
        for (field, value) in fields {
            batch.set(entity.kind, entity.id, field, (*value).clone());
        }
        ops.extend(batch.into_ops());
    }
    ops
}

/// Serialized size of a group's operations in a push body.
pub fn group_bytes(group: &EntityOps) -> usize {
    group
        .ops
        .iter()
        .map(|op| serde_json::to_vec(op).map_or(0, |v| v.len()) + 1)
        .sum()
}

/// Splits entity groups into push batches of at most `max_ops` operations
/// and `max_bytes` serialized bytes (a single entity larger than that gets
/// its own batch, which the caller must not send if it exceeds `max_bytes`;
/// see [`group_bytes`]). Order is preserved, so areas and
/// projects precede tasks and parents precede subtasks.
pub fn batches(groups: &[EntityOps], max_ops: usize, max_bytes: usize) -> Vec<Vec<Op>> {
    let max_ops = max_ops.max(1);
    let mut out: Vec<Vec<Op>> = Vec::new();
    let mut current: Vec<Op> = Vec::new();
    let mut current_bytes = 0;
    for group in groups {
        let bytes = group_bytes(group);
        if !current.is_empty()
            && (current.len() + group.ops.len() > max_ops || current_bytes + bytes > max_bytes)
        {
            out.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current.extend(group.ops.iter().cloned());
        current_bytes += bytes;
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
            created_ms: None,
            closed_ms: None,
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

    const DAY: u64 = 24 * 60 * 60 * 1000;

    #[test]
    fn new_tasks_are_dated_to_their_clickup_history() {
        let now = 1_791_676_800_000;
        let task = |id: u128, status: &str, created: u64, closed: Option<u64>| Desired {
            created_ms: Some(created),
            closed_ms: closed,
            ..desired(
                id,
                &[
                    ("title", json!("x")),
                    ("location", json!({"kind": "inbox"})),
                    ("status", json!(status)),
                ],
            )
        };
        let old = task(1, "done", now - 30 * DAY, Some(now - 10 * DAY));
        let recent = task(2, "dropped", now - 30 * DAY, Some(now - DAY));
        let open = task(3, "open", now - 5 * DAY, None);
        let future = task(4, "done", now + 5 * DAY, Some(now + 6 * DAY));
        let mut clock = import_clock(None);
        let (groups, _) = plan_ops([&old, &recent, &open, &future], None, &mut clock, now);
        let ops: Vec<Op> = groups.iter().flat_map(|g| g.ops.clone()).collect();
        let state = State::from_ops(&ops);

        // Closed ten days ago: hidden from the default views right away.
        assert!(!state.is_visible_by_default(old.id, now));
        assert_eq!(
            state.status_changed_at(old.id).unwrap().wall_ms(),
            now - 10 * DAY
        );
        assert_eq!(
            state
                .created_at(EntityKind::Task, old.id)
                .unwrap()
                .wall_ms(),
            now - 30 * DAY
        );
        // Closed yesterday: still visible for the rest of the 3 days.
        assert!(state.is_visible_by_default(recent.id, now));
        assert!(!state.is_visible_by_default(recent.id, now + 2 * DAY));
        assert!(state.is_visible_by_default(open.id, now));
        assert_eq!(
            state
                .created_at(EntityKind::Task, open.id)
                .unwrap()
                .wall_ms(),
            now - 5 * DAY
        );
        // Timestamps from the future are capped at the import time.
        assert!(groups
            .iter()
            .flat_map(|g| &g.ops)
            .all(|op| op.hlc.wall_ms() <= now));

        // Updates of existing entities use the import clock again, so they
        // win against what is in Zunder.
        let reopened = Desired {
            fields: vec![("status".into(), json!("open"))],
            ..old.clone()
        };
        let mut clock = import_clock(Some(&state));
        let (groups, stats) = plan_ops([&reopened], Some(&state), &mut clock, now);
        assert_eq!(stats.updated, 1);
        assert_eq!(groups[0].ops[0].hlc.wall_ms(), now);
    }

    #[test]
    fn batches_respect_the_byte_limit() {
        let mut clock = import_clock(None);
        let text = "x".repeat(1000);
        let ds: Vec<Desired> = (0..6)
            .map(|i| desired(i, &[("title", json!("t")), ("description", json!(text))]))
            .collect();
        let (groups, _) = plan_ops(&ds, None, &mut clock, 1);
        let size = group_bytes(&groups[0]);
        assert!(size > 1000);
        // Far below the op limit, but only two groups fit into the bytes.
        let b = batches(&groups, 500, 2 * size + size / 2);
        assert_eq!(b.iter().map(Vec::len).collect::<Vec<_>>(), [4, 4, 4]);
        // A group larger than the limit still gets a batch of its own.
        let b = batches(&groups, 500, size / 2);
        assert_eq!(b.len(), 6);
    }

    #[test]
    fn relaxing_changes_of_existing_tasks_come_first() {
        let project = Desired {
            kind: EntityKind::Project,
            ..desired(100, &[("kind", json!("standard"))])
        };
        let task = desired(
            1,
            &[
                ("title", json!("B")),
                ("type", json!("aufgabe")),
                ("status", json!("in_progress")),
            ],
        );
        let mut web = Clock::new(NodeId::new("web").unwrap());
        let mut b = OpBatch::new(&mut web, 1_000);
        b.set(EntityKind::Project, project.id, "kind", "dev");
        b.set_task(task.id, "title", "A");
        b.set_task(task.id, "type", "bug");
        b.set_task(task.id, "status", "review");
        let state = State::from_ops(b.ops());
        let new_task = desired(2, &[("title", json!("C")), ("type", json!("aufgabe"))]);

        let mut clock = import_clock(Some(&state));
        let (groups, _) = plan_ops(
            [&project, &task, &new_task],
            Some(&state),
            &mut clock,
            2_000,
        );
        let order: Vec<(u128, Vec<&str>)> = groups
            .iter()
            .map(|g| {
                (
                    g.id.as_u128(),
                    g.ops.iter().map(|o| o.field.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            order,
            [
                (1, vec!["type", "status"]),
                (100, vec!["kind"]),
                (1, vec!["title"]),
                (2, vec!["title", "type"]),
            ]
        );
    }

    #[test]
    fn batches_never_split_an_entity() {
        let mut clock = import_clock(None);
        let ds: Vec<Desired> = (0..5)
            .map(|i| desired(i, &[("title", json!("x")), ("status", json!("open"))]))
            .collect();
        let (groups, _) = plan_ops(&ds, None, &mut clock, 1);
        let b = batches(&groups, 3, MAX_PUSH_BYTES);
        assert_eq!(b.iter().map(Vec::len).collect::<Vec<_>>(), [2, 2, 2, 2, 2]);
        let b = batches(&groups, 4, MAX_PUSH_BYTES);
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
