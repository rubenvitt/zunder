//! Ergonomic construction of operation batches.
//!
//! Every operation gets its own, strictly increasing HLC from the given
//! [`Clock`] and a fresh UUIDv7 `op_id`. A batch is meant to be pushed as a
//! whole (creating an entity = several field operations in one push).
//!
//! ```
//! use zunder_core::builder::{NewTask, OpBatch};
//! use zunder_core::hlc::{Clock, NodeId};
//! use zunder_core::model::TaskStatus;
//!
//! let mut clock = Clock::new(NodeId::new("server").unwrap());
//! let mut batch = OpBatch::new(&mut clock, 1_760_000_000_000);
//! let task = batch.create_task(NewTask::new("Zahnarzt anrufen"));
//! batch.set_status(task, TaskStatus::Done);
//! let ops = batch.into_ops();
//! assert_eq!(ops.len(), 5); // title, type, status, location, status
//! ```

use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::hlc::Clock;
use crate::ids;
use crate::model::{
    fields, format_rfc3339, AreaState, Due, Link, Location, Phoenix, ProjectKind, ProjectStatus,
    ProjectView, TaskStatus, TaskType,
};
use crate::op::{EntityKind, Op};
use crate::state::State;

/// Fields for a new area.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewArea {
    /// Explicit ID (e.g. deterministic import ID); a UUIDv7 otherwise.
    pub id: Option<Uuid>,
    pub name: String,
    pub state: AreaState,
    pub vault_path: Option<String>,
}

impl NewArea {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

/// Fields for a new project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProject {
    pub id: Option<Uuid>,
    pub name: String,
    pub area_id: Uuid,
    pub status: ProjectStatus,
    pub kind: ProjectKind,
    pub repo: Option<String>,
    pub description: Option<String>,
    /// `None` = default for the kind (not written).
    pub view: Option<ProjectView>,
    /// Prefix of the task references (`ZUN`).
    pub key: Option<String>,
}

impl NewProject {
    pub fn new(name: impl Into<String>, area_id: Uuid) -> Self {
        Self {
            id: None,
            name: name.into(),
            area_id,
            status: ProjectStatus::default(),
            kind: ProjectKind::default(),
            repo: None,
            description: None,
            view: None,
            key: None,
        }
    }
}

/// Fields for a new task. Unset optional fields are not written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewTask {
    pub id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub location: Location,
    pub parent_id: Option<Uuid>,
    pub due: Option<Due>,
    pub remind_at: Option<OffsetDateTime>,
    pub vault_links: Vec<String>,
    pub links: Vec<Link>,
    pub phoenix: Option<Phoenix>,
    pub plan: Option<String>,
    pub branch: Option<String>,
    pub pr_url: Option<String>,
    pub clickup_id: Option<String>,
    pub rank: Option<String>,
}

impl NewTask {
    /// A plain task in the inbox.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    /// A subtask of `parent_id` (its own location is irrelevant).
    pub fn subtask(title: impl Into<String>, parent_id: Uuid) -> Self {
        Self {
            parent_id: Some(parent_id),
            ..Self::new(title)
        }
    }

    pub fn at(mut self, location: Location) -> Self {
        self.location = location;
        self
    }

    pub fn with_type(mut self, task_type: TaskType) -> Self {
        self.task_type = task_type;
        self
    }

    pub fn with_status(mut self, status: TaskStatus) -> Self {
        self.status = status;
        self
    }
}

/// A batch of operations under construction.
#[derive(Debug)]
pub struct OpBatch<'c> {
    clock: &'c mut Clock,
    physical_ms: u64,
    ops: Vec<Op>,
}

impl<'c> OpBatch<'c> {
    /// Starts a batch; `physical_ms` is the current physical time (see
    /// [`crate::hlc::system_time_ms`]).
    pub fn new(clock: &'c mut Clock, physical_ms: u64) -> Self {
        Self {
            clock,
            physical_ms,
            ops: Vec::new(),
        }
    }

    /// Appends a raw field assignment. No validation happens here.
    pub fn set(
        &mut self,
        entity: EntityKind,
        entity_id: Uuid,
        field: &str,
        value: impl Into<Value>,
    ) -> &mut Self {
        let hlc = self.clock.now(self.physical_ms);
        self.ops.push(Op {
            op_id: ids::new_id(),
            entity,
            entity_id,
            field: field.to_owned(),
            value: value.into(),
            hlc,
        });
        self
    }

    pub fn set_area(&mut self, id: Uuid, field: &str, value: impl Into<Value>) -> &mut Self {
        self.set(EntityKind::Area, id, field, value)
    }

    pub fn set_project(&mut self, id: Uuid, field: &str, value: impl Into<Value>) -> &mut Self {
        self.set(EntityKind::Project, id, field, value)
    }

    pub fn set_task(&mut self, id: Uuid, field: &str, value: impl Into<Value>) -> &mut Self {
        self.set(EntityKind::Task, id, field, value)
    }

    /// Resets an optional field (`value: null`).
    pub fn clear(&mut self, entity: EntityKind, id: Uuid, field: &str) -> &mut Self {
        self.set(entity, id, field, Value::Null)
    }

    /// Creates an area; returns its ID.
    pub fn create_area(&mut self, new: NewArea) -> Uuid {
        use fields::area as f;
        let id = new.id.unwrap_or_else(ids::new_id);
        self.set_area(id, f::NAME, new.name);
        self.set_area(id, f::STATE, new.state);
        if let Some(path) = new.vault_path {
            self.set_area(id, f::VAULT_PATH, path);
        }
        id
    }

    /// Creates a project; returns its ID.
    pub fn create_project(&mut self, new: NewProject) -> Uuid {
        use fields::project as f;
        let id = new.id.unwrap_or_else(ids::new_id);
        self.set_project(id, f::NAME, new.name);
        self.set_project(id, f::AREA_ID, new.area_id.to_string());
        self.set_project(id, f::STATUS, new.status);
        self.set_project(id, f::KIND, new.kind);
        if let Some(repo) = new.repo {
            self.set_project(id, f::REPO, repo);
        }
        if let Some(description) = new.description {
            self.set_project(id, f::DESCRIPTION, description);
        }
        if let Some(view) = new.view {
            self.set_project(id, f::VIEW, view);
        }
        if let Some(key) = new.key {
            self.set_project(id, f::KEY, key);
        }
        id
    }

    /// Creates a task; returns its ID. `title`, `type`, `status` and
    /// `location` are always written, everything else only when set.
    pub fn create_task(&mut self, new: NewTask) -> Uuid {
        use fields::task as f;
        let id = new.id.unwrap_or_else(ids::new_id);
        self.set_task(id, f::TITLE, new.title);
        self.set_task(id, f::TYPE, new.task_type);
        self.set_task(id, f::STATUS, new.status);
        self.set_task(id, f::LOCATION, new.location);
        if let Some(parent) = new.parent_id {
            self.set_task(id, f::PARENT_ID, parent.to_string());
        }
        let optional_text = [
            (f::DESCRIPTION, new.description),
            (f::PLAN, new.plan),
            (f::BRANCH, new.branch),
            (f::PR_URL, new.pr_url),
            (f::CLICKUP_ID, new.clickup_id),
            (f::RANK, new.rank),
        ];
        for (field, value) in optional_text {
            if let Some(value) = value {
                self.set_task(id, field, value);
            }
        }
        if let Some(due) = new.due {
            self.set_task(id, f::DUE, due);
        }
        if let Some(remind_at) = new.remind_at {
            self.set_task(id, f::REMIND_AT, format_rfc3339(remind_at));
        }
        if !new.vault_links.is_empty() {
            self.set_task(id, f::VAULT_LINKS, new.vault_links);
        }
        if !new.links.is_empty() {
            let links = serde_json::to_value(new.links).expect("links serialize");
            self.set_task(id, f::LINKS, links);
        }
        if let Some(phoenix) = new.phoenix {
            self.set_task(id, f::PHOENIX, phoenix);
        }
        id
    }

    pub fn set_status(&mut self, task_id: Uuid, status: TaskStatus) -> &mut Self {
        self.set_task(task_id, fields::task::STATUS, status)
    }

    /// Moves a task to a location and detaches it from any parent
    /// (`location` + `parent_id: null`).
    pub fn move_task(&mut self, task_id: Uuid, location: Location) -> &mut Self {
        self.set_task(task_id, fields::task::LOCATION, location);
        self.clear(EntityKind::Task, task_id, fields::task::PARENT_ID)
    }

    /// Sets or clears the parent of a task.
    pub fn set_parent(&mut self, task_id: Uuid, parent_id: Option<Uuid>) -> &mut Self {
        let value = parent_id.map_or(Value::Null, |p| Value::String(p.to_string()));
        self.set_task(task_id, fields::task::PARENT_ID, value)
    }

    /// Tombstones a single entity.
    pub fn delete(&mut self, entity: EntityKind, id: Uuid) -> &mut Self {
        self.set(entity, id, fields::DELETED, true)
    }

    /// Tombstones an entity together with every live entity that depends on
    /// it, so the push passes validation: a task with all descendants; a
    /// project with all tasks located in it; an area with its projects (and
    /// their tasks) and the tasks located directly in it.
    pub fn delete_cascade(&mut self, state: &State, entity: EntityKind, id: Uuid) -> &mut Self {
        for (kind, dep) in cascade(state, entity, id) {
            self.delete(kind, dep);
        }
        self
    }

    /// The operations built so far.
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn into_ops(self) -> Vec<Op> {
        self.ops
    }
}

/// `(kind, id)` of an entity and all its live dependents, dependents last.
fn cascade(state: &State, entity: EntityKind, id: Uuid) -> Vec<(EntityKind, Uuid)> {
    let mut out = vec![(entity, id)];
    match entity {
        EntityKind::Task => {
            out.extend(
                state
                    .descendants(id)
                    .into_iter()
                    .map(|t| (EntityKind::Task, t)),
            );
        }
        EntityKind::Project => {
            out.extend(
                state
                    .live_tasks()
                    .filter(|t| state.effective_project(t.id) == Some(id))
                    .map(|t| (EntityKind::Task, t.id)),
            );
        }
        EntityKind::Area => {
            let projects: Vec<Uuid> = state
                .live_projects()
                .filter(|p| p.area_id == id)
                .map(|p| p.id)
                .collect();
            for project in projects {
                out.extend(cascade(state, EntityKind::Project, project));
            }
            for task in state.live_tasks() {
                if task.parent_id.is_none() && task.location == (Location::Area { id }) {
                    out.extend(cascade(state, EntityKind::Task, task.id));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hlc::NodeId;
    use crate::validate::validate_push;
    use serde_json::json;

    const NOW: u64 = 1_760_000_000_000;

    fn clock() -> Clock {
        Clock::new(NodeId::new("test").unwrap())
    }

    #[test]
    fn hlcs_strictly_increase_within_a_batch() {
        let mut clock = clock();
        let mut batch = OpBatch::new(&mut clock, NOW);
        batch.create_task(NewTask::new("A"));
        batch.create_task(NewTask::new("B"));
        let ops = batch.into_ops();
        assert!(ops.windows(2).all(|w| w[0].hlc < w[1].hlc));
        let ids: std::collections::HashSet<_> = ops.iter().map(|o| o.op_id).collect();
        assert_eq!(ids.len(), ops.len());
    }

    #[test]
    fn create_full_structure_passes_validation() {
        let mut clock = clock();
        let mut batch = OpBatch::new(&mut clock, NOW);
        let area = batch.create_area(NewArea::new("Arbeit"));
        let project = batch.create_project(NewProject {
            kind: ProjectKind::Dev,
            repo: Some("rubeen/zunder".into()),
            ..NewProject::new("Zunder", area)
        });
        let parent = batch.create_task(NewTask {
            due: Due::parse("2026-10-12"),
            remind_at: crate::model::parse_rfc3339("2026-10-12T09:00:00+02:00"),
            links: vec![Link {
                url: "https://example.org".into(),
                title: None,
            }],
            vault_links: vec!["Projekte/Zunder.md".into()],
            phoenix: Some(Phoenix {
                role: crate::model::PhoenixRole::Flamme,
                fire: crate::model::IsoWeek::parse("2026-W42").unwrap(),
            }),
            ..NewTask::new("Sync bauen")
                .at(Location::Project { id: project })
                .with_type(TaskType::Feature)
        });
        let child = batch.create_task(NewTask::subtask("HLC", parent).with_type(TaskType::Bug));
        let ops = batch.into_ops();
        let state = State::new();
        let outcome = validate_push(&state, &ops, NOW);
        assert!(outcome.is_fully_accepted(), "{:?}", outcome.rejected);
        let state = State::from_ops(&outcome.new_ops);
        let task = state.task(parent).unwrap();
        assert_eq!(task.title, "Sync bauen");
        assert_eq!(task.links.len(), 1);
        assert_eq!(state.effective_project(child), Some(project));
        assert_eq!(state.project(project).unwrap().view, ProjectView::Board);
    }

    #[test]
    fn move_task_detaches_from_parent() {
        let mut clock = clock();
        let mut batch = OpBatch::new(&mut clock, NOW);
        let parent = batch.create_task(NewTask::new("Eltern"));
        let child = batch.create_task(NewTask::subtask("Kind", parent));
        let mut state = State::from_ops(batch.ops());
        assert_eq!(state.children(parent), vec![child]);

        let mut batch = OpBatch::new(&mut clock, NOW + 1);
        batch.move_task(child, Location::Loose);
        for op in batch.ops() {
            state.apply(op);
        }
        assert!(state.children(parent).is_empty());
        assert_eq!(state.effective_location(child), Some(Location::Loose));
        assert_eq!(
            state
                .entity(EntityKind::Task, child)
                .unwrap()
                .field("parent_id")
                .unwrap()
                .value,
            json!(null)
        );
    }

    #[test]
    fn delete_cascade_covers_dependents_and_validates() {
        let mut clock = clock();
        let mut batch = OpBatch::new(&mut clock, NOW);
        let area = batch.create_area(NewArea::new("Haus"));
        let project = batch.create_project(NewProject::new("Umzug", area));
        let in_project =
            batch.create_task(NewTask::new("Kisten").at(Location::Project { id: project }));
        let sub = batch.create_task(NewTask::subtask("Klebeband", in_project));
        let in_area = batch.create_task(NewTask::new("Rasen").at(Location::Area { id: area }));
        let unrelated = batch.create_task(NewTask::new("Inbox"));
        let state = State::from_ops(batch.ops());

        let mut batch = OpBatch::new(&mut clock, NOW + 10);
        batch.delete(EntityKind::Area, area);
        let outcome = validate_push(&state, batch.ops(), NOW + 10);
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(
            outcome.rejected[0].code,
            crate::validate::RejectCode::HasDependents
        );

        let mut batch = OpBatch::new(&mut clock, NOW + 20);
        batch.delete_cascade(&state, EntityKind::Area, area);
        let deleted: std::collections::HashSet<Uuid> =
            batch.ops().iter().map(|o| o.entity_id).collect();
        assert_eq!(
            deleted,
            [area, project, in_project, sub, in_area]
                .into_iter()
                .collect()
        );
        assert!(!deleted.contains(&unrelated));
        let outcome = validate_push(&state, batch.ops(), NOW + 20);
        assert!(outcome.is_fully_accepted(), "{:?}", outcome.rejected);
    }
}
