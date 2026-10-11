//! Mock servers for the integration tests: a ClickUp API v2 serving the
//! synthetic fixtures and a Zunder sync API backed by `zunder-core`.

#![allow(dead_code)] // not every test binary uses every helper

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Map, Value};
use zunder_core::hlc::system_time_ms;
use zunder_core::{validate_push, Op, State};

use zunder_import::clickup::{HttpClickUp, RetryPolicy};
use zunder_import::mapping::Mapping;

pub const CLICKUP_TOKEN: &str = "pk_0000_SYNTHETISCH";
pub const ZUNDER_TOKEN: &str = "zt_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn load_json(name: &str) -> Value {
    let text = std::fs::read_to_string(fixture_path(name)).expect("fixture readable");
    serde_json::from_str(&text).expect("fixture is JSON")
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

// ----- ClickUp ----------------------------------------------------------------

pub struct ClickUpState {
    /// Route key → response body; tests may change it between calls.
    pub routes: Mutex<Map<String, Value>>,
    pub time_entries: Vec<Value>,
    /// Every request as `path?query`.
    pub requests: Mutex<Vec<String>>,
    /// Path → number of 429 responses still to send.
    pub rate_limits: Mutex<HashMap<String, u32>>,
}

pub struct ClickUpMock {
    pub url: String,
    pub state: Arc<ClickUpState>,
}

impl ClickUpMock {
    pub async fn start() -> Self {
        let Value::Object(routes) = load_json("clickup/routes.json") else {
            panic!("routes.json is an object");
        };
        let Value::Array(time_entries) = load_json("clickup/time_entries.json") else {
            panic!("time_entries.json is an array");
        };
        let state = Arc::new(ClickUpState {
            routes: Mutex::new(routes),
            time_entries,
            requests: Mutex::new(Vec::new()),
            rate_limits: Mutex::new(HashMap::new()),
        });
        let shared = state.clone();
        let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
            let state = shared.clone();
            async move { clickup_handler(&state, &uri, &headers) }
        });
        Self {
            url: serve(app).await,
            state,
        }
    }

    /// A client with fast retries for tests.
    pub fn client(&self) -> HttpClickUp {
        HttpClickUp::new(CLICKUP_TOKEN, &self.url).with_retry(RetryPolicy {
            max_attempts: 4,
            max_wait: std::time::Duration::from_secs(2),
            fallback_wait: std::time::Duration::from_millis(10),
        })
    }

    pub fn requests(&self) -> Vec<String> {
        self.state.requests.lock().unwrap().clone()
    }

    pub fn requests_to(&self, path: &str) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|r| r.split('?').next() == Some(path))
            .collect()
    }

    pub fn rate_limit(&self, path: &str, times: u32) {
        self.state
            .rate_limits
            .lock()
            .unwrap()
            .insert(path.to_owned(), times);
    }

    /// Edits a task inside a route body (by task ID).
    pub fn edit_task(
        &self,
        route: &str,
        task_id: &str,
        edit: impl FnOnce(&mut Map<String, Value>),
    ) {
        let mut routes = self.state.routes.lock().unwrap();
        let tasks = routes
            .get_mut(route)
            .and_then(|b| b.get_mut("tasks"))
            .and_then(Value::as_array_mut)
            .expect("route with tasks");
        let task = tasks
            .iter_mut()
            .find(|t| t["id"] == task_id)
            .and_then(Value::as_object_mut)
            .expect("task in route");
        edit(task);
    }
}

fn clickup_error(status: StatusCode, err: &str, code: &str) -> Response {
    (status, Json(json!({"err": err, "ECODE": code}))).into_response()
}

fn clickup_handler(state: &ClickUpState, uri: &Uri, headers: &HeaderMap) -> Response {
    let path = uri.path().to_owned();
    let query = uri.query().unwrap_or("").to_owned();
    state
        .requests
        .lock()
        .unwrap()
        .push(format!("{path}?{query}"));

    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(CLICKUP_TOKEN) {
        return clickup_error(StatusCode::UNAUTHORIZED, "Token invalid", "OAUTH_025");
    }
    {
        let mut limits = state.rate_limits.lock().unwrap();
        if let Some(left) = limits.get_mut(&path).filter(|n| **n > 0) {
            *left -= 1;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [
                    ("x-ratelimit-limit", "100".to_owned()),
                    ("x-ratelimit-remaining", "0".to_owned()),
                    ("x-ratelimit-reset", now.to_string()),
                ],
                Json(json!({"err": "Rate limit reached", "ECODE": "APP_002"})),
            )
                .into_response();
        }
    }

    let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();

    if path.ends_with("/time_entries") {
        let start: i64 = params["start_date"].parse().unwrap();
        let end: i64 = params["end_date"].parse().unwrap();
        let data: Vec<Value> = state
            .time_entries
            .iter()
            .filter(|e| {
                let s: i64 = e["start"].as_str().unwrap().parse().unwrap();
                (start..=end).contains(&s)
            })
            .cloned()
            .collect();
        return Json(json!({ "data": data })).into_response();
    }

    let archived = params.get("archived").map(String::as_str) == Some("true");
    let mut parts = Vec::new();
    if archived {
        parts.push("archived=true".to_owned());
    }
    if let Some(page) = params.get("page") {
        parts.push(format!("page={page}"));
    }
    if let Some(start_id) = params.get("start_id") {
        parts.push(format!("start_id={start_id}"));
    }
    let key = if parts.is_empty() {
        path.clone()
    } else {
        format!("{path}?{}", parts.join("&"))
    };
    if let Some(body) = state.routes.lock().unwrap().get(&key) {
        return Json(body.clone()).into_response();
    }
    // No comments unless a route says otherwise.
    if path.ends_with("/comment") || path.ends_with("/reply") {
        return Json(json!({"comments": []})).into_response();
    }
    // GET /task/{id} for a task that a list route contains.
    if let Some(id) = path.strip_prefix("/task/").filter(|id| !id.contains('/')) {
        let routes = state.routes.lock().unwrap();
        let task = routes
            .values()
            .filter_map(|body| body.get("tasks")?.as_array())
            .flatten()
            .find(|t| t["id"] == id);
        if let Some(task) = task {
            return Json(task.clone()).into_response();
        }
    }
    if archived {
        let body = if path.ends_with("/task") {
            json!({"tasks": [], "last_page": true})
        } else if path.ends_with("/space") {
            json!({"spaces": []})
        } else if path.ends_with("/folder") {
            json!({"folders": []})
        } else {
            json!({"lists": []})
        };
        return Json(body).into_response();
    }
    clickup_error(StatusCode::NOT_FOUND, "Not found", "ITEM_013")
}

// ----- Zunder -----------------------------------------------------------------

#[derive(Default)]
pub struct ZunderState {
    pub state: Mutex<State>,
    pub log: Mutex<Vec<Op>>,
    /// Number of operations per push request.
    pub pushes: Mutex<Vec<usize>>,
    /// (`client_id`, `X-Zunder-Client`) per push.
    pub clients: Mutex<Vec<(String, String)>>,
}

pub struct ZunderMock {
    pub url: String,
    pub inner: Arc<ZunderState>,
}

impl ZunderMock {
    pub async fn start() -> Self {
        let inner = Arc::new(ZunderState::default());
        let snapshot_state = inner.clone();
        let push_state = inner.clone();
        let app = Router::new()
            .route(
                "/api/snapshot",
                get(move |headers: HeaderMap| {
                    let inner = snapshot_state.clone();
                    async move {
                        if !authorized(&headers) {
                            return unauthorized();
                        }
                        let state = inner.state.lock().unwrap();
                        let cursor = inner.log.lock().unwrap().len();
                        Json(json!({"cursor": cursor, "entities": state.to_snapshot()}))
                            .into_response()
                    }
                }),
            )
            .route(
                "/api/sync/push",
                post(move |headers: HeaderMap, body: Bytes| {
                    let inner = push_state.clone();
                    async move { push(&inner, &headers, &body) }
                }),
            );
        Self {
            url: serve(app).await,
            inner,
        }
    }

    pub fn state(&self) -> State {
        self.inner.state.lock().unwrap().clone()
    }

    pub fn pushes(&self) -> Vec<usize> {
        self.inner.pushes.lock().unwrap().clone()
    }

    pub fn log_len(&self) -> usize {
        self.inner.log.lock().unwrap().len()
    }

    /// Applies operations directly (e.g. edits "made in the web UI").
    pub fn apply(&self, ops: &[Op]) {
        let mut state = self.inner.state.lock().unwrap();
        let mut log = self.inner.log.lock().unwrap();
        for op in ops {
            state.apply(op);
            log.push(op.clone());
        }
    }
}

fn authorized(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {ZUNDER_TOKEN}"))
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": {"code": "unauthorized", "message": "Nicht angemeldet"}})),
    )
        .into_response()
}

fn push(inner: &ZunderState, headers: &HeaderMap, body: &[u8]) -> Response {
    if !authorized(headers) {
        return unauthorized();
    }
    let body: Value = serde_json::from_slice(body).expect("JSON body");
    let ops: Vec<Op> = serde_json::from_value(body["ops"].clone()).expect("valid ops");
    let client_header = headers
        .get("x-zunder-client")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    inner.clients.lock().unwrap().push((
        body["client_id"].as_str().unwrap_or_default().to_owned(),
        client_header,
    ));
    inner.pushes.lock().unwrap().push(ops.len());

    let mut state = inner.state.lock().unwrap();
    let outcome = validate_push(&state, &ops, system_time_ms());
    let mut log = inner.log.lock().unwrap();
    for op in &outcome.new_ops {
        state.apply(op);
        log.push(op.clone());
    }
    let mut response = serde_json::to_value(&outcome).unwrap();
    response["cursor"] = json!(log.len());
    Json(response).into_response()
}

// ----- mapping ------------------------------------------------------------------

/// The mapping a user would end up with after `plan` and editing:
/// "Werkzeugkiste" is a dev project, "eingefroren" means blocked.
pub fn edited_mapping(mut mapping: Mapping) -> Mapping {
    let tools = mapping.projects.get_mut("werkzeugkiste").expect("project");
    tools.kind = Some("dev".into());
    tools.repo = Some("beispiel/werkzeugkiste".into());
    mapping
        .lists
        .get_mut("9002101")
        .expect("backlog list")
        .statuses
        .insert("eingefroren".into(), "blocked".into());
    mapping
}
