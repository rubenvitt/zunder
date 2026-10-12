//! Reading a workspace from ClickUp: hierarchy (spaces, folders, lists) and
//! tasks (all pages, closed ones, subtasks, missing parents).

use std::collections::{BTreeSet, HashSet};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::clickup::model::{self, Folder, List, Raw, Space, Task, Team};
use crate::clickup::{ClickUpApi, COMMENTS_PAGE_SIZE};

/// Safety net against an API that never reports its last page.
const MAX_PAGES: u32 = 10_000;

/// Separator used in human-readable list paths.
pub const PATH_SEPARATOR: &str = " › ";

/// A workspace with all spaces, folders and lists (active and archived).
#[derive(Debug, Clone)]
pub struct Hierarchy {
    pub team: Raw<Team>,
    pub spaces: Vec<SpaceNode>,
}

#[derive(Debug, Clone)]
pub struct SpaceNode {
    pub space: Raw<Space>,
    pub folders: Vec<FolderNode>,
    /// Lists directly in the space (no folder).
    pub lists: Vec<Raw<List>>,
}

#[derive(Debug, Clone)]
pub struct FolderNode {
    pub folder: Raw<Folder>,
    pub lists: Vec<Raw<List>>,
}

/// A list together with its position in the hierarchy.
#[derive(Debug, Clone, Copy)]
pub struct ListRef<'a> {
    pub space: &'a Space,
    pub folder: Option<&'a Folder>,
    pub list: &'a Raw<List>,
}

impl ListRef<'_> {
    /// `Space › Folder › List` (without folder: `Space › List`).
    pub fn path(&self) -> String {
        let mut parts = vec![self.space.name.as_str()];
        if let Some(folder) = self.folder {
            parts.push(folder.name.as_str());
        }
        parts.push(self.list.item.name.as_str());
        parts.join(PATH_SEPARATOR)
    }

    /// Archived itself or inside an archived folder/space.
    pub fn is_archived(&self) -> bool {
        self.list.item.archived || self.space.archived || self.folder.is_some_and(|f| f.archived)
    }
}

impl Hierarchy {
    /// All lists in hierarchy order (per space: folders first, then
    /// folderless lists).
    pub fn lists(&self) -> Vec<ListRef<'_>> {
        let mut out = Vec::new();
        for node in &self.spaces {
            for folder in &node.folders {
                for list in &folder.lists {
                    out.push(ListRef {
                        space: &node.space.item,
                        folder: Some(&folder.folder.item),
                        list,
                    });
                }
            }
            for list in &node.lists {
                out.push(ListRef {
                    space: &node.space.item,
                    folder: None,
                    list,
                });
            }
        }
        out
    }

    pub fn list(&self, list_id: &str) -> Option<ListRef<'_>> {
        self.lists().into_iter().find(|l| l.list.item.id == list_id)
    }
}

fn parse_items<T: serde::de::DeserializeOwned>(
    body: &Value,
    key: &str,
    what: &str,
) -> Result<Vec<Raw<T>>> {
    model::items(body, key).map_err(|e| anyhow::anyhow!("{what}: {e}"))
}

/// Concatenates active and archived items, dropping duplicates by ID.
fn merge_unique<T>(
    active: Vec<Raw<T>>,
    archived: Vec<Raw<T>>,
    id: impl Fn(&T) -> &str,
) -> Vec<Raw<T>> {
    let mut seen = HashSet::new();
    active
        .into_iter()
        .chain(archived)
        .filter(|r| seen.insert(id(&r.item).to_owned()))
        .collect()
}

/// Picks the workspace: the given ID, or the only one the token can see.
pub async fn select_team(api: &impl ClickUpApi, team_id: Option<&str>) -> Result<Raw<Team>> {
    let body = api.teams().await?;
    let teams: Vec<Raw<Team>> = parse_items(&body, "teams", "Workspaces")?;
    if let Some(id) = team_id {
        return teams
            .into_iter()
            .find(|t| t.item.id == id)
            .with_context(|| format!("Workspace {id} ist mit diesem Token nicht sichtbar"));
    }
    match teams.len() {
        0 => bail!("Der Token sieht keinen ClickUp-Workspace"),
        1 => Ok(teams.into_iter().next().expect("one team")),
        _ => {
            let names: Vec<String> = teams
                .iter()
                .map(|t| format!("{} ({})", t.item.name, t.item.id))
                .collect();
            bail!(
                "Mehrere Workspaces sichtbar, bitte mit --team wählen: {}",
                names.join(", ")
            )
        }
    }
}

/// Reads spaces, folders and lists, each active and archived. With
/// `details`, every list is additionally read via `GET /list/{id}` (for its
/// statuses); the detailed response then replaces the raw list.
pub async fn fetch_hierarchy(
    api: &impl ClickUpApi,
    team: Raw<Team>,
    details: bool,
) -> Result<Hierarchy> {
    let team_id = team.item.id.clone();
    let spaces = merge_unique(
        parse_items(&api.spaces(&team_id, false).await?, "spaces", "Spaces")?,
        parse_items(&api.spaces(&team_id, true).await?, "spaces", "Spaces")?,
        |s: &Space| &s.id,
    );
    let mut nodes = Vec::with_capacity(spaces.len());
    for space in spaces {
        let space_id = space.item.id.clone();
        let folders = merge_unique(
            parse_items(&api.folders(&space_id, false).await?, "folders", "Ordner")?,
            parse_items(&api.folders(&space_id, true).await?, "folders", "Ordner")?,
            |f: &Folder| &f.id,
        );
        let mut folder_nodes = Vec::with_capacity(folders.len());
        for folder in folders {
            let folder_id = folder.item.id.clone();
            let lists = merge_unique(
                parse_items(
                    &api.folder_lists(&folder_id, false).await?,
                    "lists",
                    "Listen",
                )?,
                parse_items(
                    &api.folder_lists(&folder_id, true).await?,
                    "lists",
                    "Listen",
                )?,
                |l: &List| &l.id,
            );
            folder_nodes.push(FolderNode {
                folder,
                lists: with_details(api, lists, details).await?,
            });
        }
        let lists = merge_unique(
            parse_items(
                &api.folderless_lists(&space_id, false).await?,
                "lists",
                "Listen",
            )?,
            parse_items(
                &api.folderless_lists(&space_id, true).await?,
                "lists",
                "Listen",
            )?,
            |l: &List| &l.id,
        );
        nodes.push(SpaceNode {
            space,
            folders: folder_nodes,
            lists: with_details(api, lists, details).await?,
        });
    }
    Ok(Hierarchy {
        team,
        spaces: nodes,
    })
}

async fn with_details(
    api: &impl ClickUpApi,
    lists: Vec<Raw<List>>,
    details: bool,
) -> Result<Vec<Raw<List>>> {
    if !details {
        return Ok(lists);
    }
    let mut out = Vec::with_capacity(lists.len());
    for list in lists {
        let raw = api.list(&list.item.id).await?;
        let mut detailed: Raw<List> = model::item(raw).map_err(anyhow::Error::msg)?;
        // The detail response may omit the archived flag of the listing.
        detailed.item.archived |= list.item.archived;
        out.push(detailed);
    }
    Ok(out)
}

/// All raw task objects of one list (all pages, closed ones and subtasks
/// included).
pub async fn fetch_list_tasks_raw(
    api: &impl ClickUpApi,
    list_id: &str,
    archived: bool,
) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for page in 0..MAX_PAGES {
        let body = api.tasks_page(list_id, page, archived).await?;
        let tasks = match body.get("tasks") {
            Some(Value::Array(tasks)) => tasks.clone(),
            _ => bail!("Antwort für Aufgaben der Liste {list_id} ohne \"tasks\""),
        };
        let empty = tasks.is_empty();
        out.extend(tasks);
        let last = body
            .get("last_page")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if empty || last {
            return Ok(out);
        }
    }
    bail!("Liste {list_id}: mehr als {MAX_PAGES} Seiten, Abbruch")
}

/// Tasks of the given lists, deduplicated by ID, plus missing parents
/// fetched one by one. `archived` additionally loads archived tasks.
#[derive(Debug, Clone, Default)]
pub struct TaskSet {
    pub tasks: Vec<Raw<Task>>,
    /// Parent IDs ClickUp does not return (deleted or inaccessible).
    pub missing_parents: BTreeSet<String>,
}

pub async fn fetch_tasks(
    api: &impl ClickUpApi,
    lists: &[(String, String)],
    archived: bool,
) -> Result<TaskSet> {
    let mut set = TaskSet::default();
    let mut seen = HashSet::new();
    for (list_id, label) in lists {
        let mut raw = fetch_list_tasks_raw(api, list_id, false).await?;
        if archived {
            raw.extend(fetch_list_tasks_raw(api, list_id, true).await?);
        }
        eprintln!("  {label}: {} Aufgaben", raw.len());
        for value in raw {
            let task: Raw<Task> = model::item(value).map_err(anyhow::Error::msg)?;
            if seen.insert(task.item.id.clone()) {
                set.tasks.push(task);
            }
        }
    }
    let (parents, missing) = fetch_parents(api, &set.tasks).await?;
    set.tasks.extend(parents);
    set.missing_parents = missing;
    Ok(set)
}

/// Fetches parents that are not among `tasks` (archived, in another list or
/// deleted) one by one via `GET /task/{id}`, transitively. Returns the
/// additional tasks and the IDs ClickUp does not know.
pub async fn fetch_parents(
    api: &impl ClickUpApi,
    tasks: &[Raw<Task>],
) -> Result<(Vec<Raw<Task>>, BTreeSet<String>)> {
    let mut seen: HashSet<String> = tasks.iter().map(|t| t.item.id.clone()).collect();
    let mut extra: Vec<Raw<Task>> = Vec::new();
    let mut missing = BTreeSet::new();
    let mut pending: BTreeSet<String> = tasks
        .iter()
        .filter_map(|t| t.item.parent.clone())
        .filter(|p| !seen.contains(p))
        .collect();
    while let Some(id) = pending.pop_first() {
        if seen.contains(&id) || missing.contains(&id) {
            continue;
        }
        let task: Option<Raw<Task>> = match api.task(&id).await? {
            Some(value) => Some(model::item(value).map_err(anyhow::Error::msg)?),
            None => None,
        };
        match task {
            // A response for a different ID counts as missing.
            Some(task) if task.item.id == id => {
                if let Some(parent) = task.item.parent.clone() {
                    pending.insert(parent);
                }
                seen.insert(id);
                extra.push(task);
            }
            _ => {
                missing.insert(id);
            }
        }
    }
    Ok((extra, missing))
}

/// What comments belong to.
#[derive(Debug, Clone, Copy)]
pub enum CommentParent<'a> {
    Task(&'a str),
    List(&'a str),
}

/// A JSON string or number as string (ClickUp is not consistent).
fn scalar(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn comments_array(body: &Value, what: &str) -> Result<Vec<Value>> {
    match body.get("comments") {
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(Value::Null) | None => Ok(Vec::new()),
        Some(_) => bail!("Kommentare von {what}: \"comments\" ist keine Liste"),
    }
}

/// All comments of a task or list as raw JSON, newest first (all pages).
/// Every comment with replies additionally carries them, raw, under the
/// key `"replies"` (the only field the backup adds). Unknown task/list →
/// empty.
pub async fn fetch_comments(
    api: &impl ClickUpApi,
    parent: CommentParent<'_>,
) -> Result<Vec<Value>> {
    let what = match parent {
        CommentParent::Task(id) => format!("Aufgabe {id}"),
        CommentParent::List(id) => format!("Liste {id}"),
    };
    let mut out: Vec<Value> = Vec::new();
    let mut seen = HashSet::new();
    let mut start: Option<(String, String)> = None;
    for _ in 0..MAX_PAGES {
        let start_ref = start.as_ref().map(|(d, i)| (d.as_str(), i.as_str()));
        let body = match parent {
            CommentParent::Task(id) => api.task_comments(id, start_ref).await?,
            CommentParent::List(id) => api.list_comments(id, start_ref).await?,
        };
        let Some(body) = body else {
            break;
        };
        let page = comments_array(&body, &what)?;
        let oldest = page
            .last()
            .and_then(|c| Some((scalar(c.get("date"))?, scalar(c.get("id"))?)));
        let full = page.len() >= COMMENTS_PAGE_SIZE;
        let mut new = 0;
        for comment in page {
            if scalar(comment.get("id")).is_some_and(|id| !seen.insert(id)) {
                continue;
            }
            new += 1;
            out.push(comment);
        }
        // Next page from the oldest comment; stop on a short page or when a
        // page brings nothing new (never loop on an inclusive `start`).
        match oldest {
            Some(next) if full && new > 0 => start = Some(next),
            _ => break,
        }
    }
    for comment in &mut out {
        let replies = comment
            .get("reply_count")
            .and_then(|c| scalar(Some(c)))
            .and_then(|c| c.parse::<u64>().ok())
            .unwrap_or(0);
        let Some(id) = scalar(comment.get("id")).filter(|_| replies > 0) else {
            continue;
        };
        let body = api.comment_replies(&id).await?;
        let replies = match body {
            Some(body) => comments_array(&body, &format!("Kommentar {id}"))?,
            None => Vec::new(),
        };
        if let Value::Object(map) = comment {
            map.insert("replies".into(), Value::Array(replies));
        }
    }
    Ok(out)
}
