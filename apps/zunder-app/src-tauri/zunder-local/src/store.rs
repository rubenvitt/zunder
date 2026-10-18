//! The app's local SQLite store.
//!
//! It holds
//! - the operation log: `confirmed` operations (accepted by the server, with
//!   their server `seq` once pulled) and `pending` operations (applied
//!   locally, not yet pushed),
//! - the materialized **confirmed** state (`entities`, one snapshot entity
//!   per row), so startup does not replay the whole log,
//! - the sync cursor, the configuration and the scheduled reminders.
//!
//! In memory it keeps two [`State`]s: `confirmed` (what the server accepted)
//! and `view` = `confirmed` + pending operations. The UI always sees `view`.
//! When the server rejects a pending operation, `view` is rebuilt from
//! `confirmed` and the remaining pending operations (rollback).
//!
//! Pending operations are grouped by local change: all operations one
//! [`LocalStore::apply_local`] call queues share a `change_id` and never go
//! into two different pushes (see [`LocalStore::pending_batch`]), because
//! server rules compare the state before and after a push (recurrence
//! follow-ups, `blocked_by` status, numbering).
//!
//! The first sync of an empty store loads `GET /api/snapshot`; the log then
//! starts at the snapshot's cursor.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use zunder_core::ids::Id;
use zunder_core::{
    validate_push, EntityKind, Hlc, NodeId, Op, RejectCode, Rejection, SnapshotEntity, State,
    Warning,
};

use crate::error::{Error, Result};
use crate::reminders::{Reminder, ReminderKind, ScheduledReminder};
use crate::wire::{PullPage, PushResponse, ServerSnapshot};

const SCHEMA_VERSION: i32 = 4;

const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE ops (
    local_seq  INTEGER PRIMARY KEY AUTOINCREMENT,
    op_id      TEXT NOT NULL UNIQUE,
    entity     TEXT NOT NULL,
    entity_id  TEXT NOT NULL,
    field      TEXT NOT NULL,
    value_json TEXT NOT NULL,
    hlc        TEXT NOT NULL,
    status     TEXT NOT NULL CHECK (status IN ('pending', 'confirmed')),
    server_seq INTEGER
);
CREATE INDEX ops_status ON ops (status, local_seq);
CREATE TABLE entities (
    entity    TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    data_json TEXT NOT NULL,
    PRIMARY KEY (entity, entity_id)
);
CREATE TABLE reminders (
    notification_id INTEGER PRIMARY KEY,
    task_id         TEXT NOT NULL,
    kind            TEXT NOT NULL,
    at_ms           INTEGER NOT NULL,
    title           TEXT NOT NULL,
    body            TEXT NOT NULL,
    UNIQUE (task_id, kind)
);
"#,
    // Finds the operation at the cursor (see `LocalStore::cursor_anchor`).
    "CREATE INDEX ops_server_seq ON ops (server_seq);",
    // IDs became CUID2; `upgrade_legacy_ids` rewrites the stored ones.
    "SELECT 1;",
    // The local change a pending operation belongs to: the `local_seq` of
    // the change's first operation. Operations pending from before had no
    // grouping; each of them counts as a change of its own.
    r#"
ALTER TABLE ops ADD COLUMN change_id INTEGER;
UPDATE ops SET change_id = local_seq WHERE status = 'pending';
"#,
];

/// The schema version whose migration rewrites legacy UUIDs.
const CUID2_VERSION: usize = 3;

const META_CURSOR: &str = "cursor";
const META_NODE_ID: &str = "node_id";
const META_SERVER_URL: &str = "server_url";
/// The server the local data (log, entities, cursor) belongs to; unlike
/// `server_url` it survives clearing the address.
const META_DATA_SERVER_URL: &str = "data_server_url";
const META_API_TOKEN: &str = "api_token";
const META_LAST_SYNC: &str = "last_sync";
/// A started login waiting for the browser ([`crate::login::PendingLogin`]).
const META_PENDING_LOGIN: &str = "pending_login";
/// Unix ms of the desktop reminder timer's last check (catch-up after a
/// restart).
const META_REMINDERS_CHECKED_AT: &str = "reminders_checked_at";

const API_TOKEN_MAX_LEN: usize = 512;

/// Largest local change (serialized operations) [`LocalStore::apply_local`]
/// accepts. A change always goes into one push, and the server takes at most
/// 8 MiB per push request; the rest is left for the request envelope.
pub const MAX_CHANGE_BYTES: usize = 7 * 1024 * 1024;

/// Result of the `local_snapshot` command: the `/api/snapshot` shape plus the
/// number of pending (not yet pushed) operations.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LocalSnapshot {
    pub cursor: i64,
    pub entities: Vec<SnapshotEntity>,
    pub pending: usize,
}

/// Result of [`LocalStore::apply_local`] (the `local_apply` command): the
/// keys of a push response, judged locally, plus the pending count.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ApplyResult {
    pub accepted: Vec<Id>,
    pub rejected: Vec<Rejection>,
    pub warnings: Vec<Warning>,
    pub pending: usize,
}

/// Effect of a push response on the store.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PushApplied {
    /// Number of pushed operations the server accepted.
    pub accepted: usize,
    /// Operations the server rejected; they are rolled back locally.
    pub rejected: Vec<Rejection>,
    pub warnings: Vec<Warning>,
    /// Whether the visible state changed (only through rollbacks).
    pub view_changed: bool,
}

/// Outcome of the last sync attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastSync {
    pub at_ms: i64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of the `get_config` command. The API token itself is never
/// returned, only whether one is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigView {
    pub server_url: Option<String>,
    pub has_api_token: bool,
    /// Node ID of this device; the web UI uses it for its HLC.
    pub node_id: String,
    pub cursor: i64,
    pub pending: usize,
    pub last_sync: Option<LastSync>,
}

/// Argument of the `set_config` command. Omitted fields stay unchanged,
/// `null` clears a field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigUpdate {
    #[serde(default, deserialize_with = "present")]
    pub server_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub api_token: Option<Option<String>>,
    /// Required to switch to another server while operations are pending:
    /// switching resets the local data, pending operations included.
    #[serde(default)]
    pub discard_pending: bool,
}

/// Distinguishes a present `null` (`Some(None)`) from an absent key (`None`).
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

/// Connection data for the sync client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub server_url: String,
    pub api_token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpStatus {
    Pending,
    Confirmed,
}

impl OpStatus {
    fn as_str(self) -> &'static str {
        match self {
            OpStatus::Pending => "pending",
            OpStatus::Confirmed => "confirmed",
        }
    }
}

pub struct LocalStore {
    conn: Connection,
    confirmed: State,
    view: State,
    /// Pending operations in local order, grouped by local change (no
    /// empty groups).
    pending: Vec<Vec<Op>>,
    cursor: i64,
    node_id: NodeId,
    /// Incremented whenever the local data is reset for another server; a
    /// sync round started before must not apply its results afterwards.
    generation: u64,
}

impl LocalStore {
    /// Opens (and creates or migrates) the store at `path`.
    ///
    /// The database holds the API token, so on Unix the file is readable by
    /// its owner only (`0600`); SQLite gives `-wal` and `-shm` the same mode.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        restrict_to_owner(path, true)?;
        let conn = Connection::open(path)?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        // Left over from a version that did not restrict them yet.
        for suffix in ["-wal", "-shm"] {
            let mut side = path.as_os_str().to_owned();
            side.push(suffix);
            restrict_to_owner(Path::new(&side), false)?;
        }
        Self::init(conn)
    }

    /// An in-memory store (tests).
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        migrate(&mut conn)?;
        let node_id = match get_meta(&conn, META_NODE_ID)? {
            Some(id) => NodeId::new(id).map_err(|e| Error::Corrupt(e.to_string()))?,
            None => {
                let id = new_node_id();
                set_meta(&conn, META_NODE_ID, id.as_str())?;
                id
            }
        };
        let loaded = Loaded::read(&conn)?;
        Ok(Self {
            conn,
            view: view_of(&loaded.confirmed, &loaded.pending),
            confirmed: loaded.confirmed,
            pending: loaded.pending,
            cursor: loaded.cursor,
            node_id,
            generation: 0,
        })
    }

    /// Replaces the in-memory state with what SQLite holds.
    fn reload(&mut self) -> Result<()> {
        let loaded = Loaded::read(&self.conn)?;
        self.view = view_of(&loaded.confirmed, &loaded.pending);
        self.confirmed = loaded.confirmed;
        self.pending = loaded.pending;
        self.cursor = loaded.cursor;
        Ok(())
    }

    /// Runs a write that changes memory before its transaction commits. If
    /// it fails (disk full, I/O error), memory is reloaded from SQLite, so a
    /// retry sees the change as new again and persists it.
    fn in_step<T>(&mut self, write: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let result = write(self);
        if result.is_err() {
            // The original error is the one worth reporting; if even the
            // reload fails, the next start reads the database again anyway.
            let _ = self.reload();
        }
        result
    }

    // ----- reading ------------------------------------------------------------

    /// The visible state: confirmed plus pending operations.
    pub fn state(&self) -> &State {
        &self.view
    }

    /// The state the server has confirmed.
    pub fn confirmed_state(&self) -> &State {
        &self.confirmed
    }

    pub fn cursor(&self) -> i64 {
        self.cursor
    }

    /// See [`LocalStore::set_config`]: changes when a server switch resets
    /// the local data.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    pub fn pending_count(&self) -> usize {
        self.pending.iter().map(Vec::len).sum()
    }

    /// The next push: whole local changes from the oldest on, at most
    /// `max_ops` operations and `max_bytes` serialized operations. A change
    /// is never split; the first one goes whole even if it alone exceeds a
    /// limit (the server does not limit the number of operations, and
    /// [`MAX_CHANGE_BYTES`] keeps a change below its request size limit).
    pub fn pending_batch(&self, max_ops: usize, max_bytes: usize) -> Vec<Op> {
        let mut batch = Vec::new();
        let mut bytes = 0;
        for change in &self.pending {
            let size = serialized_len(change);
            if !batch.is_empty()
                && (batch.len() + change.len() > max_ops || bytes + size > max_bytes)
            {
                break;
            }
            batch.extend_from_slice(change);
            bytes += size;
        }
        batch
    }

    /// Whether the store has never received server data (first sync loads
    /// the snapshot instead of pulling the whole log).
    pub fn needs_snapshot(&self) -> bool {
        self.cursor == 0
    }

    /// The ID of the operation at the cursor's `seq`, if this store pulled
    /// it (not yet right after a snapshot). Each pull overlaps by this one
    /// operation to check that the server's log still matches.
    pub fn cursor_anchor(&self) -> Result<Option<Id>> {
        if self.cursor == 0 {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT op_id FROM ops WHERE server_seq = ?1 AND status = 'confirmed'",
                [self.cursor],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|id| parse_id(&id))
            .transpose()
    }

    pub fn snapshot(&self) -> LocalSnapshot {
        LocalSnapshot {
            cursor: self.cursor,
            entities: self.view.to_snapshot(),
            pending: self.pending_count(),
        }
    }

    // ----- local changes ------------------------------------------------------

    /// Validates `ops` against the visible state like the server would,
    /// applies the accepted new ones optimistically and queues them for push
    /// as one local change (always pushed together, see
    /// [`LocalStore::pending_batch`]).
    ///
    /// A change larger than [`MAX_CHANGE_BYTES`] could never be pushed; it
    /// is rejected as a whole (`invalid_value`) and nothing is queued.
    pub fn apply_local(&mut self, ops: &[Op], now_ms: u64) -> Result<ApplyResult> {
        if serialized_len(ops) > MAX_CHANGE_BYTES {
            let message = format!(
                "Die Änderung ist zu groß für den Server (mehr als {} MiB)",
                MAX_CHANGE_BYTES / (1024 * 1024)
            );
            return Ok(ApplyResult {
                rejected: ops
                    .iter()
                    .map(|op| Rejection {
                        op_id: op.op_id,
                        code: RejectCode::InvalidValue,
                        message: message.clone(),
                    })
                    .collect(),
                pending: self.pending_count(),
                ..ApplyResult::default()
            });
        }
        let outcome = validate_push(&self.view, ops, now_ms);
        if !outcome.new_ops.is_empty() {
            let tx = self.conn.transaction()?;
            let mut change_id = None;
            for op in &outcome.new_ops {
                insert_op(&tx, op, OpStatus::Pending, None, change_id)?;
                change_id.get_or_insert(tx.last_insert_rowid());
            }
            // The first operation was inserted before its own ID was known.
            tx.execute(
                "UPDATE ops SET change_id = ?1 WHERE local_seq = ?1",
                [change_id],
            )?;
            tx.commit()?;
            for op in &outcome.new_ops {
                self.view.apply(op);
            }
            self.pending.push(outcome.new_ops);
        }
        Ok(ApplyResult {
            accepted: outcome.accepted,
            rejected: outcome.rejected,
            warnings: outcome.warnings,
            pending: self.pending_count(),
        })
    }

    // ----- sync ---------------------------------------------------------------

    /// Records the server's verdict on `pushed`: accepted operations become
    /// confirmed, rejected ones are dropped and rolled back. Operations the
    /// response does not mention stay pending.
    pub fn apply_push_response(
        &mut self,
        pushed: &[Op],
        response: &PushResponse,
    ) -> Result<PushApplied> {
        self.in_step(|s| s.push_response(pushed, response))
    }

    fn push_response(&mut self, pushed: &[Op], response: &PushResponse) -> Result<PushApplied> {
        let accepted: HashSet<Id> = response.accepted.iter().copied().collect();
        let rejected: HashMap<Id, &Rejection> =
            response.rejected.iter().map(|r| (r.op_id, r)).collect();
        let mut done = HashSet::new();
        let mut touched = BTreeSet::new();
        let mut result = PushApplied {
            warnings: response.warnings.clone(),
            ..PushApplied::default()
        };

        let tx = self.conn.transaction()?;
        for op in pushed {
            if accepted.contains(&op.op_id) {
                tx.execute(
                    "UPDATE ops SET status = 'confirmed' WHERE op_id = ?1 AND status = 'pending'",
                    params![op.op_id.to_string()],
                )?;
                if self.confirmed.apply(op) {
                    touched.insert((op.entity, op.entity_id));
                }
                result.accepted += 1;
                done.insert(op.op_id);
            } else if let Some(rejection) = rejected.get(&op.op_id) {
                tx.execute(
                    "DELETE FROM ops WHERE op_id = ?1 AND status = 'pending'",
                    params![op.op_id.to_string()],
                )?;
                result.rejected.push((*rejection).clone());
                done.insert(op.op_id);
            }
        }
        save_entities(&tx, &self.confirmed, &touched)?;
        tx.commit()?;

        self.forget_pending(&done);
        if !result.rejected.is_empty() {
            self.view = view_of(&self.confirmed, &self.pending);
            result.view_changed = true;
        }
        Ok(result)
    }

    /// Removes the operations in `op_ids` from the pending changes; what is
    /// left of a change stays one change.
    fn forget_pending(&mut self, op_ids: &HashSet<Id>) {
        for change in &mut self.pending {
            change.retain(|op| !op_ids.contains(&op.op_id));
        }
        self.pending.retain(|change| !change.is_empty());
    }

    /// Merges one page of `GET /api/sync/pull` and advances the cursor.
    /// Returns whether the visible state changed.
    pub fn apply_pull_page(&mut self, page: &PullPage) -> Result<bool> {
        self.in_step(|s| s.pull_page(page))
    }

    fn pull_page(&mut self, page: &PullPage) -> Result<bool> {
        let mut touched = BTreeSet::new();
        let mut confirmed_ids = HashSet::new();
        let mut view_changed = false;
        let cursor = self.cursor.max(page.cursor);

        let tx = self.conn.transaction()?;
        for pulled in &page.ops {
            let op = &pulled.op;
            upsert_confirmed_op(&tx, op, pulled.seq)?;
            if self.confirmed.apply(op) {
                touched.insert((op.entity, op.entity_id));
            }
            view_changed |= self.view.apply(op);
            confirmed_ids.insert(op.op_id);
        }
        save_entities(&tx, &self.confirmed, &touched)?;
        set_meta(&tx, META_CURSOR, &cursor.to_string())?;
        tx.commit()?;

        // An operation whose push response got lost comes back here.
        self.forget_pending(&confirmed_ids);
        self.cursor = cursor;
        Ok(view_changed)
    }

    /// Merges a server snapshot (first sync) and takes over its cursor.
    /// Returns whether the visible state changed.
    pub fn apply_server_snapshot(&mut self, snapshot: &ServerSnapshot) -> Result<bool> {
        self.in_step(|s| s.server_snapshot(snapshot))
    }

    fn server_snapshot(&mut self, snapshot: &ServerSnapshot) -> Result<bool> {
        let before = self.view.clone();
        self.confirmed.merge_snapshot(&snapshot.entities);
        self.view.merge_snapshot(&snapshot.entities);
        let touched: BTreeSet<_> = snapshot
            .entities
            .iter()
            .map(|e| (e.entity, e.entity_id))
            .collect();
        let cursor = self.cursor.max(snapshot.cursor);

        let tx = self.conn.transaction()?;
        save_entities(&tx, &self.confirmed, &touched)?;
        set_meta(&tx, META_CURSOR, &cursor.to_string())?;
        tx.commit()?;

        self.cursor = cursor;
        Ok(self.view != before)
    }

    /// Forgets everything the server confirmed (log, entities, cursor) and
    /// keeps the pending operations; the next sync loads the snapshot again.
    /// For a server whose log no longer matches (restored backup).
    pub fn reset_confirmed(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM ops WHERE status = 'confirmed'; DELETE FROM entities;
             DELETE FROM meta WHERE key = 'cursor';",
        )?;
        tx.commit()?;
        self.confirmed = State::new();
        self.view = view_of(&self.confirmed, &self.pending);
        self.cursor = 0;
        Ok(())
    }

    pub fn last_sync(&self) -> Result<Option<LastSync>> {
        get_meta(&self.conn, META_LAST_SYNC)?
            .map(|json| {
                serde_json::from_str(&json).map_err(|e| Error::Corrupt(format!("last_sync: {e}")))
            })
            .transpose()
    }

    pub fn record_sync(&self, last: &LastSync) -> Result<()> {
        let json = serde_json::to_string(last).expect("LastSync serializes");
        set_meta(&self.conn, META_LAST_SYNC, &json)
    }

    /// When the desktop reminder timer last checked (Unix ms), if ever.
    pub fn reminders_checked_at(&self) -> Result<Option<i64>> {
        get_meta(&self.conn, META_REMINDERS_CHECKED_AT)?
            .map(|v| {
                v.parse()
                    .map_err(|e| Error::Corrupt(format!("reminders_checked_at: {e}")))
            })
            .transpose()
    }

    pub fn set_reminders_checked_at(&self, at_ms: i64) -> Result<()> {
        set_meta(&self.conn, META_REMINDERS_CHECKED_AT, &at_ms.to_string())
    }

    // ----- configuration ------------------------------------------------------

    pub fn config(&self) -> Result<ConfigView> {
        Ok(ConfigView {
            server_url: get_meta(&self.conn, META_SERVER_URL)?,
            has_api_token: get_meta(&self.conn, META_API_TOKEN)?.is_some(),
            node_id: self.node_id.to_string(),
            cursor: self.cursor,
            pending: self.pending_count(),
            last_sync: self.last_sync()?,
        })
    }

    /// Server URL and token, if both are set.
    pub fn remote(&self) -> Result<Option<Remote>> {
        let url = get_meta(&self.conn, META_SERVER_URL)?;
        let token = get_meta(&self.conn, META_API_TOKEN)?;
        Ok(url.zip(token).map(|(server_url, api_token)| Remote {
            server_url,
            api_token,
        }))
    }

    /// The started login, if any.
    pub fn pending_login(&self) -> Result<Option<crate::login::PendingLogin>> {
        get_meta(&self.conn, META_PENDING_LOGIN)?
            .map(|json| {
                serde_json::from_str(&json)
                    .map_err(|e| Error::Corrupt(format!("pending_login: {e}")))
            })
            .transpose()
    }

    /// Stores (or with `None` forgets) the started login.
    pub fn set_pending_login(&self, login: Option<&crate::login::PendingLogin>) -> Result<()> {
        let json = login.map(|l| serde_json::to_string(l).expect("PendingLogin serializes"));
        put_meta(&self.conn, META_PENDING_LOGIN, json.as_deref())
    }

    /// Fails with `pending_ops` if switching to `server_url` would drop
    /// pending operations (checked before a login opens the browser).
    pub fn check_server_switch(&self, server_url: &str, discard_pending: bool) -> Result<()> {
        let new = normalize_server_url(server_url)?;
        let data_url = match get_meta(&self.conn, META_DATA_SERVER_URL)? {
            Some(url) => Some(url),
            None => get_meta(&self.conn, META_SERVER_URL)?,
        };
        if data_url.is_some_and(|old| old != new) && !self.pending.is_empty() && !discard_pending {
            return Err(Error::PendingOps(self.pending_count()));
        }
        Ok(())
    }

    /// Applies a configuration change. Switching to a different server
    /// resets all local data (it belongs to the old server); with pending
    /// operations that needs `discard_pending`. "Different" is measured
    /// against the server the data came from, so clearing the address in
    /// between does not hide a switch. A switch increments
    /// [`LocalStore::generation`].
    pub fn set_config(&mut self, update: &ConfigUpdate) -> Result<ConfigView> {
        let server_url = match &update.server_url {
            Some(Some(url)) => Some(Some(normalize_server_url(url)?)),
            Some(None) => Some(None),
            None => None,
        };
        let api_token = match &update.api_token {
            Some(Some(token)) => Some(Some(normalize_api_token(token)?)),
            Some(None) => Some(None),
            None => None,
        };

        // Stores from before `data_server_url` existed: the configured URL.
        let data_url = match get_meta(&self.conn, META_DATA_SERVER_URL)? {
            Some(url) => Some(url),
            None => get_meta(&self.conn, META_SERVER_URL)?,
        };
        let switches_server = match &server_url {
            Some(Some(new)) => data_url.as_ref().is_some_and(|old| old != new),
            _ => false,
        };
        if switches_server && !self.pending.is_empty() && !update.discard_pending {
            return Err(Error::PendingOps(self.pending_count()));
        }

        let tx = self.conn.transaction()?;
        if let Some(url) = &server_url {
            put_meta(&tx, META_SERVER_URL, url.as_deref())?;
            if let Some(url) = url {
                set_meta(&tx, META_DATA_SERVER_URL, url)?;
            }
        }
        if let Some(token) = &api_token {
            put_meta(&tx, META_API_TOKEN, token.as_deref())?;
        }
        if switches_server {
            tx.execute_batch(
                "DELETE FROM ops; DELETE FROM entities;
                 DELETE FROM meta WHERE key IN ('cursor', 'last_sync');",
            )?;
        }
        tx.commit()?;

        if switches_server {
            self.confirmed = State::new();
            self.view = State::new();
            self.pending.clear();
            self.cursor = 0;
            self.generation += 1;
        }
        self.config()
    }

    // ----- reminders ----------------------------------------------------------

    /// The reminders handed to the notification system (see
    /// [`crate::reminders::plan_schedule`]).
    pub fn scheduled_reminders(&self) -> Result<Vec<ScheduledReminder>> {
        let mut stmt = self.conn.prepare(
            "SELECT notification_id, task_id, kind, at_ms, title, body
             FROM reminders ORDER BY at_ms, notification_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, task_id, kind, at_ms, title, body) = row?;
            out.push(ScheduledReminder {
                id,
                reminder: Reminder {
                    task_id: parse_id(&task_id)?,
                    kind: ReminderKind::parse(&kind)
                        .ok_or_else(|| Error::Corrupt(format!("Erinnerungsart {kind:?}")))?,
                    at_ms,
                    title,
                    body,
                },
            });
        }
        Ok(out)
    }

    /// Replaces the stored set of scheduled reminders.
    pub fn save_scheduled_reminders(&mut self, scheduled: &[ScheduledReminder]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM reminders", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO reminders (notification_id, task_id, kind, at_ms, title, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for s in scheduled {
                let r = &s.reminder;
                stmt.execute(params![
                    s.id,
                    r.task_id.to_string(),
                    r.kind.as_str(),
                    r.at_ms,
                    r.title,
                    r.body
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

// ----- helpers ----------------------------------------------------------------

fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(Error::SchemaTooNew(version));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        if index + 1 == CUID2_VERSION {
            upgrade_legacy_ids(&tx)?;
        }
        tx.pragma_update(None, "user_version", index as i32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

/// Rewrites legacy UUIDs (from before CUID2) in the local log, the
/// materialized entities and the reminders with the same fixed conversion as
/// the server (`zunder_core::ids::Id::from_legacy`). The server's `seq`s are
/// unchanged by its own migration, so the cursor and the pending operations
/// stay valid; nothing has to be pulled again.
fn upgrade_legacy_ids(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let ops: Vec<(i64, String, String, String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT local_seq, op_id, entity, entity_id, field, value_json FROM ops \
             WHERE length(op_id) = 36 OR length(entity_id) = 36",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (seq, op_id, entity, entity_id, field, value_json) in ops {
        let kind: EntityKind = entity
            .parse()
            .map_err(|_| Error::Corrupt(format!("Entitätsart {entity:?}")))?;
        let mut value: serde_json::Value =
            serde_json::from_str(&value_json).map_err(|e| Error::Corrupt(format!("Wert: {e}")))?;
        zunder_core::op::upgrade_legacy_value(kind, &field, &mut value);
        tx.execute(
            "UPDATE ops SET op_id = ?2, entity_id = ?3, value_json = ?4 WHERE local_seq = ?1",
            params![
                seq,
                parse_id(&op_id)?.to_string(),
                parse_id(&entity_id)?.to_string(),
                value.to_string()
            ],
        )?;
    }
    let entities: Vec<(String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT entity, entity_id, data_json FROM entities WHERE length(entity_id) = 36",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (entity, entity_id, data_json) in entities {
        // Reading converts every ID in the entity, references included.
        let converted: SnapshotEntity = serde_json::from_str(&data_json)
            .map_err(|e| Error::Corrupt(format!("Entität: {e}")))?;
        tx.execute(
            "UPDATE entities SET entity_id = ?3, data_json = ?4 WHERE entity = ?1 AND entity_id = ?2",
            params![
                entity,
                entity_id,
                converted.entity_id.to_string(),
                serde_json::to_string(&converted)
                    .map_err(|e| Error::Corrupt(format!("Entität: {e}")))?
            ],
        )?;
    }
    let reminders: Vec<(i64, String)> = {
        let mut stmt = tx
            .prepare("SELECT notification_id, task_id FROM reminders WHERE length(task_id) = 36")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (notification, task_id) in reminders {
        tx.execute(
            "UPDATE reminders SET task_id = ?2 WHERE notification_id = ?1",
            params![notification, parse_id(&task_id)?.to_string()],
        )?;
    }
    Ok(())
}

/// What SQLite holds besides configuration: the confirmed state (with the
/// IDs of all confirmed operations), the pending operations and the cursor.
struct Loaded {
    confirmed: State,
    pending: Vec<Vec<Op>>,
    cursor: i64,
}

impl Loaded {
    fn read(conn: &Connection) -> Result<Self> {
        let cursor = match get_meta(conn, META_CURSOR)? {
            Some(c) => c
                .parse()
                .map_err(|_| Error::Corrupt(format!("Cursor {c:?}")))?,
            None => 0,
        };
        let mut entities = Vec::new();
        {
            let mut stmt = conn.prepare("SELECT data_json FROM entities")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for json in rows {
                let entity: SnapshotEntity = serde_json::from_str(&json?)
                    .map_err(|e| Error::Corrupt(format!("Entität: {e}")))?;
                entities.push(entity);
            }
        }
        let mut confirmed = State::from_snapshot(&entities);
        {
            let mut stmt = conn.prepare("SELECT op_id FROM ops WHERE status = 'confirmed'")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for op_id in rows {
                confirmed.mark_op_known(parse_id(&op_id?)?);
            }
        }
        Ok(Self {
            confirmed,
            pending: load_pending(conn)?,
            cursor,
        })
    }
}

/// Makes `path` readable and writable by its owner only (Unix), creating
/// it first if `create` is set; a missing file is skipped otherwise.
#[cfg(unix)]
fn restrict_to_owner(path: &Path, create: bool) -> Result<()> {
    use std::fs::{OpenOptions, Permissions};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let opened = OpenOptions::new()
        .append(true)
        .create(create)
        .mode(0o600)
        .open(path);
    let file = match opened {
        Ok(file) => file,
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(file_error(path, &e)),
    };
    file.set_permissions(Permissions::from_mode(0o600))
        .map_err(|e| file_error(path, &e))
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path, _create: bool) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn file_error(path: &Path, e: &std::io::Error) -> Error {
    Error::Db(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
        Some(format!("{}: {e}", path.display())),
    ))
}

fn view_of(confirmed: &State, pending: &[Vec<Op>]) -> State {
    let mut view = confirmed.clone();
    for op in pending.iter().flatten() {
        view.apply(op);
    }
    view
}

/// Size of `ops` as elements of the JSON array in a push body.
fn serialized_len(ops: &[Op]) -> usize {
    ops.iter()
        .map(|op| serde_json::to_vec(op).expect("ops serialize").len() + 1)
        .sum()
}

/// `app-` plus 12 random hex digits (from the random tail of a UUIDv7).
fn new_node_id() -> NodeId {
    let simple = uuid::Uuid::now_v7().simple().to_string();
    NodeId::new(format!("app-{}", &simple[20..])).expect("valid node id")
}

fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn put_meta(conn: &Connection, key: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(v) => set_meta(conn, key, v),
        None => {
            conn.execute("DELETE FROM meta WHERE key = ?1", [key])?;
            Ok(())
        }
    }
}

fn insert_op(
    tx: &Transaction<'_>,
    op: &Op,
    status: OpStatus,
    seq: Option<i64>,
    change_id: Option<i64>,
) -> Result<()> {
    tx.execute(
        "INSERT INTO ops (op_id, entity, entity_id, field, value_json, hlc, status, server_seq,
                          change_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            op.op_id.to_string(),
            op.entity.as_str(),
            op.entity_id.to_string(),
            op.field,
            op.value.to_string(),
            op.hlc.to_string(),
            status.as_str(),
            seq,
            change_id
        ],
    )?;
    Ok(())
}

fn upsert_confirmed_op(tx: &Transaction<'_>, op: &Op, seq: i64) -> Result<()> {
    tx.execute(
        "INSERT INTO ops (op_id, entity, entity_id, field, value_json, hlc, status, server_seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (op_id) DO UPDATE SET status = excluded.status, server_seq = excluded.server_seq",
        params![
            op.op_id.to_string(),
            op.entity.as_str(),
            op.entity_id.to_string(),
            op.field,
            op.value.to_string(),
            op.hlc.to_string(),
            OpStatus::Confirmed.as_str(),
            seq
        ],
    )?;
    Ok(())
}

#[cfg(test)]
fn load_ops(conn: &Connection, status: OpStatus) -> Result<Vec<Op>> {
    Ok(load_rows(conn, status)?
        .into_iter()
        .map(|(_, op)| op)
        .collect())
}

/// The pending operations in local order, grouped by local change. Ops of
/// one change are consecutive (one transaction writes them).
fn load_pending(conn: &Connection) -> Result<Vec<Vec<Op>>> {
    let mut changes: Vec<Vec<Op>> = Vec::new();
    let mut last_change = None;
    for (change_id, op) in load_rows(conn, OpStatus::Pending)? {
        match changes.last_mut() {
            Some(change) if last_change == Some(change_id) => change.push(op),
            _ => changes.push(vec![op]),
        }
        last_change = Some(change_id);
    }
    Ok(changes)
}

/// Operations with `status` in local order, each with its change ID (its
/// own `local_seq` if it has none).
fn load_rows(conn: &Connection, status: OpStatus) -> Result<Vec<(i64, Op)>> {
    let mut stmt = conn.prepare(
        "SELECT op_id, entity, entity_id, field, value_json, hlc, COALESCE(change_id, local_seq)
         FROM ops WHERE status = ?1 ORDER BY local_seq",
    )?;
    let rows = stmt.query_map([status.as_str()], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })?;
    let mut ops = Vec::new();
    for row in rows {
        let (op_id, entity, entity_id, field, value_json, hlc, change_id) = row?;
        ops.push((
            change_id,
            Op {
                op_id: parse_id(&op_id)?,
                entity: EntityKind::from_str(&entity)
                    .map_err(|_| Error::Corrupt(format!("Entitätsart {entity:?}")))?,
                entity_id: parse_id(&entity_id)?,
                field,
                value: serde_json::from_str::<Value>(&value_json)
                    .map_err(|e| Error::Corrupt(format!("Wert: {e}")))?,
                hlc: Hlc::parse(&hlc).map_err(|e| Error::Corrupt(format!("HLC: {e}")))?,
            },
        ));
    }
    Ok(ops)
}

fn save_entities(
    tx: &Transaction<'_>,
    state: &State,
    keys: &BTreeSet<(EntityKind, Id)>,
) -> Result<()> {
    let mut stmt = tx.prepare(
        "INSERT INTO entities (entity, entity_id, data_json) VALUES (?1, ?2, ?3)
         ON CONFLICT (entity, entity_id) DO UPDATE SET data_json = excluded.data_json",
    )?;
    for (kind, id) in keys {
        if let Some(entity) = state.entity(*kind, *id) {
            let json = serde_json::to_string(&entity.to_snapshot_entity())
                .expect("snapshot entities serialize");
            stmt.execute(params![kind.as_str(), id.to_string(), json])?;
        }
    }
    Ok(())
}

fn parse_id(s: &str) -> Result<Id> {
    Id::parse_any(s).ok_or_else(|| Error::Corrupt(format!("ID {s:?}")))
}

/// `http(s)://host[:port][/path]` without query, fragment, credentials and
/// trailing slash. Plain `http` only for local hosts (see [`is_local_host`]):
/// every request carries the API token.
pub fn normalize_server_url(input: &str) -> Result<String> {
    let invalid = |why: &str| Error::InvalidConfig(format!("Server-Adresse {input:?}: {why}"));
    let url = url::Url::parse(input.trim()).map_err(|_| invalid("keine gültige URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid("nur http und https sind erlaubt"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(invalid("Host fehlt"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid("ohne Query und Fragment angeben"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("ohne Zugangsdaten angeben"));
    }
    if url.scheme() == "http" && !url.host().is_some_and(is_local_host) {
        return Err(invalid(
            "unverschlüsseltes http nur für lokale Adressen (localhost, privates Netz), sonst https",
        ));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// Hosts that plain `http` may reach: loopback, private and link-local
/// addresses (IPv4 incl. the 100.64.0.0/10 range of VPNs like Tailscale,
/// IPv6 `fc00::/7` and `fe80::/10`), names without a dot and names under
/// `localhost`, `local`, `home.arpa`, `internal` or `lan`.
fn is_local_host(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Ipv4(ip) => {
            let [a, b, ..] = ip.octets();
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || (a == 100 && b & 0xc0 == 64)
        }
        url::Host::Ipv6(ip) => {
            let first = ip.segments()[0];
            ip.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
        url::Host::Domain(name) => {
            let name = name.trim_end_matches('.');
            !name.contains('.')
                || ["localhost", "local", "home.arpa", "internal", "lan"]
                    .iter()
                    .any(|zone| name.ends_with(&format!(".{zone}")))
        }
    }
}

fn normalize_api_token(input: &str) -> Result<String> {
    let token = input.trim();
    if token.is_empty() {
        return Err(Error::InvalidConfig("API-Token ist leer".into()));
    }
    if token.len() > API_TOKEN_MAX_LEN || token.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(Error::InvalidConfig(
            "API-Token enthält ungültige Zeichen oder ist zu lang".into(),
        ));
    }
    Ok(token.to_owned())
}

#[cfg(test)]
mod tests;
