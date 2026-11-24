//! `GET /api/goal-hours`: Timing hours per goal and week (ZUN-66), against
//! a local mock of Timing's report endpoint.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use url::Url;
use zunder_core::ids::Id;
use zunder_core::GoalState;
use zunder_server::config::TimingConfig;

use common::{api_token, client_clock, mcp_err, mcp_ok, ops, Auth, TestServer, BOOTSTRAP};

#[derive(Default)]
struct MockState {
    /// Raw query strings of `GET /report`, in order.
    reports: Vec<String>,
    fail: bool,
    delay: Option<Duration>,
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
            .route("/api/v1/report", get(report))
            .route("/api/v1/projects", get(projects))
            .with_state(mock.clone());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (mock, base)
    }

    fn reports(&self) -> Vec<String> {
        self.0.lock().unwrap().reports.clone()
    }

    fn set(&self, f: impl FnOnce(&mut MockState)) {
        f(&mut self.0.lock().unwrap());
    }
}

/// Seconds per project as Timing would sum them: 1 h on 1, 30 min on 2,
/// 2 h on the flame project 9, 45 min without a project.
async fn report(State(mock): State<TimingMock>, RawQuery(query): RawQuery) -> Response {
    let (fail, delay) = {
        let mut s = mock.0.lock().unwrap();
        s.reports.push(query.unwrap_or_default());
        (s.fail, s.delay)
    };
    if let Some(d) = delay {
        tokio::time::sleep(d).await;
    }
    if fail {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"message": "Kaputt"})),
        )
            .into_response();
    }
    Json(json!({
        "data": [
            {"duration": 3600.0, "project": {"self": "/projects/1"}},
            {"duration": 1800.4, "project": {"self": "/projects/2"}},
            {"duration": 7200.0, "project": {"self": "/projects/9"}},
            {"duration": 2700.0, "project": null},
        ]
    }))
    .into_response()
}

/// Projects 1 and 9; the archived project 4 only without `hide_archived`.
async fn projects(RawQuery(query): RawQuery) -> Response {
    let mut data = vec![
        json!({"self": "/projects/1", "title": "DRK"}),
        json!({"self": "/projects/9", "title": "🔥 Flammen"}),
    ];
    if !query.unwrap_or_default().contains("hide_archived=true") {
        data.push(json!({"self": "/projects/4", "title": "Alt", "is_archived": true}));
    }
    Json(json!({"data": data})).into_response()
}

async fn with_timing(project: Option<&str>) -> (TestServer, TimingMock) {
    let (mock, base) = TimingMock::start().await;
    let project = project.map(str::to_owned);
    let s = TestServer::with_config(move |c| {
        c.timing = Some(TimingConfig {
            token: "timing-test-token".into(),
            project,
        });
        c.timing_base_url = base;
    })
    .await;
    (s, mock)
}

/// Creates a goal with the given fields over sync; returns its ID.
async fn goal(s: &TestServer, name: &str, state: GoalState, projects: Value) -> Id {
    let id = Id::new();
    let mut clock = client_clock();
    let batch = ops(&mut clock, |b| {
        b.set_goal(id, "name", name);
        b.set_goal(id, "state", state.as_str());
        b.set_goal(id, "timing_projects", projects);
    });
    let r = s.push(Auth::Bearer(BOOTSTRAP), &batch).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    id
}

async fn hours(s: &TestServer, query: &str) -> Value {
    let r = s
        .get(&format!("/api/goal-hours{query}"), Auth::Bearer(BOOTSTRAP))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    r.json()
}

#[tokio::test]
async fn without_a_token_nothing_is_asked() {
    let s = TestServer::new().await;
    goal(&s, "I&K", GoalState::Active, json!(["/projects/1"])).await;
    let body = hours(&s, "?week=2026-W48").await;
    assert_eq!(
        body,
        json!({"week": "2026-W48", "status": "not_configured", "goals": {}})
    );
}

#[tokio::test]
async fn minutes_per_active_goal_from_one_cached_report() {
    let (s, mock) = with_timing(Some("/projects/9")).await;
    let ik = goal(&s, "I&K", GoalState::Active, json!(["/projects/1"])).await;
    let both = goal(
        &s,
        "Beides",
        GoalState::Active,
        json!(["/projects/2", "/projects/9"]),
    )
    .await;
    let closed = goal(&s, "Alt", GoalState::Closed, json!(["/projects/3"])).await;
    let none = goal(&s, "Sport", GoalState::Active, json!([])).await;

    let body = hours(&s, "?week=2026-W48").await;
    assert_eq!(body["status"], "ok");
    assert_eq!(body["week"], "2026-W48");
    let goals = body["goals"].as_object().unwrap();
    assert_eq!(goals[&ik.to_string()], 60);
    // The flame project 9 never counts.
    assert_eq!(goals[&both.to_string()], 30);
    assert!(!goals.contains_key(&closed.to_string()));
    assert!(!goals.contains_key(&none.to_string()));

    let reports = mock.reports();
    assert_eq!(reports.len(), 1);
    let query: Vec<(String, String)> = url::form_urlencoded::parse(reports[0].as_bytes())
        .into_owned()
        .collect();
    let values = |k: &str| -> Vec<String> {
        query
            .iter()
            .filter(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
            .collect()
    };
    // Closed goals are not asked; each project once, sorted.
    assert_eq!(
        values("projects[]"),
        ["/projects/1", "/projects/2", "/projects/9"]
    );
    assert_eq!(values("include_child_projects"), ["0"]);
    assert_eq!(values("columns[]"), ["project"]);
    assert!(values("include_app_usage").is_empty());
    let from = values("start_date_min");
    assert!(from[0].starts_with("2026-11-23T00:00:00"), "{from:?}");
    let to = values("start_date_max");
    assert!(to[0].starts_with("2026-11-29T23:59:59"), "{to:?}");

    // Asked again: from the cache.
    hours(&s, "?week=2026-W48").await;
    assert_eq!(mock.reports().len(), 1);
}

#[tokio::test]
async fn a_flame_project_given_by_title_is_resolved_and_left_out() {
    let (s, _mock) = with_timing(Some("🔥 Flammen")).await;
    let g = goal(
        &s,
        "Beides",
        GoalState::Active,
        json!(["/projects/1", "/projects/9"]),
    )
    .await;
    let body = hours(&s, "?week=2026-W48").await;
    assert_eq!(body["goals"][g.to_string()], 60);
}

#[tokio::test]
async fn errors_are_unavailable_and_not_cached() {
    let (s, mock) = with_timing(None).await;
    let g = goal(&s, "I&K", GoalState::Active, json!(["/projects/1"])).await;
    mock.set(|m| m.fail = true);
    let body = hours(&s, "?week=2026-W47").await;
    assert_eq!(body["status"], "unavailable");
    assert_eq!(body["goals"], json!({}));
    assert!(
        body["message"].as_str().unwrap().contains("Kaputt"),
        "{body}"
    );

    mock.set(|m| m.fail = false);
    let body = hours(&s, "?week=2026-W47").await;
    assert_eq!(body["status"], "ok");
    assert_eq!(body["goals"][g.to_string()], 60);
    assert_eq!(mock.reports().len(), 2);
}

#[tokio::test]
async fn a_slow_timing_is_unavailable_after_the_timeout() {
    let (s, mock) = with_timing(None).await;
    goal(&s, "I&K", GoalState::Active, json!(["/projects/1"])).await;
    mock.set(|m| m.delay = Some(Duration::from_secs(8)));
    let started = std::time::Instant::now();
    let body = hours(&s, "?week=2026-W46").await;
    assert_eq!(body["status"], "unavailable");
    assert!(started.elapsed() < Duration::from_secs(7));
}

#[tokio::test]
async fn without_projects_timing_is_not_asked_and_a_bad_week_is_refused() {
    let (s, mock) = with_timing(None).await;
    goal(&s, "Sport", GoalState::Active, json!([])).await;
    let body = hours(&s, "").await;
    assert_eq!(body["status"], "ok");
    assert_eq!(body["goals"], json!({}));
    assert!(mock.reports().is_empty());

    let r = s
        .get("/api/goal-hours?week=quatsch", Auth::Bearer(BOOTSTRAP))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.error_code(), "invalid_request");
    let r = s.get("/api/goal-hours", Auth::None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

// ---- MCP (create_goal, update_goal, goal_input) ---------------------------

#[tokio::test]
async fn mcp_normalizes_and_checks_timing_projects() {
    let (s, _mock) = with_timing(Some("/projects/9")).await;
    let t = api_token(&s, "claude").await;

    let r = mcp_ok(
        &s,
        &t,
        "create_goal",
        json!({"name": "I&K", "timing_projects": ["https://web.timingapp.com/api/v1/projects/1/", "1"]}),
    )
    .await;
    assert_eq!(r["goal"]["timing_projects"], json!(["/projects/1"]));
    assert_eq!(r["warnings"], json!([]));
    let ik = r["goal"]["id"].as_str().unwrap().to_owned();

    // The flame project, an unknown project and garbage are refused.
    let (code, message) = mcp_err(
        &s,
        &t,
        "create_goal",
        json!({"name": "Flammen", "timing_projects": ["/projects/9"]}),
    )
    .await;
    assert_eq!(code, "invalid_arguments");
    assert!(message.contains("Flammen-Projekt"), "{message}");
    let (_, message) = mcp_err(
        &s,
        &t,
        "create_goal",
        json!({"name": "X", "timing_projects": ["/projects/5"]}),
    )
    .await;
    assert!(message.contains("/projects/5"), "{message}");
    let (_, message) = mcp_err(
        &s,
        &t,
        "create_goal",
        json!({"name": "X", "timing_projects": ["DRK"]}),
    )
    .await;
    assert!(message.contains("kein Timing-Projekt"), "{message}");

    // Taken by another active goal: refused; once that goal is closed: fine.
    let other = mcp_ok(&s, &t, "create_goal", json!({"name": "Sichtbar"})).await;
    let other = other["goal"]["id"].as_str().unwrap().to_owned();
    let (_, message) = mcp_err(
        &s,
        &t,
        "update_goal",
        json!({"goal_id": other, "timing_projects": ["/projects/1"]}),
    )
    .await;
    assert!(message.contains("„I&K“"), "{message}");
    mcp_ok(
        &s,
        &t,
        "update_goal",
        json!({"goal_id": ik, "state": "closed"}),
    )
    .await;
    let r = mcp_ok(
        &s,
        &t,
        "update_goal",
        json!({"goal_id": other, "timing_projects": ["/projects/1"]}),
    )
    .await;
    assert_eq!(r["goal"]["timing_projects"], json!(["/projects/1"]));

    // Reopening I&K would count project 1 twice: refused.
    let (_, message) = mcp_err(
        &s,
        &t,
        "update_goal",
        json!({"goal_id": ik, "state": "active"}),
    )
    .await;
    assert!(message.contains("„Sichtbar“"), "{message}");

    // An archived project still counts for past weeks: accepted.
    let r = mcp_ok(
        &s,
        &t,
        "create_goal",
        json!({"name": "Früher", "timing_projects": ["/projects/4"]}),
    )
    .await;
    assert_eq!(r["goal"]["timing_projects"], json!(["/projects/4"]));

    // null removes the list; list_goals shows the field.
    mcp_ok(
        &s,
        &t,
        "update_goal",
        json!({"goal_id": other, "timing_projects": null}),
    )
    .await;
    let goals = mcp_ok(&s, &t, "list_goals", json!({})).await;
    let listed = goals["goals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == other)
        .unwrap()
        .clone();
    assert_eq!(listed["timing_projects"], json!([]));
}

#[tokio::test]
async fn goal_input_reports_timing_minutes() {
    let (s, _mock) = with_timing(None).await;
    let t = api_token(&s, "claude").await;
    let ik = mcp_ok(
        &s,
        &t,
        "create_goal",
        json!({"name": "I&K", "timing_projects": ["/projects/1"]}),
    )
    .await;
    let ik = ik["goal"]["id"].as_str().unwrap().to_owned();
    let sport = mcp_ok(&s, &t, "create_goal", json!({"name": "Sport"})).await;
    let sport = sport["goal"]["id"].as_str().unwrap().to_owned();

    let r = mcp_ok(&s, &t, "goal_input", json!({})).await;
    assert_eq!(r["timing_status"], "ok");
    let row = |id: &str| {
        r["goals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["goal_id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(row(&ik)["timing_minutes"], 60);
    assert_eq!(row(&sport)["timing_minutes"], Value::Null);
}

#[tokio::test]
async fn goal_input_without_timing_is_not_configured() {
    let s = TestServer::new().await;
    let t = api_token(&s, "claude").await;
    let r = mcp_ok(
        &s,
        &t,
        "create_goal",
        json!({"name": "I&K", "timing_projects": ["/projects/1"]}),
    )
    .await;
    assert_eq!(
        r["warnings"],
        json!(["Timing ist nicht eingerichtet; die Projekte sind ungeprüft."])
    );
    let r = mcp_ok(&s, &t, "goal_input", json!({})).await;
    assert_eq!(r["timing_status"], "not_configured");
    assert_eq!(r["goals"][0]["timing_minutes"], Value::Null);
    assert!(r.get("timing_message").is_none());
}
