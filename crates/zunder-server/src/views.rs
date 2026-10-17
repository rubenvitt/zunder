//! Representations of tasks, comments, areas and projects for MCP tools and the flame
//! endpoints, plus resolving task addresses (`task_id` as UUID or reference
//! like `ZUN-42`, or `clickup_id`).
//!
//! Views are computed from the materialized state: the effective location
//! with resolved names, derived timestamps (`created_at`, `updated_at`,
//! `status_changed_at` from HLCs, RFC 3339 UTC), subtask progress and the
//! blocking relations in both directions.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use zunder_core::ids::Id;
use zunder_core::model::format_rfc3339;
use zunder_core::{
    AreaState, Comment, Due, EntityKind, ForWhom, Link, Location, Phoenix, Progress, ProjectKind,
    ProjectStatus, ProjectView as ProjectViewMode, Recurrence, State, Task, TaskStatus, TaskType,
    Weight,
};

use crate::app::format_ms;
use crate::error::{AppError, AppResult};

/// Effective location of a task with resolved names.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LocationView {
    /// `inbox`, `loose`, `area` or `project`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// Name of the area or project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// For projects: the area the project belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_name: Option<String>,
}

impl LocationView {
    pub fn of(state: &State, location: Location) -> Self {
        let mut view = LocationView {
            kind: location_kind(location),
            id: None,
            name: None,
            area_id: None,
            area_name: None,
        };
        match location {
            Location::Inbox | Location::Loose => {}
            Location::Area { id } => {
                view.id = Some(id);
                view.name = state.area(id).map(|a| a.name);
            }
            Location::Project { id } => {
                view.id = Some(id);
                if let Some(project) = state.project(id) {
                    view.name = Some(project.name);
                    view.area_id = Some(project.area_id);
                    view.area_name = state.area(project.area_id).map(|a| a.name);
                }
            }
        }
        view
    }
}

/// The wire code of a location kind.
pub fn location_kind(location: Location) -> &'static str {
    match location {
        Location::Inbox => "inbox",
        Location::Loose => "loose",
        Location::Area { .. } => "area",
        Location::Project { .. } => "project",
    }
}

/// A task as returned by tools.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskView {
    pub id: Id,
    /// Reference such as `ZUN-42` (tasks in projects with a key).
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub title: String,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub task_type: TaskType,
    pub status: TaskStatus,
    /// Effective location (the root task's location for subtasks).
    pub location: LocationView,
    pub parent_id: Option<Id>,
    pub due: Option<Due>,
    /// When the task was marked as deliberately undated ("bewusst undatiert").
    pub undated_ack: Option<String>,
    pub remind_at: Option<String>,
    pub links: Vec<Link>,
    pub vault_links: Vec<String>,
    pub phoenix: Option<Phoenix>,
    /// Weight for the owner (brand, glut, rauch, asche); `null` = not weighed.
    pub weight: Option<Weight>,
    /// Whom the task is for (mich, uns, andere); `null` = not stated.
    pub for_whom: Option<ForWhom>,
    /// How often the task was put off with "Heute nicht" (`defer_task`).
    pub deferrals: u32,
    /// Who has the decision (a name); `null` = nobody is waited for.
    pub waiting_on: Option<String>,
    /// Recurrence of the series (`null` for a one-off task).
    pub recurrence: Option<Recurrence>,
    /// The previous instance of the series (written by the server only).
    pub recurrence_prev: Option<Id>,
    pub claimed_by: Option<String>,
    pub claim_expires_at: Option<String>,
    /// Whether the claim is still valid (`claimed_by` set and not expired).
    pub claim_active: bool,
    pub plan: Option<String>,
    pub branch: Option<String>,
    pub pr_url: Option<String>,
    pub timing_ref: Option<String>,
    pub flame_started_at: Option<String>,
    pub flame_ended_at: Option<String>,
    /// Whether a flame is running on this task.
    pub flame_running: bool,
    pub clickup_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub status_changed_at: String,
    /// Direct, live subtasks: `done` of `total` (dropped ones excluded).
    pub progress: Progress,
    /// Live tasks this one waits for (`blocked_by`, deleted ones left out).
    pub blocked_by: Vec<TaskLinkView>,
    /// Live tasks waiting for this one (the reverse of their `blocked_by`).
    pub blocks: Vec<TaskLinkView>,
    /// Subtask tree (only from `get_task`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtasks: Option<Vec<TaskView>>,
    /// Live comments, oldest first (only from `get_task`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<CommentView>>,
}

/// Another task named in a relation (`blocked_by`, `blocks`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskLinkView {
    pub id: Id,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub title: String,
    pub status: TaskStatus,
}

impl TaskLinkView {
    fn of(state: &State, id: Id) -> Option<Self> {
        let task = state.task(id).filter(|t| !t.deleted)?;
        Some(TaskLinkView {
            id,
            reference: task.reference,
            title: task.title,
            status: task.status,
        })
    }
}

/// The reverse of `blocked_by` (blocker → waiting tasks), see
/// [`State::blocks_index`]. Computed once when building many views.
pub type BlocksIndex = HashMap<Id, Vec<Id>>;

/// A comment as returned by tools.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommentView {
    pub id: Id,
    pub task_id: Id,
    pub body: String,
    /// Agent or tool that wrote it; `null` = the owner.
    pub author: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl CommentView {
    /// The view of a comment (also tombstoned ones); `None` if unknown.
    pub fn build(state: &State, id: Id) -> Option<Self> {
        let comment = state.comment(id)?;
        Some(Self::of(state, comment))
    }

    /// The live comments of a task, oldest first.
    pub fn of_task(state: &State, task_id: Id) -> Vec<Self> {
        state
            .comments_of(task_id)
            .into_iter()
            .map(|c| Self::of(state, c))
            .collect()
    }

    fn of(state: &State, comment: Comment) -> Self {
        CommentView {
            id: comment.id,
            task_id: comment.task_id,
            created_at: hlc_time(state.created_at(EntityKind::Comment, comment.id)),
            updated_at: hlc_time(state.updated_at(EntityKind::Comment, comment.id)),
            body: comment.body,
            author: comment.author,
        }
    }
}

/// Whether a flame runs on the task: started and not ended since.
pub fn flame_running(task: &Task) -> bool {
    match (task.flame_started_at, task.flame_ended_at) {
        (Some(_), None) => true,
        (Some(started), Some(ended)) => ended < started,
        _ => false,
    }
}

/// Whether the task has a valid claim at `now_ms`.
pub fn claim_active(task: &Task, now_ms: u64) -> bool {
    task.claimed_by.is_some()
        && task
            .claim_expires_at
            .is_some_and(|t| t.unix_timestamp_nanos() / 1_000_000 > i128::from(now_ms))
}

fn hlc_time(hlc: Option<&zunder_core::Hlc>) -> String {
    format_ms(hlc.map_or(0, |h| i64::try_from(h.wall_ms()).unwrap_or(i64::MAX)))
}

impl TaskView {
    /// The view of a task (also tombstoned ones); `None` if unknown.
    /// `with_subtasks` adds the tree of live subtasks.
    pub fn build(state: &State, id: Id, now_ms: u64, with_subtasks: bool) -> Option<Self> {
        Self::build_with(state, &state.blocks_index(), id, now_ms, with_subtasks)
    }

    /// Like [`TaskView::build`] with a precomputed [`BlocksIndex`].
    pub fn build_with(
        state: &State,
        blocks: &BlocksIndex,
        id: Id,
        now_ms: u64,
        with_subtasks: bool,
    ) -> Option<Self> {
        let task = state.task(id)?;
        let location = state.effective_location(id).unwrap_or(task.location);
        let subtasks = with_subtasks.then(|| {
            let mut seen = HashSet::from([id]);
            subtask_tree(state, blocks, id, now_ms, &mut seen)
        });
        let links = |ids: &[Id]| -> Vec<TaskLinkView> {
            ids.iter()
                .filter_map(|b| TaskLinkView::of(state, *b))
                .collect()
        };
        Some(TaskView {
            id,
            location: LocationView::of(state, location),
            undated_ack: task.undated_ack.map(format_rfc3339),
            remind_at: task.remind_at.map(format_rfc3339),
            claim_expires_at: task.claim_expires_at.map(format_rfc3339),
            claim_active: claim_active(&task, now_ms),
            flame_started_at: task.flame_started_at.map(format_rfc3339),
            flame_ended_at: task.flame_ended_at.map(format_rfc3339),
            flame_running: flame_running(&task),
            created_at: hlc_time(state.created_at(EntityKind::Task, id)),
            updated_at: hlc_time(state.updated_at(EntityKind::Task, id)),
            status_changed_at: hlc_time(state.status_changed_at(id)),
            progress: state.progress(id),
            blocked_by: links(&state.blockers(id)),
            blocks: links(blocks.get(&id).map_or(&[], Vec::as_slice)),
            subtasks,
            comments: None,
            title: task.title,
            description: task.description,
            task_type: task.task_type,
            status: task.status,
            parent_id: task.parent_id,
            due: task.due,
            links: task.links,
            vault_links: task.vault_links,
            phoenix: task.phoenix,
            weight: task.weight,
            for_whom: task.for_whom,
            deferrals: task.deferrals,
            waiting_on: task.waiting_on,
            recurrence: task.recurrence,
            recurrence_prev: task.recurrence_prev,
            claimed_by: task.claimed_by,
            plan: task.plan,
            branch: task.branch,
            pr_url: task.pr_url,
            timing_ref: task.timing_ref,
            clickup_id: task.clickup_id,
            reference: task.reference,
        })
    }
}

fn subtask_tree(
    state: &State,
    blocks: &BlocksIndex,
    id: Id,
    now_ms: u64,
    seen: &mut HashSet<Id>,
) -> Vec<TaskView> {
    let mut out = Vec::new();
    // Creation order (IDs are random since CUID2).
    let mut children = state.children(id);
    children.sort_by(|a, b| {
        let created = |id: &Id| state.created_at(EntityKind::Task, *id);
        created(a).cmp(&created(b)).then(a.cmp(b))
    });
    for child in children {
        if !seen.insert(child) {
            continue;
        }
        if let Some(mut view) = TaskView::build_with(state, blocks, child, now_ms, false) {
            view.subtasks = Some(subtask_tree(state, blocks, child, now_ms, seen));
            out.push(view);
        }
    }
    out
}

/// An area as returned by tools.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AreaView {
    pub id: Id,
    pub name: String,
    pub state: AreaState,
    pub vault_path: Option<String>,
}

impl AreaView {
    pub fn build(state: &State, id: Id) -> Option<Self> {
        let area = state.area(id)?;
        Some(AreaView {
            id,
            name: area.name,
            state: area.state,
            vault_path: area.vault_path,
        })
    }
}

/// A project as returned by tools.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectView {
    pub id: Id,
    pub name: String,
    pub area_id: Id,
    pub area_name: Option<String>,
    pub status: ProjectStatus,
    pub kind: ProjectKind,
    pub repo: Option<String>,
    pub description: Option<String>,
    pub view: ProjectViewMode,
    /// Prefix of the task references (`ZUN`).
    pub key: Option<String>,
}

impl ProjectView {
    pub fn build(state: &State, id: Id) -> Option<Self> {
        let project = state.project(id)?;
        Some(ProjectView {
            id,
            area_name: state.area(project.area_id).map(|a| a.name),
            name: project.name,
            area_id: project.area_id,
            status: project.status,
            kind: project.kind,
            repo: project.repo,
            description: project.description,
            view: project.view,
            key: project.key,
        })
    }
}

/// How a tool addresses a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskRef {
    Id(Id),
    /// A reference such as `ZUN-42` (canonical form).
    Key(String),
    ClickUp(String),
}

impl TaskRef {
    /// From the `task_id` / `clickup_id` arguments: `Ok(None)` without
    /// either, an error message when both are given.
    pub fn from_args(
        task_id: Option<TaskRef>,
        clickup_id: Option<String>,
    ) -> Result<Option<Self>, &'static str> {
        match (task_id, clickup_id) {
            (Some(task), None) => Ok(Some(task)),
            (None, Some(c)) => Ok(Some(TaskRef::ClickUp(c))),
            (Some(_), Some(_)) => {
                Err("Bitte entweder task_id oder clickup_id angeben, nicht beides.")
            }
            (None, None) => Ok(None),
        }
    }

    /// The ID this reference stands for, whether or not such a task exists:
    /// a `clickup_id` is looked up among live tasks, then mapped to the
    /// deterministic import ID.
    pub fn candidate_id(&self, state: &State) -> Id {
        match self {
            TaskRef::Id(id) => *id,
            TaskRef::Key(reference) => state.task_by_ref(reference, true).map_or(Id::MIN, |t| t.id),
            TaskRef::ClickUp(clickup_id) => match state.task_by_clickup_id(clickup_id) {
                Some(task) => task.id,
                None => zunder_core::ids::clickup_entity_id(EntityKind::Task, clickup_id),
            },
        }
    }

    /// Resolves to the ID of an existing task (see [`TaskRef::candidate_id`]).
    /// Tombstoned tasks only resolve with `allow_deleted`.
    pub fn resolve(&self, state: &State, allow_deleted: bool) -> AppResult<Id> {
        let id = self.candidate_id(state);
        let Some(entity) = state.entity(EntityKind::Task, id) else {
            return Err(AppError::NotFound(match self {
                TaskRef::Id(id) => format!("Aufgabe {id} nicht gefunden."),
                TaskRef::Key(r) => format!("Keine Aufgabe mit der Kennung „{r}“ gefunden."),
                TaskRef::ClickUp(c) => format!("Keine Aufgabe mit der ClickUp-ID „{c}“ gefunden."),
            }));
        };
        if entity.is_deleted() && !allow_deleted {
            return Err(AppError::NotFound(format!("Aufgabe {id} ist gelöscht.")));
        }
        Ok(id)
    }
}
