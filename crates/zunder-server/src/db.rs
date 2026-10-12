//! SQLite persistence: connection setup, migrations and all queries.
//!
//! Every function takes a plain [`Connection`]; the single-writer discipline
//! is enforced one level up in [`crate::app`]. Timestamps are Unix
//! milliseconds.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;
use zunder_core::ids::Id;
use zunder_core::state::FieldEntry;
use zunder_core::{EntityKind, Hlc, Op, SnapshotEntity, State};

/// Embedded migrations; index + 1 is the schema version (`user_version`).
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_oauth.sql"),
    include_str!("../migrations/0003_settings.sql"),
    include_str!("../migrations/0004_devices.sql"),
    include_str!("../migrations/0005_cuid2.sql"),
];

/// The schema version this binary expects.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// Opens a connection with the pragmas Zunder relies on (WAL, foreign keys,
/// busy timeout) and brings the schema up to date.
pub fn open(path: &Path) -> anyhow::Result<Connection> {
    let mut conn = Connection::open(path)
        .with_context(|| format!("Datenbank {} lässt sich nicht öffnen", path.display()))?;
    conn.busy_timeout(Duration::from_secs(5))?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        bail!("SQLite WAL-Modus nicht verfügbar (journal_mode = {mode})");
    }
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    let legacy = legacy_ids_present(&conn)?;
    if legacy {
        backup_before_id_migration(&conn, path)?;
    }
    migrate(&mut conn)?;
    if legacy {
        migrate_legacy_ids(&mut conn, path)?;
    }
    Ok(conn)
}

/// Whether the op log still holds IDs from before CUID2 (UUIDs, 36 chars).
fn legacy_ids_present(conn: &Connection) -> anyhow::Result<bool> {
    let has_ops: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'ops')",
        [],
        |r| r.get(0),
    )?;
    if !has_ops {
        return Ok(false);
    }
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM ops WHERE length(op_id) = 36 OR length(entity_id) = 36)",
        [],
        |r| r.get(0),
    )?)
}

/// `<data>/backups/vor-cuid2-<UTC stamp>.db`, outside the rotation of the
/// nightly backups (those are named `zunder-…`), so it is never pruned.
fn backup_before_id_migration(conn: &Connection, db_path: &Path) -> anyhow::Result<()> {
    let dir = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("backups");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Backup-Verzeichnis {} anlegen", dir.display()))?;
    let dest = dir.join(format!("vor-cuid2-{}.db", utc_stamp()));
    if dest.exists() {
        bail!("Backup {} existiert schon", dest.display());
    }
    vacuum_into(conn, &dest)
        .with_context(|| format!("Backup vor der ID-Umstellung nach {}", dest.display()))?;
    tracing::warn!(backup = %dest.display(), "backup written before switching IDs to CUID2");
    Ok(())
}

fn utc_stamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

/// Rewrites the op log from legacy UUIDs to IDs in one transaction:
/// `op_id`, `entity_id` and the ID-valued fields (`area_id`, `parent_id`,
/// `location.id`), each with the fixed conversion of
/// [`zunder_core::ids::Id::from_legacy`]. `seq`, HLCs and sources stay, so
/// clients keep their cursors. The `entities` table is cleared and rebuilt
/// from the log on load. Writes the mapping of the entities to
/// `<data>/id-zuordnung-<UTC stamp>.json` (e.g. for vault `zunder_id`s).
pub fn migrate_legacy_ids(conn: &mut Connection, db_path: &Path) -> anyhow::Result<()> {
    struct Entry {
        entity: EntityKind,
        old: String,
        new: Id,
        name: Option<String>,
    }
    let tx = conn.transaction()?;
    let rows: Vec<(i64, String, String, String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT seq, op_id, entity, entity_id, field, value_json FROM ops ORDER BY seq",
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
    let mut mapping: BTreeMap<(EntityKind, String), Entry> = BTreeMap::new();
    let mut changed = 0usize;
    {
        let mut update = tx
            .prepare("UPDATE ops SET op_id = ?2, entity_id = ?3, value_json = ?4 WHERE seq = ?1")?;
        for (seq, op_id, entity, entity_id, field, value_json) in rows {
            let kind = parse_kind(&entity)?;
            let new_op = parse_entity_id(&op_id)?;
            let new_entity = parse_entity_id(&entity_id)?;
            let mut value: serde_json::Value = serde_json::from_str(&value_json)
                .with_context(|| format!("ungültiges value_json in Operation {seq}"))?;
            zunder_core::op::upgrade_legacy_value(kind, &field, &mut value);
            let new_value = serde_json::to_string(&value)?;
            let entry = mapping
                .entry((kind, entity_id.clone()))
                .or_insert_with(|| Entry {
                    entity: kind,
                    old: entity_id.clone(),
                    new: new_entity,
                    name: None,
                });
            if field == "name" || field == "title" {
                entry.name = value.as_str().map(str::to_owned);
            }
            if new_op.as_str() != op_id
                || new_entity.as_str() != entity_id
                || new_value != value_json
            {
                update.execute(params![
                    seq,
                    new_op.to_string(),
                    new_entity.to_string(),
                    new_value
                ])?;
                changed += 1;
            }
        }
    }
    tx.execute("DELETE FROM entities", [])?;
    tx.execute("DELETE FROM meta WHERE key LIKE 'materialized_seq:%'", [])?;
    tx.commit()?;

    let list: Vec<serde_json::Value> = mapping
        .values()
        .filter(|e| e.old != e.new.as_str())
        .map(|e| {
            serde_json::json!({
                "entity": e.entity,
                "old_id": e.old,
                "new_id": e.new,
                "name": e.name,
            })
        })
        .collect();
    let file = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("id-zuordnung-{}.json", utc_stamp()));
    std::fs::write(&file, serde_json::to_vec_pretty(&list)?)
        .with_context(|| format!("Zuordnungsliste {} schreiben", file.display()))?;
    tracing::warn!(
        ops = changed,
        entities = list.len(),
        mapping = %file.display(),
        "IDs switched to CUID2"
    );
    Ok(())
}

/// Applies all migrations newer than the database's `user_version`, each in
/// its own transaction.
pub fn migrate(conn: &mut Connection) -> anyhow::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current > SCHEMA_VERSION {
        bail!(
            "Datenbank hat Schema-Version {current}, dieser Server kennt nur bis {SCHEMA_VERSION}"
        );
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = index as i64 + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("Migration {version} fehlgeschlagen"))?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
        tracing::info!(version, "database migrated");
    }
    Ok(())
}

// ----- users ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: Uuid,
    pub subject: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
}

fn row_to_user(row: &Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: parse_uuid_col(row, 0)?,
        subject: row.get(1)?,
        email: row.get(2)?,
        name: row.get(3)?,
    })
}

/// The owner of this instance: the first user, created if none exists yet.
pub fn ensure_owner(conn: &Connection, now: i64) -> anyhow::Result<User> {
    let existing = conn
        .query_row(
            "SELECT id, oidc_subject, email, name FROM users ORDER BY created_at, rowid LIMIT 1",
            [],
            row_to_user,
        )
        .optional()?;
    if let Some(user) = existing {
        return Ok(user);
    }
    let id = Uuid::now_v7();
    conn.execute(
        "INSERT INTO users (id, created_at) VALUES (?1, ?2)",
        params![id.to_string(), now],
    )?;
    Ok(User {
        id,
        subject: None,
        email: None,
        name: None,
    })
}

pub fn user(conn: &Connection, id: Uuid) -> anyhow::Result<Option<User>> {
    Ok(conn
        .query_row(
            "SELECT id, oidc_subject, email, name FROM users WHERE id = ?1",
            [id.to_string()],
            row_to_user,
        )
        .optional()?)
}

/// Stores the identity (OIDC subject, email, name) of a user.
pub fn update_user_identity(
    conn: &Connection,
    id: Uuid,
    subject: Option<&str>,
    email: Option<&str>,
    name: Option<&str>,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE users SET oidc_subject = ?2, email = ?3, name = ?4 WHERE id = ?1",
        params![id.to_string(), subject, email, name],
    )?;
    Ok(())
}

// ----- ops and materialized state --------------------------------------------

/// The materialized state of one owner as loaded at startup.
pub struct LoadedState {
    pub state: State,
    /// Largest `seq` of the owner's operations (0 = none).
    pub cursor: i64,
    /// Largest HLC of all stored operations (to resume the server clock).
    pub max_hlc: Option<Hlc>,
    /// Whether the `entities` table had to be rebuilt from the op log.
    pub rebuilt: bool,
}

fn materialized_key(owner: Uuid) -> String {
    format!("materialized_seq:{owner}")
}

/// Loads the owner's state from the `entities` table, or rebuilds it from
/// the op log when the table is empty or inconsistent with the log.
pub fn load_state(conn: &mut Connection, owner: Uuid) -> anyhow::Result<LoadedState> {
    let owner_s = owner.to_string();
    let cursor: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM ops WHERE owner_id = ?1",
        [&owner_s],
        |r| r.get(0),
    )?;
    let materialized: i64 = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [materialized_key(owner)],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let entity_rows: i64 = conn.query_row(
        "SELECT COUNT(*) FROM entities WHERE owner_id = ?1",
        [&owner_s],
        |r| r.get(0),
    )?;
    let distinct_entities: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT DISTINCT entity, entity_id FROM ops WHERE owner_id = ?1)",
        [&owner_s],
        |r| r.get(0),
    )?;
    let max_hlc = conn
        .query_row("SELECT MAX(hlc) FROM ops", [], |r| {
            r.get::<_, Option<String>>(0)
        })?
        .map(|s| Hlc::parse(&s))
        .transpose()
        .map_err(|e| anyhow!("ungültiger HLC im Operations-Log: {e}"))?;

    let consistent = materialized == cursor && entity_rows == distinct_entities;
    let (state, rebuilt) = if consistent {
        let mut stmt = conn.prepare(
            "SELECT entity, entity_id, created_hlc, fields_json FROM entities WHERE owner_id = ?1",
        )?;
        let snapshots = stmt
            .query_map([&owner_s], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .map(|r| {
                let (entity, id, created, fields) = r?;
                Ok(SnapshotEntity {
                    entity: parse_kind(&entity)?,
                    entity_id: parse_entity_id(&id)?,
                    created_hlc: Some(parse_hlc(&created)?),
                    fields: serde_json::from_str::<BTreeMap<String, FieldEntry>>(&fields)
                        .context("ungültiges fields_json in entities")?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut state = State::from_snapshot(&snapshots);
        // A snapshot only knows the winning op IDs; duplicates of losing
        // ops must still be recognized, so mark the whole log as known.
        let mut stmt = conn.prepare("SELECT op_id FROM ops WHERE owner_id = ?1")?;
        for id in stmt.query_map([&owner_s], |r| r.get::<_, String>(0))? {
            state.mark_op_known(parse_entity_id(&id?)?);
        }
        (state, false)
    } else {
        let ops = ops_since(conn, owner, 0, None)?;
        let state = State::from_ops(ops.iter().map(|(_, op)| op));
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM entities WHERE owner_id = ?1", [&owner_s])?;
        write_entities(&tx, owner, &state.to_snapshot())?;
        set_materialized(&tx, owner, cursor)?;
        tx.commit()?;
        (state, true)
    };
    Ok(LoadedState {
        state,
        cursor,
        max_hlc,
        rebuilt,
    })
}

fn set_materialized(conn: &Connection, owner: Uuid, seq: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![materialized_key(owner), seq.to_string()],
    )?;
    Ok(())
}

fn write_entities(
    conn: &Connection,
    owner: Uuid,
    entities: &[SnapshotEntity],
) -> anyhow::Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO entities
             (owner_id, entity, entity_id, created_hlc, updated_hlc, deleted, fields_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(owner_id, entity, entity_id) DO UPDATE SET
             created_hlc = excluded.created_hlc,
             updated_hlc = excluded.updated_hlc,
             deleted = excluded.deleted,
             fields_json = excluded.fields_json",
    )?;
    for e in entities {
        let created = e
            .created_hlc
            .clone()
            .or_else(|| e.fields.values().map(|f| f.hlc.clone()).min())
            .ok_or_else(|| anyhow!("Entität {} ohne HLC", e.entity_id))?;
        let updated = e
            .fields
            .values()
            .map(|f| &f.hlc)
            .max()
            .cloned()
            .unwrap_or_else(|| created.clone());
        let deleted = e
            .fields
            .get(zunder_core::model::fields::DELETED)
            .is_some_and(|f| f.value == serde_json::Value::Bool(true));
        stmt.execute(params![
            owner.to_string(),
            e.entity.as_str(),
            e.entity_id.to_string(),
            created.to_string(),
            updated.to_string(),
            deleted,
            serde_json::to_string(&e.fields)?,
        ])?;
    }
    Ok(())
}

/// Appends operations to the log and upserts the given materialized
/// entities, atomically. Returns the `seq` of the last inserted operation.
#[allow(clippy::too_many_arguments)]
pub fn commit_ops(
    conn: &mut Connection,
    owner: Uuid,
    ops: &[Op],
    entities: &[SnapshotEntity],
    source: &str,
    client_id: Option<&str>,
    now: i64,
) -> anyhow::Result<i64> {
    let tx = conn.transaction()?;
    let mut last_seq = 0;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO ops
                 (op_id, owner_id, entity, entity_id, field, value_json, hlc, source, client_id, received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for op in ops {
            stmt.execute(params![
                op.op_id.to_string(),
                owner.to_string(),
                op.entity.as_str(),
                op.entity_id.to_string(),
                op.field,
                serde_json::to_string(&op.value)?,
                op.hlc.to_string(),
                source,
                client_id,
                now,
            ])?;
            last_seq = tx.last_insert_rowid();
        }
    }
    write_entities(&tx, owner, entities)?;
    set_materialized(&tx, owner, last_seq)?;
    tx.commit()?;
    Ok(last_seq)
}

fn row_to_op(row: &Row<'_>, offset: usize) -> anyhow::Result<Op> {
    let value_json: String = row.get(offset + 4)?;
    Ok(Op {
        op_id: parse_entity_id(&row.get::<_, String>(offset)?)?,
        entity: parse_kind(&row.get::<_, String>(offset + 1)?)?,
        entity_id: parse_entity_id(&row.get::<_, String>(offset + 2)?)?,
        field: row.get(offset + 3)?,
        value: serde_json::from_str(&value_json).context("ungültiges value_json in ops")?,
        hlc: parse_hlc(&row.get::<_, String>(offset + 5)?)?,
    })
}

/// The owner's operations with `seq > since` in log order, at most `limit`.
pub fn ops_since(
    conn: &Connection,
    owner: Uuid,
    since: i64,
    limit: Option<u32>,
) -> anyhow::Result<Vec<(i64, Op)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT seq, op_id, entity, entity_id, field, value_json, hlc FROM ops
         WHERE owner_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
    )?;
    let limit = limit.map_or(-1, i64::from);
    let mut rows = stmt.query(params![owner.to_string(), since, limit])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push((row.get(0)?, row_to_op(row, 1)?));
    }
    Ok(out)
}

/// One entry of an entity's change history.
#[derive(Debug, Clone)]
pub struct HistoryRow {
    pub seq: i64,
    pub op: Op,
    pub source: String,
    pub received_at: i64,
}

/// All operations on one entity in log order.
pub fn entity_history(
    conn: &Connection,
    owner: Uuid,
    entity: EntityKind,
    entity_id: Id,
) -> anyhow::Result<Vec<HistoryRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT seq, op_id, entity, entity_id, field, value_json, hlc, source, received_at
         FROM ops WHERE owner_id = ?1 AND entity_id = ?2 AND entity = ?3 ORDER BY seq",
    )?;
    let mut rows = stmt.query(params![
        owner.to_string(),
        entity_id.to_string(),
        entity.as_str()
    ])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(HistoryRow {
            seq: row.get(0)?,
            op: row_to_op(row, 1)?,
            source: row.get(7)?,
            received_at: row.get(8)?,
        });
    }
    Ok(out)
}

// ----- settings ---------------------------------------------------------------

/// All settings whose key starts with `prefix`, by key.
pub fn settings(conn: &Connection, prefix: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT key, value FROM settings WHERE substr(key, 1, length(?1)) = ?1 ORDER BY key",
    )?;
    let rows = stmt.query_map([prefix], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Inserts or replaces a setting.
pub fn set_setting(conn: &Connection, key: &str, value: &str, now: i64) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, value, now],
    )?;
    Ok(())
}

/// Removes every setting whose key starts with `prefix`.
pub fn delete_settings(conn: &Connection, prefix: &str) -> anyhow::Result<usize> {
    Ok(conn.execute(
        "DELETE FROM settings WHERE substr(key, 1, length(?1)) = ?1",
        [prefix],
    )?)
}

// ----- API tokens -------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

fn row_to_token(row: &Row<'_>) -> rusqlite::Result<TokenRow> {
    Ok(TokenRow {
        id: parse_uuid_col(row, 0)?,
        user_id: parse_uuid_col(row, 1)?,
        name: row.get(2)?,
        created_at: row.get(3)?,
        last_used_at: row.get(4)?,
    })
}

pub fn insert_token(conn: &Connection, token: &TokenRow, hash: &str) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO api_tokens (id, user_id, name, token_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            token.id.to_string(),
            token.user_id.to_string(),
            token.name,
            hash,
            token.created_at
        ],
    )?;
    Ok(())
}

/// Active (not revoked) tokens of a user, oldest first.
pub fn list_tokens(conn: &Connection, user_id: Uuid) -> anyhow::Result<Vec<TokenRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, user_id, name, created_at, last_used_at FROM api_tokens
         WHERE user_id = ?1 AND revoked_at IS NULL ORDER BY created_at, rowid",
    )?;
    let rows = stmt.query_map([user_id.to_string()], row_to_token)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Revokes an active token of the user. Returns whether one was revoked.
pub fn revoke_token(conn: &Connection, user_id: Uuid, id: Uuid, now: i64) -> anyhow::Result<bool> {
    let n = conn.execute(
        "UPDATE api_tokens SET revoked_at = ?3
         WHERE id = ?1 AND user_id = ?2 AND revoked_at IS NULL",
        params![id.to_string(), user_id.to_string(), now],
    )?;
    Ok(n > 0)
}

/// The active token with this hash.
pub fn find_token_by_hash(conn: &Connection, hash: &str) -> anyhow::Result<Option<TokenRow>> {
    Ok(conn
        .query_row(
            "SELECT id, user_id, name, created_at, last_used_at FROM api_tokens
             WHERE token_hash = ?1 AND revoked_at IS NULL",
            [hash],
            row_to_token,
        )
        .optional()?)
}

pub fn touch_token(conn: &Connection, id: Uuid, now: i64) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE api_tokens SET last_used_at = ?2 WHERE id = ?1",
        params![id.to_string(), now],
    )?;
    Ok(())
}

// ----- sessions -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub user_id: Uuid,
    pub expires_at: i64,
    pub last_seen_at: i64,
}

pub fn insert_session(
    conn: &Connection,
    id_hash: &str,
    user_id: Uuid,
    now: i64,
    expires_at: i64,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO sessions (id_hash, user_id, created_at, expires_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?3)",
        params![id_hash, user_id.to_string(), now, expires_at],
    )?;
    Ok(())
}

/// The session with this hash, if it has not expired at `now`.
pub fn find_session(
    conn: &Connection,
    id_hash: &str,
    now: i64,
) -> anyhow::Result<Option<SessionRow>> {
    Ok(conn
        .query_row(
            "SELECT user_id, expires_at, last_seen_at FROM sessions
             WHERE id_hash = ?1 AND expires_at > ?2",
            params![id_hash, now],
            |row| {
                Ok(SessionRow {
                    user_id: parse_uuid_col(row, 0)?,
                    expires_at: row.get(1)?,
                    last_seen_at: row.get(2)?,
                })
            },
        )
        .optional()?)
}

pub fn extend_session(
    conn: &Connection,
    id_hash: &str,
    now: i64,
    expires_at: i64,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE sessions SET last_seen_at = ?2, expires_at = ?3 WHERE id_hash = ?1",
        params![id_hash, now, expires_at],
    )?;
    Ok(())
}

pub fn delete_session(conn: &Connection, id_hash: &str) -> anyhow::Result<()> {
    conn.execute("DELETE FROM sessions WHERE id_hash = ?1", [id_hash])?;
    Ok(())
}

/// Removes expired sessions; returns how many.
pub fn purge_expired_sessions(conn: &Connection, now: i64) -> anyhow::Result<usize> {
    Ok(conn.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now])?)
}

// ----- OAuth (built-in authorization server for /mcp) ----------------------------

/// A dynamically registered OAuth client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClientRow {
    pub id: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

fn row_to_oauth_client(row: &Row<'_>) -> rusqlite::Result<OAuthClientRow> {
    let uris: String = row.get(2)?;
    Ok(OAuthClientRow {
        id: row.get(0)?,
        name: row.get(1)?,
        redirect_uris: serde_json::from_str(&uris).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
        })?,
        created_at: row.get(3)?,
        last_used_at: row.get(4)?,
        revoked_at: row.get(5)?,
    })
}

const OAUTH_CLIENT_COLUMNS: &str =
    "c.id, c.name, c.redirect_uris_json, c.created_at, c.last_used_at, c.revoked_at";

pub fn insert_oauth_client(conn: &Connection, client: &OAuthClientRow) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO oauth_clients (id, name, redirect_uris_json, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            client.id,
            client.name,
            serde_json::to_string(&client.redirect_uris)?,
            client.created_at
        ],
    )?;
    Ok(())
}

/// The client with this ID (also revoked ones).
pub fn oauth_client(conn: &Connection, id: &str) -> anyhow::Result<Option<OAuthClientRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {OAUTH_CLIENT_COLUMNS} FROM oauth_clients c WHERE c.id = ?1"),
            [id],
            row_to_oauth_client,
        )
        .optional()?)
}

/// Number of clients registered at or after `since` (all clients for 0).
pub fn count_oauth_clients_since(conn: &Connection, since: i64) -> anyhow::Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM oauth_clients WHERE created_at >= ?1",
        [since],
        |r| r.get(0),
    )?)
}

/// Clients of the user that hold a live grant (a token that is neither
/// revoked nor expired), oldest first.
pub fn connected_oauth_clients(
    conn: &Connection,
    user_id: Uuid,
    now: i64,
) -> anyhow::Result<Vec<OAuthClientRow>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {OAUTH_CLIENT_COLUMNS} FROM oauth_clients c
         WHERE c.revoked_at IS NULL AND EXISTS (
             SELECT 1 FROM oauth_tokens t
             WHERE t.client_id = c.id AND t.user_id = ?1
               AND t.revoked_at IS NULL AND t.expires_at > ?2)
         ORDER BY c.created_at, c.rowid"
    ))?;
    let rows = stmt.query_map(params![user_id.to_string(), now], row_to_oauth_client)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Revokes a client that holds a grant of the user, together with all its
/// tokens. Returns whether a client was revoked.
pub fn revoke_oauth_client(
    conn: &mut Connection,
    user_id: Uuid,
    id: &str,
    now: i64,
) -> anyhow::Result<bool> {
    let tx = conn.transaction()?;
    let owned: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM oauth_tokens WHERE client_id = ?1 AND user_id = ?2)",
        params![id, user_id.to_string()],
        |r| r.get(0),
    )?;
    let n = if owned {
        tx.execute(
            "UPDATE oauth_clients SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, now],
        )?
    } else {
        0
    };
    if n > 0 {
        tx.execute(
            "UPDATE oauth_tokens SET revoked_at = ?2 WHERE client_id = ?1 AND revoked_at IS NULL",
            params![id, now],
        )?;
    }
    tx.commit()?;
    Ok(n > 0)
}

pub fn touch_oauth_client(conn: &Connection, id: &str, now: i64) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE oauth_clients SET last_used_at = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    Ok(())
}

/// A stored authorization code (looked up by its hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCodeRow {
    pub client_id: String,
    pub user_id: Uuid,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub resource: String,
    pub scope: Option<String>,
    pub family_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub used_at: Option<i64>,
}

pub fn insert_oauth_code(conn: &Connection, hash: &str, code: &OAuthCodeRow) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO oauth_codes (code_hash, client_id, user_id, redirect_uri, code_challenge,
             resource, scope, family_id, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            hash,
            code.client_id,
            code.user_id.to_string(),
            code.redirect_uri,
            code.code_challenge,
            code.resource,
            code.scope,
            code.family_id,
            code.created_at,
            code.expires_at
        ],
    )?;
    Ok(())
}

pub fn oauth_code(conn: &Connection, hash: &str) -> anyhow::Result<Option<OAuthCodeRow>> {
    Ok(conn
        .query_row(
            "SELECT client_id, user_id, redirect_uri, code_challenge, resource, scope, family_id,
                    created_at, expires_at, used_at
             FROM oauth_codes WHERE code_hash = ?1",
            [hash],
            |row| {
                Ok(OAuthCodeRow {
                    client_id: row.get(0)?,
                    user_id: parse_uuid_col(row, 1)?,
                    redirect_uri: row.get(2)?,
                    code_challenge: row.get(3)?,
                    resource: row.get(4)?,
                    scope: row.get(5)?,
                    family_id: row.get(6)?,
                    created_at: row.get(7)?,
                    expires_at: row.get(8)?,
                    used_at: row.get(9)?,
                })
            },
        )
        .optional()?)
}

/// Marks a code as used; `false` if it already was (single use).
pub fn mark_oauth_code_used(conn: &Connection, hash: &str, now: i64) -> anyhow::Result<bool> {
    let n = conn.execute(
        "UPDATE oauth_codes SET used_at = ?2 WHERE code_hash = ?1 AND used_at IS NULL",
        params![hash, now],
    )?;
    Ok(n > 0)
}

/// Kind of an OAuth token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthTokenKind {
    Access,
    Refresh,
}

impl OAuthTokenKind {
    fn as_str(self) -> &'static str {
        match self {
            OAuthTokenKind::Access => "access",
            OAuthTokenKind::Refresh => "refresh",
        }
    }
}

/// A stored OAuth token (looked up by its hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthTokenRow {
    pub id: String,
    pub kind: OAuthTokenKind,
    pub client_id: String,
    pub user_id: Uuid,
    pub family_id: String,
    pub resource: String,
    pub scope: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

pub fn insert_oauth_token(
    conn: &Connection,
    hash: &str,
    token: &OAuthTokenRow,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO oauth_tokens (id, token_hash, kind, client_id, user_id, family_id, resource,
             scope, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            token.id,
            hash,
            token.kind.as_str(),
            token.client_id,
            token.user_id.to_string(),
            token.family_id,
            token.resource,
            token.scope,
            token.created_at,
            token.expires_at
        ],
    )?;
    Ok(())
}

/// The token with this hash (in any state) together with its client.
pub fn oauth_token(
    conn: &Connection,
    hash: &str,
) -> anyhow::Result<Option<(OAuthTokenRow, OAuthClientRow)>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT t.id, t.kind, t.client_id, t.user_id, t.family_id, t.resource, t.scope,
                        t.created_at, t.expires_at, t.used_at, t.revoked_at, {OAUTH_CLIENT_COLUMNS}
                 FROM oauth_tokens t JOIN oauth_clients c ON c.id = t.client_id
                 WHERE t.token_hash = ?1"
            ),
            [hash],
            |row| {
                let kind: String = row.get(1)?;
                let token = OAuthTokenRow {
                    id: row.get(0)?,
                    kind: if kind == "refresh" {
                        OAuthTokenKind::Refresh
                    } else {
                        OAuthTokenKind::Access
                    },
                    client_id: row.get(2)?,
                    user_id: parse_uuid_col(row, 3)?,
                    family_id: row.get(4)?,
                    resource: row.get(5)?,
                    scope: row.get(6)?,
                    created_at: row.get(7)?,
                    expires_at: row.get(8)?,
                    used_at: row.get(9)?,
                    revoked_at: row.get(10)?,
                };
                let client = OAuthClientRow {
                    id: row.get(11)?,
                    name: row.get(12)?,
                    redirect_uris: serde_json::from_str(&row.get::<_, String>(13)?).map_err(
                        |e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                13,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        },
                    )?,
                    created_at: row.get(14)?,
                    last_used_at: row.get(15)?,
                    revoked_at: row.get(16)?,
                };
                Ok((token, client))
            },
        )
        .optional()?)
}

/// Marks a refresh token as rotated; `false` if it already was.
pub fn mark_oauth_token_used(conn: &Connection, id: &str, now: i64) -> anyhow::Result<bool> {
    let n = conn.execute(
        "UPDATE oauth_tokens SET used_at = ?2 WHERE id = ?1 AND used_at IS NULL",
        params![id, now],
    )?;
    Ok(n > 0)
}

pub fn revoke_oauth_token(conn: &Connection, id: &str, now: i64) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE oauth_tokens SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
        params![id, now],
    )?;
    Ok(())
}

/// Revokes every token of a family, including a device logged in with it;
/// returns how many were still active.
pub fn revoke_oauth_family(conn: &Connection, family_id: &str, now: i64) -> anyhow::Result<usize> {
    let tokens = conn.execute(
        "UPDATE oauth_tokens SET revoked_at = ?2 WHERE family_id = ?1 AND revoked_at IS NULL",
        params![family_id, now],
    )?;
    let devices = conn.execute(
        "UPDATE devices SET revoked_at = ?2 WHERE family_id = ?1 AND revoked_at IS NULL",
        params![family_id, now],
    )?;
    Ok(tokens + devices)
}

/// Removes expired codes and tokens and clients that can no longer be used:
/// registered before `stale_before` without a live token, or registered
/// before `unused_before` and never used (no code, no token). Returns the
/// number of removed clients.
pub fn purge_oauth(
    conn: &Connection,
    now: i64,
    stale_before: i64,
    unused_before: i64,
) -> anyhow::Result<usize> {
    conn.execute("DELETE FROM oauth_codes WHERE expires_at <= ?1", [now])?;
    conn.execute("DELETE FROM oauth_tokens WHERE expires_at <= ?1", [now])?;
    let stale = conn.execute(
        "DELETE FROM oauth_clients
         WHERE created_at < ?2 AND NOT EXISTS (
             SELECT 1 FROM oauth_tokens t
             WHERE t.client_id = oauth_clients.id AND t.revoked_at IS NULL AND t.expires_at > ?1)",
        params![now, stale_before],
    )?;
    let unused = conn.execute(
        "DELETE FROM oauth_clients
         WHERE created_at < ?1 AND last_used_at IS NULL
           AND NOT EXISTS (SELECT 1 FROM oauth_codes c WHERE c.client_id = oauth_clients.id)
           AND NOT EXISTS (SELECT 1 FROM oauth_tokens t WHERE t.client_id = oauth_clients.id)",
        [unused_before],
    )?;
    Ok(stale + unused)
}

// ----- devices (Tauri app logged in via OAuth) ------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub family_id: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

const DEVICE_COLUMNS: &str = "id, user_id, name, family_id, created_at, last_used_at";

fn row_to_device(row: &Row<'_>) -> rusqlite::Result<DeviceRow> {
    Ok(DeviceRow {
        id: parse_uuid_col(row, 0)?,
        user_id: parse_uuid_col(row, 1)?,
        name: row.get(2)?,
        family_id: row.get(3)?,
        created_at: row.get(4)?,
        last_used_at: row.get(5)?,
    })
}

pub fn insert_device(conn: &Connection, device: &DeviceRow, hash: &str) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO devices (id, user_id, name, token_hash, family_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            device.id.to_string(),
            device.user_id.to_string(),
            device.name,
            hash,
            device.family_id,
            device.created_at
        ],
    )?;
    Ok(())
}

/// Active (not revoked) devices of a user, oldest first.
pub fn list_devices(conn: &Connection, user_id: Uuid) -> anyhow::Result<Vec<DeviceRow>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {DEVICE_COLUMNS} FROM devices
         WHERE user_id = ?1 AND revoked_at IS NULL ORDER BY created_at, rowid"
    ))?;
    let rows = stmt.query_map([user_id.to_string()], row_to_device)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The active device with this token hash.
pub fn find_device_by_hash(conn: &Connection, hash: &str) -> anyhow::Result<Option<DeviceRow>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {DEVICE_COLUMNS} FROM devices WHERE token_hash = ?1 AND revoked_at IS NULL"
            ),
            [hash],
            row_to_device,
        )
        .optional()?)
}

/// Revokes an active device of the user. Returns whether one was revoked.
pub fn revoke_device(conn: &Connection, user_id: Uuid, id: Uuid, now: i64) -> anyhow::Result<bool> {
    let n = conn.execute(
        "UPDATE devices SET revoked_at = ?3 WHERE id = ?1 AND user_id = ?2 AND revoked_at IS NULL",
        params![id.to_string(), user_id.to_string(), now],
    )?;
    Ok(n > 0)
}

/// Revokes the device with this token hash (token revocation, RFC 7009).
pub fn revoke_device_by_hash(conn: &Connection, hash: &str, now: i64) -> anyhow::Result<bool> {
    let n = conn.execute(
        "UPDATE devices SET revoked_at = ?2 WHERE token_hash = ?1 AND revoked_at IS NULL",
        params![hash, now],
    )?;
    Ok(n > 0)
}

pub fn touch_device(conn: &Connection, id: Uuid, now: i64) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE devices SET last_used_at = ?2 WHERE id = ?1",
        params![id.to_string(), now],
    )?;
    Ok(())
}

// ----- backup ---------------------------------------------------------------------

/// Writes a consistent copy of the database to `dest` (`VACUUM INTO`).
pub fn vacuum_into(conn: &Connection, dest: &Path) -> anyhow::Result<()> {
    let dest = dest
        .to_str()
        .ok_or_else(|| anyhow!("Backup-Pfad ist kein UTF-8: {}", dest.display()))?;
    conn.execute("VACUUM INTO ?1", [dest])?;
    Ok(())
}

// ----- parsing helpers ------------------------------------------------------------

fn parse_entity_id(s: &str) -> anyhow::Result<Id> {
    Id::parse_any(s).ok_or_else(|| anyhow!("ungültige ID in der Datenbank: {s}"))
}

fn parse_uuid_col(row: &Row<'_>, idx: usize) -> rusqlite::Result<Uuid> {
    let s: String = row.get(idx)?;
    Uuid::parse_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn parse_kind(s: &str) -> anyhow::Result<EntityKind> {
    s.parse().map_err(|e| anyhow!("{e}"))
}

fn parse_hlc(s: &str) -> anyhow::Result<Hlc> {
    Hlc::parse(s).map_err(|e| anyhow!("ungültiger HLC in der Datenbank: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zunder_core::{Clock, NewTask, NodeId, OpBatch};

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("zunder.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn migrations_set_user_version_and_are_idempotent() {
        let (dir, conn) = temp_db();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
        drop(conn);
        // reopening does not re-run migrations
        open(&dir.path().join("zunder.db")).unwrap();
    }

    #[test]
    fn refuses_newer_schema() {
        let (dir, conn) = temp_db();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        drop(conn);
        assert!(open(&dir.path().join("zunder.db")).is_err());
    }

    fn sample_ops() -> Vec<Op> {
        let mut clock = Clock::new(NodeId::new("test").unwrap());
        let mut batch = OpBatch::new(&mut clock, 1_760_000_000_000);
        let task = batch.create_task(NewTask::new("Erste"));
        batch.set_task(task, "title", "Zweite");
        batch.into_ops()
    }

    #[test]
    fn rebuilds_entities_when_inconsistent() {
        let (_dir, mut conn) = temp_db();
        let owner = ensure_owner(&conn, 1).unwrap().id;
        let ops = sample_ops();
        let state = State::from_ops(&ops);
        let last = commit_ops(
            &mut conn,
            owner,
            &ops,
            &state.to_snapshot(),
            "test",
            None,
            1,
        )
        .unwrap();

        let loaded = load_state(&mut conn, owner).unwrap();
        assert!(!loaded.rebuilt);
        assert_eq!(loaded.cursor, last);
        assert_eq!(loaded.state, state);
        assert!(ops.iter().all(|op| loaded.state.knows_op(&op.op_id)));
        assert_eq!(loaded.max_hlc.as_ref(), ops.iter().map(|o| &o.hlc).max());

        conn.execute("DELETE FROM entities", []).unwrap();
        let loaded = load_state(&mut conn, owner).unwrap();
        assert!(loaded.rebuilt);
        assert_eq!(loaded.state, state);
        let loaded = load_state(&mut conn, owner).unwrap();
        assert!(!loaded.rebuilt, "rebuild is persisted");
        assert_eq!(loaded.state, state);
    }

    #[test]
    fn settings_by_prefix() {
        let (_dir, conn) = temp_db();
        set_setting(&conn, "timing.token", "a", 1).unwrap();
        set_setting(&conn, "timing.token", "b", 2).unwrap();
        set_setting(&conn, "timing.project", "", 2).unwrap();
        set_setting(&conn, "timingx", "nein", 2).unwrap();
        let timing = settings(&conn, "timing.").unwrap();
        assert_eq!(timing.len(), 2);
        assert_eq!(timing["timing.token"], "b");
        assert_eq!(timing["timing.project"], "");
        assert_eq!(delete_settings(&conn, "timing.").unwrap(), 2);
        assert!(settings(&conn, "timing.").unwrap().is_empty());
        assert_eq!(settings(&conn, "timingx").unwrap().len(), 1);
    }

    #[test]
    fn ensure_owner_is_stable() {
        let (_dir, conn) = temp_db();
        let a = ensure_owner(&conn, 1).unwrap();
        let b = ensure_owner(&conn, 2).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn unused_oauth_clients_are_purged_early() {
        let (_dir, conn) = temp_db();
        let client = |id: &str, created_at: i64| OAuthClientRow {
            id: id.into(),
            name: id.into(),
            redirect_uris: vec!["https://claude.ai/api/mcp/auth_callback".into()],
            created_at,
            last_used_at: None,
            revoked_at: None,
        };
        insert_oauth_client(&conn, &client("old-unused", 1_000)).unwrap();
        insert_oauth_client(&conn, &client("old-used", 1_000)).unwrap();
        insert_oauth_client(&conn, &client("new-unused", 5_000)).unwrap();
        touch_oauth_client(&conn, "old-used", 2_000).unwrap();

        // stale_before far in the past: only the early rule applies
        assert_eq!(purge_oauth(&conn, 6_000, 0, 4_000).unwrap(), 1);
        assert!(oauth_client(&conn, "old-unused").unwrap().is_none());
        assert!(oauth_client(&conn, "old-used").unwrap().is_some());
        assert!(oauth_client(&conn, "new-unused").unwrap().is_some());
    }
}
