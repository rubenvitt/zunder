//! Push validation (contract section 2.5).
//!
//! [`validate_push`] checks a batch of operations against the current state
//! without mutating it. The caller persists [`PushOutcome::new_ops`] and then
//! applies them to its state.
//!
//! Procedure:
//!
//! 1. Operations whose `op_id` is already known (or repeated within the push)
//!    are accepted without being applied again.
//! 2. Per-operation checks: HLC more than 24 h in the future, unknown field,
//!    invalid value, entity ID already used by another kind.
//! 3. The remaining operations are applied to a scratch copy of the state.
//!    Every touched entity and every affected neighbour (projects and tasks
//!    of a touched area, tasks of a touched project, descendants of a touched
//!    task) is checked. Violations are attributed to the touched entity that
//!    caused them; all operations of that entity in this push are rejected.
//!    This repeats until no further entity is rejected, because rejecting one
//!    entity can invalidate another (e.g. a task created inside a rejected
//!    project).
//! 4. Warnings are computed on the final scratch state.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{fields, validate_field_value, Location};
use crate::op::{EntityKind, Op};
use crate::state::{Entity, State};

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
    /// A reference (`location`, `area_id`, `parent_id`) points to a missing,
    /// tombstoned or wrong-kind entity.
    InvalidReference,
    /// Deleting the entity would leave live entities referencing it.
    HasDependents,
    /// `parent_id` would create a cycle.
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
    #[serde(with = "crate::ids::canonical")]
    pub op_id: Uuid,
    pub code: RejectCode,
    /// German, user-facing.
    pub message: String,
}

/// A non-blocking hint about an entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    #[serde(with = "crate::ids::canonical")]
    pub entity_id: Uuid,
    pub code: WarningCode,
    /// German, user-facing.
    pub message: String,
}

/// Result of [`validate_push`]. Serializes to the `accepted`, `rejected` and
/// `warnings` keys of the push response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PushOutcome {
    /// All accepted op IDs in push order, including already-known ones.
    pub accepted: Vec<Uuid>,
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

type Key = (EntityKind, Uuid);

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

/// Validates a push against `state` at physical time `now_ms`.
pub fn validate_push(state: &State, ops: &[Op], now_ms: u64) -> PushOutcome {
    let mut verdicts: Vec<Option<Violation>> = vec![None; ops.len()];

    // 1. Already known op IDs (also repeats within the push).
    let mut seen = HashSet::new();
    let duplicate: Vec<bool> = ops
        .iter()
        .map(|op| state.knows_op(&op.op_id) || !seen.insert(op.op_id))
        .collect();

    // Group the new operations by entity, keeping first-seen order.
    let mut groups: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    let mut kinds_in_push: HashMap<Uuid, BTreeSet<EntityKind>> = HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        if duplicate[i] {
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
            if let Some(v) = check_op(state, &ops[i], now_ms, &kinds_in_push) {
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

    // 3. Entity checks until stable.
    let scratch = loop {
        let mut scratch = state.clone();
        let mut order: Vec<usize> = candidates.values().flatten().copied().collect();
        order.sort_unstable();
        for i in order {
            scratch.apply(&ops[i]);
        }
        let changed: HashMap<Key, HashSet<&str>> = candidates
            .iter()
            .map(|(key, indices)| {
                (
                    *key,
                    indices.iter().map(|&i| ops[i].field.as_str()).collect(),
                )
            })
            .collect();
        let blamed = check_entities(state, &scratch, &changed);
        if blamed.is_empty() {
            break scratch;
        }
        for (key, violation) in blamed {
            if let Some(indices) = candidates.remove(&key) {
                for i in indices {
                    verdicts[i] = Some(violation.clone());
                }
            }
        }
    };

    // 4. Collect results and warnings.
    let mut outcome = PushOutcome::default();
    for (i, op) in ops.iter().enumerate() {
        match verdicts[i].take() {
            Some(v) => outcome.rejected.push(Rejection {
                op_id: op.op_id,
                code: v.code,
                message: v.message,
            }),
            None => {
                outcome.accepted.push(op.op_id);
                if !duplicate[i] {
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

fn this_dative(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Area => "diesem Bereich",
        EntityKind::Project => "diesem Projekt",
        EntityKind::Task => "dieser Aufgabe",
    }
}

fn check_op(
    state: &State,
    op: &Op,
    now_ms: u64,
    kinds_in_push: &HashMap<Uuid, BTreeSet<EntityKind>>,
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
                "Die ID gehört bereits zu einem {} und kann nicht als {} verwendet werden.",
                other.label_de(),
                op.entity.label_de()
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

/// Checks touched entities and their neighbours in `scratch`; returns the
/// touched entities to reject, each with the reason.
fn check_entities(
    before: &State,
    scratch: &State,
    changed: &HashMap<Key, HashSet<&str>>,
) -> BTreeMap<Key, Violation> {
    let mut checker = Checker {
        before,
        scratch,
        changed,
        blamed: BTreeMap::new(),
    };
    for key in neighbourhood(scratch, changed) {
        checker.check(key);
    }
    checker.blamed
}

/// Touched entities plus every live entity whose validity may depend on them.
fn neighbourhood(scratch: &State, changed: &HashMap<Key, HashSet<&str>>) -> BTreeSet<Key> {
    let mut keys: BTreeSet<Key> = changed.keys().copied().collect();
    let touched_areas: HashSet<Uuid> = touched_of(changed, EntityKind::Area);
    let touched_projects: HashSet<Uuid> = touched_of(changed, EntityKind::Project);

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
    for id in touched_of(changed, EntityKind::Task) {
        for d in scratch.descendants(id) {
            keys.insert((EntityKind::Task, d));
        }
    }
    keys
}

fn touched_of(changed: &HashMap<Key, HashSet<&str>>, kind: EntityKind) -> HashSet<Uuid> {
    changed
        .keys()
        .filter(|(k, _)| *k == kind)
        .map(|(_, id)| *id)
        .collect()
}

struct Checker<'a> {
    before: &'a State,
    scratch: &'a State,
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
                            "Pflichtfeld „{field}“ fehlt für {} {}.",
                            indefinite_article(key.0),
                            key.0.label_de()
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

    /// Whether `task_id` lies on a parent cycle in the scratch state.
    fn in_cycle(&self, task_id: Uuid) -> bool {
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

fn task_project_target(state: &State, task_id: Uuid) -> Option<(EntityKind, Uuid)> {
    state.effective_location(task_id).and_then(Location::target)
}

fn definite_article(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Area => "Der",
        EntityKind::Project => "Das",
        EntityKind::Task => "Die",
    }
}

fn indefinite_article(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Area => "einen",
        EntityKind::Project => "ein",
        EntityKind::Task => "eine",
    }
}

fn dependent_phrase(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Area => "noch ein Bereich",
        EntityKind::Project => "noch ein Projekt",
        EntityKind::Task => "noch eine Aufgabe",
    }
}

#[cfg(test)]
mod tests;
