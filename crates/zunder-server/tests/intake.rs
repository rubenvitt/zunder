//! Intake channels (`/intake/v1/tasks`, `/api/intake/channels`): a foreign
//! server creates open tasks in exactly one project and can do nothing else.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use zunder_core::EntityKind;

use common::{api_token, mcp_ok, mcp_post, Auth, TestServer, BOOTSTRAP};

struct Setup {
    s: TestServer,
    /// API token for MCP calls in the tests.
    api: String,
    session: String,
    area_id: String,
    /// Dev project with key `IDAE`.
    project_id: String,
}

async fn setup() -> Setup {
    let s = TestServer::new().await;
    let api = api_token(&s, "claude").await;
    let session = s.session().await;
    let area = mcp_ok(&s, &api, "create_area", json!({"name": "Ehrenamt"})).await;
    let area_id = area["area"]["id"].as_str().unwrap().to_owned();
    let project_id = project(&s, &api, &area_id, json!({"kind": "dev", "key": "IDAE"})).await;
    Setup {
        s,
        api,
        session,
        area_id,
        project_id,
    }
}

async fn project(s: &TestServer, api: &str, area_id: &str, extra: Value) -> String {
    let mut args = json!({"name": "IDA Eingang", "area_id": area_id});
    args.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let p = mcp_ok(s, api, "create_project", args).await;
    p["project"]["id"].as_str().unwrap().to_owned()
}

impl Setup {
    /// Creates a channel through the management API and returns its token.
    async fn channel(&self, name: &str, project_id: &str) -> (String, String) {
        let r = self
            .s
            .post(
                "/api/intake/channels",
                Auth::Web(&self.session),
                json!({"name": name, "project_id": project_id}),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
        let body = r.json();
        (
            body["id"].as_str().unwrap().to_owned(),
            body["token"].as_str().unwrap().to_owned(),
        )
    }

    async fn submit(&self, token: &str, body: Value) -> common::TestResponse {
        self.s
            .post("/intake/v1/tasks", Auth::Bearer(token), body)
            .await
    }

    fn tasks_in(&self, project_id: &str) -> Vec<zunder_core::Task> {
        let id = zunder_core::ids::Id::parse(project_id).unwrap();
        self.s.app.read(|state| {
            state
                .tasks()
                .filter(|t| state.effective_project(t.id) == Some(id))
                .collect()
        })
    }
}

#[tokio::test]
async fn creates_an_open_task_with_reference_and_is_idempotent() {
    let t = setup().await;
    let (_, token) = t.channel("iuk-suite", &t.project_id).await;
    assert!(token.starts_with("zi_"));
    assert_eq!(token.len(), 3 + 43);

    let body = json!({
        "external_id": "rueckmeldung-17",
        "title": "  Karte lädt nicht  ",
        "description": "Beim Öffnen der Lage **hängt** die Karte.",
        "links": [{"url": "https://suite.example.org/rm/17", "title": "Rückmeldung"}],
    });
    let r = t.submit(&token, body.clone()).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let created = r.json();
    assert_eq!(created["ref"], "IDAE-1");
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created.as_object().unwrap().len(), 2, "only id and ref");

    let task = mcp_ok(&t.s, &t.api, "get_task", json!({"task_id": id})).await["task"].clone();
    assert_eq!(task["title"], "Karte lädt nicht");
    assert_eq!(task["status"], "open");
    assert_eq!(task["type"], "aufgabe");
    assert_eq!(task["location"]["id"], t.project_id.as_str());
    assert_eq!(task["description"], body["description"]);
    assert_eq!(task["links"], body["links"]);

    // The same external_id again: the first task, nothing written.
    let cursor = t.s.app.cursor();
    let again = t
        .submit(
            &token,
            json!({"external_id": "rueckmeldung-17", "title": "Anders"}),
        )
        .await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.text());
    assert_eq!(again.json(), created);
    assert_eq!(t.s.app.cursor(), cursor);
    assert_eq!(t.tasks_in(&t.project_id).len(), 1);

    // Still the same task after it was closed and even deleted.
    mcp_ok(
        &t.s,
        &t.api,
        "update_task",
        json!({"task_id": id, "status": "done"}),
    )
    .await;
    mcp_ok(&t.s, &t.api, "delete_task", json!({"task_id": id})).await;
    let again = t.submit(&token, body).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.json(), created);

    // A new external_id is a new task with the next reference.
    let r = t
        .submit(
            &token,
            json!({"external_id": "rueckmeldung-18", "title": "Zweite"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json()["ref"], "IDAE-2");
}

#[tokio::test]
async fn history_names_the_channel() {
    let t = setup().await;
    let (_, token) = t.channel("iuk-suite", &t.project_id).await;
    let r = t
        .submit(&token, json!({"external_id": "a", "title": "Verlauf"}))
        .await;
    let id = r.json()["id"].as_str().unwrap().to_owned();
    let history =
        t.s.get(
            &format!("/api/tasks/{id}/history"),
            Auth::Cookie(&t.session),
        )
        .await
        .json();
    let sources: Vec<(&str, &str)> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["op"]["field"].as_str().unwrap(),
                e["source"].as_str().unwrap(),
            )
        })
        .collect();
    for field in ["title", "type", "status", "location"] {
        assert!(
            sources.contains(&(field, "intake:iuk-suite")),
            "{sources:?}"
        );
    }
    // The reference comes from the server, as for every other task.
    assert!(sources.contains(&("ref", "server")), "{sources:?}");
}

#[tokio::test]
async fn task_without_project_key_has_no_reference() {
    let t = setup().await;
    let plain = project(&t.s, &t.api, &t.area_id, json!({})).await;
    let (_, token) = t.channel("ohne-schluessel", &plain).await;
    let r = t
        .submit(&token, json!({"external_id": "a", "title": "Ohne"}))
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json()["ref"], Value::Null);
}

#[tokio::test]
async fn bug_and_feature_only_in_dev_projects() {
    let t = setup().await;
    let (_, token) = t.channel("dev", &t.project_id).await;
    for (i, kind) in ["bug", "feature", "aufgabe"].iter().enumerate() {
        let r = t
            .submit(
                &token,
                json!({"external_id": format!("k{i}"), "title": kind, "type": kind}),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    }
    let types: Vec<String> = t
        .tasks_in(&t.project_id)
        .iter()
        .map(|t| t.task_type.to_string())
        .collect();
    assert_eq!(types.len(), 3);
    assert!(types.contains(&"bug".into()) && types.contains(&"feature".into()));

    // In a standard project the core rejects a bug: 400 with its German
    // message, nothing stored, the external_id stays free.
    let plain = project(&t.s, &t.api, &t.area_id, json!({})).await;
    let (_, token) = t.channel("standard", &plain).await;
    let cursor = t.s.app.cursor();
    let r = t
        .submit(
            &token,
            json!({"external_id": "b", "title": "Bug", "type": "bug"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    assert_eq!(r.error_code(), "invalid_request");
    assert!(
        r.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Dev"),
        "{}",
        r.text()
    );
    assert_eq!(t.s.app.cursor(), cursor);
    let r = t
        .submit(&token, json!({"external_id": "b", "title": "Aufgabe"}))
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
}

#[tokio::test]
async fn invalid_bodies_are_bad_requests_and_store_nothing() {
    let t = setup().await;
    let (_, token) = t.channel("iuk-suite", &t.project_id).await;
    let cursor = t.s.app.cursor();
    let long = |n: usize| "x".repeat(n);
    let cases = [
        json!({"external_id": "a", "title": "t", "status": "done"}),
        json!({"external_id": "a", "title": "t", "location": {"kind": "inbox"}}),
        json!({"external_id": "a", "title": "t", "parent_id": "abc"}),
        json!({"title": "t"}),
        json!({"external_id": "a b", "title": "t"}),
        json!({"external_id": long(129), "title": "t"}),
        json!({"external_id": "a"}),
        json!({"external_id": "a", "title": " "}),
        json!({"external_id": "a", "title": long(201)}),
        json!({"external_id": "a", "title": "t", "description": long(20_001)}),
        json!({"external_id": "a", "title": "t", "type": "epic"}),
        json!({"external_id": "a", "title": "t", "links": [{"url": "mailto:a@example.org"}]}),
        json!({"external_id": "a", "title": "t", "links": [{"url": "https://a.example", "extra": 1}]}),
        json!({"external_id": "a", "title": "t", "links": vec![json!({"url": "https://a.example"}); 6]}),
        json!(["a"]),
    ];
    for body in cases {
        let r = t.submit(&token, body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text());
        assert_eq!(r.error_code(), "invalid_request", "{body}");
    }
    // Not JSON, or without its content type.
    for (content_type, body) in [("application/json", "{"), ("text/plain", "{}")] {
        let req = Request::builder()
            .method(Method::POST)
            .uri("/intake/v1/tasks")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap();
        let r = t.s.send(req).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
    }
    assert_eq!(t.s.app.cursor(), cursor);
    assert!(t.tasks_in(&t.project_id).is_empty());

    // The limits themselves are fine.
    let r = t
        .submit(
            &token,
            json!({"external_id": long(128), "title": long(200), "description": long(20_000),
                   "links": vec![json!({"url": "http://a.example"}); 5]}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
}

#[tokio::test]
async fn body_limit_is_64_kib() {
    let t = setup().await;
    let (_, token) = t.channel("iuk-suite", &t.project_id).await;
    let r = t
        .submit(
            &token,
            json!({"external_id": "a", "title": "t", "description": "x".repeat(70 * 1024)}),
        )
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn only_channel_tokens_are_accepted() {
    let t = setup().await;
    let (channel_id, token) = t.channel("iuk-suite", &t.project_id).await;
    let body = json!({"external_id": "a", "title": "t"});

    // Missing, unknown, API, bootstrap tokens and a session: 401.
    let unknown = format!("zi_{}", "A".repeat(43));
    for auth in [
        Auth::None,
        Auth::Bearer(&unknown),
        Auth::Bearer(&t.api),
        Auth::Bearer(BOOTSTRAP),
        Auth::Web(&t.session),
    ] {
        let r = t.s.post("/intake/v1/tasks", auth, body.clone()).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.text());
        assert_eq!(r.error_code(), "unauthorized");
        assert!(r.header("www-authenticate").is_some());
    }

    // A channel token opens nothing under /api or /mcp.
    for path in [
        "/api/snapshot",
        "/api/me",
        "/api/sync/pull",
        "/api/intake/channels",
    ] {
        let r = t.s.get(path, Auth::Bearer(&token)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}");
    }
    let id = zunder_core::ids::new_id();
    let ops = common::ops(&mut common::client_clock(), |b| {
        b.set(EntityKind::Task, id, "title", "Eingeschleust");
    });
    let r = t.s.push(Auth::Bearer(&token), &ops).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = mcp_post(
        &t.s,
        &token,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Only creating: other methods and paths are not there.
    let r = t.s.get("/intake/v1/tasks", Auth::Bearer(&token)).await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    let r = t.s.get("/intake/v1/tasks/x", Auth::Bearer(&token)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // Revoked: 401 from then on.
    let r =
        t.s.request(
            Method::DELETE,
            &format!("/api/intake/channels/{channel_id}"),
            Auth::Web(&t.session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = t.submit(&token, body).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(t.tasks_in(&t.project_id).is_empty());
}

#[tokio::test]
async fn deleted_or_completed_target_project_is_a_conflict() {
    let t = setup().await;
    let (_, token) = t.channel("iuk-suite", &t.project_id).await;
    mcp_ok(
        &t.s,
        &t.api,
        "update_project",
        json!({"project_id": t.project_id, "status": "completed"}),
    )
    .await;
    let r = t
        .submit(&token, json!({"external_id": "a", "title": "t"}))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    assert_eq!(r.error_code(), "conflict");

    let other = project(&t.s, &t.api, &t.area_id, json!({"kind": "dev"})).await;
    let (_, token) = t.channel("weg", &other).await;
    let pid = zunder_core::ids::Id::parse(&other).unwrap();
    let ops = common::ops(&mut common::client_clock(), |b| {
        b.set(EntityKind::Project, pid, "deleted", true);
    });
    let r = t.s.push(Auth::Bearer(&t.api), &ops).await;
    assert_eq!(r.json()["rejected"], json!([]));
    let r = t
        .submit(&token, json!({"external_id": "a", "title": "t"}))
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    assert!(t.tasks_in(&other).is_empty());
}

#[tokio::test]
async fn at_most_30_submissions_per_hour_and_channel() {
    let t = setup().await;
    let (_, token) = t.channel("iuk-suite", &t.project_id).await;
    let (_, other) = t.channel("andere", &t.project_id).await;
    for i in 0..30 {
        let r = t
            .submit(
                &token,
                json!({"external_id": format!("n{i}"), "title": "t"}),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{i}: {}", r.text());
    }
    let r = t
        .submit(&token, json!({"external_id": "n30", "title": "t"}))
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.text());
    assert_eq!(r.error_code(), "too_many_requests");
    let retry: u64 = r.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=3600).contains(&retry), "{retry}");
    // A repetition still answers, and other channels are not affected.
    let r = t
        .submit(&token, json!({"external_id": "n0", "title": "t"}))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = t
        .submit(&other, json!({"external_id": "n30", "title": "t"}))
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(t.tasks_in(&t.project_id).len(), 31);
}

#[tokio::test]
async fn channels_are_managed_with_a_browser_session() {
    let t = setup().await;
    let r =
        t.s.get("/api/intake/channels", Auth::Cookie(&t.session))
            .await;
    assert_eq!(r.json(), json!([]));

    let r =
        t.s.post(
            "/api/intake/channels",
            Auth::Web(&t.session),
            json!({"name": " iuk-suite ", "project_id": t.project_id}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let created = r.json();
    assert_eq!(created["name"], "iuk-suite");
    assert_eq!(created["project_id"], t.project_id.as_str());
    assert_eq!(created["project_name"], "IDA Eingang");
    assert_eq!(created["last_used_at"], Value::Null);
    let id = created["id"].as_str().unwrap().to_owned();
    assert!(zunder_core::ids::Id::parse(&id).is_some(), "CUID2: {id}");
    let token = created["token"].as_str().unwrap().to_owned();

    // The list never contains the token; last_used_at follows use.
    t.submit(&token, json!({"external_id": "a", "title": "t"}))
        .await;
    let list =
        t.s.get("/api/intake/channels", Auth::Cookie(&t.session))
            .await
            .json();
    let entry = &list[0];
    assert_eq!(list.as_array().unwrap().len(), 1);
    let keys: Vec<&str> = entry
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "created_at",
            "id",
            "last_used_at",
            "name",
            "project_id",
            "project_name"
        ]
    );
    assert!(entry["last_used_at"].is_string());
    assert!(!list.to_string().contains(&token));

    // Duplicate name, bad name, dead or completed project.
    let post = |body: Value| {
        t.s.post("/api/intake/channels", Auth::Web(&t.session), body)
    };
    let r = post(json!({"name": "iuk-suite", "project_id": t.project_id})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = post(json!({"name": "", "project_id": t.project_id})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = post(json!({"name": "x", "project_id": "kein"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = post(json!({"name": "x", "project_id": zunder_core::ids::new_id()})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = post(json!({"name": "x", "project_id": t.project_id, "scope": "all"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let done = project(&t.s, &t.api, &t.area_id, json!({"status": "completed"})).await;
    let r = post(json!({"name": "x", "project_id": done})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    // Bearer tokens cannot manage channels; the CSRF rule applies.
    let r = t.s.get("/api/intake/channels", Auth::Bearer(&t.api)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r =
        t.s.post(
            "/api/intake/channels",
            Auth::Bearer(&t.api),
            json!({"name": "y", "project_id": t.project_id}),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r =
        t.s.post(
            "/api/intake/channels",
            Auth::Cookie(&t.session),
            json!({"name": "y", "project_id": t.project_id}),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Revoking frees the name; a second revocation is 404.
    let path = format!("/api/intake/channels/{id}");
    let delete = |path| {
        t.s.request(Method::DELETE, path, Auth::Web(&t.session), None)
    };
    let r = delete(&path).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = delete(&path).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = delete("/api/intake/channels/kein").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r =
        t.s.get("/api/intake/channels", Auth::Cookie(&t.session))
            .await;
    assert_eq!(r.json(), json!([]));
    let r = post(json!({"name": "iuk-suite", "project_id": t.project_id})).await;
    assert_eq!(r.status, StatusCode::CREATED);
}

#[tokio::test]
async fn submissions_survive_a_restart() {
    let t = setup().await;
    let (_, token) = t.channel("iuk-suite", &t.project_id).await;
    let first = t
        .submit(&token, json!({"external_id": "a", "title": "t"}))
        .await
        .json();
    let Setup {
        s,
        api,
        session,
        area_id,
        project_id,
    } = t;
    let t = Setup {
        s: s.restart().await,
        api,
        session,
        area_id,
        project_id,
    };
    let r = t
        .submit(&token, json!({"external_id": "a", "title": "t"}))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), first);
}
