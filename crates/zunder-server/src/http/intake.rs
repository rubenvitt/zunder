//! `/intake/v1/…`: write-only task creation through an intake channel
//! ([`crate::intake`]). Outside `/api`: no session, no CSRF header, no CORS;
//! only `Authorization: Bearer zi_…`. Tokens and bodies are never logged.

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde_json::Value;

use super::extract::ApiJson;
use crate::app::App;
use crate::auth::middleware::bearer_token;
use crate::error::{AppError, AppResult};
use crate::intake::{IntakeChannel, Submission};

/// Body limit of intake requests.
pub const INTAKE_BODY_LIMIT: usize = 64 * 1024;

/// The `/intake` routes.
pub fn routes(app: App) -> Router<App> {
    let intake = Router::new()
        .route("/v1/tasks", post(create_task))
        .route_layer(middleware::from_fn_with_state(app, require_channel))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(INTAKE_BODY_LIMIT));
    Router::new().nest("/intake", intake)
}

/// Requires the token of an active channel; anything else is 401 (an API
/// token as well).
async fn require_channel(State(app): State<App>, mut req: Request, next: Next) -> Response {
    let channel = match bearer_token(req.headers()) {
        Ok(Some(token)) => app.authenticate_intake(token).await,
        Ok(None) | Err(_) => Err(AppError::Unauthorized),
    };
    match channel {
        Ok(Some(channel)) => {
            req.extensions_mut().insert(channel);
            next.run(req).await
        }
        Ok(None) => AppError::Unauthorized.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn create_task(
    State(app): State<App>,
    Extension(channel): Extension<IntakeChannel>,
    ApiJson(body): ApiJson<Value>,
) -> AppResult<Response> {
    let submission = Submission::parse(&body)?;
    let submitted = app.submit_intake(channel, submission).await?;
    let status = if submitted.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(submitted)).into_response())
}

async fn not_found() -> AppError {
    AppError::NotFound("Unbekannter Pfad".into())
}
