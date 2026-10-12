//! The calendar feed ([`crate::calendar`]) over HTTP:
//!
//! - `GET /calendar/<token>.ics` (without auth; the feed token in the path
//!   is the credential, for calendar apps that cannot send headers). An
//!   unknown or replaced token is a plain 404.
//! - `GET /api/calendar.ics` behind the usual `/api` auth (e.g. an API
//!   token for scripts).
//!
//! Both take `?scope=flames` (default) or `?scope=all`.

use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use super::extract::ApiQuery;
use crate::app::App;
use crate::auth::Principal;
use crate::calendar::FeedScope;
use crate::error::{AppError, AppResult};

/// The public feed route (without auth layer).
pub fn routes() -> Router<App> {
    Router::new().route("/calendar/{file}", get(public_feed))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedQuery {
    scope: Option<String>,
}

async fn public_feed(
    State(app): State<App>,
    Path(file): Path<String>,
    ApiQuery(query): ApiQuery<FeedQuery>,
) -> AppResult<Response> {
    let token = file.strip_suffix(".ics").unwrap_or(&file);
    if !app.calendar_token_valid(token).await? {
        return Err(AppError::NotFound("Kalender-Feed nicht gefunden".into()));
    }
    feed(&app, &query).await
}

/// `GET /api/calendar.ics`.
pub async fn api_feed(
    State(app): State<App>,
    _principal: Principal,
    ApiQuery(query): ApiQuery<FeedQuery>,
) -> AppResult<Response> {
    feed(&app, &query).await
}

async fn feed(app: &App, query: &FeedQuery) -> AppResult<Response> {
    let scope = FeedScope::parse(query.scope.as_deref())?;
    let body = app.calendar_feed(scope).await?;
    Ok((
        [
            (
                CONTENT_TYPE,
                HeaderValue::from_static("text/calendar; charset=utf-8"),
            ),
            (
                CONTENT_DISPOSITION,
                HeaderValue::from_static("inline; filename=\"zunder.ics\""),
            ),
            (CACHE_CONTROL, HeaderValue::from_static("private, no-cache")),
        ],
        body,
    )
        .into_response())
}
