//! Minimal client for the Zunder sync API (`GET /api/snapshot`,
//! `POST /api/sync/push`), authenticated with an API token.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::header::AUTHORIZATION;
use serde::{Deserialize, Serialize};
use zunder_core::{Op, SnapshotEntity};

/// Value of `client_id` and `X-Zunder-Client` for import pushes.
pub const CLIENT_ID: &str = "import";

#[derive(Debug, Clone)]
pub struct ZunderClient {
    http: reqwest::Client,
    base: String,
    token: String,
}

/// `GET /api/snapshot`.
#[derive(Debug, Clone, Deserialize)]
pub struct Snapshot {
    pub cursor: u64,
    pub entities: Vec<SnapshotEntity>,
}

/// Body of `POST /api/sync/push`.
#[derive(Debug, Serialize)]
struct PushRequest<'a> {
    client_id: &'a str,
    ops: &'a [Op],
}

/// Response of `POST /api/sync/push`. Codes stay strings so that unknown
/// future codes do not break the import.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushResponse {
    #[serde(default)]
    pub accepted: Vec<String>,
    #[serde(default)]
    pub rejected: Vec<RejectedOp>,
    #[serde(default)]
    pub warnings: Vec<PushWarning>,
    #[serde(default)]
    pub cursor: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedOp {
    pub op_id: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushWarning {
    pub entity_id: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

impl ZunderClient {
    /// `base_url` is the public server URL (e.g. `https://zunder.example`),
    /// `token` an API token (`zt_…`).
    pub fn new(base_url: &str, token: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("zunder-import/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(300))
            .build()
            .expect("HTTP client builds");
        Self {
            http,
            base: base_url.trim_end_matches('/').to_owned(),
            token: token.into(),
        }
    }

    pub async fn snapshot(&self) -> Result<Snapshot> {
        let response = self
            .http
            .get(format!("{}/api/snapshot", self.base))
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header("X-Zunder-Client", CLIENT_ID)
            .send()
            .await
            .with_context(|| format!("Zunder unter {} nicht erreichbar", self.base))?;
        parse(response, "GET /api/snapshot").await
    }

    pub async fn push(&self, ops: &[Op]) -> Result<PushResponse> {
        let response = self
            .http
            .post(format!("{}/api/sync/push", self.base))
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header("X-Zunder-Client", CLIENT_ID)
            .json(&PushRequest {
                client_id: CLIENT_ID,
                ops,
            })
            .send()
            .await
            .with_context(|| format!("Zunder unter {} nicht erreichbar", self.base))?;
        parse(response, "POST /api/sync/push").await
    }
}

async fn parse<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    what: &str,
) -> Result<T> {
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        if let Ok(err) = serde_json::from_str::<ErrorBody>(&body) {
            bail!(
                "{what}: HTTP {} – {} ({})",
                status.as_u16(),
                err.error.message,
                err.error.code
            );
        }
        bail!("{what}: HTTP {} – {body}", status.as_u16());
    }
    serde_json::from_str(&body).with_context(|| format!("{what}: unerwartete Antwort"))
}
