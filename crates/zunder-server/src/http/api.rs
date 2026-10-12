//! `/api` handlers (all behind [`crate::auth::middleware::require_auth`]).

use std::collections::HashMap;
use std::convert::Infallible;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE, VARY};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures_util::{stream, Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;
use zunder_core::Op;

use super::extract::{parse_id, parse_uuid, ApiJson, ApiQuery};
use crate::app::{App, CreatedToken, HistoryEntry, PullPage, PushResult, Snapshot, TokenInfo};
use crate::auth::Principal;
use crate::error::{AppError, AppResult};
use crate::flame::{self, FlameOutcome};
use crate::oauth;

/// Body limit of `/api` requests except push.
pub const API_BODY_LIMIT: usize = 64 * 1024;
/// Body limit of `POST /api/sync/push` (imports send large batches).
pub const PUSH_BODY_LIMIT: usize = 8 * 1024 * 1024;
/// Maximum length of `client_id`.
const MAX_CLIENT_ID_CHARS: usize = 128;

/// The `/api` routes (without auth layer).
pub fn routes() -> Router<App> {
    Router::new()
        .route("/me", get(me))
        .route("/snapshot", get(snapshot))
        .route(
            "/sync/push",
            post(push).layer(DefaultBodyLimit::max(PUSH_BODY_LIMIT)),
        )
        .route("/sync/pull", get(pull))
        .route("/sync/events", get(events))
        .route("/tasks/{id}/history", get(history))
        .route("/tasks/{id}/flame/start", post(flame_start))
        .route("/tasks/{id}/flame/stop", post(flame_stop))
        .route("/tokens", get(list_tokens).post(create_token))
        .route("/tokens/{id}", delete(revoke_token))
        .route("/oauth/clients", get(list_oauth_clients))
        .route("/oauth/clients/{id}", delete(revoke_oauth_client))
        .layer(DefaultBodyLimit::max(API_BODY_LIMIT))
}

async fn me(State(app): State<App>, principal: Principal) -> AppResult<Json<serde_json::Value>> {
    let user = app
        .user(principal.user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    Ok(Json(json!({
        "user_id": user.id,
        "name": user.name,
        "email": user.email,
    })))
}

/// Content coding of a snapshot response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Coding {
    Br,
    Gzip,
    Identity,
}

impl Coding {
    /// Brotli before gzip before identity, per `Accept-Encoding` (`q=0`
    /// refuses a coding).
    fn negotiate(headers: &HeaderMap) -> Self {
        let (mut br, mut gzip) = (false, false);
        let items = headers
            .get_all(ACCEPT_ENCODING)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','));
        for item in items {
            let mut parts = item.split(';').map(str::trim);
            let coding = parts.next().unwrap_or_default();
            let refused = parts.any(|p| {
                p.split_once('=').is_some_and(|(k, q)| {
                    k.trim().eq_ignore_ascii_case("q")
                        && q.trim().parse::<f32>().is_ok_and(|q| q <= 0.0)
                })
            });
            if refused {
                continue;
            }
            br |= coding.eq_ignore_ascii_case("br");
            gzip |= coding.eq_ignore_ascii_case("gzip");
        }
        match (br, gzip) {
            (true, _) => Coding::Br,
            (false, true) => Coding::Gzip,
            _ => Coding::Identity,
        }
    }

    fn header(self) -> Option<HeaderValue> {
        match self {
            Coding::Br => Some(HeaderValue::from_static("br")),
            Coding::Gzip => Some(HeaderValue::from_static("gzip")),
            Coding::Identity => None,
        }
    }

    /// Serializes `snapshot` as JSON in this coding.
    fn encode(self, snapshot: &Snapshot) -> std::io::Result<Vec<u8>> {
        use std::io::Write;
        match self {
            Coding::Identity => Ok(serde_json::to_vec(snapshot)?),
            Coding::Gzip => {
                let mut w = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                serde_json::to_writer(&mut w, snapshot)?;
                w.finish()
            }
            Coding::Br => {
                // Quality 5 compresses large JSON well at a fraction of the
                // time of higher levels.
                let mut w = brotli::CompressorWriter::new(Vec::new(), 64 * 1024, 5, 22);
                serde_json::to_writer(&mut w, snapshot)?;
                w.flush()?;
                Ok(w.into_inner())
            }
        }
    }
}

/// The full state. Copying, serializing and compressing it takes long for a
/// large state, so all of it runs on the blocking pool (the response carries
/// its own `Content-Encoding`, which the compression layer leaves alone).
async fn snapshot(
    State(app): State<App>,
    _principal: Principal,
    headers: HeaderMap,
) -> AppResult<Response> {
    let coding = Coding::negotiate(&headers);
    let body = tokio::task::spawn_blocking(move || coding.encode(&app.snapshot()))
        .await
        .map_err(|e| AppError::Internal(e.into()))?
        .map_err(|e| AppError::Internal(e.into()))?;
    let mut response = Response::new(axum::body::Body::from(body));
    let h = response.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(VARY, HeaderValue::from_static("accept-encoding"));
    if let Some(value) = coding.header() {
        h.insert(CONTENT_ENCODING, value);
    }
    Ok(response)
}

#[derive(Deserialize)]
struct PushBody {
    #[serde(default)]
    client_id: Option<String>,
    ops: Vec<SentOp>,
}

/// An operation as sent, with its IDs in the form the client used. Clients
/// from before CUID2 send UUIDs; the server converts them, but answers with
/// the IDs it was given, so such a client recognizes its own operations.
struct SentOp {
    op: Op,
    op_id: String,
    entity_id: String,
}

impl<'de> Deserialize<'de> for SentOp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = serde_json::Value::deserialize(deserializer)?;
        let text = |key: &str| {
            raw.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let (op_id, entity_id) = (text("op_id"), text("entity_id"));
        let op = Op::deserialize(raw).map_err(serde::de::Error::custom)?;
        Ok(SentOp {
            op,
            op_id,
            entity_id,
        })
    }
}

/// Writes IDs back in the form the client sent them (see [`SentOp`]).
fn echo_sent_ids(result: &PushResult, sent: &[SentOp]) -> serde_json::Result<Value> {
    let mut ops = HashMap::new();
    let mut entities = HashMap::new();
    for s in sent {
        if s.op.op_id.as_str() != s.op_id {
            ops.insert(s.op.op_id.to_string(), s.op_id.clone());
        }
        if s.op.entity_id.as_str() != s.entity_id {
            entities.insert(s.op.entity_id.to_string(), s.entity_id.clone());
        }
    }
    let mut value = serde_json::to_value(result)?;
    if ops.is_empty() && entities.is_empty() {
        return Ok(value);
    }
    let replace = |v: &mut Value, map: &HashMap<String, String>| {
        if let Some(original) = v.as_str().and_then(|s| map.get(s)) {
            *v = Value::String(original.clone());
        }
    };
    for v in value["accepted"].as_array_mut().into_iter().flatten() {
        replace(v, &ops);
    }
    for r in value["rejected"].as_array_mut().into_iter().flatten() {
        replace(&mut r["op_id"], &ops);
    }
    for w in value["warnings"].as_array_mut().into_iter().flatten() {
        replace(&mut w["entity_id"], &entities);
    }
    Ok(value)
}

async fn push(
    State(app): State<App>,
    principal: Principal,
    ApiJson(body): ApiJson<PushBody>,
) -> AppResult<Json<Value>> {
    if body
        .client_id
        .as_ref()
        .is_some_and(|c| c.chars().count() > MAX_CLIENT_ID_CHARS)
    {
        return Err(AppError::bad_request(format!(
            "client_id darf höchstens {MAX_CLIENT_ID_CHARS} Zeichen lang sein"
        )));
    }
    let ops = body.ops.iter().map(|s| s.op.clone()).collect();
    let result = app
        .push(ops, principal.sync_source(), body.client_id)
        .await?;
    let echoed = echo_sent_ids(&result, &body.ops).map_err(|e| AppError::Internal(e.into()))?;
    Ok(Json(echoed))
}

#[derive(Deserialize)]
struct PullQuery {
    #[serde(default)]
    since: i64,
    limit: Option<u32>,
}

async fn pull(
    State(app): State<App>,
    _principal: Principal,
    ApiQuery(q): ApiQuery<PullQuery>,
) -> AppResult<Json<PullPage>> {
    if q.since < 0 {
        return Err(AppError::bad_request("since darf nicht negativ sein"));
    }
    if q.limit == Some(0) {
        return Err(AppError::bad_request("limit muss mindestens 1 sein"));
    }
    let limit = q.limit.unwrap_or(crate::app::DEFAULT_PULL_LIMIT);
    Ok(Json(app.pull(q.since, limit).await?))
}

/// Server-Sent Events: `event: cursor` with the current cursor right away
/// and after every change. Ends on shutdown.
async fn events(State(app): State<App>, _principal: Principal) -> Response {
    // Subscribe before reading the cursor, so no change is missed.
    let rx = app.subscribe();
    let initial = app.cursor();
    let lagged_app = app.clone();
    let cursors =
        stream::once(async move { initial }).chain(BroadcastStream::new(rx).map(move |item| {
            match item {
                Ok(cursor) => cursor,
                // Missed notifications collapse into the current cursor.
                Err(_) => lagged_app.cursor(),
            }
        }));
    let events: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> = Box::pin(
        cursors
            .map(|cursor| Ok(Event::default().event("cursor").data(cursor.to_string())))
            .take_until(app.shutdown_token().cancelled_owned()),
    );
    let mut response = Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response();
    // Ask reverse proxies not to buffer the stream.
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

async fn history(
    State(app): State<App>,
    _principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<HistoryEntry>>> {
    let id = parse_id(&id)?;
    Ok(Json(app.task_history(id).await?))
}

async fn flame_start(
    State(app): State<App>,
    _principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<FlameOutcome>> {
    let id = parse_id(&id)?;
    Ok(Json(flame::start(&app, id).await?))
}

async fn flame_stop(
    State(app): State<App>,
    _principal: Principal,
    Path(id): Path<String>,
) -> AppResult<Json<FlameOutcome>> {
    let id = parse_id(&id)?;
    Ok(Json(flame::stop(&app, id).await?))
}

fn require_browser(principal: &Principal) -> AppResult<()> {
    if principal.is_browser() {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "API-Tokens lassen sich nur mit einer Browser-Sitzung verwalten".into(),
        ))
    }
}

async fn list_tokens(
    State(app): State<App>,
    principal: Principal,
) -> AppResult<Json<Vec<TokenInfo>>> {
    require_browser(&principal)?;
    Ok(Json(app.list_tokens(principal.user_id).await?))
}

#[derive(Deserialize)]
struct CreateTokenBody {
    name: String,
}

async fn create_token(
    State(app): State<App>,
    principal: Principal,
    ApiJson(body): ApiJson<CreateTokenBody>,
) -> AppResult<(StatusCode, Json<CreatedToken>)> {
    require_browser(&principal)?;
    let token = app.create_token(principal.user_id, &body.name).await?;
    Ok((StatusCode::CREATED, Json(token)))
}

async fn revoke_token(
    State(app): State<App>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    require_browser(&principal)?;
    let id = parse_uuid(&id)?;
    app.revoke_token(principal.user_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_oauth_clients(
    State(app): State<App>,
    principal: Principal,
) -> AppResult<Json<Vec<oauth::ClientInfo>>> {
    require_browser(&principal)?;
    Ok(Json(
        oauth::connected_clients(&app, principal.user_id).await?,
    ))
}

async fn revoke_oauth_client(
    State(app): State<App>,
    principal: Principal,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    require_browser(&principal)?;
    oauth::revoke_client(&app, principal.user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
