//! Proposals for unsorted tasks from a decision model (ZUN-52, ZUN-56), against a
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
    /// Answer the weight with 0.95 instead of 0.6.
    sure: bool,
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
    let glut = if m.sure { 0.95 } else { 0.6 };
    let mut answers = json!({
        "gewicht": {"type": "choice", "choice": "glut", "probabilities": {"brand": 0.1, "glut": glut, "rauch": 0.2, "asche": 0.05, "unklar": 0.05}, "confidence": 0.5},
        "fuer_wen": {"type": "choice", "choice": "mich", "probabilities": {"mich": 0.7, "uns": 0.1, "andere": 0.1, "unklar": 0.1}, "confidence": 0.6},
    });
    // Pick the first project offered as the place (only asked for inbox tasks).
    if let Some(criteria) = body["questions"]["ort"]["criteria"].as_object() {
        let place = criteria
            .iter()
            .find(|(_, d)| d.as_str().unwrap().starts_with("Projekt"))
            .map(|(k, _)| k.clone())
            .unwrap();
        answers["ort"] = json!({"type": "choice", "choice": place, "probabilities": {place.clone(): 0.8, "nichts": 0.2}, "confidence": 0.7});
    }
    (
        StatusCode::OK,
        Json(json!({
            "model": "jev-1.14.0",
            "answers": answers,
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

/// A tally with one proposal of probability `p`, as `stats` shows it.
fn tally(decided: u64, followed: u64, p: f64) -> Value {
    tally_auto(decided, followed, p, 0, 0)
}

/// [`tally`] with proposals the UI set by itself.
fn tally_auto(decided: u64, followed: u64, p: f64, auto_set: u64, auto_changed: u64) -> Value {
    let rows: Vec<Value> = [0.0, 0.5, 0.6, 0.7, 0.8, 0.9]
        .iter()
        .map(|&min: &f64| {
            let n = u64::from(p >= min);
            json!({"min_probability": min, "decided": n * decided, "followed": n * followed})
        })
        .collect();
    json!({"decided": decided, "followed": followed, "by_threshold": rows,
        "auto_set": auto_set, "auto_changed": auto_changed})
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
async fn suggestion_for_an_unsorted_task() {
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
    assert_eq!(sug["auto"], json!([]), "0.6 and 0.7 are not sure enough");
    // Every UI may ask whether Jev is set up; the answer holds no key.
    let r = s.get("/api/jev", Auth::Bearer(&t)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"configured": true, "auto_apply_min_probability": 0.9})
    );

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
    // Mock probabilities: weight 0.6, for whom 0.7, place 0.8.
    assert_eq!(
        stats,
        json!({"asked": 1, "weight": tally(1, 1, 0.6), "for_whom": tally(1, 0, 0.7), "place": tally(1, 1, 0.8), "input_tokens": 321})
    );

    // Outside the inbox too, but without the place question; the place asked
    // in the inbox stays the follow rate's baseline.
    let r = s.post(&path, Auth::Web(&session), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["place"]["id"], project_id);
    let stats = s.get("/api/settings/jev", Auth::Web(&session)).await.json()["stats"].clone();
    assert_eq!(stats["place"], tally(1, 1, 0.8));
    let (_, body) = mock.0.lock().unwrap().requests.pop().unwrap();
    assert!(body["questions"].get("ort").is_none(), "{body}");
    assert!(!body.to_string().contains("Steuer 2026"));

    // Never for tasks in private areas (also via their project) or dev projects.
    let asked = mock.0.lock().unwrap().requests.len();
    let in_private = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "Blumen", "location": {"kind": "area", "id": private_id}}),
    )
    .await;
    let r = s
        .post(
            &format!(
                "/api/tasks/{}/suggestion",
                in_private["task"]["id"].as_str().unwrap()
            ),
            Auth::Web(&session),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    let dev = mcp_ok(
        &s,
        &t,
        "create_project",
        json!({"name": "Code", "area_id": open_id, "kind": "dev", "repo": "rubenvitt/zunder"}),
    )
    .await;
    let in_dev = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "Fix", "location": {"kind": "project", "id": dev["project"]["id"]}}),
    )
    .await;
    let r = s
        .post(
            &format!(
                "/api/tasks/{}/suggestion",
                in_dev["task"]["id"].as_str().unwrap()
            ),
            Auth::Web(&session),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    assert_eq!(mock.0.lock().unwrap().requests.len(), asked);

    // Only open root tasks: no subtasks, nothing closed.
    let asked = mock.0.lock().unwrap().requests.len();
    let root = mcp_ok(&s, &t, "create_task", json!({"title": "Umzug"})).await;
    let root_id = root["task"]["id"].as_str().unwrap().to_owned();
    let child = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "Kartons", "parent_id": root_id}),
    )
    .await;
    let r = s
        .post(
            &format!(
                "/api/tasks/{}/suggestion",
                child["task"]["id"].as_str().unwrap()
            ),
            Auth::Web(&session),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    let closed = mcp_ok(&s, &t, "create_task", json!({"title": "Erledigt"})).await;
    let closed_id = closed["task"]["id"].as_str().unwrap().to_owned();
    mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": closed_id, "status": "done"}),
    )
    .await;
    let r = s
        .post(
            &format!("/api/tasks/{closed_id}/suggestion"),
            Auth::Web(&session),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    assert_eq!(mock.0.lock().unwrap().requests.len(), asked);

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

#[tokio::test]
async fn eval_run_against_sorted_tasks() {
    let (s, mock) = server().await;
    let t = api_token(&s, "claude").await;
    let session = s.session().await;
    let eval = |body: Value| {
        let (s, session) = (&s, &session);
        async move {
            s.post("/api/settings/jev/eval", Auth::Web(session), body)
                .await
        }
    };

    let private = mcp_ok(&s, &t, "create_area", json!({"name": "Meine Liebste"})).await;
    let private_id = private["area"]["id"].as_str().unwrap().to_owned();
    let area = mcp_ok(&s, &t, "create_area", json!({"name": "Finanzen"})).await;
    let area_id = area["area"]["id"].as_str().unwrap().to_owned();
    let project = mcp_ok(
        &s,
        &t,
        "create_project",
        json!({"name": "Steuer 2026", "area_id": area_id}),
    )
    .await;
    let project_id = project["project"]["id"].as_str().unwrap().to_owned();
    let create = |body: Value| {
        let (s, t) = (&s, &t);
        async move {
            mcp_ok(s, t, "create_task", body).await["task"]["id"]
                .as_str()
                .unwrap()
                .to_owned()
        }
    };
    // All three known; the mock answers glut, mich and the project: 3 hits.
    let full = create(
        json!({"title": "Belege sammeln", "weight": "glut", "for_whom": "mich",
        "location": {"kind": "project", "id": project_id}}),
    )
    .await;
    // Weight and place known, both missed (rauch, area instead of project).
    create(json!({"title": "Konto kündigen", "weight": "rauch",
        "location": {"kind": "area", "id": area_id}}))
    .await;
    // Never sent: private area, inbox, nothing known, subtask.
    create(json!({"title": "Jahrestag planen", "weight": "glut",
        "location": {"kind": "area", "id": private_id}}))
    .await;
    create(json!({"title": "Inbox-Sache", "weight": "glut"})).await;
    create(json!({"title": "Lose Sache", "location": {"kind": "loose"}})).await;
    create(json!({"title": "Teilschritt", "weight": "asche", "parent_id": full})).await;

    // Without a key: conflict; agents may not run it.
    assert_eq!(eval(json!({})).await.status, StatusCode::CONFLICT);
    put_jev(
        &s,
        &session,
        json!({"token": "tsk_secret_0123456789", "private_area_ids": [private_id]}),
    )
    .await;
    assert_eq!(
        s.post("/api/settings/jev/eval", Auth::Bearer(&t), json!({}))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    for bad in [
        json!({"limit": 0}),
        json!({"limit": 101}),
        json!({"anzahl": 3}),
    ] {
        assert_eq!(eval(bad).await.status, StatusCode::BAD_REQUEST);
    }

    // limit 1: the case with the most known answers.
    let r = eval(json!({"limit": 1})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let report = r.json();
    assert_eq!(report["asked"], 1);
    assert_eq!(report["cases"][0]["title"], "Belege sammeln");
    assert_eq!(report["weight"]["hits"], tally(1, 1, 0.6));

    let r = eval(json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let report = r.json();
    assert_eq!(report["model"], "jev-1.14.0");
    assert_eq!(report["asked"], 2);
    assert_eq!(report["failed"], 0);
    assert_eq!(report["input_tokens"], 642);
    assert_eq!(report["weight"]["known"], 2);
    assert_eq!(report["weight"]["unclear"], 0);
    assert_eq!(report["weight"]["hits"], tally(2, 1, 0.6));
    assert_eq!(report["for_whom"]["known"], 1);
    assert_eq!(report["for_whom"]["hits"], tally(1, 1, 0.7));
    assert_eq!(report["place"]["known"], 2);
    assert_eq!(report["place"]["hits"], tally(2, 1, 0.8));
    let miss = &report["cases"][1];
    assert_eq!(miss["title"], "Konto kündigen");
    assert_eq!(
        miss["weight"],
        json!({"expected": "rauch", "proposed": "glut", "probability": 0.6})
    );
    assert_eq!(miss["place"]["expected"], "Finanzen");
    assert_eq!(miss["place"]["proposed"], "Steuer 2026");
    assert_eq!(miss["for_whom"]["expected"], Value::Null);

    let sent: Vec<String> = mock
        .0
        .lock()
        .unwrap()
        .requests
        .iter()
        .map(|(_, b)| b.to_string())
        .collect();
    assert_eq!(sent.len(), 3);
    for body in &sent {
        assert!(
            !body.contains("Meine Liebste") && !body.contains("Jahrestag"),
            "{body}"
        );
        assert!(
            !body.contains("Inbox-Sache") && !body.contains("Teilschritt"),
            "{body}"
        );
    }

    // Nothing stored: no proposal, no follow rate.
    let stored = s
        .get(
            &format!("/api/tasks/{full}/suggestion"),
            Auth::Web(&session),
        )
        .await;
    assert_eq!(stored.json(), Value::Null);
    let settings = s.get("/api/settings/jev", Auth::Web(&session)).await.json();
    assert_eq!(settings["stats"]["asked"], 0);

    // Every case failing: one German error.
    mock.0.lock().unwrap().status = 401;
    let r = eval(json!({})).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(r.text().contains("lehnt den Key ab"), "{}", r.text());
}

#[tokio::test]
async fn sure_proposals_are_marked_and_kept_apart() {
    let (s, mock) = server().await;
    let t = api_token(&s, "claude").await;
    let session = s.session().await;
    let r = put_jev(&s, &session, json!({"token": "tsk_secret_0123456789"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    mock.0.lock().unwrap().sure = true;

    // A loose task (no place question): the sure weight is marked, the
    // for-whom at 0.7 is not.
    let task = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "Laufschuhe kaufen", "location": {"kind": "loose"}}),
    )
    .await;
    let id = task["task"]["id"].as_str().unwrap().to_owned();
    let path = format!("/api/tasks/{id}/suggestion");
    let r = s.post(&path, Auth::Web(&session), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["auto"], json!(["weight"]));
    assert_eq!(
        s.get(&path, Auth::Web(&session)).await.json()["auto"],
        json!(["weight"])
    );

    // The UI sets it: counted apart, not as followed.
    mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": id, "weight": "glut"}),
    )
    .await;
    let stats = s.get("/api/settings/jev", Auth::Web(&session)).await.json()["stats"].clone();
    assert_eq!(stats["weight"], tally_auto(0, 0, 0.95, 1, 0));
    assert_eq!(stats["for_whom"], tally(0, 0, 0.7));

    // Jev never grades itself: an unchanged auto value is no known answer.
    let r = s
        .post("/api/settings/jev/eval", Auth::Web(&session), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());

    // The owner corrects it.
    mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": id, "weight": "asche"}),
    )
    .await;
    let stats = s.get("/api/settings/jev", Auth::Web(&session)).await.json()["stats"].clone();
    assert_eq!(stats["weight"], tally_auto(0, 0, 0.95, 1, 1));

    // Asking again keeps the mark while Jev proposes the same value, so the
    // correction still counts as one.
    let r = s.post(&path, Auth::Web(&session), json!({})).await;
    assert_eq!(r.json()["auto"], json!(["weight"]));
    let stats = s.get("/api/settings/jev", Auth::Web(&session)).await.json()["stats"].clone();
    assert_eq!(stats["weight"], tally_auto(0, 0, 0.95, 1, 1));

    // A field that is set already is never newly marked.
    let other = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "Zahnarzt", "location": {"kind": "loose"}, "weight": "brand"}),
    )
    .await;
    let r = s
        .post(
            &format!(
                "/api/tasks/{}/suggestion",
                other["task"]["id"].as_str().unwrap()
            ),
            Auth::Web(&session),
            json!({}),
        )
        .await;
    assert_eq!(r.json()["auto"], json!([]));
}
