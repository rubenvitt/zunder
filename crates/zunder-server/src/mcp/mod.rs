//! MCP endpoint: Streamable HTTP under `/mcp` (implementation contract
//! section 4), hand-rolled JSON-RPC 2.0.
//!
//! - `POST /mcp` takes one JSON-RPC message or a batch (array). Requests get
//!   `200 application/json`; notifications and responses alone get `202`
//!   without body. There is no server-initiated stream: `GET` and `DELETE`
//!   answer `405`.
//! - Stateless: no `Mcp-Session-Id` is issued; a header sent by the client
//!   is ignored.
//! - Auth: bearer token only (API token, bootstrap token or OAuth access
//!   token of [`crate::oauth`]); session cookies and dev auth are not
//!   accepted. A `401` names the protected resource metadata in
//!   `WWW-Authenticate` (MCP authorization). Operations carry
//!   `mcp:<token-name>` (`mcp:oauth:<client_name>` for OAuth clients).
//! - A present `Origin` header must match `ZUNDER_PUBLIC_URL` (protection
//!   against DNS rebinding); a present `MCP-Protocol-Version` header must
//!   name a supported version.
//! - Tool failures are results with `isError: true` and a German message;
//!   protocol failures are JSON-RPC errors.

pub mod args;
pub mod schema;
pub mod tools;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::header::WWW_AUTHENTICATE;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Map, Value};

use crate::app::{App, Source};
use crate::auth::middleware::bearer_token;
use crate::auth::{Principal, PrincipalKind};
use crate::error::AppError;
use crate::oauth;

/// The protocol version this server speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// All accepted protocol versions (newest first).
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26"];
/// Body limit of `POST /mcp`.
pub const MCP_BODY_LIMIT: usize = 1024 * 1024;

/// JSON-RPC error codes.
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
}

const INSTRUCTIONS: &str = "Zunder ist eine persönliche Aufgabenverwaltung. \
Struktur: Bereiche (areas) enthalten Projekte (projects, Art standard oder dev mit GitHub-Repo). \
Jede Aufgabe liegt an genau einer Stelle (location): inbox (uneingeordnet, soll auf null), \
loose (bewusst ohne Zuordnung), area oder project. Unteraufgaben (parent_id, beliebig tief) liegen \
immer an der Stelle ihrer Wurzelaufgabe. Status (englische Codes): open, in_progress, review \
(nur Dev-Projekte), blocked, done, dropped. Typen: aufgabe; bug und feature nur in Dev-Projekten. \
Ein Epic ist einfach eine Aufgabe mit Unteraufgaben (progress zeigt erledigt/gesamt). \
filter_tasks blendet erledigte und verworfene Aufgaben 3 Tage nach der Statusänderung aus \
(include_closed zeigt sie), search findet sie immer. Aufgaben werden über task_id (ID oder \
Kennung wie ZUN-42) oder clickup_id adressiert. Projekte können einen Schlüssel (key, z. B. ZUN) \
haben; ihre Aufgaben bekommen dann vom Server eine feste Kennung (ref, ZUN-1, ZUN-2 …), die beim \
Verschieben bleibt. IDs von Bereichen und Projekten liefert list_structure (die ID ist die zunder_id im Vault). \
Dev-Modus: claim_task reserviert eine Aufgabe mit Ablaufzeit, plan, branch und pr_url halten \
Ergebnisse fest. Phönix: phoenix markiert Feuer-Schritte und Flammen einer ISO-Woche; \
start_flame/stop_flame messen Deep-Work-Blöcke (mit Timing, falls angebunden). \
Zeitangaben: due als JJJJ-MM-TT oder RFC 3339 mit Offset, remind_at als RFC 3339. \
Abgelehnte Änderungen kommen mit deutscher Begründung zurück; nichts wird still verworfen.";

/// The `/mcp` route (without state).
pub fn routes() -> Router<App> {
    Router::new()
        .route("/mcp", post(handle_post))
        .layer(DefaultBodyLimit::max(MCP_BODY_LIMIT))
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn check_origin(app: &App, headers: &HeaderMap) -> Result<(), AppError> {
    let Some(origin) = headers.get(axum::http::header::ORIGIN) else {
        return Ok(());
    };
    let allowed = app.config().public_origin();
    if origin.to_str().ok() == Some(allowed.as_str()) {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "Origin ist für /mcp nicht erlaubt".into(),
        ))
    }
}

/// `401` with the MCP authorization challenge: `WWW-Authenticate: Bearer
/// resource_metadata="…"` plus `error="invalid_token"` when a token was
/// presented but not accepted.
fn challenge(app: &App, error: Option<&str>) -> Response {
    let mut response = AppError::Unauthorized.into_response();
    let mut value = format!(
        "Bearer resource_metadata=\"{}\"",
        oauth::resource_metadata_url(app.config())
    );
    if let Some(error) = error {
        value.push_str(&format!(", error=\"{error}\""));
    }
    if let Ok(v) = HeaderValue::from_str(&value) {
        response.headers_mut().insert(WWW_AUTHENTICATE, v);
    }
    response
}

/// Authenticates `/mcp`: OAuth access tokens (`zoa_…`) via the built-in
/// authorization server, everything else as API or bootstrap token. Device
/// tokens of the app are only for `/api`.
async fn authenticate(app: &App, headers: &HeaderMap) -> Result<Principal, Response> {
    let token = match bearer_token(headers) {
        Ok(Some(token)) => token,
        Ok(None) => return Err(challenge(app, None)),
        Err(_) => return Err(challenge(app, Some("invalid_request"))),
    };
    let found = if token.starts_with(oauth::ACCESS_TOKEN_PREFIX) {
        oauth::authenticate_access(app, token).await
    } else {
        app.authenticate_token(token).await
    };
    match found {
        Ok(Some(principal)) if !matches!(principal.kind, PrincipalKind::Device { .. }) => {
            Ok(principal)
        }
        Ok(_) => Err(challenge(app, Some("invalid_token"))),
        Err(e) => Err(e.into_response()),
    }
}

async fn handle_post(
    State(app): State<App>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    if let Err(e) = check_origin(&app, &headers) {
        return e.into_response();
    }
    let principal = match authenticate(&app, &headers).await {
        Ok(p) => p,
        Err(response) => return response,
    };
    if let Some(version) = headers.get("mcp-protocol-version") {
        let supported = version
            .to_str()
            .is_ok_and(|v| SUPPORTED_VERSIONS.contains(&v));
        if !supported {
            let message = format!(
                "Nicht unterstützte MCP-Protokollversion. Unterstützt: {}",
                SUPPORTED_VERSIONS.join(", ")
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(rpc_error(Value::Null, codes::INVALID_REQUEST, message)),
            )
                .into_response();
        }
    }
    let body = match body {
        Ok(b) => b,
        Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return AppError::PayloadTooLarge("Die Anfrage ist zu groß".into()).into_response()
        }
        Err(rejection) => return AppError::bad_request(rejection.body_text()).into_response(),
    };
    let message: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(rpc_error(
                    Value::Null,
                    codes::PARSE_ERROR,
                    format!("Ungültiges JSON: {e}"),
                )),
            )
                .into_response()
        }
    };

    let source = principal.mcp_source();
    let reply = match message {
        Value::Array(items) if items.is_empty() => Some(rpc_error(
            Value::Null,
            codes::INVALID_REQUEST,
            "Leerer Batch",
        )),
        Value::Array(items) => {
            let mut responses = Vec::new();
            for item in items {
                if let Some(r) = handle_message(&app, &source, item).await {
                    responses.push(r);
                }
            }
            (!responses.is_empty()).then_some(Value::Array(responses))
        }
        single => handle_message(&app, &source, single).await,
    };
    match reply {
        Some(body) => Json(body).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// Handles one message; `None` for notifications and client responses.
async fn handle_message(app: &App, source: &Source, message: Value) -> Option<Value> {
    let Value::Object(obj) = message else {
        return Some(rpc_error(
            Value::Null,
            codes::INVALID_REQUEST,
            "Eine JSON-RPC-Nachricht muss ein Objekt sein",
        ));
    };
    let id = obj.get("id").cloned();
    let valid_id = matches!(id, Some(Value::String(_)) | Some(Value::Number(_)));
    let reply_id = if valid_id {
        id.clone().unwrap()
    } else {
        Value::Null
    };
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(rpc_error(
            reply_id,
            codes::INVALID_REQUEST,
            "jsonrpc muss \"2.0\" sein",
        ));
    }
    let Some(method) = obj.get("method") else {
        // A response from the client (we never send requests): ignore.
        if id.is_some() && (obj.contains_key("result") || obj.contains_key("error")) {
            return None;
        }
        return Some(rpc_error(reply_id, codes::INVALID_REQUEST, "method fehlt"));
    };
    let Some(method) = method.as_str() else {
        return Some(rpc_error(
            reply_id,
            codes::INVALID_REQUEST,
            "method muss ein Text sein",
        ));
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    match id {
        // Notification (initialized, cancelled …): nothing to answer.
        None => None,
        Some(_) if !valid_id => Some(rpc_error(
            Value::Null,
            codes::INVALID_REQUEST,
            "id muss ein Text oder eine Zahl sein",
        )),
        Some(id) => Some(handle_request(app, source, id, method, params).await),
    }
}

async fn handle_request(
    app: &App,
    source: &Source,
    id: Value,
    method: &str,
    params: Value,
) -> Value {
    if !(params.is_null() || params.is_object()) {
        return rpc_error(id, codes::INVALID_PARAMS, "params muss ein Objekt sein");
    }
    match method {
        "initialize" => {
            let requested = params.get("protocolVersion").and_then(Value::as_str);
            let version = requested
                .filter(|v| SUPPORTED_VERSIONS.contains(v))
                .unwrap_or(PROTOCOL_VERSION);
            rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {
                        "name": "zunder",
                        "title": "Zunder",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, tools::list()),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return rpc_error(id, codes::INVALID_PARAMS, "params.name fehlt");
            };
            let args = match params.get("arguments") {
                None | Some(Value::Null) => Map::new(),
                Some(Value::Object(map)) => map.clone(),
                Some(_) => {
                    return rpc_error(
                        id,
                        codes::INVALID_PARAMS,
                        "params.arguments muss ein Objekt sein",
                    )
                }
            };
            match tools::call(app, source.clone(), name, args).await {
                Some(result) => rpc_result(id, result),
                None => rpc_error(
                    id,
                    codes::INVALID_PARAMS,
                    format!("Unbekanntes Werkzeug: {name}"),
                ),
            }
        }
        other => rpc_error(
            id,
            codes::METHOD_NOT_FOUND,
            format!("Unbekannte Methode: {other}"),
        ),
    }
}
