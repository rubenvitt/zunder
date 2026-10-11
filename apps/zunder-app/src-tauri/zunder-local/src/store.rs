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
//! The first sync of an empty store loads `GET /api/snapshot`; the log then
//! starts at the snapshot's cursor.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use uuid::Uuid;
use zunder_core::{
    validate_push, EntityKind, Hlc, NodeId, Op, Rejection, SnapshotEntity, State, Warning,
};

use crate::error::{Error, Result};
use crate::reminders::{Reminder, ReminderKind, ScheduledReminder};
use crate::wire::{PullPage, PushResponse, ServerSnapshot};

const SCHEMA_VERSION: i32 = 1;

const MIGRATIONS: &[&str] = &[r#"
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
"#];

const META_CURSOR: &str = "cursor";
const META_NODE_ID: &str = "node_id";
const META_SERVER_URL: &str = "server_url";
const META_API_TOKEN: &str = "api_token";
const META_LAST_SYNC: &str = "last_sync";

const API_TOKEN_MAX_LEN: usize = 512;

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
    pub accepted: Vec<Uuid>,
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
    /// Pending operations in local order.
    pending: Vec<Op>,
    cursor: i64,
    node_id: NodeId,
}

impl LocalStore {
    /// Opens (and creates or migrates) the store at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
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
        let cursor = match get_meta(&conn, META_CURSOR)? {
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
                confirmed.mark_op_known(parse_uuid(&op_id?)?);
            }
        }
        let pending = load_ops(&conn, OpStatus::Pending)?;
        let view = view_of(&confirmed, &pending);

        Ok(Self {
            conn,
            confirmed,
            view,
            pending,
            cursor,
            node_id,
        })
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

    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// The oldest pending operations, at most `limit`.
    pub fn pending_batch(&self, limit: usize) -> Vec<Op> {
        self.pending.iter().take(limit).cloned().collect()
    }

    /// Whether the store has never received server data (first sync loads
    /// the snapshot instead of pulling the whole log).
    pub fn needs_snapshot(&self) -> bool {
        self.cursor == 0
    }

    pub fn snapshot(&self) -> LocalSnapshot {
        LocalSnapshot {
            cursor: self.cursor,
            entities: self.view.to_snapshot(),
            pending: self.pending.len(),
        }
    }

    // ----- local changes ------------------------------------------------------

    /// Validates `ops` against the visible state like the server would,
    /// applies the accepted new ones optimistically and queues them for push.
    pub fn apply_local(&mut self, ops: &[Op], now_ms: u64) -> Result<ApplyResult> {
        let outcome = validate_push(&self.view, ops, now_ms);
        if !outcome.new_ops.is_empty() {
            let tx = self.conn.transaction()?;
            for op in &outcome.new_ops {
                insert_op(&tx, op, OpStatus::Pending, None)?;
            }
            tx.commit()?;
            for op in outcome.new_ops {
                self.view.apply(&op);
                self.pending.push(op);
            }
        }
        Ok(ApplyResult {
            accepted: outcome.accepted,
            rejected: outcome.rejected,
            warnings: outcome.warnings,
            pending: self.pending.len(),
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
        let accepted: HashSet<Uuid> = response.accepted.iter().copied().collect();
        let rejected: HashMap<Uuid, &Rejection> =
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

        self.pending.retain(|op| !done.contains(&op.op_id));
        if !result.rejected.is_empty() {
            self.view = view_of(&self.confirmed, &self.pending);
            result.view_changed = true;
        }
        Ok(result)
    }

    /// Merges one page of `GET /api/sync/pull` and advances the cursor.
    /// Returns whether the visible state changed.
    pub fn apply_pull_page(&mut self, page: &PullPage) -> Result<bool> {
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
        self.pending.retain(|op| !confirmed_ids.contains(&op.op_id));
        self.cursor = cursor;
        Ok(view_changed)
    }

    /// Merges a server snapshot (first sync) and takes over its cursor.
    /// Returns whether the visible state changed.
    pub fn apply_server_snapshot(&mut self, snapshot: &ServerSnapshot) -> Result<bool> {
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

    // ----- configuration ------------------------------------------------------

    pub fn config(&self) -> Result<ConfigView> {
        Ok(ConfigView {
            server_url: get_meta(&self.conn, META_SERVER_URL)?,
            has_api_token: get_meta(&self.conn, META_API_TOKEN)?.is_some(),
            node_id: self.node_id.to_string(),
            cursor: self.cursor,
            pending: self.pending.len(),
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

    /// Applies a configuration change. Switching to a different server
    /// resets all local data (it belongs to the old server); with pending
    /// operations that needs `discard_pending`.
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

        let current_url = get_meta(&self.conn, META_SERVER_URL)?;
        let switches_server = match &server_url {
            Some(Some(new)) => current_url.as_ref().is_some_and(|old| old != new),
            _ => false,
        };
        if switches_server && !self.pending.is_empty() && !update.discard_pending {
            return Err(Error::PendingOps(self.pending.len()));
        }

        let tx = self.conn.transaction()?;
        if let Some(url) = &server_url {
            put_meta(&tx, META_SERVER_URL, url.as_deref())?;
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
                    task_id: parse_uuid(&task_id)?,
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
        tx.pragma_update(None, "user_version", index as i32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

fn view_of(confirmed: &State, pending: &[Op]) -> State {
    let mut view = confirmed.clone();
    for op in pending {
        view.apply(op);
    }
    view
}

/// `app-` plus 12 random hex digits (from the random tail of a UUIDv7).
fn new_node_id() -> NodeId {
    let simple = Uuid::now_v7().simple().to_string();
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

fn insert_op(tx: &Transaction<'_>, op: &Op, status: OpStatus, seq: Option<i64>) -> Result<()> {
    tx.execute(
        "INSERT INTO ops (op_id, entity, entity_id, field, value_json, hlc, status, server_seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            op.op_id.to_string(),
            op.entity.as_str(),
            op.entity_id.to_string(),
            op.field,
            op.value.to_string(),
            op.hlc.to_string(),
            status.as_str(),
            seq
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

fn load_ops(conn: &Connection, status: OpStatus) -> Result<Vec<Op>> {
    let mut stmt = conn.prepare(
        "SELECT op_id, entity, entity_id, field, value_json, hlc
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
        ))
    })?;
    let mut ops = Vec::new();
    for row in rows {
        let (op_id, entity, entity_id, field, value_json, hlc) = row?;
        ops.push(Op {
            op_id: parse_uuid(&op_id)?,
            entity: EntityKind::from_str(&entity)
                .map_err(|_| Error::Corrupt(format!("Entitätsart {entity:?}")))?,
            entity_id: parse_uuid(&entity_id)?,
            field,
            value: serde_json::from_str::<Value>(&value_json)
                .map_err(|e| Error::Corrupt(format!("Wert: {e}")))?,
            hlc: Hlc::parse(&hlc).map_err(|e| Error::Corrupt(format!("HLC: {e}")))?,
        });
    }
    Ok(ops)
}

fn save_entities(
    tx: &Transaction<'_>,
    state: &State,
    keys: &BTreeSet<(EntityKind, Uuid)>,
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

fn parse_uuid(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|_| Error::Corrupt(format!("ID {s:?}")))
}

/// `http(s)://host[:port][/path]` without query, fragment, credentials and
/// trailing slash.
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
    Ok(url.as_str().trim_end_matches('/').to_owned())
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
