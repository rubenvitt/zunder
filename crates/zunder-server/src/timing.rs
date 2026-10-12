//! Client for the Timing web API (<https://web.timingapp.com/docs/>), used by
//! the flame service ([`crate::flame`]).
//!
//! Three operations are needed:
//!
//! - **start**: `POST /time-entries/start` with `title` and optionally
//!   `project`. Timing stops a running timer by itself when a new one
//!   starts. The response identifies the entry by its self link
//!   (`data.self`, e.g. `/time-entries/2`); Zunder stores that link as
//!   `timing_ref`.
//! - **stop**: `PUT /time-entries/stop` stops *the* running timer. To never
//!   stop a timer the user started elsewhere, the client first reads the
//!   entry Zunder started (`GET` on its self link, appended to the API root)
//!   and only stops when that entry still runs. Only one timer runs at a
//!   time, so a running entry is the running timer. It does not ask
//!   `GET /time-entries/running`: that answers with a redirect, and a stop
//!   must not depend on how the redirect is followed.
//! - **check**: the same `GET /time-entries/running` tells whether Timing
//!   accepts the token (settings view, "Verbindung prüfen").
//! - **projects**: `GET /projects?hide_archived=true` lists the projects the
//!   settings view offers for new time entries.
//!
//! The bridge sits behind [`TimingApi`] so the flame service does not depend
//! on HTTP details; tests run [`TimingClient`] against a mock server.

use futures_util::future::BoxFuture;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

/// Base URL of the Timing web API.
pub const DEFAULT_BASE_URL: &str = "https://web.timingapp.com/api/v1";

/// Request to start a timer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartTimer {
    pub title: String,
    /// Self link (`/projects/1`) or title of a Timing project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// `false`: never adjust or delete other entries. Timing stops the
    /// running timer anyway when a new one starts.
    pub replace_existing: bool,
}

/// A time entry as far as Zunder cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimingEntry {
    /// Self link, e.g. `/time-entries/2` (Timing has no separate ID field).
    pub self_ref: String,
    pub is_running: bool,
}

/// A Timing project for the settings view's project picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimingProject {
    /// Self link, e.g. `/projects/1`; what the settings store.
    #[serde(rename = "self")]
    pub self_ref: String,
    pub title: String,
    /// Titles from the root project down to this one.
    #[serde(default)]
    pub title_chain: Vec<String>,
    #[serde(default)]
    pub color: Option<String>,
}

/// Errors of the Timing API (messages are German, shown to users).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimingError {
    #[error("Timing ist nicht erreichbar: {0}")]
    Transport(String),
    #[error("Timing antwortet mit Status {status}: {message}")]
    Status { status: u16, message: String },
    #[error("Unerwartete Antwort von Timing: {0}")]
    InvalidResponse(String),
}

/// The operations the flame service needs from Timing.
pub trait TimingApi: Send + Sync {
    /// Starts a timer and returns the new entry.
    fn start(&self, request: StartTimer) -> BoxFuture<'_, Result<TimingEntry, TimingError>>;

    /// Stops the running timer if it is the entry `self_ref`. Returns
    /// whether it was stopped (`false`: that entry no longer runs, e.g.
    /// because another timer was started in Timing).
    fn stop_if_running<'a>(&'a self, self_ref: &'a str)
        -> BoxFuture<'a, Result<bool, TimingError>>;

    /// Checks that Timing is reachable and accepts the token.
    fn check(&self) -> BoxFuture<'_, Result<(), TimingError>>;

    /// The projects that are not archived.
    fn projects(&self) -> BoxFuture<'_, Result<Vec<TimingProject>, TimingError>>;
}

/// HTTP implementation of [`TimingApi`].
#[derive(Debug, Clone)]
pub struct TimingClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

#[derive(Deserialize)]
struct EntryEnvelope {
    data: EntryData,
}

#[derive(Deserialize)]
struct ProjectsEnvelope {
    data: Vec<TimingProject>,
}

#[derive(Deserialize)]
struct EntryData {
    #[serde(rename = "self")]
    self_ref: String,
    #[serde(default)]
    is_running: bool,
}

impl TimingClient {
    pub fn new(http: reqwest::Client, base_url: &Url, token: impl Into<String>) -> Self {
        Self {
            http,
            base_url: base_url.as_str().trim_end_matches('/').to_owned(),
            token: token.into(),
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&StartTimer>,
    ) -> Result<(StatusCode, Value), TimingError> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| TimingError::Transport(e.to_string()))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| TimingError::Transport(e.to_string()))?;
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Ok((status, json))
    }

    fn entry(json: Value) -> Result<TimingEntry, TimingError> {
        let envelope: EntryEnvelope = serde_json::from_value(json)
            .map_err(|e| TimingError::InvalidResponse(format!("Eintrag ohne data.self ({e})")))?;
        Ok(TimingEntry {
            self_ref: envelope.data.self_ref,
            is_running: envelope.data.is_running,
        })
    }

    fn status_error(status: StatusCode, json: &Value) -> TimingError {
        let message = json
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| status.canonical_reason().unwrap_or("unbekannter Fehler"))
            .to_owned();
        TimingError::Status {
            status: status.as_u16(),
            message,
        }
    }

    /// The running timer, `None` if no timer runs.
    pub async fn running(&self) -> Result<Option<TimingEntry>, TimingError> {
        let (status, json) = self
            .send(Method::GET, "/time-entries/running", None)
            .await?;
        match status {
            StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => Self::entry(json).map(Some),
            s => Err(Self::status_error(s, &json)),
        }
    }

    /// The entry behind a self link (`/time-entries/2` or an absolute URL),
    /// `None` if Timing does not know it (any more).
    pub async fn entry_by_ref(&self, self_ref: &str) -> Result<Option<TimingEntry>, TimingError> {
        let (status, json) = self.send(Method::GET, entry_path(self_ref), None).await?;
        match status {
            StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => Self::entry(json).map(Some),
            s => Err(Self::status_error(s, &json)),
        }
    }

    pub async fn start_timer(&self, request: &StartTimer) -> Result<TimingEntry, TimingError> {
        let (status, json) = self
            .send(Method::POST, "/time-entries/start", Some(request))
            .await?;
        if !status.is_success() {
            return Err(Self::status_error(status, &json));
        }
        Self::entry(json)
    }

    /// Projects that are not archived, with the title chain filled in.
    pub async fn list_projects(&self) -> Result<Vec<TimingProject>, TimingError> {
        let (status, json) = self
            .send(Method::GET, "/projects?hide_archived=true", None)
            .await?;
        if !status.is_success() {
            return Err(Self::status_error(status, &json));
        }
        let envelope: ProjectsEnvelope = serde_json::from_value(json)
            .map_err(|e| TimingError::InvalidResponse(format!("Projektliste ({e})")))?;
        Ok(envelope
            .data
            .into_iter()
            .map(|mut p| {
                if p.title_chain.is_empty() {
                    p.title_chain = vec![p.title.clone()];
                }
                p
            })
            .collect())
    }

    pub async fn stop_timer(&self) -> Result<TimingEntry, TimingError> {
        let (status, json) = self.send(Method::PUT, "/time-entries/stop", None).await?;
        if !status.is_success() {
            return Err(Self::status_error(status, &json));
        }
        Self::entry(json)
    }
}

impl TimingApi for TimingClient {
    fn start(&self, request: StartTimer) -> BoxFuture<'_, Result<TimingEntry, TimingError>> {
        Box::pin(async move { self.start_timer(&request).await })
    }

    fn stop_if_running<'a>(
        &'a self,
        self_ref: &'a str,
    ) -> BoxFuture<'a, Result<bool, TimingError>> {
        Box::pin(async move {
            match self.entry_by_ref(self_ref).await? {
                Some(entry) if entry.is_running => {
                    self.stop_timer().await?;
                    Ok(true)
                }
                _ => Ok(false),
            }
        })
    }

    fn check(&self) -> BoxFuture<'_, Result<(), TimingError>> {
        Box::pin(async move { self.running().await.map(|_| ()) })
    }

    fn projects(&self) -> BoxFuture<'_, Result<Vec<TimingProject>, TimingError>> {
        Box::pin(async move { self.list_projects().await })
    }
}

/// Compares self links, tolerating an absolute URL on one side.
/// The path of an entry's self link below the API root (`/time-entries/2`),
/// also for an absolute URL.
fn entry_path(self_ref: &str) -> &str {
    let s = self_ref.trim_end_matches('/');
    match s.find("/time-entries/") {
        Some(i) => &s[i..],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_paths_below_the_api_root() {
        assert_eq!(entry_path("/time-entries/2"), "/time-entries/2");
        assert_eq!(
            entry_path("https://web.timingapp.com/api/v1/time-entries/2/"),
            "/time-entries/2"
        );
    }

    #[test]
    fn start_request_shape() {
        let body = serde_json::to_value(StartTimer {
            title: "Sync bauen".into(),
            project: None,
            replace_existing: false,
        })
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({"title": "Sync bauen", "replace_existing": false})
        );
    }
}
