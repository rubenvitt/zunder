//! ClickUp API v2: the [`ClickUpApi`] trait, its HTTP implementation and
//! typed views of the responses.
//!
//! The trait returns raw JSON so that the backup stays lossless; typed
//! parsing happens in [`model`] and [`crate::fetch`].

mod http;
pub mod model;

use std::future::Future;

use serde_json::Value;

pub use http::{HttpClickUp, RetryPolicy, DEFAULT_BASE_URL};

/// Errors of the ClickUp client.
#[derive(Debug, thiserror::Error)]
pub enum ClickUpError {
    #[error("ClickUp lehnt den Token ab (HTTP 401): {0}")]
    Unauthorized(String),
    #[error("ClickUp meldet HTTP {status} für {path}: {body}")]
    Status {
        status: u16,
        path: String,
        body: String,
    },
    #[error("ClickUp-Rate-Limit: nach {attempts} Versuchen für {path} aufgegeben")]
    RateLimited { path: String, attempts: u32 },
    #[error("Netzwerkfehler bei {path}: {source}")]
    Network {
        path: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("Unerwartete Antwort von ClickUp für {path}: {message}")]
    InvalidResponse { path: String, message: String },
}

pub type Result<T, E = ClickUpError> = std::result::Result<T, E>;

/// The ClickUp API v2 endpoints the importer needs.
///
/// Every method returns the raw JSON body of the response. Implementations
/// handle authentication and rate limits.
pub trait ClickUpApi {
    /// `GET /team` → `{"teams":[…]}` (workspaces).
    fn teams(&self) -> impl Future<Output = Result<Value>> + Send;

    /// `GET /team/{team_id}/space?archived=…` → `{"spaces":[…]}`.
    fn spaces(&self, team_id: &str, archived: bool) -> impl Future<Output = Result<Value>> + Send;

    /// `GET /space/{space_id}/folder?archived=…` → `{"folders":[…]}`.
    fn folders(&self, space_id: &str, archived: bool)
        -> impl Future<Output = Result<Value>> + Send;

    /// `GET /folder/{folder_id}/list?archived=…` → `{"lists":[…]}`.
    fn folder_lists(
        &self,
        folder_id: &str,
        archived: bool,
    ) -> impl Future<Output = Result<Value>> + Send;

    /// `GET /space/{space_id}/list?archived=…` → `{"lists":[…]}` (lists
    /// without folder).
    fn folderless_lists(
        &self,
        space_id: &str,
        archived: bool,
    ) -> impl Future<Output = Result<Value>> + Send;

    /// `GET /list/{list_id}` → the list including its `statuses`.
    fn list(&self, list_id: &str) -> impl Future<Output = Result<Value>> + Send;

    /// `GET /list/{list_id}/task?page=…&include_closed=true&subtasks=true…`
    /// → `{"tasks":[…],"last_page":bool}`.
    fn tasks_page(
        &self,
        list_id: &str,
        page: u32,
        archived: bool,
    ) -> impl Future<Output = Result<Value>> + Send;

    /// `GET /task/{task_id}`; `None` if ClickUp does not know the task (or it
    /// is not accessible).
    fn task(&self, task_id: &str) -> impl Future<Output = Result<Option<Value>>> + Send;

    /// `GET /team/{team_id}/time_entries?start_date=…&end_date=…` →
    /// `{"data":[…]}`. Both bounds are Unix milliseconds.
    fn time_entries(
        &self,
        team_id: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> impl Future<Output = Result<Value>> + Send;
}
