//! Request and response bodies of the server's sync API (contract section 3).

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zunder_core::{Op, Rejection, SnapshotEntity, Warning};

/// Body of `POST /api/sync/push`.
#[derive(Debug, Clone, Serialize)]
pub struct PushRequest<'a> {
    pub client_id: &'a str,
    pub ops: &'a [Op],
}

/// Response of `POST /api/sync/push`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PushResponse {
    pub accepted: Vec<Uuid>,
    pub rejected: Vec<Rejection>,
    pub warnings: Vec<Warning>,
    pub cursor: i64,
}

/// One operation of a pull page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulledOp {
    pub seq: i64,
    pub op: Op,
}

/// Response of `GET /api/sync/pull`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PullPage {
    pub ops: Vec<PulledOp>,
    pub cursor: i64,
    pub has_more: bool,
}

/// Response of `GET /api/snapshot`; also the shape of the app's
/// `local_snapshot` command (plus local extras, see [`crate::LocalSnapshot`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerSnapshot {
    pub cursor: i64,
    pub entities: Vec<SnapshotEntity>,
}

/// Error body of the server: `{"error":{"code","message"}}`.
#[derive(Debug, Clone, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}
