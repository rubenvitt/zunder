//! Tool handlers. Every mutation goes through [`App::mutate_atomic`]
//! (validation, op log, broadcast) with the caller's source
//! (`mcp:<token-name>`); flames go through [`crate::flame`].

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{json, Map, Value};
use time::Date;
use zunder_core::hlc::system_time_ms;
use zunder_core::ids::Id;
use zunder_core::model::fields::{area as fa, comment as fc, project as fp, task as ft};
use zunder_core::search::search;
use zunder_core::{
    AreaState, Due, EntityKind, IsoWeek, Location, PhoenixRole, ProjectStatus, State, TaskStatus,
    TaskType,
};

use super::args::Args;
use super::schema::{definitions, ToolDef};
use crate::app::{format_ms, App, Source};
use crate::error::{AppError, AppResult};
use crate::flame;
use crate::views::{
    claim_active, flame_running, location_kind, AreaView, CommentView, ProjectView, TaskRef,
    TaskView,
};

/// Task fields that `create_task` and `update_task` pass through to the
/// core unchanged (besides `title`).
const TASK_FIELDS: &[&str] = &[
    ft::DESCRIPTION,
    ft::TYPE,
    ft::STATUS,
    ft::DUE,
    ft::REMIND_AT,
    ft::LINKS,
    ft::VAULT_LINKS,
    ft::PHOENIX,
    ft::PLAN,
    ft::BRANCH,
    ft::PR_URL,
];

/// Fields of a subtask in `decompose_task`.
const SUBTASK_FIELDS: &[&str] = &[
    ft::TITLE,
    ft::DESCRIPTION,
    ft::TYPE,
    ft::STATUS,
    ft::DUE,
    ft::REMIND_AT,
    ft::PLAN,
];

/// Highest accepted `undated_ack_weeks` of filter_tasks (ten years).
const MAX_UNDATED_ACK_WEEKS: u64 = 520;

const MAX_SUBTASKS: usize = 50;
const MAX_AGENT_CHARS: usize = 200;

fn tools() -> &'static [ToolDef] {
    static TOOLS: OnceLock<Vec<ToolDef>> = OnceLock::new();
    TOOLS.get_or_init(definitions)
}

/// The `tools/list` result.
pub fn list() -> Value {
    json!({"tools": tools().iter().map(ToolDef::to_json).collect::<Vec<_>>()})
}

/// Runs a tool and returns the `tools/call` result (`None` for an unknown
/// tool). Errors become results with `isError: true`.
pub async fn call(
    app: &App,
    source: Source,
    name: &str,
    args: Map<String, Value>,
) -> Option<Value> {
    let def = tools().iter().find(|t| t.name == name)?;
    let args = Args::new(args);
    let result = match args.check(def.properties(), def.required()) {
        Ok(()) => dispatch(app, source, name, &args).await,
        Err(e) => Err(e),
    };
    Some(tool_result(result))
}

fn tool_result(result: AppResult<Value>) -> Value {
    match result {
        Ok(value) => json!({
            "content": [{"type": "text", "text": value.to_string()}],
            "structuredContent": value,
            "isError": false,
        }),
        Err(e) => {
            if let AppError::Internal(inner) = &e {
                tracing::error!(error = ?inner, "MCP tool failed");
            }
            let message = e.to_string();
            json!({
                "content": [{"type": "text", "text": message}],
                "structuredContent": {"error": {"code": e.code(), "message": message}},
                "isError": true,
            })
        }
    }
}

async fn dispatch(app: &App, source: Source, name: &str, args: &Args) -> AppResult<Value> {
    match name {
        "filter_tasks" => filter_tasks(app, args).await,
        "get_task" => get_task(app, args),
        "create_task" => create_task(app, source, args).await,
        "update_task" => update_task(app, source, args).await,
        "move_task" => move_task(app, source, args).await,
        "delete_task" => delete_task(app, source, args).await,
        "search" => search_all(app, args).await,
        "claim_task" => claim_task(app, source, args).await,
        "release_task" => release_task(app, source, args).await,
        "decompose_task" => decompose_task(app, source, args).await,
        "list_structure" => list_structure(app, args).await,
        "create_area" => create_area(app, source, args).await,
        "update_area" => update_area(app, source, args).await,
        "create_project" => create_project(app, source, args).await,
        "update_project" => update_project(app, source, args).await,
        "start_flame" => start_flame(app, args).await,
        "stop_flame" => stop_flame(app, args).await,
        "get_task_history" => task_history(app, args).await,
        "add_comment" => add_comment(app, source, args).await,
        other => Err(AppError::Internal(anyhow::anyhow!(
            "tool {other} has no handler"
        ))),
    }
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::BadRequest {
        code: "invalid_arguments",
        message: message.into(),
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> AppResult<Value> {
    serde_json::to_value(value).map_err(|e| AppError::Internal(e.into()))
}

/// The view of a task after a change, with the warnings of the change.
fn task_result(app: &App, id: Id, warnings: &[zunder_core::Warning]) -> AppResult<Value> {
    let view = app
        .read(|s| TaskView::build(s, id, system_time_ms(), false))
        .ok_or_else(|| AppError::NotFound(format!("Aufgabe {id} nicht gefunden.")))?;
    Ok(json!({"task": view, "warnings": warnings}))
}

/// Present values of `keys` (explicit `null` included when `keep_null`).
fn present(args: &Args, keys: &[&'static str], keep_null: bool) -> Vec<(&'static str, Value)> {
    keys.iter()
        .filter_map(|k| {
            let v = if keep_null {
                args.raw(k)
            } else {
                args.value(k)
            };
            v.map(|v| (*k, v.clone()))
        })
        .collect()
}

fn live_area(state: &State, id: Id) -> AppResult<()> {
    state
        .live_entity(EntityKind::Area, id)
        .map(|_| ())
        .ok_or_else(|| AppError::NotFound(format!("Bereich {id} nicht gefunden.")))
}

fn live_project(state: &State, id: Id) -> AppResult<()> {
    state
        .live_entity(EntityKind::Project, id)
        .map(|_| ())
        .ok_or_else(|| AppError::NotFound(format!("Projekt {id} nicht gefunden.")))
}

/// Whether a field change would actually change the stored value.
fn changes(state: &State, kind: EntityKind, id: Id, field: &str, value: &Value) -> bool {
    let current = state
        .entity(kind, id)
        .and_then(|e| e.field(field))
        .map_or(&Value::Null, |f| &f.value);
    current != value
}

// ----- reading -------------------------------------------------------------------

struct TaskFilter {
    location: Option<String>,
    area_id: Option<Id>,
    project_id: Option<Id>,
    statuses: Option<Vec<TaskStatus>>,
    types: Option<Vec<TaskType>>,
    phoenix_role: Option<PhoenixRole>,
    phoenix_fire: Option<IsoWeek>,
    due_before: Option<Date>,
    due_after: Option<Date>,
    closed_after: Option<Due>,
    closed_before: Option<Due>,
    include_closed: bool,
    /// `parent_id` as given (UUID or ClickUp ID) …
    parent: Option<TaskRef>,
    /// … and resolved against the state that is being filtered.
    parent_id: Option<Id>,
    claimed_by: Option<String>,
    flame_running: Option<bool>,
    has_due: Option<bool>,
    undated_ack: Option<bool>,
    /// A mark older than this many weeks counts as expired.
    undated_ack_weeks: Option<u64>,
    order_by: OrderBy,
    reverse: bool,
    offset: usize,
    limit: usize,
}

/// Sort order of filter_tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrderBy {
    /// Due date (undated last), then creation.
    Due,
    Created,
    Updated,
    StatusChanged,
}

const ORDER_BY_CODES: [&str; 4] = ["due", "created", "updated", "status_changed"];

impl std::str::FromStr for OrderBy {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "due" => Ok(OrderBy::Due),
            "created" => Ok(OrderBy::Created),
            "updated" => Ok(OrderBy::Updated),
            "status_changed" => Ok(OrderBy::StatusChanged),
            _ => Err(()),
        }
    }
}

/// Highest accepted `offset` of filter_tasks.
const MAX_OFFSET: u64 = 1_000_000;

fn day(args: &Args, key: &str) -> AppResult<Option<Date>> {
    Ok(moment(args, key)?.map(|d| d.date()))
}

fn moment(args: &Args, key: &str) -> AppResult<Option<Due>> {
    match args.str(key)? {
        None => Ok(None),
        Some(s) => Due::parse(s).map(Some).ok_or_else(|| {
            invalid(format!(
                "„{key}“ muss ein Datum (JJJJ-MM-TT) oder ein RFC-3339-Zeitpunkt sein."
            ))
        }),
    }
}

/// The calendar day of `ms` in the server's time zone (`TZ`).
fn local_day(ms: u64) -> Option<Date> {
    use chrono::{Datelike, TimeZone};
    let day = chrono::Local
        .timestamp_millis_opt(i64::try_from(ms).ok()?)
        .single()?
        .date_naive();
    Date::from_ordinal_date(day.year(), u16::try_from(day.ordinal()).ok()?).ok()
}

/// How `ms` compares with a bound: a date compares by calendar day in the
/// server's time zone, an RFC 3339 instant exactly.
fn cmp_moment(ms: u64, bound: &Due) -> Option<std::cmp::Ordering> {
    match bound {
        Due::Date(d) => local_day(ms).map(|day| day.cmp(d)),
        Due::DateTime(t) => Some(i128::from(ms).cmp(&(t.unix_timestamp_nanos() / 1_000_000))),
    }
}

impl TaskFilter {
    fn parse(args: &Args) -> AppResult<Self> {
        let location = args.str("location")?.map(str::to_owned);
        if let Some(l) = &location {
            if !["inbox", "loose", "area", "project"].contains(&l.as_str()) {
                return Err(invalid(format!(
                    "Ungültiger Wert „{l}“ für „location“. Erlaubt: inbox, loose, area, project."
                )));
            }
        }
        let phoenix_fire = match args.str("phoenix_fire")? {
            None => None,
            Some(s) => Some(IsoWeek::parse(s).ok_or_else(|| {
                invalid("„phoenix_fire“ muss eine ISO-Woche JJJJ-Www sein, z. B. 2026-W45.")
            })?),
        };
        let closed_after = moment(args, "closed_after")?;
        let closed_before = moment(args, "closed_before")?;
        let undated_ack = args.bool("undated_ack")?;
        let undated_ack_weeks = match args.value("undated_ack_weeks") {
            None => None,
            Some(_) if undated_ack.is_none() => {
                return Err(invalid(
                    "„undated_ack_weeks“ wirkt nur zusammen mit „undated_ack“.",
                ))
            }
            Some(_) => Some(args.int("undated_ack_weeks", 1, MAX_UNDATED_ACK_WEEKS, 1)?),
        };
        Ok(TaskFilter {
            location,
            area_id: args.id("area_id")?,
            project_id: args.id("project_id")?,
            statuses: args.codes("status", &TaskStatus::codes())?,
            types: args.codes("type", &TaskType::codes())?,
            phoenix_role: args.code("phoenix_role", &PhoenixRole::codes())?,
            phoenix_fire,
            due_before: day(args, "due_before")?,
            due_after: day(args, "due_after")?,
            // A closing date only matches closed tasks, of any age.
            include_closed: args.bool("include_closed")?.unwrap_or(false)
                || closed_after.is_some()
                || closed_before.is_some(),
            closed_after,
            closed_before,
            parent: args.task_link("parent_id")?.flatten(),
            parent_id: None,
            claimed_by: args.str("claimed_by")?.map(str::to_owned),
            flame_running: args.bool("flame_running")?,
            has_due: args.bool("has_due")?,
            undated_ack,
            undated_ack_weeks,
            order_by: args
                .code("order_by", &ORDER_BY_CODES)?
                .unwrap_or(OrderBy::Due),
            reverse: args.bool("reverse")?.unwrap_or(false),
            offset: args.int("offset", 0, MAX_OFFSET, 0)? as usize,
            limit: args.int("limit", 1, 500, 100)? as usize,
        })
    }

    fn matches(&self, state: &State, task: &zunder_core::Task, now: u64) -> bool {
        // A running flame is found whatever the task's status and age.
        let finds_flame = self.flame_running == Some(true);
        if !self.include_closed && !finds_flame && !state.is_visible_by_default(task.id, now) {
            return false;
        }
        if self
            .flame_running
            .is_some_and(|running| flame_running(task) != running)
        {
            return false;
        }
        if self.closed_after.is_some() || self.closed_before.is_some() {
            if !task.status.is_closed() {
                return false;
            }
            let changed = state.status_changed_at(task.id).map_or(0, |h| h.wall_ms());
            let after = self.closed_after.as_ref().is_none_or(|b| {
                cmp_moment(changed, b).is_some_and(|o| o != std::cmp::Ordering::Less)
            });
            let before = self.closed_before.as_ref().is_none_or(|b| {
                cmp_moment(changed, b).is_some_and(|o| o != std::cmp::Ordering::Greater)
            });
            if !after || !before {
                return false;
            }
        }
        let location = state.effective_location(task.id).unwrap_or(task.location);
        if self
            .location
            .as_deref()
            .is_some_and(|k| k != location_kind(location))
        {
            return false;
        }
        if let Some(area) = self.area_id {
            let in_area = match location {
                Location::Area { id } => id == area,
                Location::Project { id } => state.project(id).is_some_and(|p| p.area_id == area),
                _ => false,
            };
            if !in_area {
                return false;
            }
        }
        if self
            .project_id
            .is_some_and(|p| location != (Location::Project { id: p }))
        {
            return false;
        }
        if self
            .statuses
            .as_ref()
            .is_some_and(|s| !s.contains(&task.status))
        {
            return false;
        }
        if self
            .types
            .as_ref()
            .is_some_and(|t| !t.contains(&task.task_type))
        {
            return false;
        }
        if self
            .phoenix_role
            .is_some_and(|r| task.phoenix.is_none_or(|p| p.role != r))
        {
            return false;
        }
        if self
            .phoenix_fire
            .is_some_and(|f| task.phoenix.is_none_or(|p| p.fire != f))
        {
            return false;
        }
        if self.has_due.is_some_and(|has| task.due.is_some() != has) {
            return false;
        }
        if self
            .undated_ack
            .is_some_and(|marked| self.undated_ack_valid(task, now) != marked)
        {
            return false;
        }
        let due = task.due.map(|d| d.date());
        if self.due_before.is_some_and(|b| due.is_none_or(|d| d > b)) {
            return false;
        }
        if self.due_after.is_some_and(|a| due.is_none_or(|d| d < a)) {
            return false;
        }
        if self.parent_id.is_some_and(|p| task.parent_id != Some(p)) {
            return false;
        }
        if let Some(agent) = &self.claimed_by {
            if task.claimed_by.as_deref() != Some(agent) || !claim_active(task, now) {
                return false;
            }
        }
        true
    }

    /// Whether the task carries an "undated on purpose" mark that has not
    /// expired (`undated_ack_weeks`) at `now`.
    fn undated_ack_valid(&self, task: &zunder_core::Task, now: u64) -> bool {
        let Some(marked) = task.undated_ack else {
            return false;
        };
        self.undated_ack_weeks.is_none_or(|weeks| {
            let marked_ms = marked.unix_timestamp_nanos() / 1_000_000;
            let max_age_ms = i128::from(weeks) * 7 * 24 * 60 * 60 * 1000;
            marked_ms + max_age_ms > i128::from(now)
        })
    }
}

// The full scans below run on the blocking pool (`read_blocking`).

async fn filter_tasks(app: &App, args: &Args) -> AppResult<Value> {
    let mut filter = TaskFilter::parse(args)?;
    let now = system_time_ms();
    let (offset, limit) = (filter.offset, filter.limit);
    let (tasks, total) = app
        .read_blocking(move |state| {
            filter.parent_id = filter.parent.as_ref().map(|p| p.candidate_id(state));
            let mut hits: Vec<_> = state
                .live_tasks()
                .filter(|t| filter.matches(state, t, now))
                .map(|t| {
                    let key = match filter.order_by {
                        OrderBy::Due | OrderBy::Created => {
                            state.created_at(EntityKind::Task, t.id).cloned()
                        }
                        OrderBy::Updated => state.updated_at(EntityKind::Task, t.id).cloned(),
                        OrderBy::StatusChanged => state.status_changed_at(t.id).cloned(),
                    };
                    (t.due.map(|d| d.date()), key, t.id)
                })
                .collect();
            hits.sort_by(|a, b| {
                let due = match (filter.order_by, a.0, b.0) {
                    (OrderBy::Due, Some(x), Some(y)) => x.cmp(&y),
                    (OrderBy::Due, Some(_), None) => std::cmp::Ordering::Less,
                    (OrderBy::Due, None, Some(_)) => std::cmp::Ordering::Greater,
                    _ => std::cmp::Ordering::Equal,
                };
                let order = due.then_with(|| a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2));
                if filter.reverse {
                    order.reverse()
                } else {
                    order
                }
            });
            let total = hits.len();
            let tasks: Vec<TaskView> = hits
                .into_iter()
                .skip(offset)
                .take(limit)
                .filter_map(|(_, _, id)| TaskView::build(state, id, now, false))
                .collect();
            (tasks, total)
        })
        .await?;
    let end = offset + tasks.len();
    Ok(json!({
        "tasks": tasks,
        "count": tasks.len(),
        "total": total,
        "offset": offset,
        "truncated": end < total,
        "next_offset": (end < total).then_some(end),
    }))
}

fn get_task(app: &App, args: &Args) -> AppResult<Value> {
    let target = args.task_ref()?;
    let now = system_time_ms();
    app.read(|state| {
        let id = target.resolve(state, false)?;
        let mut view = TaskView::build(state, id, now, true)
            .ok_or_else(|| AppError::NotFound(format!("Aufgabe {id} nicht gefunden.")))?;
        view.comments = Some(CommentView::of_task(state, id));
        Ok(json!({"task": view}))
    })
}

async fn search_all(app: &App, args: &Args) -> AppResult<Value> {
    let query = args
        .non_empty_str("query")?
        .ok_or_else(|| invalid("Parameter „query“ fehlt."))?
        .to_owned();
    let limit = args.int("limit", 1, 200, 20)? as usize;
    let now = system_time_ms();
    let hits: Vec<Value> = app
        .read_blocking(move |state| {
            search(state, &query, limit)
                .into_iter()
                .map(|hit| {
                    let mut v = json!({
                        "entity": hit.entity,
                        "id": hit.entity_id,
                        "title": hit.title,
                        "score": hit.score,
                    });
                    match hit.entity {
                        EntityKind::Task => {
                            if let Some(t) = TaskView::build(state, hit.entity_id, now, false) {
                                v["status"] = json!(t.status);
                                v["type"] = json!(t.task_type);
                                v["location"] = json!(t.location);
                                v["parent_id"] = json!(t.parent_id);
                                v["due"] = json!(t.due);
                                v["clickup_id"] = json!(t.clickup_id);
                                v["ref"] = json!(t.reference);
                            }
                        }
                        EntityKind::Project => {
                            if let Some(p) = ProjectView::build(state, hit.entity_id) {
                                v["area_id"] = json!(p.area_id);
                                v["area_name"] = json!(p.area_name);
                                v["status"] = json!(p.status);
                                v["kind"] = json!(p.kind);
                                v["key"] = json!(p.key);
                            }
                        }
                        EntityKind::Area => {
                            if let Some(a) = AreaView::build(state, hit.entity_id) {
                                v["state"] = json!(a.state);
                            }
                        }
                        // Not searched (zunder_core::search).
                        EntityKind::Comment => {}
                    }
                    v
                })
                .collect()
        })
        .await?;
    Ok(json!({"hits": hits, "count": hits.len()}))
}

async fn list_structure(app: &App, args: &Args) -> AppResult<Value> {
    let include_archived = args.bool("include_archived")?.unwrap_or(false);
    app.read_blocking(move |state| {
        let mut inbox = 0usize;
        let mut loose = 0usize;
        let mut open: HashMap<Id, usize> = HashMap::new();
        for task in state.live_tasks().filter(|t| !t.status.is_closed()) {
            match state.effective_location(task.id).unwrap_or(task.location) {
                Location::Inbox => inbox += 1,
                Location::Loose => loose += 1,
                Location::Area { id } | Location::Project { id } => {
                    *open.entry(id).or_default() += 1
                }
            }
        }
        let mut areas: Vec<_> = state
            .live_areas()
            .filter(|a| include_archived || a.state == AreaState::Active)
            .collect();
        areas.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        let mut out = Vec::new();
        for area in areas {
            let mut projects: Vec<_> = state
                .live_projects()
                .filter(|p| p.area_id == area.id)
                .filter(|p| include_archived || p.status != ProjectStatus::Completed)
                .collect();
            projects.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
            let mut project_values = Vec::new();
            for project in projects {
                let mut v = to_json(&ProjectView::build(state, project.id))?;
                v["open_tasks"] = json!(open.get(&project.id).copied().unwrap_or(0));
                project_values.push(v);
            }
            let mut v = to_json(&AreaView::build(state, area.id))?;
            v["open_tasks"] = json!(open.get(&area.id).copied().unwrap_or(0));
            v["projects"] = Value::Array(project_values);
            out.push(v);
        }
        Ok(json!({"areas": out, "inbox_open": inbox, "loose_open": loose}))
    })
    .await?
}

async fn task_history(app: &App, args: &Args) -> AppResult<Value> {
    let target = args.task_ref()?;
    let limit = args.int("limit", 1, 1000, 100)? as usize;
    let id = app.read(|s| target.resolve(s, true))?;
    let entries = app.task_history(id).await?;
    let total = entries.len();
    let entries: Vec<Value> = entries
        .into_iter()
        .skip(total.saturating_sub(limit))
        .map(|e| {
            json!({
                "seq": e.seq,
                "field": e.op.field,
                "value": e.op.value,
                "hlc": e.op.hlc,
                "source": e.source,
                "received_at": e.received_at,
            })
        })
        .collect();
    Ok(json!({
        "task_id": id,
        "entries": entries,
        "count": entries.len(),
        "truncated": total > entries.len(),
    }))
}

// ----- task changes ---------------------------------------------------------------

async fn create_task(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let title = args.raw("title").cloned().unwrap_or(Value::Null);
    let location = args
        .value("location")
        .cloned()
        .unwrap_or_else(|| Location::Inbox.to_value());
    let parent = args.task_link("parent_id")?.flatten();
    let mut fields = present(args, TASK_FIELDS, false);
    if let Some(ack) = undated_ack_value(args)?.filter(|v| !v.is_null()) {
        fields.push((ft::UNDATED_ACK, ack));
    }
    let task_type = take(&mut fields, ft::TYPE).unwrap_or_else(|| TaskType::Aufgabe.into());
    let status = take(&mut fields, ft::STATUS).unwrap_or_else(|| TaskStatus::Open.into());
    let (id, warnings) = app
        .mutate_atomic(source, move |state, batch| {
            let id = zunder_core::ids::new_id();
            batch.set_task(id, ft::TITLE, title);
            batch.set_task(id, ft::TYPE, task_type);
            batch.set_task(id, ft::STATUS, status);
            batch.set_task(id, ft::LOCATION, location);
            if let Some(parent) = parent {
                batch.set_task(id, ft::PARENT_ID, parent_value(state, &parent)?);
            }
            for (field, value) in fields {
                batch.set_task(id, field, value);
            }
            Ok(id)
        })
        .await?;
    task_result(app, id, &warnings)
}

/// The value written to `parent_id`: a UUID as given (the core checks the
/// reference), a ClickUp ID resolved to its task.
fn parent_value(state: &State, parent: &TaskRef) -> AppResult<Value> {
    let id = match parent {
        TaskRef::Id(id) => *id,
        TaskRef::Key(_) | TaskRef::ClickUp(_) => parent.resolve(state, false)?,
    };
    Ok(Value::String(id.to_string()))
}

/// The value written to `undated_ack`: `true` marks the task as undated on
/// purpose now (a renewed mark restarts its age), `false`/`null` removes the
/// mark. `None` when the parameter is absent.
fn undated_ack_value(args: &Args) -> AppResult<Option<Value>> {
    match args.raw(ft::UNDATED_ACK) {
        None => Ok(None),
        Some(Value::Null | Value::Bool(false)) => Ok(Some(Value::Null)),
        Some(Value::Bool(true)) => Ok(Some(Value::String(format_ms(system_time_ms() as i64)))),
        Some(_) => Err(invalid(format!(
            "„{}“ muss true, false oder null sein.",
            ft::UNDATED_ACK
        ))),
    }
}

fn take(fields: &mut Vec<(&'static str, Value)>, key: &str) -> Option<Value> {
    let pos = fields.iter().position(|(k, _)| *k == key)?;
    Some(fields.remove(pos).1)
}

async fn update_task(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let target = args.task_ref()?;
    let mut fields = present(args, &[ft::TITLE], true);
    fields.extend(present(args, TASK_FIELDS, true));
    if let Some(ack) = undated_ack_value(args)? {
        fields.push((ft::UNDATED_ACK, ack));
    }
    if fields.is_empty() {
        return Err(invalid("Keine Änderung angegeben."));
    }
    let (id, warnings) = app
        .mutate_atomic(source, move |state, batch| {
            let id = target.resolve(state, false)?;
            for (field, value) in fields {
                if changes(state, EntityKind::Task, id, field, &value) {
                    batch.set_task(id, field, value);
                }
            }
            Ok(id)
        })
        .await?;
    task_result(app, id, &warnings)
}

async fn move_task(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let target = args.task_ref()?;
    let location = args.value("location").cloned();
    let parent = args.task_link("parent_id")?;
    if location.is_none() && parent.is_none() {
        return Err(invalid("location oder parent_id angeben."));
    }
    let (id, warnings) = app
        .mutate_atomic(source, move |state, batch| {
            let id = target.resolve(state, false)?;
            match parent {
                // Detach from the parent; keep the effective location
                // unless a new one is given.
                Some(None) => {
                    let location = match location {
                        Some(l) => l,
                        None => state.effective_location(id).unwrap_or_default().to_value(),
                    };
                    batch.set_task(id, ft::LOCATION, location);
                    batch.clear(EntityKind::Task, id, ft::PARENT_ID);
                }
                Some(Some(parent)) => {
                    let parent = parent_value(state, &parent)?;
                    if let Some(location) = location {
                        batch.set_task(id, ft::LOCATION, location);
                    }
                    batch.set_task(id, ft::PARENT_ID, parent);
                }
                None => {
                    let location = location.expect("checked above");
                    batch.set_task(id, ft::LOCATION, location);
                    if state.task(id).is_some_and(|t| t.parent_id.is_some()) {
                        batch.clear(EntityKind::Task, id, ft::PARENT_ID);
                    }
                }
            }
            Ok(id)
        })
        .await?;
    task_result(app, id, &warnings)
}

async fn delete_task(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let target = args.task_ref()?;
    let (deleted, _) = app
        .mutate_atomic(source, move |state, batch| {
            let id = target.resolve(state, false)?;
            batch.delete_cascade(state, EntityKind::Task, id);
            Ok(batch
                .ops()
                .iter()
                .map(|op| op.entity_id)
                .collect::<Vec<_>>())
        })
        .await?;
    Ok(json!({"deleted": deleted, "count": deleted.len()}))
}

async fn claim_task(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let target = args.task_ref()?;
    let agent = agent_name(args)?.ok_or_else(|| invalid("Parameter „agent“ fehlt."))?;
    let ttl = args.int("ttl_minutes", 1, 10_080, 120)?;
    let now = system_time_ms();
    let expires = format_ms((now + ttl * 60_000) as i64);
    let (id, warnings) = app
        .mutate_atomic(source, move |state, batch| {
            let id = target.resolve(state, false)?;
            let task = state.task(id).expect("resolved task exists");
            if let Some(holder) = &task.claimed_by {
                if *holder != agent && claim_active(&task, now) {
                    let until = task
                        .claim_expires_at
                        .map(zunder_core::model::format_rfc3339)
                        .unwrap_or_default();
                    return Err(AppError::Conflict(format!(
                        "Die Aufgabe ist bis {until} von „{holder}“ reserviert."
                    )));
                }
            }
            batch.set_task(id, ft::CLAIMED_BY, agent);
            batch.set_task(id, ft::CLAIM_EXPIRES_AT, expires);
            Ok(id)
        })
        .await?;
    task_result(app, id, &warnings)
}

fn agent_name(args: &Args) -> AppResult<Option<String>> {
    agent_like(args, "agent")
}

/// A trimmed agent name of at most [`MAX_AGENT_CHARS`] characters.
fn agent_like(args: &Args, key: &str) -> AppResult<Option<String>> {
    match args.non_empty_str(key)? {
        None => Ok(None),
        Some(a) if a.chars().count() > MAX_AGENT_CHARS => Err(invalid(format!(
            "„{key}“ darf höchstens {MAX_AGENT_CHARS} Zeichen lang sein."
        ))),
        Some(a) => Ok(Some(a.trim().to_owned())),
    }
}

async fn release_task(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let target = args.task_ref()?;
    let agent = agent_name(args)?;
    let force = args.bool("force")?.unwrap_or(false);
    if agent.is_none() && !force {
        return Err(invalid(
            "Parameter „agent“ fehlt (oder force: true angeben).",
        ));
    }
    let now = system_time_ms();
    let ((id, released), warnings) = app
        .mutate_atomic(source, move |state, batch| {
            let id = target.resolve(state, false)?;
            let task = state.task(id).expect("resolved task exists");
            if task.claimed_by.is_none() && task.claim_expires_at.is_none() {
                return Ok((id, false));
            }
            if let Some(holder) = &task.claimed_by {
                let own = agent.as_deref() == Some(holder.as_str());
                if !own && !force && claim_active(&task, now) {
                    return Err(AppError::Conflict(format!(
                        "Die Aufgabe ist von „{holder}“ reserviert; nur dieser Agent (oder force) kann sie freigeben."
                    )));
                }
            }
            batch.clear(EntityKind::Task, id, ft::CLAIMED_BY);
            batch.clear(EntityKind::Task, id, ft::CLAIM_EXPIRES_AT);
            Ok((id, true))
        })
        .await?;
    let mut result = task_result(app, id, &warnings)?;
    result["released"] = json!(released);
    Ok(result)
}

async fn decompose_task(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let target = args.task_ref()?;
    let items = args.objects("subtasks")?;
    if items.is_empty() || items.len() > MAX_SUBTASKS {
        return Err(invalid(format!(
            "„subtasks“ muss 1 bis {MAX_SUBTASKS} Einträge enthalten."
        )));
    }
    let mut subtasks = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        item.check(SUBTASK_FIELDS.iter().copied(), [ft::TITLE])
            .map_err(|e| invalid(format!("Unteraufgabe {}: {e}", i + 1)))?;
        subtasks.push(present(item, SUBTASK_FIELDS, false));
    }
    let ((parent, created), warnings) = app
        .mutate_atomic(source, move |state, batch| {
            let parent = target.resolve(state, false)?;
            let mut created = Vec::with_capacity(subtasks.len());
            for mut fields in subtasks {
                let id = zunder_core::ids::new_id();
                let title = take(&mut fields, ft::TITLE).unwrap_or(Value::Null);
                let task_type =
                    take(&mut fields, ft::TYPE).unwrap_or_else(|| TaskType::Aufgabe.into());
                let status =
                    take(&mut fields, ft::STATUS).unwrap_or_else(|| TaskStatus::Open.into());
                batch.set_task(id, ft::TITLE, title);
                batch.set_task(id, ft::TYPE, task_type);
                batch.set_task(id, ft::STATUS, status);
                batch.set_task(id, ft::LOCATION, Location::Inbox);
                batch.set_task(id, ft::PARENT_ID, parent.to_string());
                for (field, value) in fields {
                    batch.set_task(id, field, value);
                }
                created.push(id);
            }
            Ok((parent, created))
        })
        .await?;
    let view = app
        .read(|s| TaskView::build(s, parent, system_time_ms(), true))
        .ok_or_else(|| AppError::NotFound(format!("Aufgabe {parent} nicht gefunden.")))?;
    Ok(json!({"task": view, "created": created, "warnings": warnings}))
}

// ----- comments -------------------------------------------------------------------

async fn add_comment(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let target = args.task_ref()?;
    let body = args.raw("body").cloned().unwrap_or(Value::Null);
    let author = match agent_like(args, "author")? {
        Some(author) => Some(author),
        None => default_author(&source),
    };
    let (id, _) = app
        .mutate_atomic(source, move |state, batch| {
            let task = target.resolve(state, false)?;
            let id = zunder_core::ids::new_id();
            batch.set_comment(id, fc::TASK_ID, task.to_string());
            batch.set_comment(id, fc::BODY, body);
            if let Some(author) = author {
                batch.set_comment(id, fc::AUTHOR, author);
            }
            Ok(id)
        })
        .await?;
    let comment = app
        .read(|s| CommentView::build(s, id))
        .ok_or_else(|| AppError::NotFound(format!("Kommentar {id} nicht gefunden.")))?;
    Ok(json!({"comment": comment}))
}

/// Author of a comment without `author`: the name of the token or the
/// OAuth client (`mcp:oauth:Claude` → `Claude`).
fn default_author(source: &Source) -> Option<String> {
    match source {
        Source::Mcp(name) | Source::Api(name) | Source::App(name) | Source::Intake(name) => {
            Some(name.strip_prefix("oauth:").unwrap_or(name).to_owned())
        }
        Source::Web | Source::Server => None,
    }
}

// ----- structure ----------------------------------------------------------------------

fn new_entity_id(state: &State, wanted: Option<Id>) -> AppResult<Id> {
    match wanted {
        Some(id) if state.kinds_of(id).next().is_some() => Err(AppError::Conflict(format!(
            "Die ID {id} ist bereits vergeben."
        ))),
        Some(id) => Ok(id),
        None => Ok(zunder_core::ids::new_id()),
    }
}

async fn create_area(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let wanted = args.id("id")?;
    let name = args.raw(fa::NAME).cloned().unwrap_or(Value::Null);
    let state_value = args
        .value(fa::STATE)
        .cloned()
        .unwrap_or_else(|| AreaState::Active.into());
    let vault_path = args.value(fa::VAULT_PATH).cloned();
    let (id, _) = app
        .mutate_atomic(source, move |state, batch| {
            let id = new_entity_id(state, wanted)?;
            batch.set_area(id, fa::NAME, name);
            batch.set_area(id, fa::STATE, state_value);
            if let Some(path) = vault_path {
                batch.set_area(id, fa::VAULT_PATH, path);
            }
            Ok(id)
        })
        .await?;
    let view = app.read(|s| AreaView::build(s, id));
    Ok(json!({"area": view}))
}

async fn update_area(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let id = args.required_id("area_id")?;
    let fields = present(args, &[fa::NAME, fa::STATE, fa::VAULT_PATH], true);
    if fields.is_empty() {
        return Err(invalid("Keine Änderung angegeben."));
    }
    app.mutate_atomic(source, move |state, batch| {
        live_area(state, id)?;
        for (field, value) in fields {
            if changes(state, EntityKind::Area, id, field, &value) {
                batch.set_area(id, field, value);
            }
        }
        Ok(())
    })
    .await?;
    let view = app.read(|s| AreaView::build(s, id));
    Ok(json!({"area": view}))
}

const PROJECT_FIELDS: &[&str] = &[
    fp::NAME,
    fp::AREA_ID,
    fp::STATUS,
    fp::KIND,
    fp::REPO,
    fp::DESCRIPTION,
    fp::VIEW,
    fp::KEY,
];

async fn create_project(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let wanted = args.id("id")?;
    let mut fields = present(args, PROJECT_FIELDS, false);
    let status = take(&mut fields, fp::STATUS).unwrap_or_else(|| ProjectStatus::Active.into());
    let kind =
        take(&mut fields, fp::KIND).unwrap_or_else(|| zunder_core::ProjectKind::Standard.into());
    let (id, _) = app
        .mutate_atomic(source, move |state, batch| {
            let id = new_entity_id(state, wanted)?;
            batch.set_project(id, fp::STATUS, status);
            batch.set_project(id, fp::KIND, kind);
            for (field, value) in fields {
                batch.set_project(id, field, value);
            }
            Ok(id)
        })
        .await?;
    let view = app.read(|s| ProjectView::build(s, id));
    Ok(json!({"project": view}))
}

async fn update_project(app: &App, source: Source, args: &Args) -> AppResult<Value> {
    let id = args.required_id("project_id")?;
    let fields = present(args, PROJECT_FIELDS, true);
    if fields.is_empty() {
        return Err(invalid("Keine Änderung angegeben."));
    }
    app.mutate_atomic(source, move |state, batch| {
        live_project(state, id)?;
        for (field, value) in fields {
            if changes(state, EntityKind::Project, id, field, &value) {
                batch.set_project(id, field, value);
            }
        }
        Ok(())
    })
    .await?;
    let view = app.read(|s| ProjectView::build(s, id));
    Ok(json!({"project": view}))
}

// ----- flames ---------------------------------------------------------------------------

fn resolve_live(app: &App, target: &TaskRef) -> AppResult<Id> {
    app.read(|s| target.resolve(s, false))
}

async fn start_flame(app: &App, args: &Args) -> AppResult<Value> {
    let id = resolve_live(app, &args.task_ref()?)?;
    to_json(&flame::start(app, id).await?)
}

/// Stops the flame of the given task, or without a task the one that is
/// running (the most recently started, should several run after a sync).
async fn stop_flame(app: &App, args: &Args) -> AppResult<Value> {
    let id = match args.optional_task_ref()? {
        Some(target) => resolve_live(app, &target)?,
        None => app
            .read_blocking(|s| {
                s.live_tasks()
                    .filter(flame_running)
                    .max_by_key(|t| (t.flame_started_at, t.id))
                    .map(|t| t.id)
            })
            .await?
            .ok_or_else(|| AppError::Conflict("Es läuft keine Flamme.".into()))?,
    };
    to_json(&flame::stop(app, id).await?)
}
