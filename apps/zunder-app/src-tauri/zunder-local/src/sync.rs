//! One sync round: push pending operations, then fetch what is new.
//!
//! The store sits behind a `std::sync::Mutex` that is only held for the
//! short, synchronous store calls, never across a network request, so the UI
//! can keep applying local changes while a sync is in flight.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;
use zunder_core::{Rejection, Warning};

use crate::client::{HttpClient, SyncApi, PULL_LIMIT};
use crate::error::{Error, Result};
use crate::store::{LastSync, LocalStore};

/// Operations per push request (well below the server's 8 MiB body limit).
pub const PUSH_BATCH: usize = 500;

/// Regular interval of the background sync.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(30);

/// First retry delay after a network error.
const RETRY_BASE: Duration = Duration::from_secs(5);

/// Delay before the next background sync after `failures` consecutive
/// network errors: 5 s, 10 s, 20 s, then the regular interval. Short delays
/// pick up a regained connection quickly without polling a dead network
/// every few seconds forever.
pub fn retry_delay(failures: u32) -> Duration {
    if failures == 0 {
        return SYNC_INTERVAL;
    }
    let factor = 1u32 << (failures - 1).min(8);
    (RETRY_BASE * factor).min(SYNC_INTERVAL)
}

/// Result of the `sync_now` command.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SyncReport {
    /// Pending operations the server accepted.
    pub pushed: usize,
    /// Pending operations the server rejected (rolled back locally).
    pub rejected: Vec<Rejection>,
    pub warnings: Vec<Warning>,
    /// Operations received from the server.
    pub pulled: usize,
    pub cursor: i64,
    /// Operations still waiting for a push.
    pub pending: usize,
    /// Whether the visible state changed (the UI should reload its snapshot).
    pub changed: bool,
}

/// Locks the store; a poisoned lock is taken over (the store's invariants
/// hold between calls, every write is a committed transaction).
pub fn lock(store: &Mutex<LocalStore>) -> MutexGuard<'_, LocalStore> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs one sync round against `api` and records its outcome in the store.
pub async fn sync_once<A: SyncApi>(
    store: &Mutex<LocalStore>,
    api: &A,
    now_ms: i64,
) -> Result<SyncReport> {
    let result = run(store, api).await;
    let last = LastSync {
        at_ms: now_ms,
        ok: result.is_ok(),
        error: result.as_ref().err().map(ToString::to_string),
    };
    lock(store).record_sync(&last)?;
    result
}

async fn run<A: SyncApi>(store: &Mutex<LocalStore>, api: &A) -> Result<SyncReport> {
    let mut report = SyncReport::default();

    loop {
        let (batch, client_id) = {
            let s = lock(store);
            (s.pending_batch(PUSH_BATCH), s.node_id().to_string())
        };
        if batch.is_empty() {
            break;
        }
        let response = api.push(&client_id, &batch).await?;
        let applied = lock(store).apply_push_response(&batch, &response)?;
        if applied.accepted == 0 && applied.rejected.is_empty() {
            return Err(Error::Protocol(
                "der Server hat keine der Operationen angenommen oder abgelehnt".into(),
            ));
        }
        report.pushed += applied.accepted;
        report.rejected.extend(applied.rejected);
        report.warnings.extend(applied.warnings);
        report.changed |= applied.view_changed;
    }

    let needs_snapshot = lock(store).needs_snapshot();
    if needs_snapshot {
        let snapshot = api.snapshot().await?;
        report.changed |= lock(store).apply_server_snapshot(&snapshot)?;
    }

    loop {
        let since = lock(store).cursor();
        let page = api.pull(since, PULL_LIMIT).await?;
        report.pulled += page.ops.len();
        report.changed |= lock(store).apply_pull_page(&page)?;
        if !page.has_more {
            break;
        }
        if page.cursor <= since {
            return Err(Error::Protocol("Pull-Cursor rückt nicht vor".into()));
        }
    }

    let s = lock(store);
    report.cursor = s.cursor();
    report.pending = s.pending_count();
    Ok(report)
}

/// Serializes sync rounds and reuses the HTTP client (connection pool) as
/// long as server URL and token stay the same.
#[derive(Debug, Default)]
pub struct Syncer {
    client: tokio::sync::Mutex<Option<HttpClient>>,
}

impl Syncer {
    pub fn new() -> Self {
        Self::default()
    }

    /// One sync round with the configured server. Concurrent calls wait for
    /// each other.
    pub async fn sync(&self, store: &Mutex<LocalStore>, now_ms: i64) -> Result<SyncReport> {
        let mut client = self.client.lock().await;
        let remote = lock(store).remote()?.ok_or(Error::NotConfigured)?;
        let reusable = client.as_ref().is_some_and(|c| c.remote() == &remote);
        if !reusable {
            *client = Some(HttpClient::new(&remote)?);
        }
        let client = client.as_ref().expect("client was just set");
        sync_once(store, client, now_ms).await
    }
}

#[cfg(test)]
mod tests;
