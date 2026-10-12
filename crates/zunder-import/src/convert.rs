//! Conversion of ClickUp tasks into the desired Zunder state.
//!
//! The result describes, per entity, the field values the import wants to
//! see in Zunder ([`Desired`]). Turning that into operations (and diffing
//! against the server state) is the job of [`crate::ops`]. All checks that
//! the report shows are collected in [`Diagnostics`].

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{SecondsFormat, TimeZone, Timelike};
use chrono_tz::Tz;
use serde::Serialize;
use serde_json::Value;
use zunder_core::hlc::{Clock, NodeId};
use zunder_core::ids::clickup_entity_id;
use zunder_core::ids::Id;
use zunder_core::model::{fields, MAX_TITLE_CHARS};
use zunder_core::{
    Due, EntityKind, Link, Location, NewArea, NewProject, NewTask, OpBatch, ProjectKind,
    TaskStatus, TaskType,
};

use crate::clickup::model::Task;
use crate::fetch::TaskSet;
use crate::mapping::{Resolved, Target};

/// HLC node of all import operations (contract section 6).
pub const IMPORT_NODE: &str = "import";

/// Title of the link back to the ClickUp task.
pub const CLICKUP_LINK_TITLE: &str = "ClickUp";

/// Title used when a ClickUp task has none.
pub const EMPTY_TITLE: &str = "(ohne Titel)";

/// Task fields the import owns: if ClickUp has no value, the import resets
/// them in Zunder (`null`).
const OWNED_OPTIONAL_TASK_FIELDS: &[&str] = &[
    fields::task::DESCRIPTION,
    fields::task::PARENT_ID,
    fields::task::DUE,
    fields::task::LINKS,
    fields::task::RANK,
];

const OWNED_OPTIONAL_PROJECT_FIELDS: &[&str] = &[fields::project::REPO];

/// The desired field values of one entity. A `null` value means "must be
/// unset" (only written if Zunder currently has a value).
#[derive(Debug, Clone, PartialEq)]
pub struct Desired {
    pub kind: EntityKind,
    pub id: Id,
    /// Human-readable name for messages.
    pub label: String,
    pub fields: Vec<(String, Value)>,
    /// When the entity was created at the source (Unix ms). A new entity's
    /// operations are dated to this time instead of the import time, so
    /// `created_at` keeps the ClickUp history (see [`crate::ops::plan_ops`]).
    pub created_ms: Option<u64>,
    /// When a closed (done/dropped) task was closed at the source (Unix ms).
    /// The status operation of a new task is dated to this time, so tasks
    /// closed long ago do not count as "just closed" (3-day rule).
    pub closed_ms: Option<u64>,
}

impl Desired {
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.fields.iter().find(|(f, _)| f == name).map(|(_, v)| v)
    }
}

/// Result of [`convert`].
#[derive(Debug, Clone, Default)]
pub struct Conversion {
    /// Areas to create/update (not those mapped via `existing_id`).
    pub areas: Vec<Desired>,
    /// Projects to create/update, after the areas.
    pub projects: Vec<Desired>,
    /// Tasks, parents before their subtasks.
    pub tasks: Vec<Desired>,
    /// Zunder IDs of areas/projects mapped via `existing_id`.
    pub existing_refs: Vec<(EntityKind, Id, String)>,
    /// Projects mapped via `existing_id` with the `kind` the mapping file
    /// declares. The dev rules of the conversion use that kind, so `run`
    /// checks it against the real project in Zunder.
    pub existing_project_kinds: Vec<(Id, String, ProjectKind)>,
    pub diagnostics: Diagnostics,
}

impl Conversion {
    /// All desired entities in push order (areas, projects, tasks).
    pub fn entities(&self) -> impl Iterator<Item = &Desired> {
        self.areas.iter().chain(&self.projects).chain(&self.tasks)
    }
}

/// Everything the report shows. Serializes to the `--json` output.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Diagnostics {
    /// Tasks loaded from ClickUp (incl. fetched parents).
    pub tasks_loaded: usize,
    /// Tasks that will be imported.
    pub tasks_imported: usize,
    /// Tasks in lists with target `skip`.
    pub tasks_skipped: usize,
    /// Imported tasks per effective target (the root task's list target).
    pub targets: Vec<TargetCount>,
    /// Lists whose tasks have no target (list not in the mapping or target
    /// empty). Blocks `run`.
    pub without_target: Vec<ListIssue>,
    /// Statuses without a Zunder status. Blocks `run`.
    pub unmapped_statuses: Vec<StatusIssue>,
    /// Subtasks whose parent is missing or not imported; imported as
    /// top-level tasks of their own list target.
    pub orphans: Vec<TaskIssue>,
    /// `bug`/`feature`/`review` outside a dev project; imported as
    /// `aufgabe` / `in_progress`.
    pub dev_downgrades: Vec<TaskIssue>,
    /// Empty or overlong titles that were adjusted.
    pub title_fixes: Vec<TaskIssue>,
}

impl Diagnostics {
    /// Problems that must be fixed in the mapping before `run`.
    pub fn is_blocking(&self) -> bool {
        !self.without_target.is_empty() || !self.unmapped_statuses.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetCount {
    /// `area`, `project`, `inbox` or `loose`.
    pub kind: String,
    /// Mapping key of the area/project (empty for inbox/loose).
    pub key: String,
    pub name: String,
    pub tasks: usize,
    pub subtasks: usize,
    pub open: usize,
    pub closed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListIssue {
    pub list_id: String,
    pub path: String,
    pub tasks: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusIssue {
    pub list_id: String,
    pub path: String,
    pub status: String,
    pub tasks: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskIssue {
    pub clickup_id: String,
    pub title: String,
    pub detail: String,
}

/// Why a task cannot be imported (as decided per task, before parents).
enum Disposition<'m> {
    Import {
        target: &'m Target,
        status: TaskStatus,
    },
    Skip,
    NoTarget,
    UnmappedStatus,
}

/// Converts all tasks according to the mapping. `list_paths` provides
/// readable names for lists that are not in the mapping (may be empty).
pub fn convert(
    mapping: &Resolved,
    tasks: &TaskSet,
    list_paths: &HashMap<String, String>,
) -> Conversion {
    let mut conv = Conversion::default();
    let diag = &mut conv.diagnostics;
    diag.tasks_loaded = tasks.tasks.len();

    let path_of = |list_id: &str| -> String {
        mapping
            .lists
            .get(list_id)
            .map(|l| l.path.clone())
            .or_else(|| list_paths.get(list_id).cloned())
            .unwrap_or_else(|| format!("Liste {list_id}"))
    };

    // 1. Per-task disposition.
    let by_id: HashMap<&str, &Task> = tasks
        .tasks
        .iter()
        .map(|t| (t.item.id.as_str(), &t.item))
        .collect();
    let mut disposition: HashMap<&str, Disposition> = HashMap::new();
    let mut no_target: BTreeMap<String, usize> = BTreeMap::new();
    let mut unmapped: BTreeMap<(String, String), usize> = BTreeMap::new();
    for raw in &tasks.tasks {
        let task = &raw.item;
        let list_id = task.list.id.as_str();
        let d = match mapping.lists.get(list_id) {
            None | Some(crate::mapping::ResolvedList { target: None, .. }) => {
                *no_target.entry(list_id.to_owned()).or_default() += 1;
                Disposition::NoTarget
            }
            Some(list) => match list.target.as_ref().expect("checked above") {
                Target::Skip => Disposition::Skip,
                target => match list.statuses.get(&task.status.key()) {
                    Some(Some(status)) => Disposition::Import {
                        target,
                        status: *status,
                    },
                    _ => {
                        *unmapped
                            .entry((list_id.to_owned(), task.status.key()))
                            .or_default() += 1;
                        Disposition::UnmappedStatus
                    }
                },
            },
        };
        disposition.insert(task.id.as_str(), d);
    }
    diag.tasks_skipped = disposition
        .values()
        .filter(|d| matches!(d, Disposition::Skip))
        .count();
    diag.without_target = no_target
        .into_iter()
        .map(|(list_id, tasks)| ListIssue {
            path: path_of(&list_id),
            list_id,
            tasks,
        })
        .collect();
    diag.unmapped_statuses = unmapped
        .into_iter()
        .map(|((list_id, status), tasks)| StatusIssue {
            path: path_of(&list_id),
            list_id,
            status,
            tasks,
        })
        .collect();

    let importable = |id: &str| matches!(disposition.get(id), Some(Disposition::Import { .. }));

    // 2. Parents: an imported parent or none (orphan).
    let mut parent_of: HashMap<&str, &str> = HashMap::new();
    for raw in &tasks.tasks {
        let task = &raw.item;
        if !importable(&task.id) {
            continue;
        }
        let Some(parent) = task.parent.as_deref() else {
            continue;
        };
        let reason = if tasks.missing_parents.contains(parent) || !by_id.contains_key(parent) {
            Some(format!(
                "Eltern-Aufgabe {parent} existiert in ClickUp nicht (mehr)"
            ))
        } else {
            match disposition.get(parent) {
                Some(Disposition::Import { .. }) => None,
                Some(Disposition::Skip) => Some(format!(
                    "Eltern-Aufgabe {parent} liegt in einer übersprungenen Liste"
                )),
                _ => Some(format!(
                    "Eltern-Aufgabe {parent} kann (noch) nicht importiert werden"
                )),
            }
        };
        match reason {
            None => {
                parent_of.insert(task.id.as_str(), parent);
            }
            Some(detail) => diag.orphans.push(TaskIssue {
                clickup_id: task.id.clone(),
                title: task.name.clone(),
                detail: format!("{detail}; wird als eigenständige Aufgabe importiert"),
            }),
        }
    }
    // Break cycles (cannot happen in ClickUp, but never loop forever).
    let ids: Vec<&str> = parent_of.keys().copied().collect();
    for id in ids {
        let mut seen = HashSet::from([id]);
        let mut current = id;
        while let Some(&p) = parent_of.get(current) {
            if !seen.insert(p) {
                parent_of.remove(id);
                diag.orphans.push(TaskIssue {
                    clickup_id: id.to_owned(),
                    title: by_id[id].name.clone(),
                    detail: "Eltern-Kette bildet einen Zyklus; wird als eigenständige Aufgabe \
                             importiert"
                        .into(),
                });
                break;
            }
            current = p;
        }
    }
    let root_of = |id: &str| -> String {
        let mut current = id;
        while let Some(&p) = parent_of.get(current) {
            current = p;
        }
        current.to_owned()
    };
    let depth_of = |id: &str| -> usize {
        let mut depth = 0;
        let mut current = id;
        while let Some(&p) = parent_of.get(current) {
            depth += 1;
            current = p;
        }
        depth
    };

    // 3. Areas and projects.
    let location_of = |target: &Target| -> Location {
        match target {
            Target::Inbox | Target::Skip => Location::Inbox,
            Target::Loose => Location::Loose,
            Target::Area(k) => Location::Area {
                id: mapping.areas[k].id,
            },
            Target::Project(k) => Location::Project {
                id: mapping.projects[k].id,
            },
        }
    };
    let mut scratch_clock = Clock::new(NodeId::new(IMPORT_NODE).expect("valid node"));
    for area in mapping.areas.values() {
        if area.existing {
            conv.existing_refs
                .push((EntityKind::Area, area.id, area.name.clone()));
            continue;
        }
        let mut batch = OpBatch::new(&mut scratch_clock, 0);
        batch.create_area(NewArea {
            id: Some(area.id),
            name: area.name.clone(),
            state: area.state,
            vault_path: None,
        });
        conv.areas.push(desired(
            EntityKind::Area,
            area.id,
            format!("Bereich „{}“", area.name),
            batch,
            &[],
        ));
    }
    for project in mapping.projects.values() {
        if project.existing {
            conv.existing_refs
                .push((EntityKind::Project, project.id, project.name.clone()));
            conv.existing_project_kinds
                .push((project.id, project.name.clone(), project.kind));
            continue;
        }
        let mut batch = OpBatch::new(&mut scratch_clock, 0);
        batch.create_project(NewProject {
            id: Some(project.id),
            name: project.name.clone(),
            area_id: project.area_id,
            status: project.status,
            kind: project.kind,
            repo: project.repo.clone(),
            description: None,
            view: None,
            key: None,
        });
        conv.projects.push(desired(
            EntityKind::Project,
            project.id,
            format!("Projekt „{}“", project.name),
            batch,
            OWNED_OPTIONAL_PROJECT_FIELDS,
        ));
    }

    // 4. Tasks, parents first (stable within a depth).
    let mut order: Vec<(usize, usize, &Task)> = tasks
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| importable(&t.item.id))
        .map(|(i, t)| (depth_of(&t.item.id), i, &t.item))
        .collect();
    order.sort_by_key(|(depth, i, _)| (*depth, *i));

    let mut counts: BTreeMap<(u8, String), TargetCount> = BTreeMap::new();
    for (depth, _, task) in order {
        let Some(Disposition::Import { target, status }) = disposition.get(task.id.as_str()) else {
            unreachable!("only importable tasks are ordered")
        };
        let root = root_of(&task.id);
        let Some(Disposition::Import {
            target: root_target,
            ..
        }) = disposition.get(root.as_str())
        else {
            unreachable!("roots of imported tasks are imported")
        };
        let in_dev = matches!(root_target, Target::Project(k)
            if mapping.projects[k].kind == ProjectKind::Dev);

        let title = clean_title(&task.name);
        let trimmed = task.name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_TITLE_CHARS {
            conv.diagnostics.title_fixes.push(TaskIssue {
                clickup_id: task.id.clone(),
                title: task.name.clone(),
                detail: if task.name.trim().is_empty() {
                    format!("leerer Titel, importiert als „{EMPTY_TITLE}“")
                } else {
                    format!("Titel auf {MAX_TITLE_CHARS} Zeichen gekürzt")
                },
            });
        }

        let mut task_type = task
            .tags
            .iter()
            .find_map(|t| mapping.type_tags.get(&t.name.trim().to_lowercase()))
            .copied()
            .unwrap_or_default();
        let mut status = *status;
        if !in_dev {
            let mut notes = Vec::new();
            if task_type.is_dev_only() {
                notes.push(format!("Typ {task_type} → aufgabe"));
                task_type = TaskType::Aufgabe;
            }
            if status.is_dev_only() {
                notes.push(format!("Status {status} → in_progress"));
                status = TaskStatus::InProgress;
            }
            if !notes.is_empty() {
                conv.diagnostics.dev_downgrades.push(TaskIssue {
                    clickup_id: task.id.clone(),
                    title: task.name.clone(),
                    detail: format!("kein Dev-Projekt: {}", notes.join(", ")),
                });
            }
        }

        let id = clickup_entity_id(EntityKind::Task, &task.id);
        let parent_id = parent_of
            .get(task.id.as_str())
            .map(|p| clickup_entity_id(EntityKind::Task, p));
        let links = task
            .url
            .as_deref()
            .filter(|u| zunder_core::model::is_absolute_url(u))
            .map(|url| {
                vec![Link {
                    url: url.to_owned(),
                    title: Some(CLICKUP_LINK_TITLE.to_owned()),
                }]
            })
            .unwrap_or_default();
        let new = NewTask {
            id: Some(id),
            title: title.clone(),
            description: description(task),
            task_type,
            status,
            location: location_of(target),
            parent_id,
            due: task
                .due_date
                .and_then(|ms| due_from_clickup(ms, task.due_date_time, mapping.timezone)),
            links,
            clickup_id: Some(task.id.clone()),
            rank: task.orderindex.as_deref().and_then(rank_from_orderindex),
            ..NewTask::default()
        };
        let mut batch = OpBatch::new(&mut scratch_clock, 0);
        batch.create_task(new);
        let mut entity = desired(
            EntityKind::Task,
            id,
            format!("Aufgabe „{title}“ (ClickUp {})", task.id),
            batch,
            OWNED_OPTIONAL_TASK_FIELDS,
        );
        entity.created_ms = unix_ms(task.date_created);
        if status.is_closed() {
            entity.closed_ms = unix_ms(task.date_closed)
                .or(unix_ms(task.date_done))
                .or(unix_ms(task.date_updated));
        }
        conv.tasks.push(entity);

        // Counts per effective target.
        let (order_key, kind, key, name) = match root_target {
            Target::Area(k) => (1, "area", k.clone(), mapping.areas[k].name.clone()),
            Target::Project(k) => {
                let p = &mapping.projects[k];
                let area = mapping
                    .areas
                    .values()
                    .find(|a| a.id == p.area_id)
                    .map(|a| a.name.as_str())
                    .unwrap_or("?");
                (
                    2,
                    "project",
                    k.clone(),
                    format!("{area}{}{}", crate::fetch::PATH_SEPARATOR, p.name),
                )
            }
            Target::Inbox | Target::Skip => (3, "inbox", String::new(), "Inbox".to_owned()),
            Target::Loose => (4, "loose", String::new(), "Lose".to_owned()),
        };
        let entry = counts
            .entry((order_key, key.clone()))
            .or_insert_with(|| TargetCount {
                kind: kind.to_owned(),
                key,
                name,
                tasks: 0,
                subtasks: 0,
                open: 0,
                closed: 0,
            });
        if depth == 0 {
            entry.tasks += 1;
        } else {
            entry.subtasks += 1;
        }
        if status.is_closed() {
            entry.closed += 1;
        } else {
            entry.open += 1;
        }
    }
    conv.diagnostics.tasks_imported = conv.tasks.len();
    conv.diagnostics.targets = counts.into_values().collect();
    conv
}

/// Collects the field values the core builder produced for one entity and
/// adds `null` for owned optional fields without a value.
fn desired(
    kind: EntityKind,
    id: Id,
    label: String,
    batch: OpBatch<'_>,
    owned_optional: &[&str],
) -> Desired {
    let mut fields: Vec<(String, Value)> = batch
        .into_ops()
        .into_iter()
        .map(|op| (op.field, op.value))
        .collect();
    for field in owned_optional {
        if !fields.iter().any(|(f, _)| f == field) {
            fields.push(((*field).to_owned(), Value::Null));
        }
    }
    Desired {
        kind,
        id,
        label,
        fields,
        created_ms: None,
        closed_ms: None,
    }
}

/// A positive ClickUp timestamp as Unix milliseconds.
fn unix_ms(ms: Option<i64>) -> Option<u64> {
    ms.and_then(|ms| u64::try_from(ms).ok())
        .filter(|ms| *ms > 0)
}

/// Trimmed title, never empty, at most [`MAX_TITLE_CHARS`] characters.
pub fn clean_title(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return EMPTY_TITLE.to_owned();
    }
    if trimmed.chars().count() <= MAX_TITLE_CHARS {
        return trimmed.to_owned();
    }
    let mut out: String = trimmed.chars().take(MAX_TITLE_CHARS - 1).collect();
    out.push('…');
    out
}

/// Markdown description, falling back to plain text.
pub fn description(task: &Task) -> Option<String> {
    [
        &task.markdown_description,
        &task.text_content,
        &task.description,
    ]
    .into_iter()
    .flatten()
    .find(|s| !s.trim().is_empty())
    .cloned()
}

/// Converts a ClickUp due date (Unix ms) into a Zunder due value.
///
/// - `has_time = Some(true)`: date and time (RFC 3339 in `tz`, seconds).
/// - `has_time = Some(false)`: calendar date in `tz`.
/// - `has_time = None` (field missing in the response): date only if the
///   local time is exactly 00:00 or 04:00 (ClickUp's placeholders for
///   date-only due dates), otherwise date and time.
pub fn due_from_clickup(ms: i64, has_time: Option<bool>, tz: Tz) -> Option<Due> {
    let local = tz.timestamp_millis_opt(ms).single()?;
    let with_time = has_time.unwrap_or_else(|| {
        let placeholder = local.minute() == 0
            && local.second() == 0
            && local.nanosecond() == 0
            && matches!(local.hour(), 0 | 4);
        !placeholder
    });
    let wire = if with_time {
        local.to_rfc3339_opts(SecondsFormat::Secs, false)
    } else {
        local.format("%Y-%m-%d").to_string()
    };
    Due::parse(&wire)
}

/// Integer digits of a rank (enough for any ClickUp orderindex).
const RANK_INT_DIGITS: usize = 13;
const RANK_FRAC_DIGITS: usize = 6;

/// A lexicographically sortable rank from a ClickUp `orderindex`
/// (fixed width: 13 integer digits, 6 decimals; negative values become 0).
pub fn rank_from_orderindex(orderindex: &str) -> Option<String> {
    let value: f64 = orderindex.trim().parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    let max = 10f64.powi(RANK_INT_DIGITS as i32) - 1.0;
    let value = value.clamp(0.0, max);
    Some(format!(
        "{value:0width$.prec$}",
        width = RANK_INT_DIGITS + 1 + RANK_FRAC_DIGITS,
        prec = RANK_FRAC_DIGITS
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BERLIN: Tz = chrono_tz::Europe::Berlin;

    #[test]
    fn due_with_explicit_flag() {
        // 2026-10-12 07:30 UTC = 09:30 Berlin (CEST).
        let ms = 1_791_790_200_000;
        assert_eq!(
            due_from_clickup(ms, Some(true), BERLIN).unwrap().to_wire(),
            "2026-10-12T09:30:00+02:00"
        );
        assert_eq!(
            due_from_clickup(ms, Some(false), BERLIN).unwrap().to_wire(),
            "2026-10-12"
        );
    }

    #[test]
    fn due_date_only_uses_local_date() {
        // 2026-12-14 23:00 UTC = 2026-12-15 00:00 Berlin (CET): date-only due
        // dates must not fall on the previous day.
        let ms = 1_797_289_200_000;
        assert_eq!(
            due_from_clickup(ms, Some(false), BERLIN).unwrap().to_wire(),
            "2026-12-15"
        );
        // Without the flag, local midnight and 04:00 count as date-only.
        assert_eq!(
            due_from_clickup(ms, None, BERLIN).unwrap().to_wire(),
            "2026-12-15"
        );
        let four_am = ms + 4 * 3_600_000;
        assert_eq!(
            due_from_clickup(four_am, None, BERLIN).unwrap().to_wire(),
            "2026-12-15"
        );
        let other = ms + 9 * 3_600_000 + 15 * 60_000;
        assert_eq!(
            due_from_clickup(other, None, BERLIN).unwrap().to_wire(),
            "2026-12-15T09:15:00+01:00"
        );
    }

    #[test]
    fn ranks_sort_like_numbers() {
        let values = ["0", "1", "1.5", "2", "10", "100.25", "12345678"];
        let ranks: Vec<String> = values
            .iter()
            .map(|v| rank_from_orderindex(v).unwrap())
            .collect();
        let mut sorted = ranks.clone();
        sorted.sort();
        assert_eq!(ranks, sorted);
        assert_eq!(rank_from_orderindex("1.5").unwrap(), "0000000000001.500000");
        assert_eq!(
            rank_from_orderindex("-3").unwrap(),
            rank_from_orderindex("0").unwrap()
        );
        assert_eq!(rank_from_orderindex("abc"), None);
    }

    #[test]
    fn titles_are_cleaned() {
        assert_eq!(clean_title("  Hallo "), "Hallo");
        assert_eq!(clean_title("   "), EMPTY_TITLE);
        let long = "x".repeat(600);
        let cleaned = clean_title(&long);
        assert_eq!(cleaned.chars().count(), MAX_TITLE_CHARS);
        assert!(cleaned.ends_with('…'));
    }
}
