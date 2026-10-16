//! Push validation (contract section 2.5).
//!
//! [`validate_push`] checks a batch of operations against the current state
//! without mutating it. The caller persists [`PushOutcome::new_ops`] and then
//! applies them to its state.
//!
//! Procedure:
//!
//! 1. Operations whose `op_id` is already known are accepted without being
//!    applied again. A repeat of an op ID within the push shares the outcome
//!    of its first copy (accepted only if that copy is accepted).
//! 2. Per-operation checks: HLC more than 24 h in the future, unknown field,
//!    invalid value, entity ID already used by another kind.
//! 3. The remaining operations are applied to a copy-on-write overlay of the
//!    state (only touched entities are copied). Every touched entity and
//!    every affected neighbour (projects and tasks of a touched area, tasks
//!    of a touched project, descendants of a touched task) is checked.
//!    Violations are attributed to the touched entity that caused them; all
//!    operations of that entity in this push are rejected. Entities that
//!    reference a rejected entity which only exists (or is only live)
//!    through this push are rejected along with it in the same pass, however
//!    deep the chain. Because rejecting an entity can invalidate others in
//!    further ways, the neighbourhood of the rejected entities is checked
//!    again until no further entity is rejected.
//! 4. Warnings are computed on the final scratch state.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::ids::Id;
use serde::{Deserialize, Serialize};

use crate::model::{fields, validate_field_value, Location};
use crate::op::{EntityKind, Op};
use crate::state::{Entity, Lookup, Overlay, State};

/// Operations whose `wall_ms` lies further in the future are rejected.
pub const MAX_FUTURE_MS: u64 = 24 * 60 * 60 * 1000;

/// Stable, machine-readable rejection codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectCode {
    /// The field does not exist for this entity kind.
    UnknownField,
    /// Wrong type or invalid value.
    InvalidValue,
    /// The entity lacks a required field after the push.
    MissingRequired,
    /// `bug`/`feature`/`review` outside a dev project, `repo` on a standard
    /// project, or moving such a task out of a dev project.
    DevOnly,
    /// A project change (kind → standard) or a move of a parent task would
    /// leave `bug`/`feature`/`review` tasks outside a dev project.
    DevTasksPresent,
    /// A reference (`location`, `area_id`, `parent_id`, a new entry of
    /// `blocked_by`) points to a missing, tombstoned or wrong-kind entity.
    InvalidReference,
    /// Deleting the entity would leave live entities referencing it.
    HasDependents,
    /// `parent_id` or `blocked_by` would create a cycle.
    Cycle,
    /// `wall_ms` of the HLC is more than 24 h in the future.
    FutureHlc,
    /// The entity ID is already used by an entity of another kind.
    KindMismatch,
    /// Rejected because another operation for the same entity in this push
    /// was rejected.
    EntityRejected,
}

impl RejectCode {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectCode::UnknownField => "unknown_field",
            RejectCode::InvalidValue => "invalid_value",
            RejectCode::MissingRequired => "missing_required",
            RejectCode::DevOnly => "dev_only",
            RejectCode::DevTasksPresent => "dev_tasks_present",
            RejectCode::InvalidReference => "invalid_reference",
            RejectCode::HasDependents => "has_dependents",
            RejectCode::Cycle => "cycle",
            RejectCode::FutureHlc => "future_hlc",
            RejectCode::KindMismatch => "kind_mismatch",
            RejectCode::EntityRejected => "entity_rejected",
        }
    }
}

/// Stable, machine-readable warning codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    /// A task was set to `done` while it still has open subtasks.
    OpenSubtasks,
}

impl WarningCode {
    pub fn as_str(self) -> &'static str {
        match self {
            WarningCode::OpenSubtasks => "open_subtasks",
        }
    }
}

/// A rejected operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    pub op_id: Id,
    pub code: RejectCode,
    /// German, user-facing.
    pub message: String,
}

/// A non-blocking hint about an entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    pub entity_id: Id,
    pub code: WarningCode,
    /// German, user-facing.
    pub message: String,
}

/// Result of [`validate_push`]. Serializes to the `accepted`, `rejected` and
/// `warnings` keys of the push response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PushOutcome {
    /// All accepted op IDs in push order, including already-known ones.
    pub accepted: Vec<Id>,
    /// Rejected operations in push order.
    pub rejected: Vec<Rejection>,
    pub warnings: Vec<Warning>,
    /// Accepted operations that are new (not known before), in push order.
    /// These must be persisted and applied by the caller.
    #[serde(skip)]
    pub new_ops: Vec<Op>,
}

impl PushOutcome {
    pub fn is_fully_accepted(&self) -> bool {
        self.rejected.is_empty()
    }
}

type Key = (EntityKind, Id);

#[derive(Debug, Clone)]
struct Violation {
    code: RejectCode,
    message: String,
}

impl Violation {
    fn new(code: RejectCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Who writes a push. Some fields are reserved for the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    /// Clients, agents and tools: everything but the reserved fields.
    Client,
    /// The server's own bookkeeping (task references, see
    /// [`crate::numbering`]; `recurrence_prev` of the next instance of a
    /// series).
    Server,
}

/// Validates a client push against `state` at physical time `now_ms`.
pub fn validate_push(state: &State, ops: &[Op], now_ms: u64) -> PushOutcome {
    validate_push_as(state, ops, now_ms, Author::Client)
}

/// Validates a push by `author` against `state` at physical time `now_ms`.
pub fn validate_push_as(state: &State, ops: &[Op], now_ms: u64, author: Author) -> PushOutcome {
    let mut verdicts: Vec<Option<Violation>> = vec![None; ops.len()];

    // 1. Already known op IDs and repeats within the push. Neither is
    //    validated again; a repeat shares the outcome of its first copy.
    let known: Vec<bool> = ops.iter().map(|op| state.knows_op(&op.op_id)).collect();
    let mut first_copy: HashMap<Id, usize> = HashMap::new();
    let repeat_of: Vec<Option<usize>> = ops
        .iter()
        .enumerate()
        .map(|(i, op)| match first_copy.entry(op.op_id) {
            Entry::Occupied(first) => Some(*first.get()),
            Entry::Vacant(slot) => {
                slot.insert(i);
                None
            }
        })
        .collect();

    // Group the new operations by entity, keeping first-seen order.
    let mut groups: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    let mut kinds_in_push: HashMap<Id, BTreeSet<EntityKind>> = HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        if known[i] || repeat_of[i].is_some() {
            continue;
        }
        groups.entry((op.entity, op.entity_id)).or_default().push(i);
        kinds_in_push
            .entry(op.entity_id)
            .or_default()
            .insert(op.entity);
    }

    // 2. Per-operation checks.
    let mut failed: BTreeSet<Key> = BTreeSet::new();
    for (key, indices) in &groups {
        let mut first_failure: Option<String> = None;
        for &i in indices {
            if let Some(v) = check_op(state, &ops[i], now_ms, &kinds_in_push, author) {
                first_failure.get_or_insert_with(|| v.message.clone());
                verdicts[i] = Some(v);
            }
        }
        if let Some(cause) = first_failure {
            failed.insert(*key);
            for &i in indices {
                if verdicts[i].is_none() {
                    verdicts[i] = Some(Violation::new(
                        RejectCode::EntityRejected,
                        format!(
                            "Abgelehnt, weil eine andere Änderung an {} ungültig ist: {cause}",
                            this_dative(key.0)
                        ),
                    ));
                }
            }
        }
    }
    let mut candidates: BTreeMap<Key, Vec<usize>> = groups
        .into_iter()
        .filter(|(key, _)| !failed.contains(key))
        .collect();

    // 3. Entity checks until stable, on an overlay of the state that copies
    //    only the touched entities. The first pass checks the neighbourhood
    //    of every candidate. Rejected entities are reverted in the overlay;
    //    candidates that depend on them by reference are rejected right away
    //    (in one pass, however deep the chain), and the next pass re-checks
    //    only the neighbourhood of what was rejected.
    let mut scratch = Overlay::new(state);
    let mut order: Vec<usize> = candidates.values().flatten().copied().collect();
    order.sort_unstable();
    for i in order {
        scratch.apply(&ops[i]);
    }
    let mut seeds: BTreeSet<Key> = candidates.keys().copied().collect();
    while !seeds.is_empty() {
        let changed: HashMap<Key, HashSet<&str>> = candidates
            .iter()
            .map(|(key, indices)| {
                (
                    *key,
                    indices.iter().map(|&i| ops[i].field.as_str()).collect(),
                )
            })
            .collect();
        let blamed = check_entities(state, &scratch, &changed, &seeds);
        let mut rejected: Vec<(Key, String)> = Vec::new();
        for (key, violation) in blamed {
            if let Some(indices) = candidates.remove(&key) {
                for i in indices {
                    verdicts[i] = Some(violation.clone());
                }
                scratch.revert(key);
                rejected.push((key, violation.message));
            }
        }
        seeds = reject_dependents(&mut scratch, &mut candidates, &mut verdicts, rejected);
    }

    // 4. Collect results and warnings.
    let mut outcome = PushOutcome::default();
    for (i, op) in ops.iter().enumerate() {
        let verdict = match repeat_of[i] {
            Some(first) if !known[i] => &verdicts[first],
            _ => &verdicts[i],
        };
        match verdict {
            Some(v) => outcome.rejected.push(Rejection {
                op_id: op.op_id,
                code: v.code,
                message: v.message.clone(),
            }),
            None => {
                outcome.accepted.push(op.op_id);
                if !known[i] && repeat_of[i].is_none() {
                    outcome.new_ops.push(op.clone());
                }
            }
        }
    }
    let mut warned = HashSet::new();
    for op in &outcome.new_ops {
        let sets_done = op.entity == EntityKind::Task
            && op.field == fields::task::STATUS
            && op.value.as_str() == Some("done");
        if !sets_done || !warned.insert(op.entity_id) {
            continue;
        }
        let Some(task) = scratch.live_entity(EntityKind::Task, op.entity_id) else {
            continue;
        };
        if task.task_status() != crate::model::TaskStatus::Done {
            continue;
        }
        let open = scratch.open_subtask_count(op.entity_id);
        if open > 0 {
            outcome.warnings.push(Warning {
                entity_id: op.entity_id,
                code: WarningCode::OpenSubtasks,
                message: if open == 1 {
                    "Aufgabe ist erledigt, hat aber noch 1 offene Unteraufgabe.".to_owned()
                } else {
                    format!("Aufgabe ist erledigt, hat aber noch {open} offene Unteraufgaben.")
                },
            });
        }
    }
    outcome
}

/// The entity a live entity references and depends on: a task's parent, or
/// the location target of a root task; a project's area; a comment's task.
fn reference_of(scratch: &Overlay<'_>, key: Key) -> Option<Key> {
    let entity = scratch.live_entity(key.0, key.1)?;
    match key.0 {
        EntityKind::Area => None,
        EntityKind::Project => entity.area_id().map(|a| (EntityKind::Area, a)),
        EntityKind::Task => match entity.parent_id() {
            Some(parent) => Some((EntityKind::Task, parent)),
            None => entity.own_location().target(),
        },
        EntityKind::Comment => entity.comment_task_id().map(|t| (EntityKind::Task, t)),
    }
}

/// Rejects every remaining candidate whose reference leads (directly or
/// through other rejected candidates) to a rejected entity that is not live
/// without its operations, e.g. the subtasks of a rejected new task. Takes
/// the entities rejected in this round with their cause; returns them plus
/// the dependents rejected here, whose neighbourhood must be re-checked.
fn reject_dependents(
    scratch: &mut Overlay<'_>,
    candidates: &mut BTreeMap<Key, Vec<usize>>,
    verdicts: &mut [Option<Violation>],
    rejected: Vec<(Key, String)>,
) -> BTreeSet<Key> {
    let mut removed: BTreeSet<Key> = rejected.iter().map(|(key, _)| *key).collect();
    if rejected.is_empty() {
        return removed;
    }
    let mut dependents: HashMap<Key, Vec<Key>> = HashMap::new();
    for &key in candidates.keys() {
        if let Some(target) = reference_of(scratch, key) {
            dependents.entry(target).or_default().push(key);
        }
    }
    // (rejected entity, root cause of its rejection)
    let mut stack = rejected;
    while let Some((target, cause)) = stack.pop() {
        if scratch.live_entity(target.0, target.1).is_some() {
            continue; // still valid without the rejected operations
        }
        for &key in dependents.get(&target).into_iter().flatten() {
            let Some(indices) = candidates.remove(&key) else {
                continue;
            };
            let what = match (key.0, target.0) {
                (EntityKind::Task, EntityKind::Task) => "die übergeordnete Aufgabe",
                (_, EntityKind::Task) => "die Aufgabe",
                (_, EntityKind::Project) => "das Projekt",
                _ => "der Bereich",
            };
            let violation = Violation::new(
                RejectCode::InvalidReference,
                format!("Abgelehnt, weil {what} in diesem Push abgelehnt wurde: {cause}"),
            );
            for i in indices {
                verdicts[i] = Some(violation.clone());
            }
            scratch.revert(key);
            removed.insert(key);
            stack.push((key, cause.clone()));
        }
    }
    removed
}

fn this_dative(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Area => "diesem Bereich",
        EntityKind::Project => "diesem Projekt",
        EntityKind::Task => "dieser Aufgabe",
        EntityKind::Comment => "diesem Kommentar",
    }
}

fn check_op(
    state: &State,
    op: &Op,
    now_ms: u64,
    kinds_in_push: &HashMap<Id, BTreeSet<EntityKind>>,
    author: Author,
) -> Option<Violation> {
    if op.hlc.wall_ms() > now_ms.saturating_add(MAX_FUTURE_MS) {
        return Some(Violation::new(
            RejectCode::FutureHlc,
            "Der Zeitstempel liegt mehr als 24 Stunden in der Zukunft. Bitte die Uhr des Geräts prüfen.",
        ));
    }
    if let Err(e) = validate_field_value(op.entity, &op.field, &op.value) {
        return Some(Violation::new(e.code, e.message));
    }
    if author == Author::Client && op.entity == EntityKind::Task {
        let reserved = match op.field.as_str() {
            fields::task::REF => Some(
                "Die Kennung („ref“) vergibt der Server, sobald die Aufgabe in einem Projekt mit Schlüssel liegt.",
            ),
            fields::task::RECURRENCE_PREV => Some(
                "Die Vorgängerin in einer Serie („recurrence_prev“) setzt nur der Server, wenn er die nächste Instanz anlegt.",
            ),
            _ => None,
        };
        if let Some(message) = reserved {
            return Some(Violation::new(RejectCode::InvalidValue, message));
        }
    }
    let other_in_state = state.kinds_of(op.entity_id).find(|k| *k != op.entity);
    let ambiguous_in_push = other_in_state.is_none()
        && state.kinds_of(op.entity_id).next().is_none()
        && kinds_in_push
            .get(&op.entity_id)
            .is_some_and(|kinds| kinds.len() > 1);
    if let Some(other) = other_in_state {
        return Some(Violation::new(
            RejectCode::KindMismatch,
            format!(
                "Die ID ist bereits als {} vergeben und kann nicht für {} verwendet werden.",
                other.label_de(),
                indefinite_accusative(op.entity)
            ),
        ));
    }
    if ambiguous_in_push {
        return Some(Violation::new(
            RejectCode::KindMismatch,
            "Dieselbe ID wird in diesem Push für verschiedene Entitätsarten verwendet.",
        ));
    }
    None
}

/// Checks the entities in the neighbourhood of `seeds` in `scratch`; returns
/// the touched entities (keys of `changed`) to reject, each with the reason.
fn check_entities(
    before: &State,
    scratch: &Overlay<'_>,
    changed: &HashMap<Key, HashSet<&str>>,
    seeds: &BTreeSet<Key>,
) -> BTreeMap<Key, Violation> {
    #[cfg(test)]
    tests::CHECK_PASSES.with(|n| n.set(n.get() + 1));
    let mut checker = Checker {
        before,
        scratch,
        changed,
        blamed: BTreeMap::new(),
    };
    for key in neighbourhood(scratch, seeds) {
        checker.check(key);
    }
    checker.blamed
}

/// `seeds` plus every live entity whose validity may depend on them.
fn neighbourhood(scratch: &Overlay<'_>, seeds: &BTreeSet<Key>) -> BTreeSet<Key> {
    let mut keys: BTreeSet<Key> = seeds.clone();
    let touched_areas: HashSet<Id> = seeds_of(seeds, EntityKind::Area);
    let touched_projects: HashSet<Id> = seeds_of(seeds, EntityKind::Project);

    if !touched_areas.is_empty() {
        for p in scratch.entities_of(EntityKind::Project) {
            if !p.is_deleted() && p.area_id().is_some_and(|a| touched_areas.contains(&a)) {
                keys.insert((EntityKind::Project, p.id()));
            }
        }
    }
    if !touched_areas.is_empty() || !touched_projects.is_empty() {
        for t in scratch.entities_of(EntityKind::Task) {
            if t.is_deleted() {
                continue;
            }
            let in_area = t.parent_id().is_none()
                && matches!(t.own_location(), Location::Area { id } if touched_areas.contains(&id));
            let in_project = !touched_projects.is_empty()
                && matches!(
                    scratch.effective_location(t.id()),
                    Some(Location::Project { id }) if touched_projects.contains(&id)
                );
            if in_area || in_project {
                keys.insert((EntityKind::Task, t.id()));
            }
        }
    }
    // Live descendants of every seed task, in one traversal.
    let mut stack: Vec<Id> = seeds_of(seeds, EntityKind::Task).into_iter().collect();
    let mut seen: HashSet<Id> = stack.iter().copied().collect();
    while let Some(id) = stack.pop() {
        for child in scratch.children(id) {
            if seen.insert(child) {
                keys.insert((EntityKind::Task, child));
                stack.push(child);
            }
        }
    }
    keys
}

fn seeds_of(seeds: &BTreeSet<Key>, kind: EntityKind) -> HashSet<Id> {
    seeds
        .iter()
        .filter(|(k, _)| *k == kind)
        .map(|(_, id)| *id)
        .collect()
}

struct Checker<'a> {
    before: &'a State,
    scratch: &'a Overlay<'a>,
    changed: &'a HashMap<Key, HashSet<&'a str>>,
    blamed: BTreeMap<Key, Violation>,
}

impl Checker<'_> {
    fn touched(&self, key: Key) -> bool {
        self.changed.contains_key(&key)
    }

    /// Whether the push changed one of `names` on `key`. Restoring an entity
    /// (`deleted` changed and it is live again) counts as changing every
    /// field: all its rules apply anew.
    fn changed_any(&self, key: Key, names: &[&str]) -> bool {
        self.changed.get(&key).is_some_and(|set| {
            names.iter().any(|n| set.contains(n))
                || (set.contains(fields::DELETED)
                    && self.scratch.live_entity(key.0, key.1).is_some())
        })
    }

    fn blame(&mut self, key: Key, code: RejectCode, message: impl Into<String>) {
        if self.touched(key) {
            self.blamed
                .entry(key)
                .or_insert_with(|| Violation::new(code, message));
        }
    }

    fn check(&mut self, key: Key) {
        let Some(entity) = self.scratch.entity(key.0, key.1) else {
            return;
        };
        if entity.is_deleted() {
            return;
        }
        if self.touched(key) {
            for field in fields::required(key.0) {
                if entity.value(field).is_none() {
                    self.blame(
                        key,
                        RejectCode::MissingRequired,
                        format!(
                            "Pflichtfeld „{field}“ fehlt für {}.",
                            indefinite_accusative(key.0)
                        ),
                    );
                    return;
                }
            }
        }
        match key.0 {
            EntityKind::Area => {}
            EntityKind::Project => self.check_project(entity),
            EntityKind::Task => self.check_task(entity),
            EntityKind::Comment => self.check_comment(entity),
        }
    }

    /// A reference from `key` (via `via_fields`) to `target` that does not
    /// resolve to a live entity of the right kind.
    fn broken_reference(&mut self, key: Key, via_fields: &[&str], target: Key) {
        if self.scratch.live_entity(target.0, target.1).is_some() {
            return;
        }
        if self.changed_any(key, via_fields) {
            let message = if self.scratch.entity(target.0, target.1).is_some() {
                format!(
                    "{} {} ist gelöscht.",
                    definite_article(target.0),
                    target.0.label_de()
                )
            } else {
                format!(
                    "{} {} existiert nicht.",
                    definite_article(target.0),
                    target.0.label_de()
                )
            };
            self.blame(key, RejectCode::InvalidReference, message);
        } else if self.touched(target) {
            self.blame(
                target,
                RejectCode::HasDependents,
                format!(
                    "{} {} kann nicht gelöscht werden, solange {} darauf verweist. Bitte zuerst verschieben oder mitlöschen.",
                    definite_article(target.0),
                    target.0.label_de(),
                    dependent_phrase(key.0)
                ),
            );
        } else {
            self.blame(
                key,
                RejectCode::InvalidReference,
                format!(
                    "{} referenzierte {} existiert nicht mehr.",
                    definite_article(target.0),
                    target.0.label_de()
                ),
            );
        }
    }

    fn check_project(&mut self, project: &Entity) {
        use fields::project as f;
        let key = (EntityKind::Project, project.id());
        if let Some(area) = project.area_id() {
            self.broken_reference(
                key,
                &[f::AREA_ID, fields::DELETED],
                (EntityKind::Area, area),
            );
        }
        if let Some(own) = project.str_value(f::KEY) {
            if self.changed_any(key, &[f::KEY]) {
                let holder = self.scratch.entities_of(EntityKind::Project).find(|p| {
                    p.id() != project.id() && !p.is_deleted() && p.str_value(f::KEY) == Some(own)
                });
                if let Some(holder) = holder {
                    let name = holder.str_value(f::NAME).unwrap_or_default().to_owned();
                    self.blame(
                        key,
                        RejectCode::InvalidValue,
                        format!("Der Schlüssel „{own}“ gehört schon zum Projekt „{name}“."),
                    );
                }
            }
        }
        if project.value(f::REPO).is_some()
            && project.project_kind() != crate::model::ProjectKind::Dev
            && self.changed_any(key, &[f::REPO, f::KIND])
        {
            self.blame(
                key,
                RejectCode::DevOnly,
                "Ein Repo ist nur bei Dev-Projekten erlaubt. Bitte „repo“ zurücksetzen oder die Art „dev“ wählen.",
            );
        }
    }

    fn check_task(&mut self, task: &Entity) {
        use fields::task as f;
        let key = (EntityKind::Task, task.id());
        let reference_fields = [f::PARENT_ID, f::LOCATION, fields::DELETED];

        if let Some(parent) = task.parent_id() {
            let parent_key = (EntityKind::Task, parent);
            self.broken_reference(key, &reference_fields, parent_key);
            if self.in_cycle(task.id()) && self.changed_any(key, &[f::PARENT_ID]) {
                self.blame(
                    key,
                    RejectCode::Cycle,
                    "Die Aufgabe kann nicht unter sich selbst oder eine ihrer Unteraufgaben gehängt werden.",
                );
                return;
            }
        } else if let Some(target) = task.own_location().target() {
            self.broken_reference(key, &reference_fields, target);
        }
        if self.changed_any(key, &[f::BLOCKED_BY]) {
            self.check_blocked_by(task);
        }
        if task.value(f::RECURRENCE).is_some()
            && self.changed_any(key, &[f::RECURRENCE, f::DUE, f::PARENT_ID])
        {
            self.check_recurring(task);
        }

        let dev_only_type = task.task_type().is_dev_only();
        let dev_only_status = task.task_status().is_dev_only();
        if !(dev_only_type || dev_only_status) {
            return;
        }
        let Some(effective) = self.scratch.effective_location(task.id()) else {
            return; // cyclic chain; reported via the cycle check
        };
        if self.scratch.is_dev_location(effective) {
            return;
        }
        let what = if dev_only_type {
            format!("Typ „{}“", task.task_type())
        } else {
            "Status „review“".to_owned()
        };
        let is_root = task.parent_id().is_none();
        let own_cause = self.changed_any(key, &[f::TYPE, f::STATUS, f::PARENT_ID, fields::DELETED])
            || (is_root && self.changed_any(key, &[f::LOCATION]));
        if own_cause {
            let was_dev = self
                .before
                .effective_location(task.id())
                .is_some_and(|l| self.before.is_dev_location(l));
            let message = if was_dev {
                format!(
                    "Eine Aufgabe mit {what} darf nicht aus einem Dev-Projekt heraus verschoben werden."
                )
            } else {
                format!("{what} ist nur in Dev-Projekten erlaubt.")
            };
            self.blame(key, RejectCode::DevOnly, message);
            return;
        }

        let title = task.str_value(f::TITLE).unwrap_or_default().to_owned();
        let mut responsible = false;
        // Ancestors that were moved in this push.
        let mut seen = HashSet::from([task.id()]);
        let mut current = task.parent_id();
        while let Some(id) = current {
            if !seen.insert(id) {
                break;
            }
            let ancestor_key = (EntityKind::Task, id);
            let Some(ancestor) = self.scratch.entity(EntityKind::Task, id) else {
                break;
            };
            let ancestor_is_root = ancestor.parent_id().is_none();
            if self.changed_any(ancestor_key, &[f::PARENT_ID])
                || (ancestor_is_root && self.changed_any(ancestor_key, &[f::LOCATION]))
            {
                responsible = true;
                self.blame(
                    ancestor_key,
                    RejectCode::DevTasksPresent,
                    format!(
                        "Unteraufgabe „{title}“ hat {what} und darf nicht aus einem Dev-Projekt heraus verschoben werden."
                    ),
                );
            }
            current = ancestor.parent_id();
        }
        // The project whose kind changed. A deleted project is reported by
        // the reference check of the root task (has_dependents).
        if let Some((EntityKind::Project, pid)) = task_project_target(self.scratch, task.id()) {
            let project_key = (EntityKind::Project, pid);
            if self.changed_any(project_key, &[fields::DELETED])
                && self.scratch.live_entity(EntityKind::Project, pid).is_none()
            {
                responsible = true;
            } else if self.changed_any(project_key, &[fields::project::KIND]) {
                responsible = true;
                self.blame(
                    project_key,
                    RejectCode::DevTasksPresent,
                    format!(
                        "Das Projekt enthält noch die Aufgabe „{title}“ mit {what} und kann deshalb kein Standard-Projekt werden."
                    ),
                );
            }
        }
        if !responsible && self.touched(key) {
            self.blame(
                key,
                RejectCode::DevOnly,
                format!("{what} ist nur in Dev-Projekten erlaubt."),
            );
        }
    }

    /// Blockers newly added to `blocked_by` must be other live tasks that do
    /// not (directly or transitively) wait for this task themselves. Entries
    /// that were already there are kept as they are: deleting a blocker is
    /// allowed (it simply no longer blocks), and an unrelated edit must not
    /// fail because of it.
    fn check_blocked_by(&mut self, task: &Entity) {
        let key = (EntityKind::Task, task.id());
        let before: HashSet<Id> = self
            .before
            .entity(EntityKind::Task, task.id())
            .map(|e| e.blocked_by().into_iter().collect())
            .unwrap_or_default();
        for blocker in task.blocked_by() {
            if before.contains(&blocker) {
                continue;
            }
            if blocker == task.id() {
                self.blame(
                    key,
                    RejectCode::InvalidValue,
                    "Eine Aufgabe kann nicht von sich selbst blockiert werden.",
                );
                return;
            }
            let Some(entity) = self.scratch.live_entity(EntityKind::Task, blocker) else {
                let message = if self.scratch.entity(EntityKind::Task, blocker).is_some() {
                    "Die blockierende Aufgabe ist gelöscht."
                } else {
                    "Die blockierende Aufgabe existiert nicht."
                };
                self.blame(key, RejectCode::InvalidReference, message);
                return;
            };
            if self.waits_for(blocker, task.id()) {
                let title = entity.str_value(fields::task::TITLE).unwrap_or_default();
                self.blame(
                    key,
                    RejectCode::Cycle,
                    format!(
                        "„{title}“ wartet selbst (direkt oder über andere Aufgaben) auf diese Aufgabe. Zwei Aufgaben können sich nicht gegenseitig blockieren."
                    ),
                );
                return;
            }
        }
    }

    /// A recurring task needs a due date and must be a root task. Checked
    /// when the push sets the recurrence, the due date or the parent (or
    /// restores the task); sending `recurrence: null` along lifts both rules.
    /// A missing due date is explained by what the push did: setting the
    /// recurrence asks for a due date, removing the due date (or restoring
    /// the task) for removing the recurrence.
    fn check_recurring(&mut self, task: &Entity) {
        use fields::task as f;
        let key = (EntityKind::Task, task.id());
        if task.value(f::DUE).is_none() {
            let sets_recurrence = self
                .changed
                .get(&key)
                .is_some_and(|set| set.contains(f::RECURRENCE));
            let message = if sets_recurrence {
                "Ein Rhythmus braucht eine Fälligkeit. Bitte zusammen mit dem Rhythmus eine Fälligkeit setzen."
            } else {
                "Eine wiederkehrende Aufgabe braucht eine Fälligkeit. Bitte zuerst den Rhythmus entfernen."
            };
            self.blame(key, RejectCode::InvalidValue, message);
        } else if task.value(f::PARENT_ID).is_some() {
            self.blame(
                key,
                RejectCode::InvalidValue,
                "Nur Aufgaben ohne übergeordnete Aufgabe können wiederkehren.",
            );
        }
    }

    /// Whether live task `from` waits for `target` along `blocked_by`
    /// (through live tasks only) in the scratch state.
    fn waits_for(&self, from: Id, target: Id) -> bool {
        let mut seen = HashSet::from([from]);
        let mut stack = vec![from];
        while let Some(id) = stack.pop() {
            let Some(entity) = self.scratch.live_entity(EntityKind::Task, id) else {
                continue;
            };
            for next in entity.blocked_by() {
                if next == target {
                    return true;
                }
                if seen.insert(next) {
                    stack.push(next);
                }
            }
        }
        false
    }

    /// A comment needs a live task when it is written to one (created, moved
    /// or restored). Comments do not block deleting their task: they are
    /// hidden with it and come back when it is restored.
    fn check_comment(&mut self, comment: &Entity) {
        let key = (EntityKind::Comment, comment.id());
        let via = [fields::comment::TASK_ID, fields::DELETED];
        if let Some(task) = comment.comment_task_id() {
            if self.changed_any(key, &via) {
                self.broken_reference(key, &via, (EntityKind::Task, task));
            }
        }
    }

    /// Whether `task_id` lies on a parent cycle in the scratch state.
    fn in_cycle(&self, task_id: Id) -> bool {
        let mut seen = HashSet::new();
        let mut current = self
            .scratch
            .entity(EntityKind::Task, task_id)
            .and_then(Entity::parent_id);
        while let Some(id) = current {
            if id == task_id {
                return true;
            }
            if !seen.insert(id) {
                return false;
            }
            current = self
                .scratch
                .entity(EntityKind::Task, id)
                .and_then(Entity::parent_id);
        }
        false
    }
}

fn task_project_target(state: &Overlay<'_>, task_id: Id) -> Option<(EntityKind, Id)> {
    state.effective_location(task_id).and_then(Location::target)
}

fn definite_article(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Area => "Der",
        EntityKind::Project => "Das",
        EntityKind::Task => "Die",
        EntityKind::Comment => "Der",
    }
}

fn indefinite_accusative(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Area => "einen Bereich",
        EntityKind::Project => "ein Projekt",
        EntityKind::Task => "eine Aufgabe",
        EntityKind::Comment => "einen Kommentar",
    }
}

fn dependent_phrase(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Area => "noch ein Bereich",
        EntityKind::Project => "noch ein Projekt",
        EntityKind::Task => "noch eine Aufgabe",
        EntityKind::Comment => "noch ein Kommentar",
    }
}

#[cfg(test)]
mod tests;
