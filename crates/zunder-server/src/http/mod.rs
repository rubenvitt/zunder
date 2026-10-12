//! HTTP layer: router, middleware stack and handlers.
//!
//! MCP lives under `/mcp` ([`crate::mcp`], bearer tokens only: API tokens
//! or OAuth access tokens from [`crate::oauth`]), the flame
//! endpoints under `/api/tasks/{id}/flame/*`. Protected routes use
//! [`crate::auth::middleware::require_auth`] and the [`crate::auth::Principal`]
//! extractor and call into [`App`].

pub mod api;
pub mod auth_routes;
pub mod extract;
pub mod static_files;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::header::{REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{middleware, Json, Router};
use serde_json::json;
use tower_http::compression::CompressionLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::RequestBodyTimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::app::App;
use crate::auth::middleware::require_auth;
use crate::auth::oidc::OidcClient;
use crate::error::AppError;

/// A request body that sends nothing for this long fails, so a stalled
/// upload does not hold its connection (and a graceful shutdown) open.
pub const REQUEST_BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// Builds the complete router.
pub fn router(app: App, oidc: Option<Arc<OidcClient>>) -> Router {
    let api = api::routes()
        .fallback(api_not_found)
        .route_layer(middleware::from_fn_with_state(app.clone(), require_auth));

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api)
        .merge(crate::mcp::routes())
        .merge(crate::oauth::routes::routes(
            crate::oauth::routes::OAuthState::new(app.clone()),
        ))
        .merge(auth_routes::routes(auth_routes::AuthState {
            app: app.clone(),
            oidc,
        }))
        .fallback(static_files::serve)
        .with_state(app)
        // Compression skips text/event-stream, so SSE stays unbuffered.
        .layer(CompressionLayer::new().gzip(true).br(true))
        .layer(RequestBodyTimeoutLayer::new(REQUEST_BODY_IDLE_TIMEOUT))
        .layer(SetResponseHeaderLayer::if_not_present(
            X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(TraceLayer::new_for_http())
}

async fn healthz(State(app): State<App>) -> Response {
    let version = env!("CARGO_PKG_VERSION");
    match app.health().await {
        Ok(()) => Json(json!({"status": "ok", "version": version})).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "health check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"status": "error", "version": version})),
            )
                .into_response()
        }
    }
}

async fn api_not_found() -> AppError {
    AppError::NotFound("Unbekannter API-Pfad".into())
}
