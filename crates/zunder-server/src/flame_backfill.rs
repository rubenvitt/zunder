//! One-time reconstruction of `flame_log` from the op log.
//!
//! Before the flame history existed, a finished flame only survived as a
//! pair of `flame_started_at` / `flame_ended_at` operations. This walks all
//! of them in log order, pairs every start with the next end after it and
//! writes the pairs as `flame_log` (source `server`) on tasks that have none
//! yet. A start without end (still running, or lost) is dropped, as are
//! cleared or unparsable values and pairs whose end is not after the start.
//! The row `backfill.flame_log` = `1` in the settings marks the run as done.

use std::collections::BTreeMap;

use time::OffsetDateTime;
use zunder_core::ids::Id;
use zunder_core::model::fields::task as f;
use zunder_core::model::{parse_rfc3339, FlameInterval, MAX_FLAME_LOG};

use crate::app::{now_ms, App, Source};
use crate::db;
use crate::error::AppResult;

/// Settings row that marks the backfill as done.
pub const BACKFILL_KEY: &str = "backfill.flame_log";

/// Reconstructs `flame_log` from old operations, once. Returns the number
/// of tasks written (0 on every later call).
pub async fn backfill_flame_log(app: &App) -> AppResult<usize> {
    let done = app
        .with_reader(|conn| Ok(db::settings(conn, BACKFILL_KEY)?.contains_key(BACKFILL_KEY)))
        .await?;
    if done {
        return Ok(0);
    }

    let owner = app.owner_id();
    let rows = app
        .with_reader(move |conn| Ok(db::flame_ops(conn, owner)?))
        .await?;
    let mut logs: BTreeMap<Id, Vec<FlameInterval>> = BTreeMap::new();
    let mut pending: BTreeMap<Id, OffsetDateTime> = BTreeMap::new();
    for row in rows {
        let id = row.op.entity_id;
        let value = row.op.value.as_str().and_then(parse_rfc3339);
        match row.op.field.as_str() {
            f::FLAME_STARTED_AT => match value {
                Some(start) => {
                    pending.insert(id, start);
                }
                None => {
                    pending.remove(&id);
                }
            },
            // Starting clears the end in the same batch: a null end is no end.
            f::FLAME_ENDED_AT if row.op.value.is_null() => {}
            f::FLAME_ENDED_AT => {
                let start = pending.remove(&id);
                if let (Some(start), Some(end)) = (start, value) {
                    if end > start {
                        logs.entry(id)
                            .or_default()
                            .push(FlameInterval { start, end });
                    }
                }
            }
            _ => {}
        }
    }

    let (written, _) = app
        .mutate(Source::Server, move |state, batch| {
            let mut written = 0;
            for (id, mut log) in logs {
                if !state.task(id).is_some_and(|t| t.flame_log.is_empty()) {
                    continue;
                }
                if log.len() > MAX_FLAME_LOG {
                    log.drain(..log.len() - MAX_FLAME_LOG);
                }
                batch.set_task(id, f::FLAME_LOG, serde_json::json!(log));
                written += 1;
            }
            Ok(written)
        })
        .await?;

    app.with_writer(|conn| {
        db::set_setting(conn, BACKFILL_KEY, "1", now_ms())?;
        Ok(())
    })
    .await?;
    Ok(written)
}
