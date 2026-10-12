//! The mapping file (`mapping.toml`): which ClickUp list goes where in
//! Zunder and how ClickUp statuses translate to the Zunder workflow.
//!
//! [`propose`] builds a first version from the ClickUp hierarchy
//! (`zunder-import plan`), [`render`] writes it as commented TOML,
//! [`Mapping::resolve`] checks an edited file and turns it into typed,
//! ready-to-use form.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{bail, Context, Result};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use zunder_core::ids::Id;
use zunder_core::ids::{clickup_entity_id, deterministic_id, parse_id};
use zunder_core::model::is_valid_repo;
use zunder_core::{AreaState, EntityKind, ProjectKind, ProjectStatus, TaskStatus, TaskType};

use crate::fetch::Hierarchy;

/// Current format version of the mapping file.
pub const MAPPING_VERSION: u32 = 1;

/// Default time zone for due dates.
pub const DEFAULT_TIMEZONE: &str = "Europe/Berlin";

/// The mapping file as written and read (strings, human-editable).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub version: u32,
    /// ClickUp workspace ("team") the mapping belongs to.
    pub team_id: String,
    #[serde(default)]
    pub settings: Settings,
    /// ClickUp tag (lowercase) → Zunder task type (`bug`, `feature`).
    #[serde(default)]
    pub type_tags: BTreeMap<String, String>,
    #[serde(default)]
    pub areas: BTreeMap<String, AreaMapping>,
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectMapping>,
    /// Keyed by ClickUp list ID.
    #[serde(default)]
    pub lists: BTreeMap<String, ListMapping>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default)]
    pub import_archived_tasks: bool,
}

fn default_timezone() -> String {
    DEFAULT_TIMEZONE.to_owned()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timezone: default_timezone(),
            import_archived_tasks: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AreaMapping {
    pub name: String,
    /// ClickUp space ID; determines the stable Zunder ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clickup_id: Option<String>,
    /// Map onto an existing Zunder area instead of creating one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectMapping {
    pub name: String,
    /// Key of an area in this file.
    pub area: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// ClickUp list ID or `folder-<folder ID>`; determines the stable ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clickup_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListMapping {
    /// Informational: `Space › Folder › List`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `area:<key>`, `project:<key>`, `inbox`, `loose` or `skip`.
    #[serde(default)]
    pub target: String,
    /// Lowercase ClickUp status → Zunder status code; empty = not decided.
    #[serde(default)]
    pub statuses: BTreeMap<String, String>,
}

impl Mapping {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Zuordnungsdatei {} nicht lesbar", path.display()))?;
        Self::parse(&text).with_context(|| format!("Zuordnungsdatei {} fehlerhaft", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mapping: Mapping = toml::from_str(text)?;
        if mapping.version != MAPPING_VERSION {
            bail!(
                "Version {} wird nicht unterstützt (erwartet {MAPPING_VERSION})",
                mapping.version
            );
        }
        Ok(mapping)
    }

    /// Checks the whole file and returns the typed form. All problems are
    /// reported at once.
    pub fn resolve(&self) -> Result<Resolved> {
        let mut errors = Vec::new();

        let timezone = self.settings.timezone.parse::<Tz>().unwrap_or_else(|_| {
            errors.push(format!(
                "settings.timezone: unbekannte Zeitzone {:?}",
                self.settings.timezone
            ));
            Tz::UTC
        });

        let mut type_tags = HashMap::new();
        for (tag, value) in &self.type_tags {
            match value.parse::<TaskType>() {
                Ok(t) => {
                    type_tags.insert(tag.trim().to_lowercase(), t);
                }
                Err(_) => errors.push(format!(
                    "type_tags.{tag}: {value:?} ist kein Typ (aufgabe, bug, feature)"
                )),
            }
        }

        let mut areas = BTreeMap::new();
        for (key, area) in &self.areas {
            let ctx = format!("areas.{key}");
            if area.name.trim().is_empty() {
                errors.push(format!("{ctx}: name fehlt"));
            }
            let state = parse_code(&ctx, "state", area.state.as_deref(), &mut errors);
            let (id, existing) = entity_id(
                &ctx,
                EntityKind::Area,
                key,
                &area.clickup_id,
                &area.existing_id,
                &mut errors,
            );
            areas.insert(
                key.clone(),
                ResolvedArea {
                    key: key.clone(),
                    id,
                    name: area.name.trim().to_owned(),
                    state,
                    existing,
                },
            );
        }

        let mut projects = BTreeMap::new();
        for (key, project) in &self.projects {
            let ctx = format!("projects.{key}");
            if project.name.trim().is_empty() {
                errors.push(format!("{ctx}: name fehlt"));
            }
            let area_id = match areas.get(&project.area) {
                Some(a) => a.id,
                None => {
                    errors.push(format!(
                        "{ctx}: Bereich {:?} ist in [areas] nicht definiert",
                        project.area
                    ));
                    Id::MIN
                }
            };
            let kind: ProjectKind = parse_code(&ctx, "kind", project.kind.as_deref(), &mut errors);
            let status = parse_code(&ctx, "status", project.status.as_deref(), &mut errors);
            let repo = project
                .repo
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty());
            if let Some(repo) = repo {
                if kind != ProjectKind::Dev {
                    errors.push(format!("{ctx}: repo ist nur bei kind = \"dev\" erlaubt"));
                } else if !is_valid_repo(repo) {
                    errors.push(format!(
                        "{ctx}: repo {repo:?} hat nicht die Form \"owner/name\""
                    ));
                }
            }
            let (id, existing) = entity_id(
                &ctx,
                EntityKind::Project,
                key,
                &project.clickup_id,
                &project.existing_id,
                &mut errors,
            );
            projects.insert(
                key.clone(),
                ResolvedProject {
                    key: key.clone(),
                    id,
                    name: project.name.trim().to_owned(),
                    area_id,
                    status,
                    kind,
                    repo: repo.map(str::to_owned),
                    existing,
                },
            );
        }

        let mut lists = HashMap::new();
        for (list_id, list) in &self.lists {
            let ctx = format!("lists.{list_id}");
            let target = match Target::parse(&list.target) {
                Ok(t) => t,
                Err(e) => {
                    errors.push(format!("{ctx}.target: {e}"));
                    None
                }
            };
            match &target {
                Some(Target::Area(k)) if !areas.contains_key(k) => errors.push(format!(
                    "{ctx}.target: Bereich {k:?} ist in [areas] nicht definiert"
                )),
                Some(Target::Project(k)) if !projects.contains_key(k) => errors.push(format!(
                    "{ctx}.target: Projekt {k:?} ist in [projects] nicht definiert"
                )),
                _ => {}
            }
            let mut statuses = HashMap::new();
            for (name, value) in &list.statuses {
                let value = value.trim();
                let status = if value.is_empty() {
                    None
                } else {
                    match value.parse::<TaskStatus>() {
                        Ok(s) => Some(s),
                        Err(_) => {
                            errors.push(format!(
                                "{ctx}.statuses.{name:?}: {value:?} ist kein Zunder-Status \
                                 (open, in_progress, review, blocked, done, dropped)"
                            ));
                            None
                        }
                    }
                };
                statuses.insert(name.trim().to_lowercase(), status);
            }
            lists.insert(
                list_id.clone(),
                ResolvedList {
                    path: list.path.clone().unwrap_or_else(|| list_id.clone()),
                    target,
                    statuses,
                },
            );
        }

        if !errors.is_empty() {
            bail!(
                "Die Zuordnungsdatei enthält Fehler:\n  - {}",
                errors.join("\n  - ")
            );
        }
        Ok(Resolved {
            team_id: self.team_id.clone(),
            timezone,
            import_archived_tasks: self.settings.import_archived_tasks,
            type_tags,
            areas,
            projects,
            lists,
        })
    }
}

fn parse_code<T: std::str::FromStr + Default>(
    ctx: &str,
    field: &str,
    value: Option<&str>,
    errors: &mut Vec<String>,
) -> T {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => T::default(),
        Some(v) => v.parse().unwrap_or_else(|_| {
            errors.push(format!("{ctx}.{field}: ungültiger Wert {v:?}"));
            T::default()
        }),
    }
}

/// The Zunder ID of an area or project:
///
/// - `existing_id` → that ID (nothing is created)
/// - `clickup_id` → `deterministic_id("clickup:<kind>:<clickup_id>")`
/// - neither → `deterministic_id("import:<kind>:<key>")`
fn entity_id(
    ctx: &str,
    kind: EntityKind,
    key: &str,
    clickup_id: &Option<String>,
    existing_id: &Option<String>,
    errors: &mut Vec<String>,
) -> (Id, bool) {
    if let Some(existing) = existing_id.as_deref().map(str::trim) {
        return match parse_id(existing) {
            Some(id) => (id, true),
            None => {
                errors.push(format!(
                    "{ctx}.existing_id: {existing:?} ist keine Zunder-ID"
                ));
                (Id::MIN, true)
            }
        };
    }
    match clickup_id
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        Some(c) => (clickup_entity_id(kind, c), false),
        None => (
            deterministic_id(&format!("import:{}:{key}", kind.as_str())),
            false,
        ),
    }
}

/// Where the tasks of a list go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Skip,
    Inbox,
    Loose,
    Area(String),
    Project(String),
}

impl Target {
    /// Parses a target; an empty string means "not decided" (`None`).
    pub fn parse(s: &str) -> Result<Option<Self>, String> {
        let s = s.trim();
        Ok(Some(match s {
            "" => return Ok(None),
            "skip" => Target::Skip,
            "inbox" => Target::Inbox,
            "loose" => Target::Loose,
            _ => match s.split_once(':') {
                Some(("area", k)) if !k.trim().is_empty() => Target::Area(k.trim().to_owned()),
                Some(("project", k)) if !k.trim().is_empty() => {
                    Target::Project(k.trim().to_owned())
                }
                _ => {
                    return Err(format!(
                        "{s:?} ist kein Ziel (area:<key>, project:<key>, inbox, loose, skip)"
                    ))
                }
            },
        }))
    }

    pub fn to_wire(&self) -> String {
        match self {
            Target::Skip => "skip".into(),
            Target::Inbox => "inbox".into(),
            Target::Loose => "loose".into(),
            Target::Area(k) => format!("area:{k}"),
            Target::Project(k) => format!("project:{k}"),
        }
    }
}

/// A checked mapping file.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub team_id: String,
    pub timezone: Tz,
    pub import_archived_tasks: bool,
    pub type_tags: HashMap<String, TaskType>,
    pub areas: BTreeMap<String, ResolvedArea>,
    pub projects: BTreeMap<String, ResolvedProject>,
    pub lists: HashMap<String, ResolvedList>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedArea {
    pub key: String,
    pub id: Id,
    pub name: String,
    pub state: AreaState,
    /// Maps onto an existing Zunder area; nothing is written for it.
    pub existing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProject {
    pub key: String,
    pub id: Id,
    pub name: String,
    pub area_id: Id,
    pub status: ProjectStatus,
    pub kind: ProjectKind,
    pub repo: Option<String>,
    pub existing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedList {
    pub path: String,
    /// `None` = no target decided yet.
    pub target: Option<Target>,
    /// `None` value = status not decided yet.
    pub statuses: HashMap<String, Option<TaskStatus>>,
}

// ----- proposal -------------------------------------------------------------

/// List names that stand for "the area itself" rather than a project.
const GENERIC_LIST_NAMES: &[&str] = &[
    "list",
    "liste",
    "allgemein",
    "general",
    "aufgaben",
    "tasks",
    "todo",
    "to do",
    "todos",
    "sonstiges",
    "misc",
];

const INBOX_LIST_NAMES: &[&str] = &["inbox", "eingang", "posteingang", "eingangskorb"];

/// Proposes a Zunder status for a ClickUp status name and type
/// (`open`/`custom`/`done`/`closed`). `None` = no confident guess.
///
/// A status of type `done` or `closed` is always proposed as `done` or
/// `dropped` (spec section 8: closed tasks are imported as done/dropped),
/// whatever its name suggests ("Reviewed", "In Progress done").
pub fn propose_status(name: &str, kind: &str) -> Option<TaskStatus> {
    let norm: String = name
        .to_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let exact = |words: &[&str]| words.contains(&norm.as_str());
    let contains = |parts: &[&str]| parts.iter().any(|p| norm.contains(p));
    let dropped = exact(&[
        "cancelled",
        "canceled",
        "verworfen",
        "abgebrochen",
        "dropped",
        "won't do",
        "wont do",
        "obsolete",
        "rejected",
        "abgelehnt",
        "storniert",
    ]) || contains(&["cancel", "verworfen", "abgebrochen", "won't", "obsolet"]);

    if matches!(kind, "done" | "closed") {
        return Some(if dropped {
            TaskStatus::Dropped
        } else {
            TaskStatus::Done
        });
    }

    if exact(&[
        "to do",
        "todo",
        "open",
        "offen",
        "backlog",
        "new",
        "neu",
        "planned",
        "geplant",
        "ready",
        "bereit",
        "not started",
        "nicht begonnen",
        "zu erledigen",
        "später",
        "later",
        "someday",
        "ideen",
        "ideas",
    ]) {
        return Some(TaskStatus::Open);
    }
    if exact(&[
        "in progress",
        "doing",
        "in arbeit",
        "in bearbeitung",
        "wip",
        "working",
        "active",
        "aktiv",
        "started",
        "begonnen",
        "läuft",
        "in umsetzung",
    ]) {
        return Some(TaskStatus::InProgress);
    }
    if exact(&[
        "review",
        "in review",
        "code review",
        "qa",
        "testing",
        "test",
        "prüfung",
        "in prüfung",
        "abnahme",
    ]) {
        return Some(TaskStatus::Review);
    }
    if exact(&[
        "blocked",
        "blockiert",
        "waiting",
        "wartet",
        "warten",
        "on hold",
        "paused",
        "pausiert",
        "pending",
    ]) {
        return Some(TaskStatus::Blocked);
    }
    if exact(&[
        "complete",
        "completed",
        "done",
        "closed",
        "erledigt",
        "fertig",
        "abgeschlossen",
        "finished",
        "resolved",
        "shipped",
        "released",
        "geschlossen",
    ]) {
        return Some(TaskStatus::Done);
    }
    // Looser keyword matches.
    if dropped {
        return Some(TaskStatus::Dropped);
    }
    // "unblocked" / "entblockt" mean the opposite of blocked.
    if contains(&["block", "waiting", "wartet", "warten auf", "on hold"])
        && !contains(&["unblock", "entblock"])
    {
        return Some(TaskStatus::Blocked);
    }
    if contains(&["review", "prüfung"]) {
        return Some(TaskStatus::Review);
    }
    if contains(&["progress", "in arbeit", "doing"]) {
        return Some(TaskStatus::InProgress);
    }
    match kind {
        "open" => Some(TaskStatus::Open),
        _ => None,
    }
}

/// Lowercase ASCII slug for mapping keys (`Küche & Bad` → `kueche-bad`).
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        match c {
            'ä' => out.push_str("ae"),
            'ö' => out.push_str("oe"),
            'ü' => out.push_str("ue"),
            'ß' => out.push_str("ss"),
            c if c.is_ascii_alphanumeric() => out.push(c),
            _ => {
                if !out.ends_with('-') {
                    out.push('-');
                }
            }
        }
    }
    out.trim_matches('-').to_owned()
}

fn unique_key(base: String, fallback: &str, taken: &BTreeMap<String, impl Sized>) -> String {
    let base = if base.is_empty() {
        fallback.to_owned()
    } else {
        base
    };
    if !taken.contains_key(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|k| !taken.contains_key(k))
        .expect("some suffix is free")
}

/// Proposes a mapping for the whole hierarchy (`zunder-import plan`).
///
/// - Every space becomes an area (archived space → `state = "archived"`).
/// - Every folder becomes a project in the space's area; all its lists go
///   there (archived folder → `status = "completed"`).
/// - A list without folder becomes its own project, except generic names
///   ("Allgemein", "Tasks", …) or the space's own name, which go to the area
///   directly, and inbox-like names, which go to the Zunder inbox.
/// - Statuses are translated by [`propose_status`]; unknown ones stay empty.
pub fn propose(hierarchy: &Hierarchy) -> Mapping {
    let mut areas: BTreeMap<String, AreaMapping> = BTreeMap::new();
    let mut projects: BTreeMap<String, ProjectMapping> = BTreeMap::new();
    let mut lists = BTreeMap::new();

    for node in &hierarchy.spaces {
        let space = &node.space.item;
        let area_key = unique_key(slug(&space.name), "bereich", &areas);
        areas.insert(
            area_key.clone(),
            AreaMapping {
                name: space.name.trim().to_owned(),
                clickup_id: Some(space.id.clone()),
                existing_id: None,
                state: space.archived.then(|| AreaState::Archived.to_string()),
            },
        );

        for folder_node in &node.folders {
            let folder = &folder_node.folder.item;
            let project_key = unique_key(slug(&folder.name), "projekt", &projects);
            projects.insert(
                project_key.clone(),
                ProjectMapping {
                    name: folder.name.trim().to_owned(),
                    area: area_key.clone(),
                    kind: Some(ProjectKind::Standard.to_string()),
                    repo: None,
                    status: folder
                        .archived
                        .then(|| ProjectStatus::Completed.to_string()),
                    clickup_id: Some(format!("folder-{}", folder.id)),
                    existing_id: None,
                },
            );
            for list in &folder_node.lists {
                lists.insert(
                    list.item.id.clone(),
                    list_mapping(
                        hierarchy,
                        &list.item.id,
                        Target::Project(project_key.clone()),
                    ),
                );
            }
        }

        for list in &node.lists {
            let name = list.item.name.trim().to_lowercase();
            let target = if INBOX_LIST_NAMES.contains(&name.as_str()) {
                Target::Inbox
            } else if GENERIC_LIST_NAMES.contains(&name.as_str())
                || name == space.name.trim().to_lowercase()
            {
                Target::Area(area_key.clone())
            } else {
                let project_key = unique_key(slug(&list.item.name), "projekt", &projects);
                projects.insert(
                    project_key.clone(),
                    ProjectMapping {
                        name: list.item.name.trim().to_owned(),
                        area: area_key.clone(),
                        kind: Some(ProjectKind::Standard.to_string()),
                        repo: None,
                        status: list
                            .item
                            .archived
                            .then(|| ProjectStatus::Completed.to_string()),
                        clickup_id: Some(list.item.id.clone()),
                        existing_id: None,
                    },
                );
                Target::Project(project_key)
            };
            lists.insert(
                list.item.id.clone(),
                list_mapping(hierarchy, &list.item.id, target),
            );
        }
    }

    Mapping {
        version: MAPPING_VERSION,
        team_id: hierarchy.team.item.id.clone(),
        settings: Settings::default(),
        type_tags: [("bug", "bug"), ("feature", "feature")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
        areas,
        projects,
        lists,
    }
}

fn list_mapping(hierarchy: &Hierarchy, list_id: &str, target: Target) -> ListMapping {
    let list_ref = hierarchy.list(list_id).expect("list from hierarchy");
    let mut statuses = BTreeMap::new();
    for status in &list_ref.list.item.statuses {
        let key = status.key();
        if key.is_empty() {
            continue;
        }
        let proposed = propose_status(&status.status, &status.kind)
            .map(|s| s.to_string())
            .unwrap_or_default();
        statuses.insert(key, proposed);
    }
    ListMapping {
        path: Some(list_ref.path()),
        target: target.to_wire(),
        statuses,
    }
}

// ----- rendering --------------------------------------------------------------

/// TOML basic string with escapes.
pub fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A key: bare when possible, quoted otherwise.
pub fn toml_key(s: &str) -> String {
    let bare = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare {
        s.to_owned()
    } else {
        toml_str(s)
    }
}

/// Writes the mapping as commented, human-editable TOML.
pub fn render(mapping: &Mapping, team_name: &str, generated_on: &str) -> String {
    let mut o = String::new();
    let w = &mut o;
    let _ = writeln!(
        w,
        "# zunder-import – Zuordnungsdatei\n\
         #\n\
         # Vorgeschlagen von `zunder-import plan` am {generated_on} für den ClickUp-Workspace\n\
         # {} ({}). Bitte prüfen und anpassen, dann mit `zunder-import report` kontrollieren.\n\
         #\n\
         # Bereiche und Projekte bekommen stabile IDs aus ihrer ClickUp-ID (clickup_id).\n\
         # Mit existing_id = \"<zunder_id>\" wird stattdessen auf einen bestehenden Zunder-Bereich\n\
         # bzw. ein bestehendes Projekt abgebildet; dann legt der Import dafür nichts an.\n\
         # Eigene Bereiche/Projekte dürfen ergänzt werden (ohne clickup_id: ID aus dem Schlüssel).\n\
         #\n\
         # Ziele (target) je Liste: \"area:<schlüssel>\", \"project:<schlüssel>\", \"inbox\",\n\
         # \"loose\" (Lose) oder \"skip\" (nicht importieren). Leer = noch offen.\n\
         #\n\
         # Status je Liste (klein geschrieben) → Zunder-Status: open, in_progress, review,\n\
         # blocked, done, dropped. Leer = noch offen (der Import verweigert dann den Lauf).\n\
         # \"review\" sowie die Typen bug/feature wirken nur in Dev-Projekten; anderswo\n\
         # wird daraus \"in_progress\" bzw. \"aufgabe\" (der Bericht zeigt das an).\n",
        toml_str(team_name),
        mapping.team_id
    );
    let _ = writeln!(w, "version = {}", mapping.version);
    let _ = writeln!(w, "team_id = {}\n", toml_str(&mapping.team_id));

    let _ = writeln!(w, "[settings]");
    let _ = writeln!(
        w,
        "# Zeitzone, in der Fälligkeiten aus ClickUp gelesen werden (IANA-Name)."
    );
    let _ = writeln!(w, "timezone = {}", toml_str(&mapping.settings.timezone));
    let _ = writeln!(
        w,
        "# true = auch archivierte ClickUp-Aufgaben importieren (Status laut Zuordnung)."
    );
    let _ = writeln!(
        w,
        "import_archived_tasks = {}\n",
        mapping.settings.import_archived_tasks
    );

    let _ = writeln!(
        w,
        "# ClickUp-Tag (klein geschrieben) → Zunder-Typ (\"bug\" oder \"feature\")."
    );
    let _ = writeln!(w, "[type_tags]");
    for (tag, value) in &mapping.type_tags {
        let _ = writeln!(w, "{} = {}", toml_key(tag), toml_str(value));
    }

    let _ = writeln!(
        w,
        "\n# ── Bereiche ────────────────────────────────────────────────────────────────"
    );
    for (key, area) in &mapping.areas {
        let _ = writeln!(w, "\n[areas.{}]", toml_key(key));
        let _ = writeln!(w, "name = {}", toml_str(&area.name));
        if let Some(c) = &area.clickup_id {
            let _ = writeln!(w, "clickup_id = {}", toml_str(c));
        }
        match &area.state {
            Some(s) => {
                let _ = writeln!(w, "state = {}", toml_str(s));
            }
            None => {
                let _ = writeln!(w, "# state = \"archived\"");
            }
        }
        match &area.existing_id {
            Some(e) => {
                let _ = writeln!(w, "existing_id = {}", toml_str(e));
            }
            None => {
                let _ = writeln!(w, "# existing_id = \"…\"");
            }
        }
    }

    let _ = writeln!(
        w,
        "\n# ── Projekte ────────────────────────────────────────────────────────────────\n\
         # kind = \"dev\" für Entwicklungsprojekte (Board, bug/feature/review erlaubt),\n\
         # dann optional repo = \"owner/name\". status: active, paused, completed.\n\
         # Mit existing_id wird nichts angelegt; kind muss dann der Art des vorhandenen\n\
         # Zunder-Projekts entsprechen (run prüft das)."
    );
    for (key, project) in &mapping.projects {
        let _ = writeln!(w, "\n[projects.{}]", toml_key(key));
        let _ = writeln!(w, "name = {}", toml_str(&project.name));
        let _ = writeln!(w, "area = {}", toml_str(&project.area));
        match &project.kind {
            Some(k) => {
                let _ = writeln!(w, "kind = {}", toml_str(k));
            }
            None => {
                let _ = writeln!(w, "# kind = \"dev\"");
            }
        }
        match &project.repo {
            Some(r) => {
                let _ = writeln!(w, "repo = {}", toml_str(r));
            }
            None => {
                let _ = writeln!(w, "# repo = \"owner/name\"");
            }
        }
        if let Some(s) = &project.status {
            let _ = writeln!(w, "status = {}", toml_str(s));
        }
        if let Some(c) = &project.clickup_id {
            let _ = writeln!(w, "clickup_id = {}", toml_str(c));
        }
        match &project.existing_id {
            Some(e) => {
                let _ = writeln!(w, "existing_id = {}", toml_str(e));
            }
            None => {
                let _ = writeln!(w, "# existing_id = \"…\"");
            }
        }
    }

    let _ = writeln!(
        w,
        "\n# ── Listen ──────────────────────────────────────────────────────────────────"
    );
    for (id, list) in &mapping.lists {
        let _ = writeln!(w, "\n[lists.{}]", toml_str(id));
        if let Some(path) = &list.path {
            let _ = writeln!(w, "path = {}", toml_str(path));
        }
        let _ = writeln!(w, "target = {}", toml_str(&list.target));
        let _ = writeln!(w, "[lists.{}.statuses]", toml_str(id));
        for (name, value) in &list.statuses {
            if value.is_empty() {
                let _ = writeln!(
                    w,
                    "{} = \"\"  # UNBEKANNT – bitte festlegen",
                    toml_str(name)
                );
            } else {
                let _ = writeln!(w, "{} = {}", toml_str(name), toml_str(value));
            }
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_heuristics() {
        use TaskStatus::*;
        let cases = [
            ("to do", "open", Some(Open)),
            ("Open", "open", Some(Open)),
            ("BACKLOG", "custom", Some(Open)),
            ("in progress", "custom", Some(InProgress)),
            ("Doing", "custom", Some(InProgress)),
            ("in-arbeit", "custom", Some(InProgress)),
            ("review", "custom", Some(Review)),
            ("Code Review", "custom", Some(Review)),
            ("blocked", "custom", Some(Blocked)),
            ("Waiting for feedback", "custom", Some(Blocked)),
            ("complete", "closed", Some(Done)),
            ("done", "done", Some(Done)),
            ("Closed", "closed", Some(Done)),
            ("cancelled", "closed", Some(Dropped)),
            ("verworfen", "custom", Some(Dropped)),
            ("eingefroren", "custom", None),
            ("Abgehakt", "closed", Some(Done)),
            ("Startklar", "open", Some(Open)),
            // A done/closed type always wins over keywords in the name.
            ("Reviewed", "done", Some(Done)),
            ("Reviewed", "closed", Some(Done)),
            ("Review", "closed", Some(Done)),
            ("In Progress done", "closed", Some(Done)),
            ("Blocked (closed)", "closed", Some(Done)),
            ("Cancelled - won't fix", "closed", Some(Dropped)),
            ("verworfen", "done", Some(Dropped)),
            ("Unblocked", "custom", None),
        ];
        for (name, kind, expected) in cases {
            assert_eq!(propose_status(name, kind), expected, "{name}");
        }
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Küche & Bad"), "kueche-bad");
        assert_eq!(slug("  Straße 12 "), "strasse-12");
        assert_eq!(slug("!!!"), "");
    }

    #[test]
    fn targets_parse() {
        assert_eq!(Target::parse("").unwrap(), None);
        assert_eq!(Target::parse("skip").unwrap(), Some(Target::Skip));
        assert_eq!(
            Target::parse("project:web").unwrap(),
            Some(Target::Project("web".into()))
        );
        assert!(Target::parse("project:").is_err());
        assert!(Target::parse("folder:x").is_err());
    }

    #[test]
    fn toml_escaping_roundtrips() {
        let tricky = "Zitat \"x\" \\ Tab\t Ende";
        let doc = format!("{} = {}", toml_key("a b"), toml_str(tricky));
        let parsed: toml::Table = toml::from_str(&doc).unwrap();
        assert_eq!(parsed["a b"].as_str(), Some(tricky));
    }

    fn base_mapping() -> Mapping {
        Mapping::parse(
            r#"
            version = 1
            team_id = "1"
            [areas.privat]
            name = "Privat"
            clickup_id = "10"
            [projects.web]
            name = "Web"
            area = "privat"
            kind = "dev"
            repo = "beispiel/web"
            [lists."100"]
            target = "project:web"
            [lists."100".statuses]
            "to do" = "open"
            "qa" = ""
            "#,
        )
        .unwrap()
    }

    #[test]
    fn resolve_derives_ids_and_keeps_open_statuses() {
        let r = base_mapping().resolve().unwrap();
        let area = &r.areas["privat"];
        assert_eq!(area.id, clickup_entity_id(EntityKind::Area, "10"));
        assert!(!area.existing);
        let project = &r.projects["web"];
        assert_eq!(project.area_id, area.id);
        assert_eq!(project.kind, ProjectKind::Dev);
        assert_eq!(
            project.id,
            deterministic_id("import:project:web"),
            "without clickup_id the key decides"
        );
        let list = &r.lists["100"];
        assert_eq!(list.statuses["to do"], Some(TaskStatus::Open));
        assert_eq!(list.statuses["qa"], None);
        assert_eq!(r.timezone, chrono_tz::Europe::Berlin);
    }

    #[test]
    fn resolve_reports_all_errors() {
        let mut m = base_mapping();
        m.projects.get_mut("web").unwrap().kind = Some("standard".into());
        m.lists.get_mut("100").unwrap().target = "area:nirgends".into();
        m.lists
            .get_mut("100")
            .unwrap()
            .statuses
            .insert("x".into(), "erledigt".into());
        m.areas.get_mut("privat").unwrap().existing_id = Some("ABC".into());
        let err = m.resolve().unwrap_err().to_string();
        assert!(err.contains("repo ist nur bei kind"), "{err}");
        assert!(err.contains("nirgends"), "{err}");
        assert!(err.contains("\"erledigt\" ist kein Zunder-Status"), "{err}");
        assert!(err.contains("existing_id"), "{err}");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err =
            Mapping::parse("version = 1\nteam_id = \"1\"\n[areas.a]\nnmae = \"x\"\n").unwrap_err();
        assert!(format!("{err:#}").contains("nmae"), "{err:#}");
    }
}
