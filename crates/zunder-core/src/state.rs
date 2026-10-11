//! Materialized state: a last-writer-wins register per (entity, field).
//!
//! Merging is commutative, associative and idempotent: the winner of a field
//! is the entry with the greatest `(hlc, op_id)`; should even those be equal
//! (a malformed duplicate), a total order on the JSON value decides, so the
//! result never depends on the order in which operations arrive.
//!
//! The merge itself never validates; see [`crate::validate`].

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::hlc::Hlc;
use crate::model::{
    fields, Area, AreaState, Location, Project, ProjectKind, ProjectStatus, ProjectView, Task,
    TaskStatus, TaskType,
};
use crate::op::{EntityKind, Op};

/// Done/dropped tasks stay in the default views for this long after their
/// status changed (3 days).
pub const CLOSED_VISIBLE_MS: u64 = 3 * 24 * 60 * 60 * 1000;

/// The winning value of one field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldEntry {
    #[serde(deserialize_with = "crate::op::present_value")]
    pub value: Value,
    pub hlc: Hlc,
    #[serde(with = "crate::ids::canonical")]
    pub op_id: Uuid,
}

impl FieldEntry {
    /// Total LWW order: `(hlc, op_id)`, then the value as a last resort.
    fn lww_cmp(&self, other: &FieldEntry) -> Ordering {
        self.hlc
            .cmp(&other.hlc)
            .then_with(|| self.op_id.cmp(&other.op_id))
            .then_with(|| cmp_json(&self.value, &other.value))
    }
}

/// One entity with all its field registers.
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    kind: EntityKind,
    id: Uuid,
    fields: BTreeMap<String, FieldEntry>,
    /// Smallest HLC of any operation ever applied to this entity.
    created: Hlc,
}

/// Result of [`Entity::merge`].
struct Merged {
    /// Field winner replaced or creation HLC lowered.
    changed: bool,
    /// For a task whose winning `parent_id` changed: old and new parent.
    parent_move: Option<(Option<Uuid>, Option<Uuid>)>,
}

impl Entity {
    fn new(kind: EntityKind, id: Uuid, created: Hlc) -> Self {
        Entity {
            kind,
            id,
            fields: BTreeMap::new(),
            created,
        }
    }

    /// Merges one field register (LWW) and lowers the creation HLC.
    fn merge(&mut self, field: &str, entry: FieldEntry, created: &Hlc) -> Merged {
        let mut changed = false;
        if *created < self.created {
            self.created = created.clone();
            changed = true;
        }
        let wins = self
            .fields
            .get(field)
            .is_none_or(|current| entry.lww_cmp(current).is_gt());
        if !wins {
            return Merged {
                changed,
                parent_move: None,
            };
        }
        let tracks_parent = self.kind == EntityKind::Task && field == fields::task::PARENT_ID;
        let old_parent = self.parent_id();
        self.fields.insert(field.to_owned(), entry);
        let new_parent = self.parent_id();
        Merged {
            changed: true,
            parent_move: (tracks_parent && old_parent != new_parent)
                .then_some((old_parent, new_parent)),
        }
    }

    pub fn kind(&self) -> EntityKind {
        self.kind
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    /// All field registers (including those reset to `null`).
    pub fn fields(&self) -> &BTreeMap<String, FieldEntry> {
        &self.fields
    }

    pub fn field(&self, name: &str) -> Option<&FieldEntry> {
        self.fields.get(name)
    }

    /// The winning value of a field, `None` if unset or reset to `null`.
    pub fn value(&self, name: &str) -> Option<&Value> {
        self.fields
            .get(name)
            .map(|e| &e.value)
            .filter(|v| !v.is_null())
    }

    pub fn str_value(&self, name: &str) -> Option<&str> {
        self.value(name).and_then(Value::as_str)
    }

    fn string(&self, name: &str) -> Option<String> {
        self.str_value(name).map(str::to_owned)
    }

    fn parsed<T: DeserializeOwned>(&self, name: &str) -> Option<T> {
        self.value(name)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    fn code<T: std::str::FromStr + Default>(&self, name: &str) -> T {
        self.str_value(name)
            .and_then(|s| s.parse().ok())
            .unwrap_or_default()
    }

    fn id_value(&self, name: &str) -> Option<Uuid> {
        self.str_value(name).and_then(crate::ids::parse_canonical)
    }

    /// This entity in the snapshot format.
    pub fn to_snapshot_entity(&self) -> SnapshotEntity {
        SnapshotEntity {
            entity: self.kind,
            entity_id: self.id,
            created_hlc: Some(self.created.clone()),
            fields: self.fields.clone(),
        }
    }

    /// Tombstoned (`deleted: true`).
    pub fn is_deleted(&self) -> bool {
        self.value(fields::DELETED) == Some(&Value::Bool(true))
    }

    /// Derived `created_at`: the smallest HLC of the entity.
    pub fn created_at(&self) -> &Hlc {
        &self.created
    }

    /// Derived `updated_at`: the largest HLC of the entity.
    pub fn updated_at(&self) -> &Hlc {
        self.fields
            .values()
            .map(|e| &e.hlc)
            .max()
            .unwrap_or(&self.created)
    }

    /// The task's own `parent_id` (only meaningful for tasks).
    pub fn parent_id(&self) -> Option<Uuid> {
        if self.kind == EntityKind::Task {
            self.id_value(fields::task::PARENT_ID)
        } else {
            None
        }
    }

    /// The task's own `location` (default inbox; invalid values read as inbox).
    pub fn own_location(&self) -> Location {
        self.parsed(fields::task::LOCATION).unwrap_or_default()
    }

    pub fn task_type(&self) -> TaskType {
        self.code(fields::task::TYPE)
    }

    pub fn task_status(&self) -> TaskStatus {
        self.code(fields::task::STATUS)
    }

    pub fn project_kind(&self) -> ProjectKind {
        self.code(fields::project::KIND)
    }

    pub fn area_id(&self) -> Option<Uuid> {
        if self.kind == EntityKind::Project {
            self.id_value(fields::project::AREA_ID)
        } else {
            None
        }
    }

    fn to_area(&self) -> Option<Area> {
        use fields::area as f;
        Some(Area {
            id: self.id,
            name: self.string(f::NAME)?,
            state: self.code::<AreaState>(f::STATE),
            vault_path: self.string(f::VAULT_PATH),
            deleted: self.is_deleted(),
        })
    }

    fn to_project(&self) -> Option<Project> {
        use fields::project as f;
        let kind = self.project_kind();
        Some(Project {
            id: self.id,
            name: self.string(f::NAME)?,
            area_id: self.area_id()?,
            status: self.code::<ProjectStatus>(f::STATUS),
            kind,
            repo: self.string(f::REPO),
            description: self.string(f::DESCRIPTION),
            view: self
                .str_value(f::VIEW)
                .and_then(|s| s.parse().ok())
                .unwrap_or(ProjectView::default_for(kind)),
            deleted: self.is_deleted(),
        })
    }

    fn to_task(&self) -> Option<Task> {
        use fields::task as f;
        Some(Task {
            id: self.id,
            title: self.string(f::TITLE)?,
            description: self.string(f::DESCRIPTION),
            task_type: self.task_type(),
            status: self.task_status(),
            location: self.own_location(),
            parent_id: self.parent_id(),
            due: self.parsed(f::DUE),
            remind_at: self
                .str_value(f::REMIND_AT)
                .and_then(crate::model::parse_rfc3339),
            vault_links: self.parsed(f::VAULT_LINKS).unwrap_or_default(),
            links: self.parsed(f::LINKS).unwrap_or_default(),
            phoenix: self.parsed(f::PHOENIX),
            claimed_by: self.string(f::CLAIMED_BY),
            claim_expires_at: self
                .str_value(f::CLAIM_EXPIRES_AT)
                .and_then(crate::model::parse_rfc3339),
            plan: self.string(f::PLAN),
            branch: self.string(f::BRANCH),
            pr_url: self.string(f::PR_URL),
            timing_ref: self.string(f::TIMING_REF),
            flame_started_at: self
                .str_value(f::FLAME_STARTED_AT)
                .and_then(crate::model::parse_rfc3339),
            flame_ended_at: self
                .str_value(f::FLAME_ENDED_AT)
                .and_then(crate::model::parse_rfc3339),
            clickup_id: self.string(f::CLICKUP_ID),
            rank: self.string(f::RANK),
            deleted: self.is_deleted(),
        })
    }
}

/// Subtask progress: `done` of `total` direct, live subtasks; dropped
/// subtasks count towards neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// One entity in the snapshot format of `GET /api/snapshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotEntity {
    pub entity: EntityKind,
    #[serde(with = "crate::ids::canonical")]
    pub entity_id: Uuid,
    /// Smallest HLC of any operation on the entity (derived `created_at`).
    /// Optional when reading; defaults to the smallest field HLC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_hlc: Option<Hlc>,
    pub fields: BTreeMap<String, FieldEntry>,
}

/// The materialized state of all entities.
#[derive(Debug, Clone, Default)]
pub struct State {
    entities: BTreeMap<(EntityKind, Uuid), Entity>,
    /// Derived index: parent task -> tasks whose winning `parent_id` is it.
    children: HashMap<Uuid, BTreeSet<Uuid>>,
    /// Every op_id applied so far (winners and losers).
    known_ops: HashSet<Uuid>,
}

/// Two states are equal when their entities (all registers and derived
/// creation HLCs) are equal. Indexes and the set of known op IDs are not
/// compared.
impl PartialEq for State {
    fn eq(&self, other: &Self) -> bool {
        self.entities == other.entities
    }
}

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a state by applying all operations.
    pub fn from_ops<'a>(ops: impl IntoIterator<Item = &'a Op>) -> Self {
        let mut state = Self::new();
        for op in ops {
            state.apply(op);
        }
        state
    }

    /// Merges one operation. Returns whether anything observable changed
    /// (entity created, field winner replaced or creation HLC lowered).
    pub fn apply(&mut self, op: &Op) -> bool {
        self.known_ops.insert(op.op_id);
        let entry = FieldEntry {
            value: op.value.clone(),
            hlc: op.hlc.clone(),
            op_id: op.op_id,
        };
        self.merge_entry(op.entity, op.entity_id, &op.field, entry, &op.hlc)
    }

    fn merge_entry(
        &mut self,
        kind: EntityKind,
        id: Uuid,
        field: &str,
        entry: FieldEntry,
        created: &Hlc,
    ) -> bool {
        let mut created_now = false;
        let entity = self.entities.entry((kind, id)).or_insert_with(|| {
            created_now = true;
            Entity::new(kind, id, created.clone())
        });
        let merged = entity.merge(field, entry, created);
        if let Some((old, new)) = merged.parent_move {
            if let Some(old) = old {
                if let Some(set) = self.children.get_mut(&old) {
                    set.remove(&id);
                    if set.is_empty() {
                        self.children.remove(&old);
                    }
                }
            }
            if let Some(new) = new {
                self.children.entry(new).or_default().insert(id);
            }
        }
        created_now || merged.changed
    }

    /// Whether an operation with this ID has been applied (or marked known).
    pub fn knows_op(&self, op_id: &Uuid) -> bool {
        self.known_ops.contains(op_id)
    }

    /// Records an op ID as known without applying anything (e.g. when the
    /// state was loaded from a snapshot and the full op log lives elsewhere).
    pub fn mark_op_known(&mut self, op_id: Uuid) {
        self.known_ops.insert(op_id);
    }

    // ----- raw access -------------------------------------------------------

    pub fn entity(&self, kind: EntityKind, id: Uuid) -> Option<&Entity> {
        self.entities.get(&(kind, id))
    }

    /// The entity if it exists and is not tombstoned.
    pub fn live_entity(&self, kind: EntityKind, id: Uuid) -> Option<&Entity> {
        Lookup::live_entity(self, kind, id)
    }

    /// All entities (including tombstoned ones) ordered by (kind, id).
    pub fn entities(&self) -> impl Iterator<Item = &Entity> {
        self.entities.values()
    }

    /// All entities of one kind (including tombstoned ones).
    pub fn entities_of(&self, kind: EntityKind) -> impl Iterator<Item = &Entity> {
        self.entities
            .range((kind, Uuid::nil())..=(kind, Uuid::max()))
            .map(|(_, e)| e)
    }

    /// Kinds under which an ID exists (normally at most one).
    pub fn kinds_of(&self, id: Uuid) -> impl Iterator<Item = EntityKind> + '_ {
        EntityKind::ALL
            .into_iter()
            .filter(move |k| self.entities.contains_key(&(*k, id)))
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    // ----- typed views ------------------------------------------------------

    /// Typed area view. `None` if missing or without a name.
    pub fn area(&self, id: Uuid) -> Option<Area> {
        self.entity(EntityKind::Area, id)?.to_area()
    }

    /// Typed project view. `None` if missing or without name/area.
    pub fn project(&self, id: Uuid) -> Option<Project> {
        self.entity(EntityKind::Project, id)?.to_project()
    }

    /// Typed task view. `None` if missing or without a title.
    pub fn task(&self, id: Uuid) -> Option<Task> {
        self.entity(EntityKind::Task, id)?.to_task()
    }

    /// All areas, including tombstoned ones.
    pub fn areas(&self) -> impl Iterator<Item = Area> + '_ {
        self.entities_of(EntityKind::Area)
            .filter_map(Entity::to_area)
    }

    /// All projects, including tombstoned ones.
    pub fn projects(&self) -> impl Iterator<Item = Project> + '_ {
        self.entities_of(EntityKind::Project)
            .filter_map(Entity::to_project)
    }

    /// All tasks, including tombstoned ones.
    pub fn tasks(&self) -> impl Iterator<Item = Task> + '_ {
        self.entities_of(EntityKind::Task)
            .filter_map(Entity::to_task)
    }

    pub fn live_areas(&self) -> impl Iterator<Item = Area> + '_ {
        self.areas().filter(|a| !a.deleted)
    }

    pub fn live_projects(&self) -> impl Iterator<Item = Project> + '_ {
        self.projects().filter(|p| !p.deleted)
    }

    pub fn live_tasks(&self) -> impl Iterator<Item = Task> + '_ {
        self.tasks().filter(|t| !t.deleted)
    }

    /// Finds a live task by its `clickup_id`.
    pub fn task_by_clickup_id(&self, clickup_id: &str) -> Option<Task> {
        self.entities_of(EntityKind::Task)
            .find(|e| !e.is_deleted() && e.str_value(fields::task::CLICKUP_ID) == Some(clickup_id))
            .and_then(Entity::to_task)
    }

    // ----- derived data -----------------------------------------------------

    pub fn created_at(&self, kind: EntityKind, id: Uuid) -> Option<&Hlc> {
        self.entity(kind, id).map(Entity::created_at)
    }

    pub fn updated_at(&self, kind: EntityKind, id: Uuid) -> Option<&Hlc> {
        self.entity(kind, id).map(Entity::updated_at)
    }

    /// HLC of the winning `status` operation; the creation HLC if the status
    /// was never set (default `open`).
    pub fn status_changed_at(&self, task_id: Uuid) -> Option<&Hlc> {
        let e = self.entity(EntityKind::Task, task_id)?;
        Some(
            e.field(fields::task::STATUS)
                .map(|s| &s.hlc)
                .unwrap_or(&e.created),
        )
    }

    /// Root of a task's parent chain. Stops at a missing parent; `None` if
    /// the task is missing or the chain contains a cycle.
    pub fn root_task(&self, task_id: Uuid) -> Option<Uuid> {
        Lookup::root_task(self, task_id)
    }

    /// Effective location of a task: the location of its root task (the own
    /// location of subtasks is ignored). `None` if missing or cyclic.
    pub fn effective_location(&self, task_id: Uuid) -> Option<Location> {
        Lookup::effective_location(self, task_id)
    }

    /// The project a task effectively belongs to, if any.
    pub fn effective_project(&self, task_id: Uuid) -> Option<Uuid> {
        match self.effective_location(task_id)? {
            Location::Project { id } => Some(id),
            _ => None,
        }
    }

    /// Whether a location is a live dev project.
    pub fn is_dev_location(&self, location: Location) -> bool {
        Lookup::is_dev_location(self, location)
    }

    /// IDs of all tasks whose winning `parent_id` is `task_id` (including
    /// tombstoned ones), ordered by ID.
    pub fn child_ids(&self, task_id: Uuid) -> impl Iterator<Item = Uuid> + '_ {
        self.children.get(&task_id).into_iter().flatten().copied()
    }

    /// Live direct subtasks of a task, ordered by ID.
    pub fn children(&self, task_id: Uuid) -> Vec<Uuid> {
        Lookup::children(self, task_id)
    }

    /// All live descendants (depth first, cycle safe), excluding the task.
    pub fn descendants(&self, task_id: Uuid) -> Vec<Uuid> {
        Lookup::descendants(self, task_id)
    }

    /// Progress of the direct, live subtasks (dropped ones excluded).
    pub fn progress(&self, task_id: Uuid) -> Progress {
        Lookup::progress(self, task_id)
    }

    /// Number of live direct subtasks that are neither done nor dropped.
    pub fn open_subtask_count(&self, task_id: Uuid) -> usize {
        Lookup::open_subtask_count(self, task_id)
    }

    /// Whether a task appears in the default views at `now_ms`: tombstoned
    /// tasks never; done/dropped tasks only until [`CLOSED_VISIBLE_MS`] after
    /// `status_changed_at`. Search ignores this rule.
    pub fn is_visible_by_default(&self, task_id: Uuid, now_ms: u64) -> bool {
        let Some(e) = self.live_entity(EntityKind::Task, task_id) else {
            return false;
        };
        if !e.task_status().is_closed() {
            return true;
        }
        let changed = self.status_changed_at(task_id).map_or(0, Hlc::wall_ms);
        now_ms < changed.saturating_add(CLOSED_VISIBLE_MS)
    }

    // ----- snapshots --------------------------------------------------------

    /// Exports all entities in the snapshot format (ordered by kind, id).
    pub fn to_snapshot(&self) -> Vec<SnapshotEntity> {
        self.entities
            .values()
            .map(Entity::to_snapshot_entity)
            .collect()
    }

    /// Merges snapshot entities into this state (LWW per field, so merging a
    /// snapshot is as safe as applying the operations it summarizes). The
    /// winning op IDs are recorded as known.
    pub fn merge_snapshot<'a>(&mut self, entities: impl IntoIterator<Item = &'a SnapshotEntity>) {
        for se in entities {
            let min_field = se.fields.values().map(|f| &f.hlc).min();
            let created = match (&se.created_hlc, min_field) {
                (Some(c), Some(m)) => Some(c.min(m)),
                (Some(c), None) => Some(c),
                (None, m) => m,
            };
            let Some(created) = created.cloned() else {
                continue; // an entity without fields and creation HLC carries no data
            };
            if se.fields.is_empty() {
                self.entities
                    .entry((se.entity, se.entity_id))
                    .and_modify(|e| {
                        if created < e.created {
                            e.created = created.clone();
                        }
                    })
                    .or_insert_with(|| Entity::new(se.entity, se.entity_id, created.clone()));
            }
            for (field, entry) in &se.fields {
                self.known_ops.insert(entry.op_id);
                self.merge_entry(se.entity, se.entity_id, field, entry.clone(), &created);
            }
        }
    }

    /// Builds a state from snapshot entities.
    pub fn from_snapshot<'a>(entities: impl IntoIterator<Item = &'a SnapshotEntity>) -> Self {
        let mut state = Self::new();
        state.merge_snapshot(entities);
        state
    }
}

/// Read access to entities and the child index. Implemented by [`State`] and
/// by the validation [`Overlay`], so derived data is computed the same way on
/// both.
pub(crate) trait Lookup {
    fn entity(&self, kind: EntityKind, id: Uuid) -> Option<&Entity>;

    /// Tasks whose winning `parent_id` is `parent` (including tombstoned
    /// ones).
    fn child_set(&self, parent: Uuid) -> Option<&BTreeSet<Uuid>>;

    fn live_entity(&self, kind: EntityKind, id: Uuid) -> Option<&Entity> {
        self.entity(kind, id).filter(|e| !e.is_deleted())
    }

    fn root_task(&self, task_id: Uuid) -> Option<Uuid> {
        let mut current = self.entity(EntityKind::Task, task_id)?;
        let mut seen = HashSet::from([task_id]);
        while let Some(parent) = current
            .parent_id()
            .and_then(|p| self.entity(EntityKind::Task, p))
        {
            if !seen.insert(parent.id) {
                return None;
            }
            current = parent;
        }
        Some(current.id)
    }

    fn effective_location(&self, task_id: Uuid) -> Option<Location> {
        let root = self.root_task(task_id)?;
        Some(self.entity(EntityKind::Task, root)?.own_location())
    }

    fn is_dev_location(&self, location: Location) -> bool {
        match location {
            Location::Project { id } => self
                .live_entity(EntityKind::Project, id)
                .is_some_and(|p| p.project_kind() == ProjectKind::Dev),
            _ => false,
        }
    }

    fn children(&self, task_id: Uuid) -> Vec<Uuid> {
        self.child_set(task_id)
            .into_iter()
            .flatten()
            .copied()
            .filter(|c| self.live_entity(EntityKind::Task, *c).is_some())
            .collect()
    }

    fn descendants(&self, task_id: Uuid) -> Vec<Uuid> {
        let mut out = Vec::new();
        let mut seen = HashSet::from([task_id]);
        let mut stack: Vec<Uuid> = self.children(task_id).into_iter().rev().collect();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            out.push(id);
            stack.extend(self.children(id).into_iter().rev());
        }
        out
    }

    fn progress(&self, task_id: Uuid) -> Progress {
        let mut progress = Progress::default();
        for child in self.children(task_id) {
            let Some(e) = self.entity(EntityKind::Task, child) else {
                continue;
            };
            match e.task_status() {
                TaskStatus::Dropped => {}
                TaskStatus::Done => {
                    progress.done += 1;
                    progress.total += 1;
                }
                _ => progress.total += 1,
            }
        }
        progress
    }

    fn open_subtask_count(&self, task_id: Uuid) -> usize {
        let p = self.progress(task_id);
        p.total - p.done
    }
}

impl Lookup for State {
    fn entity(&self, kind: EntityKind, id: Uuid) -> Option<&Entity> {
        self.entities.get(&(kind, id))
    }

    fn child_set(&self, parent: Uuid) -> Option<&BTreeSet<Uuid>> {
        self.children.get(&parent)
    }
}

/// A total order on JSON values (used only to break exact `(hlc, op_id)` ties
/// deterministically).
pub fn cmp_json(a: &Value, b: &Value) -> Ordering {
    fn rank(v: &Value) -> u8 {
        match v {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Number(_) => 2,
            Value::String(_) => 3,
            Value::Array(_) => 4,
            Value::Object(_) => 5,
        }
    }
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Number(x), Value::Number(y)) => x.to_string().cmp(&y.to_string()),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Array(x), Value::Array(y)) => {
            for (i, j) in x.iter().zip(y) {
                let o = cmp_json(i, j);
                if o.is_ne() {
                    return o;
                }
            }
            x.len().cmp(&y.len())
        }
        (Value::Object(x), Value::Object(y)) => {
            let mut xs: Vec<_> = x.iter().collect();
            let mut ys: Vec<_> = y.iter().collect();
            xs.sort_by(|p, q| p.0.cmp(q.0));
            ys.sort_by(|p, q| p.0.cmp(q.0));
            for ((kx, vx), (ky, vy)) in xs.iter().zip(&ys) {
                let o = kx.cmp(ky).then_with(|| cmp_json(vx, vy));
                if o.is_ne() {
                    return o;
                }
            }
            xs.len().cmp(&ys.len())
        }
        _ => rank(a).cmp(&rank(b)),
    }
}

mod overlay;
pub(crate) use overlay::Overlay;

#[cfg(test)]
mod tests;
