//! Inbox of the share extensions (iOS, macOS): what someone shared with
//! Zunder from another app, waiting to become a task.
//!
//! The extension runs as its own process and cannot reach the app's store,
//! so it leaves one JSON file per shared item in a folder of the App Group
//! container both share (`share-inbox/<id>.json`, format [`SharedItem`]).
//! The app reads the folder ([`ShareInbox::pending`]), the web UI creates the
//! inbox tasks and then removes the files ([`ShareInbox::remove`]). Until
//! then the items wait there, offline and across restarts.
//!
//! Files that cannot be an item (broken JSON, too large, nothing in them)
//! are renamed to `<name>.invalid` so they are not read again; files that
//! merely failed to read (permissions, I/O) stay for the next call.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Folder inside the App Group container.
pub const DIR_NAME: &str = "share-inbox";

/// Largest file the app reads (the extension caps the text well below).
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// One shared item as the extension writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedItem {
    /// Also the file name (`<id>.json`); letters, digits and `-` only.
    pub id: String,
    /// What the person kept in the extension's text field (first line =
    /// title); may be empty when a link was shared.
    #[serde(default)]
    pub text: Option<String>,
    /// The shared web link, if any.
    #[serde(default)]
    pub url: Option<String>,
    /// When it was shared (ms since the epoch).
    #[serde(default)]
    pub created_at_ms: i64,
}

impl SharedItem {
    fn has_content(&self) -> bool {
        let filled = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.trim().is_empty());
        filled(&self.text) || filled(&self.url)
    }
}

/// Whether `id` can name a file in the inbox (no paths, no dots).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The share folder of one App Group container.
#[derive(Debug, Clone)]
pub struct ShareInbox {
    dir: PathBuf,
}

impl ShareInbox {
    /// The inbox in `container` (the App Group container).
    pub fn in_container(container: &Path) -> Self {
        Self::new(container.join(DIR_NAME))
    }

    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// IDs of the waiting items (file names without `.json`), sorted;
    /// cheap enough to poll. A missing folder means nothing waits.
    pub fn ids(&self) -> Result<Vec<String>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_error(&self.dir, &e)),
        };
        let mut ids: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let id = name.strip_suffix(".json")?;
                valid_id(id).then(|| id.to_owned())
            })
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// The waiting items, oldest first. Unreadable files are set aside
    /// (`.invalid`) and skipped.
    pub fn pending(&self) -> Result<Vec<SharedItem>> {
        let mut items = Vec::new();
        for id in self.ids()? {
            let path = self.path(&id);
            match read_item(&path, &id) {
                Ok(Some(item)) => items.push(item),
                // Removed in between (another call took it).
                Ok(None) => {}
                // Not readable right now (permissions, I/O): try again later.
                Err(ReadFailure::Io) => {}
                Err(ReadFailure::Invalid) => {
                    let mut aside = path.clone().into_os_string();
                    aside.push(".invalid");
                    let _ = fs::rename(&path, aside);
                }
            }
        }
        items.sort_by(|a, b| a.created_at_ms.cmp(&b.created_at_ms).then(a.id.cmp(&b.id)));
        Ok(items)
    }

    /// Removes the items with these IDs (done: their tasks exist). Unknown
    /// IDs are ignored; returns how many files were removed.
    pub fn remove(&self, ids: &[String]) -> Result<usize> {
        let mut removed = 0;
        for id in ids {
            if !valid_id(id) {
                return Err(Error::Share(format!("Ungültige Kennung „{id}“")));
            }
            let path = self.path(id);
            match fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(io_error(&path, &e)),
            }
        }
        Ok(removed)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

/// Why a file did not yield an item.
enum ReadFailure {
    /// The content cannot be an item: set the file aside.
    Invalid,
    /// Reading failed for now: keep the file.
    Io,
}

/// `Ok(None)`: the file is gone.
fn read_item(path: &Path, id: &str) -> std::result::Result<Option<SharedItem>, ReadFailure> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ReadFailure::Io),
    };
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Err(ReadFailure::Invalid);
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ReadFailure::Io),
    };
    let mut item: SharedItem = serde_json::from_slice(&bytes).map_err(|_| ReadFailure::Invalid)?;
    // The file name is what `remove` deletes; it wins over the content.
    item.id = id.to_owned();
    if item.has_content() {
        Ok(Some(item))
    } else {
        Err(ReadFailure::Invalid)
    }
}

fn io_error(path: &Path, e: &std::io::Error) -> Error {
    Error::Share(format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inbox() -> (tempfile::TempDir, ShareInbox) {
        let dir = tempfile::tempdir().unwrap();
        let inbox = ShareInbox::in_container(dir.path());
        (dir, inbox)
    }

    fn put(inbox: &ShareInbox, name: &str, content: &str) {
        fs::create_dir_all(inbox.dir()).unwrap();
        fs::write(inbox.dir().join(name), content).unwrap();
    }

    #[test]
    fn missing_folder_is_empty() {
        let (_dir, inbox) = inbox();
        assert!(inbox.ids().unwrap().is_empty());
        assert!(inbox.pending().unwrap().is_empty());
        assert_eq!(inbox.remove(&["abc".into()]).unwrap(), 0);
    }

    #[test]
    fn reads_items_oldest_first() {
        let (_dir, inbox) = inbox();
        put(
            &inbox,
            "b-2.json",
            r#"{"version":1,"id":"b-2","text":"Später","created_at_ms":20}"#,
        );
        put(
            &inbox,
            "a-1.json",
            r#"{"id":"a-1","text":"Artikel lesen","url":"https://example.org/a","created_at_ms":10,"extra":true}"#,
        );
        // Written by the extension in the meantime: not an item yet.
        put(&inbox, ".c-3.json.tmp", "{");
        put(&inbox, "notes.txt", "x");

        assert_eq!(inbox.ids().unwrap(), vec!["a-1", "b-2"]);
        let items = inbox.pending().unwrap();
        assert_eq!(
            items,
            vec![
                SharedItem {
                    id: "a-1".into(),
                    text: Some("Artikel lesen".into()),
                    url: Some("https://example.org/a".into()),
                    created_at_ms: 10,
                },
                SharedItem {
                    id: "b-2".into(),
                    text: Some("Später".into()),
                    url: None,
                    created_at_ms: 20,
                },
            ]
        );
    }

    #[test]
    fn file_name_wins_over_id_in_content() {
        let (_dir, inbox) = inbox();
        put(
            &inbox,
            "abc.json",
            r#"{"id":"../evil","url":"https://x.org"}"#,
        );
        assert_eq!(inbox.pending().unwrap()[0].id, "abc");
    }

    #[test]
    fn sets_unusable_files_aside() {
        let (_dir, inbox) = inbox();
        put(&inbox, "broken.json", "{nope");
        put(
            &inbox,
            "empty.json",
            r#"{"id":"empty","text":"  ","url":null}"#,
        );
        put(&inbox, "ok.json", r#"{"id":"ok","url":"https://x.org"}"#);
        let big = format!(r#"{{"text":"{}"}}"#, "a".repeat(MAX_FILE_BYTES as usize));
        put(&inbox, "big.json", &big);

        let items = inbox.pending().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "ok");
        assert_eq!(inbox.ids().unwrap(), vec!["ok"]);
        for name in ["broken", "empty", "big"] {
            assert!(inbox.dir().join(format!("{name}.json.invalid")).exists());
        }
    }

    #[test]
    fn removes_done_items() {
        let (_dir, inbox) = inbox();
        put(&inbox, "a.json", r#"{"text":"A"}"#);
        put(&inbox, "b.json", r#"{"text":"B"}"#);
        assert_eq!(
            inbox
                .remove(&["a".into(), "unknown".into(), "a".into()])
                .unwrap(),
            1
        );
        assert_eq!(inbox.ids().unwrap(), vec!["b"]);
    }

    #[test]
    fn refuses_ids_that_are_paths() {
        let (dir, inbox) = inbox();
        fs::write(dir.path().join("keep.json"), "{}").unwrap();
        for id in ["../keep", "a/b", "", "a.json", &"a".repeat(65)] {
            let err = inbox.remove(&[id.to_string()]).unwrap_err();
            assert_eq!(err.code(), "share", "{id}");
        }
        assert!(dir.path().join("keep.json").exists());
    }
}
