//! Representations of tasks, areas and projects for MCP tools and the flame
//! endpoints, plus resolving task references (`task_id` or `clickup_id`).
//!
//! Views are computed from the materialized state: the effective location
//! with resolved names, derived timestamps (`created_at`, `updated_at`,
//! `status_changed_at` from HLCs, RFC 3339 UTC) and subtask progress.

use std::collections::HashSet;

use serde::Serialize;
use uuid::Uuid;
use zunder_core::model::format_rfc3339;
use zunder_core::{
    AreaState, Due, EntityKind, Link, Location, Phoenix, Progress, ProjectKind, ProjectStatus,
    ProjectView as ProjectViewMode, State, Task, TaskStatus, TaskType,
};

use crate::app::format_ms;
use crate::error::{AppError, AppResult};

/// Effective location of a task with resolved names.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LocationView {
    /// `inbox`, `loose`, `area` or `project`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    /// Name of the area or project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// For projects: the area the project belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_id: Option<Uuid>,
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
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub task_type: TaskType,
    pub status: TaskStatus,
    /// Effective location (the root task's location for subtasks).
    pub location: LocationView,
    pub parent_id: Option<Uuid>,
    pub due: Option<Due>,
    pub remind_at: Option<String>,
    pub links: Vec<Link>,
    pub vault_links: Vec<String>,
    pub phoenix: Option<Phoenix>,
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
    /// Subtask tree (only from `get_task`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtasks: Option<Vec<TaskView>>,
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
    pub fn build(state: &State, id: Uuid, now_ms: u64, with_subtasks: bool) -> Option<Self> {
        let task = state.task(id)?;
        let location = state.effective_location(id).unwrap_or(task.location);
        let subtasks = with_subtasks.then(|| {
            let mut seen = HashSet::from([id]);
            subtask_tree(state, id, now_ms, &mut seen)
        });
        Some(TaskView {
            id,
            location: LocationView::of(state, location),
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
            subtasks,
            title: task.title,
            description: task.description,
            task_type: task.task_type,
            status: task.status,
            parent_id: task.parent_id,
            due: task.due,
            links: task.links,
            vault_links: task.vault_links,
            phoenix: task.phoenix,
            claimed_by: task.claimed_by,
            plan: task.plan,
            branch: task.branch,
            pr_url: task.pr_url,
            timing_ref: task.timing_ref,
            clickup_id: task.clickup_id,
        })
    }
}

fn subtask_tree(state: &State, id: Uuid, now_ms: u64, seen: &mut HashSet<Uuid>) -> Vec<TaskView> {
    let mut out = Vec::new();
    for child in state.children(id) {
        if !seen.insert(child) {
            continue;
        }
        if let Some(mut view) = TaskView::build(state, child, now_ms, false) {
            view.subtasks = Some(subtask_tree(state, child, now_ms, seen));
            out.push(view);
        }
    }
    out
}

/// An area as returned by tools.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AreaView {
    pub id: Uuid,
    pub name: String,
    pub state: AreaState,
    pub vault_path: Option<String>,
}

impl AreaView {
    pub fn build(state: &State, id: Uuid) -> Option<Self> {
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
    pub id: Uuid,
    pub name: String,
    pub area_id: Uuid,
    pub area_name: Option<String>,
    pub status: ProjectStatus,
    pub kind: ProjectKind,
    pub repo: Option<String>,
    pub description: Option<String>,
    pub view: ProjectViewMode,
}

impl ProjectView {
    pub fn build(state: &State, id: Uuid) -> Option<Self> {
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
        })
    }
}

/// How a tool addresses a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskRef {
    Id(Uuid),
    ClickUp(String),
}

impl TaskRef {
    /// From the `task_id` / `clickup_id` arguments: exactly one is required.
    pub fn from_args(task_id: Option<Uuid>, clickup_id: Option<String>) -> AppResult<Self> {
        match (task_id, clickup_id) {
            (Some(id), None) => Ok(TaskRef::Id(id)),
            (None, Some(c)) => Ok(TaskRef::ClickUp(c)),
            (Some(_), Some(_)) => Err(AppError::bad_request(
                "Bitte entweder task_id oder clickup_id angeben, nicht beides.",
            )),
            (None, None) => Err(AppError::bad_request("task_id oder clickup_id fehlt.")),
        }
    }

    /// Resolves to a task ID. Tombstoned tasks only resolve with
    /// `allow_deleted`. A `clickup_id` is looked up among live tasks, then
    /// via the deterministic import ID.
    pub fn resolve(&self, state: &State, allow_deleted: bool) -> AppResult<Uuid> {
        let id = match self {
            TaskRef::Id(id) => *id,
            TaskRef::ClickUp(clickup_id) => match state.task_by_clickup_id(clickup_id) {
                Some(task) => task.id,
                None => zunder_core::ids::clickup_entity_id(EntityKind::Task, clickup_id),
            },
        };
        let Some(entity) = state.entity(EntityKind::Task, id) else {
            return Err(AppError::NotFound(match self {
                TaskRef::Id(id) => format!("Aufgabe {id} nicht gefunden."),
                TaskRef::ClickUp(c) => format!("Keine Aufgabe mit der ClickUp-ID „{c}“ gefunden."),
            }));
        };
        if entity.is_deleted() && !allow_deleted {
            return Err(AppError::NotFound(format!("Aufgabe {id} ist gelöscht.")));
        }
        Ok(id)
    }
}
