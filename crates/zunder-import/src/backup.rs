//! `zunder-import backup`: complete raw JSON export of the ClickUp data.
//!
//! Layout of the backup directory:
//!
//! ```text
//! manifest.json                       summary (counts, time, version)
//! teams.json                          GET /team
//! teams/<team>/spaces.json            spaces (active + archived)
//! teams/<team>/time_entries.json      all time entries (chunked by window)
//! spaces/<space>/folders.json         folders (active + archived)
//! spaces/<space>/lists.json           lists without folder
//! folders/<folder>/lists.json         lists of a folder
//! lists/<list>/list.json              GET /list/{id} (statuses etc.)
//! lists/<list>/tasks.json             all tasks incl. closed, archived, subtasks
//! tasks/parents.json                  parents outside the lists (GET /task/{id})
//! ```

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::clickup::model::{self, Raw, Task};
use crate::clickup::ClickUpApi;
use crate::fetch;

/// Name of the default backup directory below the home directory.
pub const DEFAULT_DIR_NAME: &str = "zunder-import-backup";

/// One day in milliseconds.
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone)]
pub struct BackupOptions {
    /// Base directory; the backup goes into a timestamped subdirectory.
    pub base_dir: PathBuf,
    /// Allow writing inside a git repository.
    pub force: bool,
    /// Start of the time entry export (Unix ms).
    pub since_ms: i64,
    /// End of the time entry export (Unix ms), usually now.
    pub until_ms: i64,
    /// Size of one time entry request window in days.
    pub window_days: u32,
    /// Name of the timestamped subdirectory.
    pub stamp: String,
}

/// The default base directory: `~/zunder-import-backup`.
pub fn default_base_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|home| PathBuf::from(home).join(DEFAULT_DIR_NAME))
}

/// The git repository (directory containing `.git`) that `path` lies in, if
/// any. Works for paths that do not exist yet; symlinks of the existing part
/// are resolved as well.
pub fn enclosing_git_repo(path: &Path) -> Option<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let mut candidates = vec![absolute.clone()];
    if let Some(existing) = absolute.ancestors().find(|a| a.exists()) {
        if let Ok(canonical) = existing.canonicalize() {
            candidates.push(canonical);
        }
    }
    candidates.iter().find_map(|c| {
        c.ancestors()
            .find(|a| a.join(".git").exists())
            .map(Path::to_path_buf)
    })
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TeamCounts {
    pub id: String,
    pub name: String,
    pub spaces: usize,
    pub folders: usize,
    pub lists: usize,
    pub tasks: usize,
    pub time_entries: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Manifest {
    pub created_at: String,
    pub tool: String,
    pub time_entries_from_ms: i64,
    pub time_entries_until_ms: i64,
    pub teams: Vec<TeamCounts>,
    pub parents_fetched: usize,
    pub missing_parents: Vec<String>,
}

struct Writer {
    root: PathBuf,
}

impl Writer {
    fn write(&self, relative: &str, value: &impl Serialize) -> Result<()> {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            create_private_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(value)?;
        std::fs::write(&path, text).with_context(|| format!("{} nicht schreibbar", path.display()))
    }
}

fn create_private_dir_all(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .with_context(|| format!("Verzeichnis {} nicht anlegbar", path.display()))
}

fn raws<T>(items: &[Raw<T>]) -> Vec<&Value> {
    items.iter().map(|r| &r.raw).collect()
}

/// Writes the complete backup and returns its directory.
pub async fn backup(api: &impl ClickUpApi, options: &BackupOptions) -> Result<PathBuf> {
    if !options.force {
        if let Some(repo) = enclosing_git_repo(&options.base_dir) {
            bail!(
                "{} liegt im Git-Repository {}. Die Sicherung enthält private Daten und gehört \
                 nicht ins Repository. Anderes Ziel wählen (--out) oder --force angeben.",
                options.base_dir.display(),
                repo.display()
            );
        }
    }
    let root = options.base_dir.join(&options.stamp);
    if root.exists() {
        bail!("{} existiert bereits", root.display());
    }
    create_private_dir_all(&root)?;
    let out = Writer { root: root.clone() };

    let mut manifest = Manifest {
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        tool: concat!("zunder-import ", env!("CARGO_PKG_VERSION")).to_owned(),
        time_entries_from_ms: options.since_ms,
        time_entries_until_ms: options.until_ms,
        ..Manifest::default()
    };

    let teams_body = api.teams().await?;
    out.write("teams.json", &teams_body)?;
    let teams: Vec<Raw<model::Team>> =
        model::items(&teams_body, "teams").map_err(anyhow::Error::msg)?;

    let mut all_tasks: Vec<Raw<Task>> = Vec::new();
    for team in teams {
        let team_id = team.item.id.clone();
        eprintln!("Sichere Workspace {} ({team_id}) …", team.item.name);
        let mut counts = TeamCounts {
            id: team_id.clone(),
            name: team.item.name.clone(),
            ..TeamCounts::default()
        };
        let hierarchy = fetch::fetch_hierarchy(api, team, true).await?;
        let spaces: Vec<&Value> = hierarchy.spaces.iter().map(|s| &s.space.raw).collect();
        out.write(&format!("teams/{team_id}/spaces.json"), &spaces)?;
        counts.spaces = spaces.len();
        for node in &hierarchy.spaces {
            let space_id = &node.space.item.id;
            let folders: Vec<&Value> = node.folders.iter().map(|f| &f.folder.raw).collect();
            out.write(&format!("spaces/{space_id}/folders.json"), &folders)?;
            out.write(&format!("spaces/{space_id}/lists.json"), &raws(&node.lists))?;
            counts.folders += folders.len();
            for folder in &node.folders {
                out.write(
                    &format!("folders/{}/lists.json", folder.folder.item.id),
                    &raws(&folder.lists),
                )?;
            }
        }
        for list_ref in hierarchy.lists() {
            let list_id = &list_ref.list.item.id;
            out.write(&format!("lists/{list_id}/list.json"), &list_ref.list.raw)?;
            let mut tasks = fetch::fetch_list_tasks_raw(api, list_id, false).await?;
            tasks.extend(fetch::fetch_list_tasks_raw(api, list_id, true).await?);
            let mut seen = HashSet::new();
            tasks.retain(|t| seen.insert(t.get("id").cloned().unwrap_or(Value::Null).to_string()));
            eprintln!("  {}: {} Aufgaben", list_ref.path(), tasks.len());
            out.write(&format!("lists/{list_id}/tasks.json"), &tasks)?;
            counts.lists += 1;
            counts.tasks += tasks.len();
            for value in tasks {
                // Only for finding missing parents; unreadable tasks stay raw.
                if let Ok(task) = model::item::<Task>(value) {
                    all_tasks.push(task);
                }
            }
        }
        let entries = time_entries(api, &team_id, options).await?;
        counts.time_entries = entries.len();
        out.write(&format!("teams/{team_id}/time_entries.json"), &entries)?;
        manifest.teams.push(counts);
    }

    let (parents, missing) = fetch::fetch_parents(api, &all_tasks).await?;
    manifest.parents_fetched = parents.len();
    manifest.missing_parents = missing.into_iter().collect();
    out.write("tasks/parents.json", &raws(&parents))?;
    out.write("manifest.json", &manifest)?;
    Ok(root)
}

/// All time entries of a workspace between `since_ms` and `until_ms`.
///
/// Without explicit bounds ClickUp only returns the last 30 days, so the
/// whole range is requested in windows of `window_days`. Entries are
/// deduplicated by ID (an entry spanning a window border may appear twice).
pub async fn time_entries(
    api: &impl ClickUpApi,
    team_id: &str,
    options: &BackupOptions,
) -> Result<Vec<Value>> {
    let window = i64::from(options.window_days.max(1)) * DAY_MS;
    let mut entries: BTreeMap<String, Value> = BTreeMap::new();
    let mut unnamed = Vec::new();
    let mut start = options.since_ms;
    while start <= options.until_ms {
        let end = (start + window - 1).min(options.until_ms);
        let body = api.time_entries(team_id, start, end).await?;
        let data = match body.get("data") {
            Some(Value::Array(a)) => a.clone(),
            Some(Value::Null) | None => Vec::new(),
            Some(_) => bail!("Zeiteinträge: \"data\" ist keine Liste"),
        };
        for entry in data {
            match entry.get("id").and_then(|id| match id {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            }) {
                Some(id) => {
                    entries.insert(id, entry);
                }
                None => unnamed.push(entry),
            }
        }
        start = end + 1;
    }
    let mut out: Vec<Value> = entries.into_values().collect();
    out.extend(unnamed);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_git_repos_for_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let inside = repo.join("a/b/c");
        assert_eq!(
            enclosing_git_repo(&inside).unwrap().canonicalize().unwrap(),
            repo.canonicalize().unwrap()
        );
        let outside = dir.path().join("elsewhere/x");
        // The temp dir itself might live inside a repository on some
        // machines; only assert that the inner repo is not reported.
        assert_ne!(
            enclosing_git_repo(&outside).map(|p| p.canonicalize().unwrap()),
            Some(repo.canonicalize().unwrap())
        );
    }
}
