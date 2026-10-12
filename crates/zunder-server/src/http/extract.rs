//! Extractors whose rejections use the JSON error format.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::StatusCode;
use serde::de::DeserializeOwned;
use uuid::Uuid;
use zunder_core::ids::Id;

use crate::error::AppError;

/// `Json<T>` with JSON errors (400, or 413 when the body limit is hit).
pub struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(json_rejection(rejection)),
        }
    }
}

fn json_rejection(rejection: JsonRejection) -> AppError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return AppError::PayloadTooLarge("Die Anfrage ist zu groß".into());
    }
    let message = match &rejection {
        JsonRejection::MissingJsonContentType(_) => {
            "Content-Type: application/json fehlt".to_owned()
        }
        other => format!("Ungültiger JSON-Body: {}", other.body_text()),
    };
    AppError::bad_request(message)
}

/// `Query<T>` with JSON errors.
pub struct ApiQuery<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for ApiQuery<T> {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(ApiQuery(value)),
            Err(rejection) => Err(query_rejection(rejection)),
        }
    }
}

fn query_rejection(rejection: QueryRejection) -> AppError {
    AppError::bad_request(format!(
        "Ungültige Query-Parameter: {}",
        rejection.body_text()
    ))
}

/// Parses an entity ID from a path segment (a legacy UUID is converted).
pub fn parse_id(raw: &str) -> Result<Id, AppError> {
    Id::parse_any(raw).ok_or_else(|| AppError::bad_request(format!("Ungültige ID: {raw}")))
}

/// Parses the UUID of a token or session from a path segment.
pub fn parse_uuid(raw: &str) -> Result<Uuid, AppError> {
    zunder_core::ids::parse_legacy_uuid(raw)
        .ok_or_else(|| AppError::bad_request(format!("Ungültige ID: {raw}")))
}
