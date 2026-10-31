//! Proposals for inbox tasks from a decision model (ZUN-52), against a
//! local mock of `POST /v1/systemone`; Jev settings from the web UI.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use url::Url;

use common::{api_token, mcp_ok, Auth, TestServer};

#[derive(Default)]
struct Mock {
    requests: Vec<(Option<String>, Value)>,
    status: u16,
}

#[derive(Clone, Default)]
struct JevMock(Arc<Mutex<Mock>>);

async fn systemone(
    State(mock): State<JevMock>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let auth = headers
        .get(header::AUTHORIZATION)
        .map(|v| v.to_str().unwrap().to_owned());
    let mut m = mock.0.lock().unwrap();
    m.requests.push((auth, body.clone()));
    if m.status != 0 {
        return (
            StatusCode::from_u16(m.status).unwrap(),
            Json(json!({"error": "nope"})),
        );
    }
    // Pick the first project offered as the place.
    let place = body["questions"]["ort"]["criteria"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, d)| d.as_str().unwrap().starts_with("Projekt"))
        .map(|(k, _)| k.clone())
        .unwrap();
    (
        StatusCode::OK,
        Json(json!({
            "model": "jev-1.14.0",
            "answers": {
                "gewicht": {"type": "choice", "choice": "glut", "probabilities": {"brand": 0.1, "glut": 0.6, "rauch": 0.2, "asche": 0.05, "unklar": 0.05}, "confidence": 0.5},
                "fuer_wen": {"type": "choice", "choice": "mich", "probabilities": {"mich": 0.7, "uns": 0.1, "andere": 0.1, "unklar": 0.1}, "confidence": 0.6},
                "ort": {"type": "choice", "choice": place, "probabilities": {place.clone(): 0.8, "nichts": 0.2}, "confidence": 0.7}
            },
            "usage": {"input_tokens": 321, "output_tokens": 12}
        })),
    )
}

async fn server() -> (TestServer, JevMock) {
    let mock = JevMock::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!(
        "http://{}/v1/systemone",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let router = Router::new()
        .route("/v1/systemone", post(systemone))
        .with_state(mock.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let s = TestServer::with_config(move |c| c.jev_base_url = base).await;
    (s, mock)
}

async fn put_jev(s: &TestServer, session: &str, body: Value) -> common::TestResponse {
    s.request(
        Method::PUT,
        "/api/settings/jev",
        Auth::Web(session),
        Some(body),
    )
    .await
}

#[tokio::test]
async fn suggestion_for_an_inbox_task() {
    let (s, mock) = server().await;
    let t = api_token(&s, "claude").await;
    let session = s.session().await;
    let private = mcp_ok(&s, &t, "create_area", json!({"name": "Meine Liebste"})).await;
    let private_id = private["area"]["id"].as_str().unwrap().to_owned();
    let open = mcp_ok(&s, &t, "create_area", json!({"name": "Finanzen"})).await;
    let open_id = open["area"]["id"].as_str().unwrap().to_owned();
    let project = mcp_ok(
        &s,
        &t,
        "create_project",
        json!({"name": "Steuer 2026", "area_id": open_id}),
    )
    .await;
    let project_id = project["project"]["id"].as_str().unwrap().to_owned();
    mcp_ok(
        &s,
        &t,
        "create_project",
        json!({"name": "Jahrestag", "area_id": private_id}),
    )
    .await;
    let task = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "Belege sammeln", "description": "für die Steuer"}),
    )
    .await;
    let id = task["task"]["id"].as_str().unwrap().to_owned();
    let path = format!("/api/tasks/{id}/suggestion");

    // Without a key: conflict, nothing sent.
    let r = s.post(&path, Auth::Web(&session), json!({})).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());

    // Settings: aliases refused, key never returned.
    let r = put_jev(
        &s,
        &session,
        json!({"token": "tsk_secret_0123456789", "model": "jev-latest"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = put_jev(
        &s,
        &session,
        json!({"token": "tsk_secret_0123456789", "private_area_ids": [private_id]}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let settings = r.json();
    assert_eq!(settings["configured"], true);
    assert_eq!(settings["model"], "jev-1.14.0");
    assert_eq!(settings["token_hint"], "…6789");
    assert!(!r.text().contains("tsk_secret"));
    // Agents' tokens may not read the settings or send tasks out.
    assert_eq!(
        s.get("/api/settings/jev", Auth::Bearer(&t)).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        s.post(&path, Auth::Bearer(&t), json!({})).await.status,
        StatusCode::FORBIDDEN
    );

    let r = s.post(&path, Auth::Web(&session), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let sug = r.json();
    assert_eq!(sug["weight"], "glut");
    assert_eq!(sug["weight_probability"], 0.6);
    assert_eq!(sug["for_whom"], "mich");
    assert_eq!(sug["place"]["id"], project_id);
    assert_eq!(sug["place"]["kind"], "project");
    assert_eq!(sug["model"], "jev-1.14.0");

    let (auth, body) = mock.0.lock().unwrap().requests.pop().unwrap();
    assert_eq!(auth.as_deref(), Some("Bearer tsk_secret_0123456789"));
    assert_eq!(body["model"], "jev-1.14.0");
    assert_eq!(body["state"]["titel"], "Belege sammeln");
    let sent = body.to_string();
    assert!(
        !sent.contains("Meine Liebste") && !sent.contains("Jahrestag"),
        "private area left the server: {sent}"
    );
    assert!(sent.contains("Steuer 2026"));

    // Nothing changed on the task; the stored proposal comes back.
    let stored = s.get(&path, Auth::Web(&session)).await.json();
    assert_eq!(stored["weight"], "glut");
    let r = mcp_ok(&s, &t, "get_task", json!({"task_id": id})).await;
    assert_eq!(r["task"]["weight"], Value::Null);
    assert_eq!(r["task"]["location"]["kind"], "inbox");

    // Follow rate: weight taken, for_whom changed, moved into the project.
    mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": id, "weight": "glut", "for_whom": "uns"}),
    )
    .await;
    mcp_ok(
        &s,
        &t,
        "move_task",
        json!({"task_id": id, "location": {"kind": "project", "id": project_id}}),
    )
    .await;
    let stats = s.get("/api/settings/jev", Auth::Web(&session)).await.json()["stats"].clone();
    assert_eq!(
        stats,
        json!({"asked": 1, "weight": {"decided": 1, "followed": 1}, "for_whom": {"decided": 1, "followed": 0}, "place": {"decided": 1, "followed": 1}, "input_tokens": 321})
    );

    // Only inbox tasks.
    let r = s.post(&path, Auth::Web(&session), json!({})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    // Provider errors are German and change nothing.
    let other = mcp_ok(&s, &t, "create_task", json!({"title": "Noch was"})).await;
    let other_path = format!(
        "/api/tasks/{}/suggestion",
        other["task"]["id"].as_str().unwrap()
    );
    mock.0.lock().unwrap().status = 401;
    let r = s.post(&other_path, Auth::Web(&session), json!({})).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(r.text().contains("lehnt den Key ab"), "{}", r.text());
    assert_eq!(
        s.get(&other_path, Auth::Web(&session)).await.json(),
        Value::Null
    );

    // Reset removes key and settings.
    let r = s
        .request(
            Method::DELETE,
            "/api/settings/jev",
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.json()["configured"], false);
}
