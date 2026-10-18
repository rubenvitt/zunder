//! One sync round: push pending operations, then fetch what is new.
//!
//! The store sits behind a `std::sync::Mutex` that is only held for the
//! short, synchronous store calls, never across a network request, so the UI
//! can keep applying local changes while a sync is in flight.
//!
//! Every step commits on its own. A round that fails halfway therefore still
//! reports what it already did ([`SyncFailure::report`]), and a round whose
//! store was reset for another server meanwhile stops before applying
//! anything ([`Error::Reconfigured`]).

use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;
use zunder_core::{Rejection, Warning};

use crate::client::{HttpClient, SyncApi, PULL_LIMIT};
use crate::error::{Error, Result};
use crate::store::{LastSync, LocalStore, Remote};

/// Operations per push request. Whole local changes only: a change larger
/// than this goes alone, but complete (see [`LocalStore::pending_batch`]).
pub const PUSH_BATCH: usize = 500;

/// Serialized operations per push request, well below the server's 8 MiB
/// body limit; like [`PUSH_BATCH`], a larger change goes alone.
pub const PUSH_BATCH_BYTES: usize = 4 * 1024 * 1024;

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
    /// The server's log no longer matched the local one (a restored
    /// backup): the confirmed data was dropped and loaded again from the
    /// snapshot; pending operations were kept.
    pub resynced: bool,
}

/// A sync round that stopped with `error`. `report` holds what the round
/// had already committed before: rejections of earlier push batches (those
/// operations are gone from the store, so no later round reports them),
/// pull pages already merged and whether the visible state changed.
/// `cursor` and `pending` are the store's current values.
#[derive(Debug)]
pub struct SyncFailure {
    pub error: Error,
    pub report: SyncReport,
}

impl SyncFailure {
    fn before_round(error: Error) -> Self {
        Self {
            error,
            report: SyncReport::default(),
        }
    }
}

impl fmt::Display for SyncFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for SyncFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl From<SyncFailure> for Error {
    fn from(failure: SyncFailure) -> Self {
        failure.error
    }
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
) -> Result<SyncReport, SyncFailure> {
    let generation = lock(store).generation();
    round(store, api, generation, now_ms).await
}

/// A round for the store generation the caller read together with the
/// connection data behind `api`.
async fn round<A: SyncApi>(
    store: &Mutex<LocalStore>,
    api: &A,
    generation: u64,
    now_ms: i64,
) -> Result<SyncReport, SyncFailure> {
    let mut report = SyncReport::default();
    let result = run(store, api, generation, &mut report).await;
    let s = lock(store);
    if s.generation() != generation {
        // The data now belongs to another server; nothing of this round
        // remains, not even its outcome.
        return Err(SyncFailure::before_round(Error::Reconfigured));
    }
    report.cursor = s.cursor();
    report.pending = s.pending_count();
    let last = LastSync {
        at_ms: now_ms,
        ok: result.is_ok(),
        error: result.as_ref().err().map(ToString::to_string),
    };
    let recorded = s.record_sync(&last);
    drop(s);
    match result.and(recorded) {
        Ok(()) => Ok(report),
        Err(error) => Err(SyncFailure { error, report }),
    }
}

/// Locks the store for applying a server response, unless a server switch
/// has reset it since the round started.
fn current(store: &Mutex<LocalStore>, generation: u64) -> Result<MutexGuard<'_, LocalStore>> {
    let s = lock(store);
    if s.generation() == generation {
        Ok(s)
    } else {
        Err(Error::Reconfigured)
    }
}

/// Push, then snapshot or pull. Every step is added to `report` as soon as
/// it is committed, so a later error does not lose it.
async fn run<A: SyncApi>(
    store: &Mutex<LocalStore>,
    api: &A,
    generation: u64,
    report: &mut SyncReport,
) -> Result<()> {
    loop {
        let (batch, client_id) = {
            let s = current(store, generation)?;
            (
                s.pending_batch(PUSH_BATCH, PUSH_BATCH_BYTES),
                s.node_id().to_string(),
            )
        };
        if batch.is_empty() {
            break;
        }
        let response = api.push(&client_id, &batch).await?;
        let applied = current(store, generation)?.apply_push_response(&batch, &response)?;
        let progressed = applied.accepted > 0 || !applied.rejected.is_empty();
        report.pushed += applied.accepted;
        report.rejected.extend(applied.rejected);
        report.warnings.extend(applied.warnings);
        report.changed |= applied.view_changed;
        if !progressed {
            return Err(Error::Protocol(
                "der Server hat keine der Operationen angenommen oder abgelehnt".into(),
            ));
        }
    }

    loop {
        if current(store, generation)?.needs_snapshot() {
            let snapshot = api.snapshot().await?;
            report.changed |= current(store, generation)?.apply_server_snapshot(&snapshot)?;
        }
        if pull(store, api, generation, report).await? {
            return Ok(());
        }
        // The server's log diverged from ours (restored backup): start over
        // from its snapshot, keeping what is still pending.
        if report.resynced {
            return Err(Error::Protocol(
                "das Operations-Log des Servers passt auch nach neuem Snapshot nicht".into(),
            ));
        }
        current(store, generation)?.reset_confirmed()?;
        report.resynced = true;
        report.changed = true;
    }
}

/// Pulls until `has_more` is false. Returns `false` if the server's log no
/// longer matches the local one (seq numbers handed out again after a
/// restore).
///
/// To notice that, every request overlaps by one: it starts at the
/// operation at our cursor, which must still be there under the same `seq`
/// and, if this store knows it ([`LocalStore::cursor_anchor`]), the same
/// ID. Right after a snapshot the overlap brings that operation in, so the
/// next request can check its ID. Applying it again changes nothing.
async fn pull<A: SyncApi>(
    store: &Mutex<LocalStore>,
    api: &A,
    generation: u64,
    report: &mut SyncReport,
) -> Result<bool> {
    loop {
        let (since, anchor) = {
            let s = current(store, generation)?;
            (s.cursor(), s.cursor_anchor()?)
        };
        let overlap = since > 0;
        let page = api
            .pull(if overlap { since - 1 } else { since }, PULL_LIMIT)
            .await?;
        if overlap {
            match page.ops.first() {
                Some(first)
                    if first.seq == since && anchor.is_none_or(|id| id == first.op.op_id) => {}
                _ => return Ok(false),
            }
        }
        report.pulled += page.ops.len() - usize::from(overlap);
        report.changed |= current(store, generation)?.apply_pull_page(&page)?;
        if !page.has_more {
            return Ok(true);
        }
        if page.cursor <= since {
            return Err(Error::Protocol("Pull-Cursor rückt nicht vor".into()));
        }
    }
}

/// Serializes sync rounds and reuses the HTTP client (connection pool) as
/// long as server URL and token stay the same.
#[derive(Debug, Default)]
pub struct Syncer {
    /// Held for a whole sync round, so rounds never overlap.
    round: tokio::sync::Mutex<()>,
    /// Client for the current server URL and token; held only briefly, so
    /// other server calls (history, flames) never wait for a sync round.
    client: Mutex<Option<HttpClient>>,
}

impl Syncer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The HTTP client for the configured server (shares the connection
    /// pool with the sync rounds). `Error::NotConfigured` without server URL
    /// and token.
    pub fn client(&self, store: &Mutex<LocalStore>) -> Result<HttpClient> {
        let remote = lock(store).remote()?.ok_or(Error::NotConfigured)?;
        self.client_for(&remote)
    }

    fn client_for(&self, remote: &Remote) -> Result<HttpClient> {
        let mut cached = self.client.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(client) = cached.as_ref().filter(|c| c.remote() == remote) {
            return Ok(client.clone());
        }
        let client = HttpClient::new(remote)?;
        *cached = Some(client.clone());
        Ok(client)
    }

    /// One sync round with the configured server. Concurrent calls wait for
    /// each other. A server switch during the round ends it with
    /// [`Error::Reconfigured`] (see [`LocalStore::generation`]).
    pub async fn sync(
        &self,
        store: &Mutex<LocalStore>,
        now_ms: i64,
    ) -> Result<SyncReport, SyncFailure> {
        let _round = self.round.lock().await;
        let (remote, generation) = {
            let s = lock(store);
            let remote = s.remote().map_err(SyncFailure::before_round)?;
            (remote, s.generation())
        };
        let remote = remote.ok_or(SyncFailure::before_round(Error::NotConfigured))?;
        let client = self
            .client_for(&remote)
            .map_err(SyncFailure::before_round)?;
        round(store, &client, generation, now_ms).await
    }
}

#[cfg(test)]
mod tests;
