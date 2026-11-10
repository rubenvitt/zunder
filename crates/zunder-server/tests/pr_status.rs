//! `POST /api/pr-status`: PR status from a local mock of GitHub's GraphQL
//! API, the cache, and the answer without a token.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use url::Url;

use common::{Auth, TestServer, BOOTSTRAP};

const GITHUB_TOKEN: &str = "github-test-token";

#[derive(Default)]
struct MockState {
    requests: Vec<(Option<String>, Value)>,
    fail: bool,
}

#[derive(Clone, Default)]
struct GithubMock(Arc<Mutex<MockState>>);

impl GithubMock {
    async fn start() -> (GithubMock, Url) {
        let mock = GithubMock::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!(
            "http://{}/graphql",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let router = Router::new()
            .route("/graphql", post(graphql))
            .with_state(mock.clone());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (mock, url)
    }

    fn requests(&self) -> Vec<(Option<String>, Value)> {
        std::mem::take(&mut self.0.lock().unwrap().requests)
    }
}

/// Knows PR 1 (open, approved, CI red) and PR 2 (merged) of `o/r`.
async fn graphql(
    State(mock): State<GithubMock>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let auth = headers
        .get(header::AUTHORIZATION)
        .map(|v| v.to_str().unwrap().to_owned());
    let mut state = mock.0.lock().unwrap();
    state.requests.push((auth, body.clone()));
    if state.fail {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message": "Bad credentials"})),
        )
            .into_response();
    }
    let vars = body["variables"].as_object().unwrap();
    let mut data = serde_json::Map::new();
    let mut i = 0;
    while let Some(number) = vars.get(&format!("n{i}")) {
        let known = vars[&format!("o{i}")] == "o" && vars[&format!("r{i}")] == "r";
        let pr = match (known, number.as_u64()) {
            (true, Some(1)) => json!({
                "number": 1, "title": "Erster PR", "state": "OPEN", "isDraft": false,
                "reviewDecision": "APPROVED", "reviewRequests": {"totalCount": 0},
                "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "FAILURE"}}}]}
            }),
            (true, Some(2)) => json!({
                "number": 2, "title": "Gemergt", "state": "MERGED", "isDraft": false,
                "reviewDecision": null, "reviewRequests": {"totalCount": 0},
                "commits": {"nodes": [{"commit": {"statusCheckRollup": null}}]}
            }),
            _ => Value::Null,
        };
        let repo = if known {
            json!({"pullRequest": pr})
        } else {
            Value::Null
        };
        data.insert(format!("p{i}"), repo);
        i += 1;
    }
    Json(json!({"data": data})).into_response()
}

async fn server_with_github() -> (TestServer, GithubMock) {
    let (mock, url) = GithubMock::start().await;
    let server = TestServer::with_config(|c| {
        c.github_token = Some(GITHUB_TOKEN.into());
        c.github_graphql_url = url;
    })
    .await;
    (server, mock)
}

#[tokio::test]
async fn reports_status_and_caches() {
    let (server, mock) = server_with_github().await;
    let urls = json!({"urls": [
        "https://github.com/o/r/pull/1",
        "https://github.com/o/r/pull/2/files",
        "https://github.com/x/y/pull/3",
        "https://example.org/kein-pr",
    ]});
    let res = server
        .post("/api/pr-status", Auth::Bearer(BOOTSTRAP), urls.clone())
        .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text());
    assert_eq!(
        res.json(),
        json!({
            "configured": true,
            "statuses": {
                "https://github.com/o/r/pull/1": {
                    "number": 1, "title": "Erster PR", "state": "open",
                    "review": "approved", "checks": "failure"
                },
                "https://github.com/o/r/pull/2/files": {
                    "number": 2, "title": "Gemergt", "state": "merged",
                    "review": null, "checks": null
                },
                "https://github.com/x/y/pull/3": null
            }
        })
    );
    let requests = mock.requests();
    assert_eq!(requests.len(), 1, "one batched request");
    assert_eq!(
        requests[0].0.as_deref(),
        Some(format!("Bearer {GITHUB_TOKEN}").as_str())
    );

    // Second ask comes from the cache.
    let again = server
        .post("/api/pr-status", Auth::Bearer(BOOTSTRAP), urls)
        .await;
    assert_eq!(again.json(), res.json());
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn github_failure_is_unavailable() {
    let (server, mock) = server_with_github().await;
    mock.0.lock().unwrap().fail = true;
    let res = server
        .post(
            "/api/pr-status",
            Auth::Bearer(BOOTSTRAP),
            json!({"urls": ["https://github.com/o/r/pull/1"]}),
        )
        .await;
    assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(res.text().contains("Bad credentials"), "{}", res.text());
}

#[tokio::test]
async fn without_token_nothing_is_asked() {
    let server = TestServer::new().await;
    let res = server
        .post(
            "/api/pr-status",
            Auth::Bearer(BOOTSTRAP),
            json!({"urls": ["https://github.com/o/r/pull/1"]}),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json(), json!({"configured": false, "statuses": {}}));
}

#[tokio::test]
async fn needs_login_and_limits_size() {
    let server = TestServer::new().await;
    let res = server
        .post("/api/pr-status", Auth::None, json!({"urls": []}))
        .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    let many: Vec<String> = (1..=201)
        .map(|n| format!("https://github.com/o/r/pull/{n}"))
        .collect();
    let res = server
        .post(
            "/api/pr-status",
            Auth::Bearer(BOOTSTRAP),
            json!({"urls": many}),
        )
        .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
}
