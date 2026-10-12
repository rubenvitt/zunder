//! Flames and the Timing bridge: HTTP endpoints and MCP tools, against a
//! local mock of the Timing web API and without Timing; Timing settings
//! from the web UI (`/api/settings/timing`).

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::{json, Value};
use url::Url;
use zunder_server::config::TimingConfig;

use common::{api_token, mcp_err, mcp_ok, Auth, TestServer, BOOTSTRAP};

const TIMING_TOKEN: &str = "timing-test-token";

#[derive(Debug, Clone, PartialEq)]
struct Recorded {
    method: String,
    path: String,
    auth: Option<String>,
    body: Value,
}

#[derive(Default)]
struct MockState {
    requests: Vec<Recorded>,
    running: Option<u32>,
    next: u32,
    fail_start: bool,
    /// `GET /time-entries/running` finds nothing although a timer runs
    /// (as when its redirect is not followed with the token).
    blind_running: bool,
}

#[derive(Clone, Default)]
struct TimingMock(Arc<Mutex<MockState>>);

impl TimingMock {
    async fn start() -> (TimingMock, Url) {
        let mock = TimingMock::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base =
            Url::parse(&format!("http://{}/api/v1", listener.local_addr().unwrap())).unwrap();
        let router = Router::new()
            .route("/api/v1/time-entries/start", post(start_timer))
            .route("/api/v1/time-entries/stop", put(stop_timer))
            .route("/api/v1/time-entries/running", get(running_timer))
            .route("/api/v1/time-entries/{id}", get(show_entry))
            .route("/api/v1/projects", get(list_projects))
            .with_state(mock.clone());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (mock, base)
    }

    fn record(&self, method: &str, path: &str, headers: &HeaderMap, body: Value) {
        self.0.lock().unwrap().requests.push(Recorded {
            method: method.into(),
            path: path.into(),
            auth: headers
                .get(header::AUTHORIZATION)
                .map(|v| v.to_str().unwrap().to_owned()),
            body,
        });
    }

    fn requests(&self) -> Vec<Recorded> {
        std::mem::take(&mut self.0.lock().unwrap().requests)
    }

    fn calls(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|r| format!("{} {}", r.method, r.path))
            .collect()
    }
}

fn entry(id: u32, running: bool, title: &str) -> Value {
    json!({
        "data": {
            "self": format!("/time-entries/{id}"),
            "start_date": "2026-10-11T10:00:00+00:00",
            "end_date": if running { Value::Null } else { json!("2026-10-11T11:00:00+00:00") },
            "duration": if running { 0 } else { 3600 },
            "project": {"self": "/projects/1"},
            "title": title,
            "notes": null,
            "is_running": running,
        }
    })
}

async fn start_timer(
    State(mock): State<TimingMock>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    mock.record("POST", "/time-entries/start", &headers, body.clone());
    let mut s = mock.0.lock().unwrap();
    if s.fail_start {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"message": "Wartungsarbeiten"})),
        )
            .into_response();
    }
    s.next += 1;
    s.running = Some(s.next);
    let title = body["title"].as_str().unwrap_or_default().to_owned();
    let mut response = entry(s.next, true, &title);
    response["message"] = json!(format!("Timer '{title}' started."));
    (StatusCode::CREATED, Json(response)).into_response()
}

async fn stop_timer(State(mock): State<TimingMock>, headers: HeaderMap) -> Response {
    mock.record("PUT", "/time-entries/stop", &headers, Value::Null);
    // Like the real Timing (behind its web server): a PUT without
    // Content-Length is refused.
    if !headers.contains_key(header::CONTENT_LENGTH) {
        return (StatusCode::LENGTH_REQUIRED, "Length Required").into_response();
    }
    let mut s = mock.0.lock().unwrap();
    match s.running.take() {
        Some(id) => Json(entry(id, false, "")).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"message": "No running timer found."})),
        )
            .into_response(),
    }
}

async fn list_projects(
    State(mock): State<TimingMock>,
    headers: HeaderMap,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Response {
    let path = match query {
        Some(q) => format!("/projects?{q}"),
        None => "/projects".to_owned(),
    };
    mock.record("GET", &path, &headers, Value::Null);
    Json(json!({
        "data": [
            {
                "self": "/projects/1", "team_id": null, "title": "Arbeit",
                "title_chain": ["Arbeit"], "color": "#FF0000", "productivity_score": 1,
                "is_archived": false, "notes": null, "children": [{"self": "/projects/7"}],
                "parent": null,
            },
            {
                "self": "/projects/7", "title": "Zunder",
                "title_chain": ["Arbeit", "Zunder"], "color": "#00FF00",
                "is_archived": false, "parent": {"self": "/projects/1"},
            },
            {"self": "/projects/9", "title": "Ohne Kette"},
        ]
    }))
    .into_response()
}

async fn running_timer(State(mock): State<TimingMock>, headers: HeaderMap) -> Response {
    mock.record("GET", "/time-entries/running", &headers, Value::Null);
    let s = mock.0.lock().unwrap();
    match s.running.filter(|_| !s.blind_running) {
        // Like the real API: a redirect to the entry.
        Some(id) => (
            StatusCode::FOUND,
            [(header::LOCATION, format!("/api/v1/time-entries/{id}"))],
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"message": "No running timer found."})),
        )
            .into_response(),
    }
}

async fn show_entry(
    State(mock): State<TimingMock>,
    headers: HeaderMap,
    Path(id): Path<u32>,
) -> Response {
    mock.record("GET", &format!("/time-entries/{id}"), &headers, Value::Null);
    let running = mock.0.lock().unwrap().running == Some(id);
    Json(entry(id, running, "")).into_response()
}

async fn with_timing(project: Option<&str>) -> (TestServer, TimingMock) {
    let (mock, base) = TimingMock::start().await;
    let project = project.map(str::to_owned);
    let s = TestServer::with_config(move |c| {
        c.timing = Some(TimingConfig {
            token: TIMING_TOKEN.into(),
            project,
        });
        c.timing_base_url = base;
    })
    .await;
    (s, mock)
}

async fn task(s: &TestServer, t: &str, title: &str) -> String {
    let r = mcp_ok(s, t, "create_task", json!({"title": title})).await;
    r["task"]["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn flame_with_timing_via_mcp() {
    let (s, mock) = with_timing(Some("/projects/1")).await;
    let t = api_token(&s, "claude").await;
    let a = task(&s, &t, "Sync bauen").await;
    let b = task(&s, &t, "Doku schreiben").await;

    let r = mcp_ok(&s, &t, "start_flame", json!({"task_id": a})).await;
    assert_eq!(r["running"], true);
    assert_eq!(
        r["timing"],
        json!({"configured": true, "used": true, "entry": "/time-entries/1", "error": null})
    );
    assert_eq!(r["task"]["timing_ref"], "/time-entries/1");
    assert_eq!(r["task"]["flame_running"], true);
    assert!(r["task"]["flame_started_at"].is_string());
    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/time-entries/start");
    assert_eq!(
        requests[0].auth.as_deref(),
        Some("Bearer timing-test-token")
    );
    assert_eq!(
        requests[0].body,
        json!({"title": "Sync bauen", "project": "/projects/1", "replace_existing": false})
    );

    // Starting again is a no-op.
    let r = mcp_ok(&s, &t, "start_flame", json!({"task_id": a})).await;
    assert_eq!(r["unchanged"], true);
    assert!(mock.calls().is_empty());

    // A second flame ends the first (Timing stops its timer by itself).
    let r = mcp_ok(&s, &t, "start_flame", json!({"task_id": b})).await;
    assert_eq!(r["stopped_task_ids"], json!([a]));
    assert_eq!(r["task"]["timing_ref"], "/time-entries/2");
    assert_eq!(mock.calls(), ["POST /time-entries/start"]);
    let first = mcp_ok(&s, &t, "get_task", json!({"task_id": a})).await;
    assert_eq!(first["task"]["flame_running"], false);
    assert!(first["task"]["flame_ended_at"].is_string());
    assert_eq!(first["task"]["timing_ref"], "/time-entries/1");

    // Stop: the running timer is ours → stopped.
    let r = mcp_ok(&s, &t, "stop_flame", json!({"task_id": b})).await;
    assert_eq!(r["running"], false);
    assert_eq!(r["timing"]["used"], true);
    assert_eq!(r["task"]["flame_running"], false);
    assert_eq!(
        mock.calls(),
        ["GET /time-entries/2", "PUT /time-entries/stop"]
    );

    let (code, message) = mcp_err(&s, &t, "stop_flame", json!({"task_id": b})).await;
    assert_eq!(code, "conflict");
    assert!(message.contains("keine Flamme"));

    // Flame operations are written by the Timing bridge.
    let history = mcp_ok(&s, &t, "get_task_history", json!({"task_id": b})).await;
    let flame_sources: Vec<&str> = history["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["field"].as_str().unwrap().starts_with("flame_"))
        .map(|e| e["source"].as_str().unwrap())
        .collect();
    assert!(!flame_sources.is_empty());
    assert!(flame_sources.iter().all(|s| *s == "server"));
}

#[tokio::test]
async fn foreign_timer_is_not_stopped() {
    let (s, mock) = with_timing(None).await;
    let t = api_token(&s, "claude").await;
    let a = task(&s, &t, "Lesen").await;
    let r = mcp_ok(&s, &t, "start_flame", json!({"task_id": a})).await;
    assert_eq!(
        mock.requests()[0].body,
        json!({"title": "Lesen", "replace_existing": false})
    );
    assert_eq!(r["task"]["timing_ref"], "/time-entries/1");

    // Somebody started another timer in Timing meanwhile.
    {
        let mut st = mock.0.lock().unwrap();
        st.next += 1;
        st.running = Some(st.next);
    }
    let r = mcp_ok(&s, &t, "stop_flame", json!({"task_id": a})).await;
    assert_eq!(r["timing"]["used"], true);
    assert_eq!(r["running"], false);
    let calls = mock.calls();
    assert!(
        !calls.contains(&"PUT /time-entries/stop".to_owned()),
        "{calls:?}"
    );
    assert_eq!(mock.0.lock().unwrap().running, Some(2));
}

#[tokio::test]
async fn stop_and_restart_do_not_rely_on_the_running_redirect() {
    let (s, mock) = with_timing(None).await;
    mock.0.lock().unwrap().blind_running = true;
    let t = api_token(&s, "claude").await;
    let a = task(&s, &t, "Folien").await;

    mcp_ok(&s, &t, "start_flame", json!({"task_id": a})).await;
    let r = mcp_ok(&s, &t, "stop_flame", json!({"task_id": a})).await;
    assert_eq!(r["timing"]["used"], true);
    assert_eq!(mock.0.lock().unwrap().running, None, "Timing timer stopped");

    // Started again: a new timer runs, and stopping ends it as well.
    let r = mcp_ok(&s, &t, "start_flame", json!({"task_id": a})).await;
    assert_eq!(r["running"], true);
    assert_eq!(r["task"]["timing_ref"], "/time-entries/2");
    assert_eq!(mock.0.lock().unwrap().running, Some(2));
    mcp_ok(&s, &t, "stop_flame", json!({"task_id": a})).await;
    assert_eq!(mock.0.lock().unwrap().running, None);
}

#[tokio::test]
async fn timing_failure_falls_back_to_timestamps() {
    let (s, mock) = with_timing(None).await;
    mock.0.lock().unwrap().fail_start = true;
    let t = api_token(&s, "claude").await;
    let a = task(&s, &t, "Schreiben").await;
    let r = mcp_ok(&s, &t, "start_flame", json!({"task_id": a})).await;
    assert_eq!(r["running"], true);
    assert_eq!(r["timing"]["configured"], true);
    assert_eq!(r["timing"]["used"], false);
    let error = r["timing"]["error"].as_str().unwrap();
    assert!(
        error.contains("500") && error.contains("Wartungsarbeiten"),
        "{error}"
    );
    assert_eq!(r["task"]["timing_ref"], Value::Null);
    assert!(r["task"]["flame_started_at"].is_string());

    let r = mcp_ok(&s, &t, "stop_flame", json!({"task_id": a})).await;
    assert_eq!(r["running"], false);
    assert_eq!(r["timing"]["used"], false);
    assert!(r["timing"]["error"]
        .as_str()
        .unwrap()
        .contains("ohne Timing"));
    // Only the failed start reached Timing.
    assert_eq!(mock.calls(), ["POST /time-entries/start"]);
}

#[tokio::test]
async fn unreachable_timing_falls_back() {
    // Nothing listens on this port.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = Url::parse(&format!("http://{}/api/v1", listener.local_addr().unwrap())).unwrap();
    drop(listener);
    let s = TestServer::with_config(move |c| {
        c.timing = Some(TimingConfig {
            token: TIMING_TOKEN.into(),
            project: None,
        });
        c.timing_base_url = base;
    })
    .await;
    let t = api_token(&s, "claude").await;
    let a = task(&s, &t, "Offline").await;
    let r = mcp_ok(&s, &t, "start_flame", json!({"task_id": a})).await;
    assert_eq!(r["running"], true);
    assert!(r["timing"]["error"]
        .as_str()
        .unwrap()
        .starts_with("Timing ist nicht erreichbar"));
}

#[tokio::test]
async fn flame_without_timing_via_http() {
    let s = TestServer::new().await;
    let t = api_token(&s, "claude").await;
    let a = task(&s, &t, "Lernen").await;
    let b = task(&s, &t, "Üben").await;
    let session = s.session().await;

    // CSRF rule as for every cookie request.
    let r = s
        .post(
            &format!("/api/tasks/{a}/flame/start"),
            Auth::Cookie(&session),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let r = s
        .request(
            axum::http::Method::POST,
            &format!("/api/tasks/{a}/flame/start"),
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let body = r.json();
    assert_eq!(body["running"], true);
    assert_eq!(
        body["timing"],
        json!({"configured": false, "used": false, "entry": null, "error": null})
    );
    assert_eq!(body["task"]["timing_ref"], Value::Null);
    assert!(body["task"]["flame_started_at"].is_string());

    // Bearer tokens work too; starting b ends a.
    let r = s
        .request(
            axum::http::Method::POST,
            &format!("/api/tasks/{b}/flame/start"),
            Auth::Bearer(BOOTSTRAP),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["stopped_task_ids"], json!([a]));

    let r = s
        .request(
            axum::http::Method::POST,
            &format!("/api/tasks/{b}/flame/stop"),
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["running"], false);
    assert!(r.json()["task"]["flame_ended_at"].is_string());

    let r = s
        .request(
            axum::http::Method::POST,
            &format!("/api/tasks/{b}/flame/stop"),
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.error_code(), "conflict");

    // A stopped flame can be started again.
    let r = s
        .request(
            axum::http::Method::POST,
            &format!("/api/tasks/{b}/flame/start"),
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["running"], true, "{}", r.text());
    assert_eq!(r.json()["unchanged"], false);
    assert_eq!(r.json()["task"]["flame_ended_at"], Value::Null);

    let r = s
        .request(
            axum::http::Method::POST,
            "/api/tasks/01929c3e-0000-7000-8000-000000000001/flame/start",
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = s
        .request(
            axum::http::Method::POST,
            "/api/tasks/nope/flame/start",
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = s
        .request(
            axum::http::Method::POST,
            &format!("/api/tasks/{a}/flame/start"),
            Auth::None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

async fn put_settings(s: &TestServer, session: &str, body: Value) -> common::TestResponse {
    s.request(
        Method::PUT,
        "/api/settings/timing",
        Auth::Web(session),
        Some(body),
    )
    .await
}

#[tokio::test]
async fn timing_settings_from_the_web_ui() {
    let (mock, base) = TimingMock::start().await;
    let s = TestServer::with_config(move |c| c.timing_base_url = base).await;
    let t = api_token(&s, "claude").await;
    let session = s.session().await;

    let r = s.get("/api/settings/timing", Auth::Web(&session)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({
            "configured": false, "token_source": null, "token_hint": null,
            "project": null, "project_source": null, "overridden": false,
            "env_token": false, "env_project": null,
        })
    );
    let r = s
        .request(
            Method::POST,
            "/api/settings/timing/check",
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = s
        .get("/api/settings/timing/projects", Auth::Web(&session))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    // Browser sessions only, with the CSRF header.
    let r = s.get("/api/settings/timing", Auth::Bearer(&t)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = s
        .request(
            Method::PUT,
            "/api/settings/timing",
            Auth::Cookie(&session),
            Some(json!({"token": "x"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = s
        .request(
            Method::PUT,
            "/api/settings/timing",
            Auth::Bearer(&t),
            Some(json!({"token": "x"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Invalid values.
    let r = put_settings(&s, &session, json!({"token": "a b"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = put_settings(&s, &session, json!({"secret": "x"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let r = put_settings(
        &s,
        &session,
        json!({"token": " ui-token-0123456789 ", "project": "/projects/7"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let body = r.json();
    assert_eq!(body["configured"], true);
    assert_eq!(body["token_source"], "settings");
    assert_eq!(body["token_hint"], "…6789");
    assert_eq!(body["project"], "/projects/7");
    assert_eq!(body["project_source"], "settings");
    assert_eq!(body["overridden"], true);
    assert!(!r.text().contains("ui-token-0123456789"));

    // The next flame uses the new settings, without a restart.
    let a = task(&s, &t, "Planen").await;
    let r = mcp_ok(&s, &t, "start_flame", json!({"task_id": a})).await;
    assert_eq!(r["timing"]["used"], true);
    let requests = mock.requests();
    assert_eq!(
        requests[0].auth.as_deref(),
        Some("Bearer ui-token-0123456789")
    );
    assert_eq!(
        requests[0].body,
        json!({"title": "Planen", "project": "/projects/7", "replace_existing": false})
    );

    let r = s
        .request(
            Method::POST,
            "/api/settings/timing/check",
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json(), json!({"ok": true, "error": null}));
    assert_eq!(mock.calls()[0], "GET /time-entries/running");

    // The project picker lists Timing's projects (without archived ones).
    let r = s
        .get("/api/settings/timing/projects", Auth::Web(&session))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(
        r.json(),
        json!([
            {"self": "/projects/1", "title": "Arbeit", "title_chain": ["Arbeit"], "color": "#FF0000"},
            {"self": "/projects/7", "title": "Zunder", "title_chain": ["Arbeit", "Zunder"], "color": "#00FF00"},
            {"self": "/projects/9", "title": "Ohne Kette", "title_chain": ["Ohne Kette"], "color": null},
        ])
    );
    let requests = mock.requests();
    assert_eq!(requests[0].path, "/projects?hide_archived=true");
    assert_eq!(
        requests[0].auth.as_deref(),
        Some("Bearer ui-token-0123456789")
    );
    let r = s
        .get("/api/settings/timing/projects", Auth::Bearer(&t))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // An empty token keeps the stored one; an empty project removes it.
    let r = put_settings(&s, &session, json!({"token": "", "project": ""})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["token_hint"], "…6789");
    assert_eq!(r.json()["project"], Value::Null);

    // Settings survive a restart.
    let s = s.restart().await;
    let session = s.session().await;
    let r = s.get("/api/settings/timing", Auth::Web(&session)).await;
    assert_eq!(r.json()["configured"], true);
    assert_eq!(r.json()["token_source"], "settings");
    assert_eq!(r.json()["project_source"], "settings");

    // Reset: without environment, Timing is off again.
    let r = s
        .request(
            Method::DELETE,
            "/api/settings/timing",
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["configured"], false);
    assert_eq!(r.json()["overridden"], false);
    let t = api_token(&s, "claude-2").await;
    let b = task(&s, &t, "Ohne Timing").await;
    let r = mcp_ok(&s, &t, "start_flame", json!({"task_id": b})).await;
    assert_eq!(r["timing"]["configured"], false);
}

#[tokio::test]
async fn timing_settings_override_the_environment() {
    let (s, mock) = with_timing(Some("/projects/1")).await;
    let t = api_token(&s, "claude").await;
    let session = s.session().await;

    let r = s.get("/api/settings/timing", Auth::Web(&session)).await;
    let body = r.json();
    assert_eq!(body["configured"], true);
    assert_eq!(body["token_source"], "env");
    assert_eq!(body["project"], "/projects/1");
    assert_eq!(body["project_source"], "env");
    assert_eq!(body["env_token"], true);
    assert_eq!(body["env_project"], "/projects/1");

    // Only the project: the token stays the environment's.
    let r = put_settings(&s, &session, json!({"project": "Deep Work"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["token_source"], "env");
    assert_eq!(r.json()["project_source"], "settings");
    let a = task(&s, &t, "Schreiben").await;
    mcp_ok(&s, &t, "start_flame", json!({"task_id": a})).await;
    let requests = mock.requests();
    assert_eq!(
        requests[0].auth.as_deref(),
        Some("Bearer timing-test-token")
    );
    assert_eq!(requests[0].body["project"], "Deep Work");

    // Reset: the environment applies again.
    let r = s
        .request(
            Method::DELETE,
            "/api/settings/timing",
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.json()["project"], "/projects/1");
    assert_eq!(r.json()["project_source"], "env");
    let b = task(&s, &t, "Lesen").await;
    mcp_ok(&s, &t, "start_flame", json!({"task_id": b})).await;
    assert_eq!(mock.requests()[0].body["project"], "/projects/1");
}
