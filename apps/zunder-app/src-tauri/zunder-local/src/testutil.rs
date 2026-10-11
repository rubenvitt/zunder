//! Test helpers: operation builders and an in-memory server with the same
//! semantics as `zunder-server` (validation, op log, cursor, pagination).

use std::collections::HashSet;
use std::sync::Mutex;

use uuid::Uuid;
use zunder_core::hlc::system_time_ms;
use zunder_core::{validate_push, Clock, NodeId, Op, OpBatch, State};

use crate::client::SyncApi;
use crate::error::{Error, Result};
use crate::wire::{PullPage, PulledOp, PushResponse, ServerSnapshot};

/// Builds operations like a client (web UI) would.
pub struct Author {
    clock: Clock,
}

impl Author {
    pub fn new(node: &str) -> Self {
        Self {
            clock: Clock::new(NodeId::new(node).unwrap()),
        }
    }

    pub fn ops(&mut self, build: impl FnOnce(&mut OpBatch<'_>)) -> Vec<Op> {
        let mut batch = OpBatch::new(&mut self.clock, system_time_ms());
        build(&mut batch);
        batch.into_ops()
    }

    /// Lets this author's clock see `ops` (causality: later ops win).
    pub fn receive(&mut self, ops: &[Op]) {
        for op in ops {
            self.clock.receive(&op.hlc);
        }
    }

    /// Like [`Author::ops`], returning the ID the closure yields.
    pub fn create(&mut self, build: impl FnOnce(&mut OpBatch<'_>) -> Uuid) -> (Uuid, Vec<Op>) {
        let mut id = Uuid::nil();
        let ops = self.ops(|b| id = build(b));
        (id, ops)
    }
}

#[derive(Default)]
struct ServerState {
    state: State,
    log: Vec<Op>,
    /// The next request fails with a network error.
    offline: bool,
    /// The next push is processed but its response is lost.
    lose_push_response: bool,
    pushes: usize,
    snapshots: usize,
    pulls: usize,
}

/// In-memory stand-in for `zunder-server`.
pub struct FakeServer {
    inner: Mutex<ServerState>,
    page_size: usize,
}

impl FakeServer {
    pub fn new() -> Self {
        Self::with_page_size(usize::MAX)
    }

    pub fn with_page_size(page_size: usize) -> Self {
        Self {
            inner: Mutex::new(ServerState::default()),
            page_size,
        }
    }

    pub fn set_offline(&self, offline: bool) {
        self.inner.lock().unwrap().offline = offline;
    }

    pub fn lose_next_push_response(&self) {
        self.inner.lock().unwrap().lose_push_response = true;
    }

    pub fn state(&self) -> State {
        self.inner.lock().unwrap().state.clone()
    }

    pub fn cursor(&self) -> i64 {
        self.inner.lock().unwrap().log.len() as i64
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let s = self.inner.lock().unwrap();
        (s.pushes, s.snapshots, s.pulls)
    }

    /// Writes operations directly (another client), panicking on rejection.
    pub fn accept(&self, ops: &[Op]) {
        let response = self.inner.lock().unwrap().push(ops);
        assert!(response.rejected.is_empty(), "{:?}", response.rejected);
    }

    fn check_online(s: &mut ServerState) -> Result<()> {
        if s.offline {
            Err(Error::Network("offline (Test)".into()))
        } else {
            Ok(())
        }
    }
}

impl ServerState {
    fn push(&mut self, ops: &[Op]) -> PushResponse {
        let outcome = validate_push(&self.state, ops, system_time_ms());
        let mut seen = HashSet::new();
        for op in &outcome.new_ops {
            if seen.insert(op.op_id) {
                self.state.apply(op);
                self.log.push(op.clone());
            }
        }
        PushResponse {
            accepted: outcome.accepted,
            rejected: outcome.rejected,
            warnings: outcome.warnings,
            cursor: self.log.len() as i64,
        }
    }
}

impl SyncApi for FakeServer {
    async fn snapshot(&self) -> Result<ServerSnapshot> {
        let mut s = self.inner.lock().unwrap();
        Self::check_online(&mut s)?;
        s.snapshots += 1;
        Ok(ServerSnapshot {
            cursor: s.log.len() as i64,
            entities: s.state.to_snapshot(),
        })
    }

    async fn push(&self, _client_id: &str, ops: &[Op]) -> Result<PushResponse> {
        let mut s = self.inner.lock().unwrap();
        Self::check_online(&mut s)?;
        s.pushes += 1;
        let response = s.push(ops);
        if std::mem::take(&mut s.lose_push_response) {
            return Err(Error::Network("Antwort verloren (Test)".into()));
        }
        Ok(response)
    }

    async fn pull(&self, since: i64, limit: u32) -> Result<PullPage> {
        let mut s = self.inner.lock().unwrap();
        Self::check_online(&mut s)?;
        s.pulls += 1;
        let start = usize::try_from(since).unwrap().min(s.log.len());
        let take = (limit as usize).min(self.page_size);
        let ops: Vec<PulledOp> = s.log[start..]
            .iter()
            .take(take)
            .enumerate()
            .map(|(i, op)| PulledOp {
                seq: (start + i + 1) as i64,
                op: op.clone(),
            })
            .collect();
        let cursor = ops.last().map_or(s.log.len() as i64, |p| p.seq);
        Ok(PullPage {
            has_more: (cursor as usize) < s.log.len(),
            ops,
            cursor,
        })
    }
}
