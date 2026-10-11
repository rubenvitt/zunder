//! Error type of the application layer and its JSON form
//! `{"error":{"code":…,"message":…}}` (contract section 3).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// An error that maps to an HTTP status, a stable `code` and a German
/// `message`. MCP tools map the same variants to tool errors.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Anmeldung erforderlich")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{message}")]
    BadRequest { code: &'static str, message: String },
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    PayloadTooLarge(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("interner Fehler")]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        AppError::BadRequest {
            code: "invalid_request",
            message: message.into(),
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::BadRequest { .. } => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            AppError::Unauthorized => "unauthorized",
            AppError::Forbidden(_) => "forbidden",
            AppError::NotFound(_) => "not_found",
            AppError::BadRequest { code, .. } => code,
            AppError::Conflict(_) => "conflict",
            AppError::PayloadTooLarge(_) => "payload_too_large",
            AppError::Unavailable(_) => "unavailable",
            AppError::Internal(_) => "internal",
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Internal(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if let AppError::Internal(e) = &self {
            tracing::error!(error = ?e, "internal error");
        }
        let body = json!({"error": {"code": self.code(), "message": self.to_string()}});
        let mut response = (self.status(), Json(body)).into_response();
        if matches!(self, AppError::Unauthorized) {
            response.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                axum::http::HeaderValue::from_static("Bearer realm=\"zunder\""),
            );
        }
        response
    }
}

pub type AppResult<T> = Result<T, AppError>;
