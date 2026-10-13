//! Contract and integration tests of the MCP endpoint (`/mcp`): protocol,
//! auth, the frozen tool schemas and every tool via HTTP.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use zunder_core::{Clock, NewTask, NodeId, OpBatch, TaskStatus};

use common::{
    api_token, mcp_call, mcp_err, mcp_ok, mcp_post, ops, tool_err, Auth, TestServer, BOOTSTRAP,
};

const SNAPSHOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/snapshots/mcp_tools.json"
);
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

fn rpc(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

async fn setup() -> (TestServer, String) {
    let s = TestServer::new().await;
    let token = api_token(&s, "claude").await;
    (s, token)
}

/// Creates an area and a dev project; returns their IDs.
async fn structure(s: &TestServer, t: &str) -> (String, String, String) {
    let area = mcp_ok(s, t, "create_area", json!({"name": "Arbeit"})).await;
    let area_id = area["area"]["id"].as_str().unwrap().to_owned();
    let dev = mcp_ok(
        s,
        t,
        "create_project",
        json!({"name": "Zunder", "area_id": area_id, "kind": "dev", "repo": "beispiel/zunder"}),
    )
    .await;
    let std = mcp_ok(
        s,
        t,
        "create_project",
        json!({"name": "Umzug", "area_id": area_id}),
    )
    .await;
    (
        area_id,
        dev["project"]["id"].as_str().unwrap().to_owned(),
        std["project"]["id"].as_str().unwrap().to_owned(),
    )
}

async fn create(s: &TestServer, t: &str, args: Value) -> String {
    let r = mcp_ok(s, t, "create_task", args).await;
    r["task"]["id"].as_str().unwrap().to_owned()
}

fn titles(result: &Value) -> Vec<String> {
    result["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap().to_owned())
        .collect()
}

// ----- protocol -------------------------------------------------------------------------

#[tokio::test]
async fn initialize_negotiates_version() {
    let (s, t) = setup().await;
    for (requested, expected) in [
        ("2025-06-18", "2025-06-18"),
        ("2025-03-26", "2025-03-26"),
        ("2024-11-05", "2025-06-18"),
    ] {
        let r = mcp_post(
            &s,
            &t,
            rpc(
                1,
                "initialize",
                json!({
                    "protocolVersion": requested,
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "1"},
                }),
            ),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r
            .header("content-type")
            .unwrap()
            .starts_with("application/json"));
        let body = r.json();
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        let result = &body["result"];
        assert_eq!(result["protocolVersion"], expected);
        assert_eq!(result["serverInfo"]["name"], "zunder");
        assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(result["capabilities"]["tools"]["listChanged"], false);
        let instructions = result["instructions"].as_str().unwrap();
        assert!(instructions.contains("inbox"));
        assert!(instructions.contains("in_progress"));
    }
    assert!(r_no_session_header(&s, &t).await);
}

async fn r_no_session_header(s: &TestServer, t: &str) -> bool {
    let r = mcp_post(
        s,
        t,
        rpc(1, "initialize", json!({"protocolVersion": "2025-06-18"})),
    )
    .await;
    r.header("mcp-session-id").is_none()
}

#[tokio::test]
async fn notifications_ping_and_batches() {
    let (s, t) = setup().await;
    let r = mcp_post(
        &s,
        &t,
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert!(r.body.is_empty());

    let r = mcp_post(&s, &t, rpc(7, "ping", json!({}))).await;
    assert_eq!(r.json(), json!({"jsonrpc": "2.0", "id": 7, "result": {}}));

    // A client response alone is accepted without answer.
    let r = mcp_post(&s, &t, json!({"jsonrpc": "2.0", "id": 3, "result": {}})).await;
    assert_eq!(r.status, StatusCode::ACCEPTED);

    let r = mcp_post(
        &s,
        &t,
        json!([
            rpc(1, "ping", json!({})),
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": "zwei", "method": "tools/list"},
        ]),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let body = r.json();
    let replies = body.as_array().unwrap();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(replies[1]["id"], "zwei");
    assert!(replies[1]["result"]["tools"].is_array());

    let r = mcp_post(
        &s,
        &t,
        json!([{"jsonrpc": "2.0", "method": "notifications/initialized"}]),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);

    let r = mcp_post(&s, &t, json!([])).await;
    assert_eq!(r.json()["error"]["code"], -32600);

    // Sessions are optional: a client-sent session ID is ignored.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(header::AUTHORIZATION, format!("Bearer {t}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("mcp-session-id", "irgendwas")
        .body(Body::from(rpc(1, "ping", json!({})).to_string()))
        .unwrap();
    assert_eq!(s.send(req).await.status, StatusCode::OK);
}

#[tokio::test]
async fn json_rpc_errors() {
    let (s, t) = setup().await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(header::AUTHORIZATION, format!("Bearer {t}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{nope"))
        .unwrap();
    let r = s.send(req).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], -32700);
    assert_eq!(r.json()["id"], Value::Null);

    let cases = [
        (rpc(1, "resources/list", json!({})), -32601),
        (json!({"jsonrpc": "1.0", "id": 1, "method": "ping"}), -32600),
        (json!({"jsonrpc": "2.0", "id": 1}), -32600),
        (
            json!({"jsonrpc": "2.0", "id": null, "method": "ping"}),
            -32600,
        ),
        (
            json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": [1]}),
            -32602,
        ),
        (
            rpc(1, "tools/call", json!({"name": "gibt_es_nicht"})),
            -32602,
        ),
        (rpc(1, "tools/call", json!({"arguments": {}})), -32602),
        (
            rpc(
                1,
                "tools/call",
                json!({"name": "get_task", "arguments": [1]}),
            ),
            -32602,
        ),
        (json!(42), -32600),
    ];
    for (body, code) in cases {
        let r = mcp_post(&s, &t, body.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{body}");
        assert_eq!(r.json()["error"]["code"], code, "{body}");
        assert!(r.json()["error"]["message"].is_string());
    }
}

#[tokio::test]
async fn auth_origin_and_methods() {
    let (s, t) = setup().await;
    let ping = rpc(1, "ping", json!({}));
    let send = |auth: Auth<'static>, origin: Option<&'static str>| {
        let mut b = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json");
        b = common::authorize(b, auth);
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(Body::from(ping.to_string())).unwrap()
    };

    let r = s.send(send(Auth::None, None)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.header("www-authenticate").unwrap().starts_with("Bearer"));
    let r = s.send(send(Auth::Bearer("zt_falsch"), None)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // Browser sessions do not work for /mcp.
    let session: &'static str = Box::leak(s.session().await.into_boxed_str());
    let r = s.send(send(Auth::Web(session), None)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // Bootstrap token works; Origin must match the public URL.
    let r = s.send(send(Auth::Bearer(BOOTSTRAP), None)).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = s
        .send(send(Auth::Bearer(BOOTSTRAP), Some("http://localhost:8080")))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = s
        .send(send(Auth::Bearer(BOOTSTRAP), Some("https://boese.example")))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Unsupported protocol version header.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(header::AUTHORIZATION, format!("Bearer {t}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("mcp-protocol-version", "1999-01-01")
        .body(Body::from(ping.to_string()))
        .unwrap();
    assert_eq!(s.send(req).await.status, StatusCode::BAD_REQUEST);

    // No server-initiated stream, no sessions to delete.
    for method in [Method::GET, Method::DELETE] {
        let r = s
            .request(method.clone(), "/mcp", Auth::Bearer(&t), None)
            .await;
        assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{method}");
    }

    // Revoked tokens stop working.
    let created = s
        .app
        .create_token(s.app.owner_id(), "kurzlebig")
        .await
        .unwrap();
    assert_eq!(
        mcp_post(&s, &created.token, ping.clone()).await.status,
        StatusCode::OK
    );
    s.app
        .revoke_token(s.app.owner_id(), created.info.id)
        .await
        .unwrap();
    assert_eq!(
        mcp_post(&s, &created.token, ping.clone()).await.status,
        StatusCode::UNAUTHORIZED
    );
}

// ----- contract -------------------------------------------------------------------------

#[tokio::test]
async fn tool_schemas_match_snapshot() {
    let (s, t) = setup().await;
    let r = mcp_post(&s, &t, rpc(1, "tools/list", json!({}))).await;
    let tools = r.json()["result"]["tools"].clone();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "filter_tasks",
            "get_task",
            "create_task",
            "update_task",
            "move_task",
            "delete_task",
            "search",
            "claim_task",
            "release_task",
            "decompose_task",
            "list_structure",
            "create_area",
            "update_area",
            "create_project",
            "update_project",
            "start_flame",
            "stop_flame",
            "get_task_history",
            "add_comment",
        ]
    );
    for tool in tools.as_array().unwrap() {
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{}", tool["name"]);
        assert_eq!(schema["additionalProperties"], false, "{}", tool["name"]);
        for key in ["oneOf", "anyOf", "allOf"] {
            assert!(
                schema.get(key).is_none(),
                "{} uses top-level {key}",
                tool["name"]
            );
        }
        assert!(tool["description"].as_str().unwrap().len() > 20);
    }

    let actual = format!("{}\n", serde_json::to_string_pretty(&tools).unwrap());
    if std::env::var("UPDATE_SNAPSHOTS").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(std::path::Path::new(SNAPSHOT).parent().unwrap()).unwrap();
        std::fs::write(SNAPSHOT, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(SNAPSHOT).unwrap_or_default();
    assert!(
        expected == actual,
        "Die Werkzeug-Schemas weichen vom Vertrag ab ({SNAPSHOT}). \
         Absichtliche Änderung? Vertrag anpassen und mit UPDATE_SNAPSHOTS=1 neu schreiben."
    );
}

// ----- tools ----------------------------------------------------------------------------

#[tokio::test]
async fn create_get_update_with_history() {
    let (s, t) = setup().await;
    let created = mcp_ok(&s, &t, "create_task", json!({"title": "Zahnarzt anrufen"})).await;
    let task = &created["task"];
    assert_eq!(task["location"], json!({"kind": "inbox"}));
    assert_eq!(task["status"], "open");
    assert_eq!(task["type"], "aufgabe");
    assert_eq!(task["progress"], json!({"done": 0, "total": 0}));
    for key in [
        "id",
        "title",
        "description",
        "type",
        "status",
        "location",
        "parent_id",
        "due",
        "undated_ack",
        "remind_at",
        "links",
        "vault_links",
        "phoenix",
        "claimed_by",
        "claim_expires_at",
        "plan",
        "branch",
        "pr_url",
        "timing_ref",
        "clickup_id",
        "created_at",
        "updated_at",
        "status_changed_at",
        "progress",
    ] {
        assert!(task.get(key).is_some(), "missing {key}");
    }
    let id = task["id"].as_str().unwrap().to_owned();

    let updated = mcp_ok(
        &s,
        &t,
        "update_task",
        json!({
            "task_id": id,
            "status": "in_progress",
            "due": "2026-10-20",
            "remind_at": "2026-10-20T09:00:00+02:00",
            "description": "Termin **vereinbaren**",
            "links": [{"url": "https://example.org/praxis", "title": "Praxis"}],
            "vault_links": ["Bereiche/Gesundheit.md"],
            "phoenix": {"role": "flamme", "fire": "2026-W43"},
        }),
    )
    .await;
    let task = &updated["task"];
    assert_eq!(task["status"], "in_progress");
    assert_eq!(task["due"], "2026-10-20");
    assert_eq!(task["remind_at"], "2026-10-20T09:00:00+02:00");
    assert_eq!(task["phoenix"]["fire"], "2026-W43");
    assert_eq!(task["links"][0]["title"], "Praxis");

    // null clears optional fields
    let cleared = mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": id, "due": null, "phoenix": null}),
    )
    .await;
    assert_eq!(cleared["task"]["due"], Value::Null);
    assert_eq!(cleared["task"]["phoenix"], Value::Null);

    let got = mcp_ok(&s, &t, "get_task", json!({"task_id": id})).await;
    assert_eq!(got["task"]["description"], "Termin **vereinbaren**");
    assert_eq!(got["task"]["subtasks"], json!([]));

    let history = mcp_ok(&s, &t, "get_task_history", json!({"task_id": id})).await;
    let entries = history["entries"].as_array().unwrap();
    assert!(entries.iter().all(|e| e["source"] == "mcp:claude"));
    assert!(entries.iter().any(|e| e["field"] == "title"));
    let limited = mcp_ok(
        &s,
        &t,
        "get_task_history",
        json!({"task_id": id, "limit": 2}),
    )
    .await;
    assert_eq!(limited["count"], 2);
    assert_eq!(limited["truncated"], true);
    assert_eq!(limited["entries"][1]["seq"], entries.last().unwrap()["seq"]);

    let (code, _) = mcp_err(&s, &t, "update_task", json!({"task_id": id})).await;
    assert_eq!(code, "invalid_arguments");
}

#[tokio::test]
async fn core_validation_messages_are_surfaced() {
    let (s, t) = setup().await;
    let (_, dev, std_project) = structure(&s, &t).await;

    let (code, message) = mcp_err(
        &s,
        &t,
        "create_task",
        json!({"title": "Absturz", "type": "bug"}),
    )
    .await;
    assert_eq!(code, "dev_only");
    assert!(message.contains("Dev-Projekt"), "{message}");

    let (code, message) = mcp_err(
        &s,
        &t,
        "create_task",
        json!({"title": "X", "due": "morgen"}),
    )
    .await;
    assert_eq!(code, "invalid_value");
    assert!(message.contains("„due“"), "{message}");

    let (code, _) = mcp_err(&s, &t, "create_task", json!({"title": "   "})).await;
    assert_eq!(code, "invalid_value");

    let (code, _) = mcp_err(
        &s,
        &t,
        "create_task",
        json!({"title": "X", "location": {"kind": "project", "id": "01929c3e-0000-7000-8000-000000000001"}}),
    )
    .await;
    assert_eq!(code, "invalid_reference");

    // Argument checks
    let (code, message) = mcp_err(&s, &t, "create_task", json!({"titel": "X"})).await;
    assert_eq!(code, "invalid_arguments");
    assert!(message.contains("titel"), "{message}");
    let (_, message) = mcp_err(&s, &t, "create_task", json!({})).await;
    assert!(message.contains("„title“ fehlt"), "{message}");
    let (_, message) = mcp_err(&s, &t, "get_task", json!({})).await;
    assert!(message.contains("task_id oder clickup_id"), "{message}");
    let (_, message) = mcp_err(&s, &t, "get_task", json!({"task_id": "nope"})).await;
    assert!(message.contains("ZUN-42"), "{message}");
    let (code, _) = mcp_err(
        &s,
        &t,
        "get_task",
        json!({"task_id": "01929c3e-0000-7000-8000-000000000001"}),
    )
    .await;
    assert_eq!(code, "not_found");

    // Bug in a dev project is fine; moving it to a standard project is not.
    let bug = create(
        &s,
        &t,
        json!({"title": "Absturz", "type": "bug", "location": {"kind": "project", "id": dev}}),
    )
    .await;
    let (code, _) = mcp_err(
        &s,
        &t,
        "move_task",
        json!({"task_id": bug, "location": {"kind": "project", "id": std_project}}),
    )
    .await;
    assert_eq!(code, "dev_only");
}

#[tokio::test]
async fn structure_tools() {
    let (s, t) = setup().await;
    let (area, dev, std_project) = structure(&s, &t).await;
    create(&s, &t, json!({"title": "Inbox 1"})).await;
    create(
        &s,
        &t,
        json!({"title": "Lose", "location": {"kind": "loose"}}),
    )
    .await;
    create(
        &s,
        &t,
        json!({"title": "Im Bereich", "location": {"kind": "area", "id": area}}),
    )
    .await;
    create(
        &s,
        &t,
        json!({"title": "Feature", "type": "feature", "location": {"kind": "project", "id": dev}}),
    )
    .await;
    create(
        &s,
        &t,
        json!({"title": "Erledigt", "status": "done", "location": {"kind": "project", "id": dev}}),
    )
    .await;

    let listed = mcp_ok(&s, &t, "list_structure", json!({})).await;
    assert_eq!(listed["inbox_open"], 1);
    assert_eq!(listed["loose_open"], 1);
    let areas = listed["areas"].as_array().unwrap();
    assert_eq!(areas.len(), 1);
    assert_eq!(areas[0]["name"], "Arbeit");
    assert_eq!(areas[0]["open_tasks"], 1);
    let projects = areas[0]["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 2);
    let zunder = projects.iter().find(|p| p["id"] == dev.as_str()).unwrap();
    assert_eq!(zunder["kind"], "dev");
    assert_eq!(zunder["view"], "board");
    assert_eq!(zunder["repo"], "beispiel/zunder");
    assert_eq!(zunder["open_tasks"], 1);
    assert_eq!(zunder["area_name"], "Arbeit");

    let r = mcp_ok(
        &s,
        &t,
        "update_project",
        json!({"project_id": std_project, "status": "completed", "description": "Fertig"}),
    )
    .await;
    assert_eq!(r["project"]["status"], "completed");
    let listed = mcp_ok(&s, &t, "list_structure", json!({})).await;
    assert_eq!(listed["areas"][0]["projects"].as_array().unwrap().len(), 1);
    let listed = mcp_ok(&s, &t, "list_structure", json!({"include_archived": true})).await;
    assert_eq!(listed["areas"][0]["projects"].as_array().unwrap().len(), 2);

    let r = mcp_ok(
        &s,
        &t,
        "update_area",
        json!({"area_id": area, "name": "Beruf", "vault_path": "Bereiche/Beruf.md"}),
    )
    .await;
    assert_eq!(r["area"]["name"], "Beruf");
    assert_eq!(r["area"]["vault_path"], "Bereiche/Beruf.md");
    let r = mcp_ok(
        &s,
        &t,
        "update_area",
        json!({"area_id": area, "vault_path": null}),
    )
    .await;
    assert_eq!(r["area"]["vault_path"], Value::Null);

    // Explicit IDs (zunder_id from the vault) and their uniqueness.
    let wanted = "tz4a98xxat96iws9zmbrgj3a";
    let r = mcp_ok(&s, &t, "create_area", json!({"name": "Haus", "id": wanted})).await;
    assert_eq!(r["area"]["id"], wanted);
    // A zunder_id from before CUID2 becomes its fixed ID.
    let legacy = "01929c3e-0000-7000-8000-0000000000ab";
    let r = mcp_ok(&s, &t, "create_area", json!({"name": "Alt", "id": legacy})).await;
    let converted = zunder_core::ids::Id::parse_any(legacy).unwrap().to_string();
    assert_eq!(r["area"]["id"], converted.as_str());
    let r = mcp_ok(
        &s,
        &t,
        "update_area",
        json!({"area_id": legacy, "name": "Alt 2"}),
    )
    .await;
    assert_eq!(r["area"]["id"], converted.as_str());
    let (code, _) = mcp_err(
        &s,
        &t,
        "create_area",
        json!({"name": "Doppelt", "id": wanted}),
    )
    .await;
    assert_eq!(code, "conflict");

    // dev → standard is refused while dev tasks exist.
    let (code, _) = mcp_err(
        &s,
        &t,
        "update_project",
        json!({"project_id": dev, "kind": "standard", "repo": null}),
    )
    .await;
    assert_eq!(code, "dev_tasks_present");
    let (code, _) = mcp_err(
        &s,
        &t,
        "create_project",
        json!({"name": "Repo", "area_id": area, "repo": "a/b"}),
    )
    .await;
    assert_eq!(code, "dev_only");
    let (code, _) = mcp_err(
        &s,
        &t,
        "update_area",
        json!({"area_id": "01929c3e-0000-7000-8000-000000000001", "name": "X"}),
    )
    .await;
    assert_eq!(code, "not_found");

    // Effective location carries resolved names.
    let task = create(
        &s,
        &t,
        json!({"title": "Kisten", "location": {"kind": "project", "id": dev}}),
    )
    .await;
    let got = mcp_ok(&s, &t, "get_task", json!({"task_id": task})).await;
    assert_eq!(
        got["task"]["location"],
        json!({"kind": "project", "id": dev, "name": "Zunder", "area_id": area, "area_name": "Beruf"})
    );
}

#[tokio::test]
async fn clickup_ids_resolve() {
    let (s, t) = setup().await;
    // An imported task, pushed via the sync API like zunder-import does.
    let mut clock = Clock::new(NodeId::new("import").unwrap());
    let id = zunder_core::ids::clickup_entity_id(zunder_core::EntityKind::Task, "86abc123");
    let pushed = ops(&mut clock, |b| {
        b.create_task(NewTask {
            id: Some(id),
            clickup_id: Some("86abc123".into()),
            ..NewTask::new("Importiert")
        });
    });
    assert_eq!(
        s.push(Auth::Bearer(BOOTSTRAP), &pushed).await.status,
        StatusCode::OK
    );

    let got = mcp_ok(&s, &t, "get_task", json!({"clickup_id": "86abc123"})).await;
    assert_eq!(got["task"]["id"], id.to_string());
    assert_eq!(got["task"]["clickup_id"], "86abc123");

    let r = mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"clickup_id": "86abc123", "status": "done"}),
    )
    .await;
    assert_eq!(r["task"]["status"], "done");
    let r = mcp_ok(
        &s,
        &t,
        "claim_task",
        json!({"clickup_id": "86abc123", "agent": "a"}),
    )
    .await;
    assert_eq!(r["task"]["claimed_by"], "a");
    let hits = mcp_ok(&s, &t, "search", json!({"query": "86abc123"})).await;
    assert_eq!(hits["hits"][0]["id"], id.to_string());

    let (code, message) = mcp_err(&s, &t, "get_task", json!({"clickup_id": "unbekannt"})).await;
    assert_eq!(code, "not_found");
    assert!(message.contains("unbekannt"));
    let (_, message) = mcp_err(
        &s,
        &t,
        "get_task",
        json!({"clickup_id": "86abc123", "task_id": id.to_string()}),
    )
    .await;
    assert!(message.contains("nicht beides"), "{message}");

    let history = mcp_ok(
        &s,
        &t,
        "get_task_history",
        json!({"clickup_id": "86abc123"}),
    )
    .await;
    let sources: Vec<&str> = history["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["source"].as_str().unwrap())
        .collect();
    assert!(sources.contains(&"api:bootstrap"));
    assert!(sources.contains(&"mcp:claude"));
}

#[tokio::test]
async fn claims_conflict_and_release() {
    let (s, t) = setup().await;
    let task = create(&s, &t, json!({"title": "Sync bauen"})).await;

    let r = mcp_ok(
        &s,
        &t,
        "claim_task",
        json!({"task_id": task, "agent": "agent-a", "ttl_minutes": 30}),
    )
    .await;
    assert_eq!(r["task"]["claimed_by"], "agent-a");
    assert_eq!(r["task"]["claim_active"], true);
    let expires = r["task"]["claim_expires_at"].as_str().unwrap().to_owned();

    let (code, message) = mcp_err(
        &s,
        &t,
        "claim_task",
        json!({"task_id": task, "agent": "agent-b"}),
    )
    .await;
    assert_eq!(code, "conflict");
    assert!(message.contains("agent-a"), "{message}");

    // The holder may extend.
    let r = mcp_ok(
        &s,
        &t,
        "claim_task",
        json!({"task_id": task, "agent": "agent-a"}),
    )
    .await;
    assert!(r["task"]["claim_expires_at"].as_str().unwrap() > expires.as_str());

    let filtered = mcp_ok(&s, &t, "filter_tasks", json!({"claimed_by": "agent-a"})).await;
    assert_eq!(filtered["count"], 1);

    let (code, _) = mcp_err(
        &s,
        &t,
        "release_task",
        json!({"task_id": task, "agent": "agent-b"}),
    )
    .await;
    assert_eq!(code, "conflict");
    let (code, _) = mcp_err(&s, &t, "release_task", json!({"task_id": task})).await;
    assert_eq!(code, "invalid_arguments");
    let r = mcp_ok(
        &s,
        &t,
        "release_task",
        json!({"task_id": task, "agent": "agent-a"}),
    )
    .await;
    assert_eq!(r["released"], true);
    assert_eq!(r["task"]["claimed_by"], Value::Null);
    let r = mcp_ok(
        &s,
        &t,
        "release_task",
        json!({"task_id": task, "agent": "agent-a"}),
    )
    .await;
    assert_eq!(r["released"], false);

    // force releases a foreign claim
    mcp_ok(
        &s,
        &t,
        "claim_task",
        json!({"task_id": task, "agent": "agent-a"}),
    )
    .await;
    let r = mcp_ok(
        &s,
        &t,
        "release_task",
        json!({"task_id": task, "agent": "agent-b", "force": true}),
    )
    .await;
    assert_eq!(r["released"], true);

    // An expired foreign claim can be taken over.
    let id: zunder_core::ids::Id = task.parse().unwrap();
    // Client clock slightly ahead, so its writes win over the server's.
    let mut clock = common::client_clock();
    let mut batch = OpBatch::new(&mut clock, zunder_core::hlc::system_time_ms() + 5_000);
    batch.set_task(id, "claimed_by", "agent-a");
    batch.set_task(id, "claim_expires_at", "2020-01-01T00:00:00Z");
    s.push(Auth::Bearer(BOOTSTRAP), &batch.into_ops()).await;
    let filtered = mcp_ok(&s, &t, "filter_tasks", json!({"claimed_by": "agent-a"})).await;
    assert_eq!(filtered["count"], 0);
    let r = mcp_ok(
        &s,
        &t,
        "claim_task",
        json!({"task_id": task, "agent": "agent-b"}),
    )
    .await;
    assert_eq!(r["task"]["claimed_by"], "agent-b");
    let (code, _) = mcp_err(
        &s,
        &t,
        "claim_task",
        json!({"task_id": task, "agent": "agent-c", "ttl_minutes": 0}),
    )
    .await;
    assert_eq!(code, "invalid_arguments");
}

#[tokio::test]
async fn decompose_is_atomic_and_builds_a_tree() {
    let (s, t) = setup().await;
    let (_, dev, _) = structure(&s, &t).await;
    let parent = create(
        &s,
        &t,
        json!({"title": "Sync", "location": {"kind": "project", "id": dev}}),
    )
    .await;

    let r = mcp_ok(
        &s,
        &t,
        "decompose_task",
        json!({
            "task_id": parent,
            "subtasks": [
                {"title": "HLC"},
                {"title": "Merge", "type": "feature", "plan": "LWW je Feld"},
                {"title": "Tests", "status": "done"},
            ],
        }),
    )
    .await;
    assert_eq!(r["created"].as_array().unwrap().len(), 3);
    let task = &r["task"];
    assert_eq!(task["progress"], json!({"done": 1, "total": 3}));
    let subtasks = task["subtasks"].as_array().unwrap();
    assert_eq!(subtasks.len(), 3);
    let hlc = subtasks.iter().find(|t| t["title"] == "HLC").unwrap();
    assert_eq!(hlc["type"], "aufgabe");
    assert_eq!(hlc["parent_id"], parent.as_str());
    assert_eq!(hlc["location"]["id"], dev.as_str());
    assert_eq!(hlc["subtasks"], json!([]));

    // Nested: decompose a subtask; get_task shows the whole tree.
    let hlc_id = hlc["id"].as_str().unwrap();
    mcp_ok(
        &s,
        &t,
        "decompose_task",
        json!({"task_id": hlc_id, "subtasks": [{"title": "Zähler"}]}),
    )
    .await;
    let got = mcp_ok(&s, &t, "get_task", json!({"task_id": parent})).await;
    let hlc = got["task"]["subtasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "HLC")
        .unwrap()
        .clone();
    assert_eq!(hlc["subtasks"][0]["title"], "Zähler");

    // One invalid subtask aborts all of them.
    let inbox = create(&s, &t, json!({"title": "Inbox-Eltern"})).await;
    let (code, _) = mcp_err(
        &s,
        &t,
        "decompose_task",
        json!({"task_id": inbox, "subtasks": [{"title": "Gut"}, {"title": "Schlecht", "type": "bug"}]}),
    )
    .await;
    assert_eq!(code, "dev_only");
    let got = mcp_ok(&s, &t, "get_task", json!({"task_id": inbox})).await;
    assert_eq!(got["task"]["subtasks"], json!([]));

    let (_, message) = mcp_err(
        &s,
        &t,
        "decompose_task",
        json!({"task_id": inbox, "subtasks": [{"title": "A", "foo": 1}]}),
    )
    .await;
    assert!(message.starts_with("Unteraufgabe 1:"), "{message}");
    let (code, _) = mcp_err(
        &s,
        &t,
        "decompose_task",
        json!({"task_id": inbox, "subtasks": []}),
    )
    .await;
    assert_eq!(code, "invalid_arguments");

    // Closing with open subtasks is refused without force; nothing changes.
    for status in ["done", "dropped"] {
        let (code, message) = mcp_err(
            &s,
            &t,
            "update_task",
            json!({"task_id": parent, "status": status, "plan": "Fertig?"}),
        )
        .await;
        assert_eq!(code, "open_subtasks");
        assert!(message.contains("2 offene Unteraufgaben"), "{message}");
        assert!(
            message.contains("„HLC“") && message.contains("„Merge“"),
            "{message}"
        );
        assert!(!message.contains("„Tests“"), "{message}");
        assert!(message.contains("force: true"), "{message}");
    }
    let got = mcp_ok(&s, &t, "get_task", json!({"task_id": parent})).await;
    assert_eq!(got["task"]["status"], "open");
    assert_eq!(got["task"]["plan"], Value::Null);

    // Other changes and non-closing statuses need no force.
    mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": parent, "status": "in_progress"}),
    )
    .await;

    // A second call with force closes it; the core still warns.
    let r = mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": parent, "status": "done", "force": true}),
    )
    .await;
    assert_eq!(r["task"]["status"], "done");
    assert_eq!(r["warnings"][0]["code"], "open_subtasks");

    // Without open subtasks no force is needed.
    let (code, _) = mcp_err(
        &s,
        &t,
        "update_task",
        json!({"task_id": parent, "force": "ja"}),
    )
    .await;
    assert_eq!(code, "invalid_arguments");
    let r = mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": inbox, "status": "dropped"}),
    )
    .await;
    assert_eq!(r["task"]["status"], "dropped");
}

/// Pushes a task that was closed `days_ago` days ago (client clock in the
/// past).
async fn closed_task(s: &TestServer, title: &str, status: TaskStatus, days_ago: u64) {
    let mut clock = Clock::new(NodeId::new("web-alt").unwrap());
    let past = zunder_core::hlc::system_time_ms() - days_ago * DAY_MS;
    let mut batch = OpBatch::new(&mut clock, past);
    batch.create_task(NewTask::new(title).with_status(status));
    let r = s.push(Auth::Bearer(BOOTSTRAP), &batch.into_ops()).await;
    assert!(r.json()["rejected"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn search_and_filter_closed_tasks() {
    let (s, t) = setup().await;
    create(&s, &t, json!({"title": "Rechnung bezahlen"})).await;
    closed_task(&s, "Rechnung archivieren", TaskStatus::Done, 5).await;
    closed_task(&s, "Rechnung prüfen", TaskStatus::Dropped, 1).await;

    let r = mcp_ok(&s, &t, "filter_tasks", json!({})).await;
    let mut got = titles(&r);
    got.sort();
    assert_eq!(got, ["Rechnung bezahlen", "Rechnung prüfen"]);
    let r = mcp_ok(&s, &t, "filter_tasks", json!({"include_closed": true})).await;
    assert_eq!(r["count"], 3);

    let hits = mcp_ok(&s, &t, "search", json!({"query": "rechnung"})).await;
    assert_eq!(hits["count"], 3);
    let archived = hits["hits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["title"] == "Rechnung archivieren")
        .unwrap();
    assert_eq!(archived["entity"], "task");
    assert_eq!(archived["status"], "done");
    assert_eq!(archived["location"]["kind"], "inbox");

    let hits = mcp_ok(&s, &t, "search", json!({"query": "prufen", "limit": 1})).await;
    assert_eq!(hits["hits"][0]["title"], "Rechnung prüfen");
    let (code, _) = mcp_err(&s, &t, "search", json!({"query": " "})).await;
    assert_eq!(code, "invalid_arguments");

    // Projects and areas are hits too.
    structure(&s, &t).await;
    let hits = mcp_ok(&s, &t, "search", json!({"query": "zunder"})).await;
    assert_eq!(hits["hits"][0]["entity"], "project");
    assert_eq!(hits["hits"][0]["kind"], "dev");
}

#[tokio::test]
async fn filter_tasks_filters() {
    let (s, t) = setup().await;
    let (area, dev, std_project) = structure(&s, &t).await;
    let in_area = create(
        &s,
        &t,
        json!({"title": "A", "location": {"kind": "area", "id": area}, "due": "2026-10-15"}),
    )
    .await;
    let bug = create(
        &s,
        &t,
        json!({"title": "B", "type": "bug", "status": "review", "location": {"kind": "project", "id": dev}, "due": "2026-10-12T18:00:00+02:00"}),
    )
    .await;
    create(
        &s,
        &t,
        json!({"title": "C", "location": {"kind": "project", "id": std_project}, "phoenix": {"role": "feuer_schritt", "fire": "2026-W42"}}),
    )
    .await;
    create(
        &s,
        &t,
        json!({"title": "D", "location": {"kind": "loose"}, "status": "blocked"}),
    )
    .await;
    create(&s, &t, json!({"title": "E", "parent_id": bug})).await;

    let f = |args: Value| {
        let s = &s;
        let t = &t;
        async move { titles(&mcp_ok(s, t, "filter_tasks", args).await) }
    };
    assert_eq!(f(json!({"location": "area"})).await, ["A"]);
    assert_eq!(f(json!({"location": "loose"})).await, ["D"]);
    assert_eq!(f(json!({"area_id": area})).await, ["B", "A", "C", "E"]);
    assert_eq!(f(json!({"project_id": dev})).await, ["B", "E"]);
    assert_eq!(
        f(json!({"status": ["review", "blocked"]})).await,
        ["B", "D"]
    );
    assert_eq!(f(json!({"type": ["bug"]})).await, ["B"]);
    assert_eq!(f(json!({"phoenix_role": "feuer_schritt"})).await, ["C"]);
    assert_eq!(f(json!({"phoenix_fire": "2026-W42"})).await, ["C"]);
    assert!(f(json!({"phoenix_fire": "2026-W43"})).await.is_empty());
    assert_eq!(f(json!({"due_before": "2026-10-12"})).await, ["B"]);
    assert_eq!(f(json!({"due_after": "2026-10-13"})).await, ["A"]);
    assert_eq!(
        f(json!({"due_after": "2026-10-01", "due_before": "2026-10-31"})).await,
        ["B", "A"]
    );
    assert_eq!(f(json!({"parent_id": bug})).await, ["E"]);

    let r = mcp_ok(&s, &t, "filter_tasks", json!({"limit": 2})).await;
    assert_eq!(r["count"], 2);
    assert_eq!(r["total"], 5);
    assert_eq!(r["truncated"], true);
    assert_eq!(r["tasks"][0]["id"], bug.as_str());
    assert_eq!(r["tasks"][1]["id"], in_area.as_str());

    for bad in [
        json!({"status": ["fertig"]}),
        json!({"location": "irgendwo"}),
        json!({"due_before": "bald"}),
        json!({"phoenix_fire": "2026-42"}),
        json!({"limit": 0}),
        json!({"include_closed": "ja"}),
    ] {
        let (code, _) = mcp_err(&s, &t, "filter_tasks", bad.clone()).await;
        assert_eq!(code, "invalid_arguments", "{bad}");
    }
}

#[tokio::test]
async fn agents_comment_on_tasks() {
    let (s, t) = setup().await;
    let id = create(
        &s,
        &t,
        json!({"title": "Server umziehen", "description": "Plan"}),
    )
    .await;

    let first = mcp_ok(
        &s,
        &t,
        "add_comment",
        json!({"task_id": id, "body": "Stand: Backup läuft."}),
    )
    .await;
    // Without `author`, the token name signs the comment.
    assert_eq!(first["comment"]["author"], "claude");
    assert_eq!(first["comment"]["task_id"], id.as_str());
    assert_eq!(first["comment"]["body"], "Stand: Backup läuft.");
    assert!(first["comment"]["created_at"]
        .as_str()
        .unwrap()
        .ends_with('Z'));
    let second = mcp_ok(
        &s,
        &t,
        "add_comment",
        json!({"task_id": id, "body": "Fertig, DNS umgestellt.", "author": " claude-code@laptop "}),
    )
    .await;
    assert_eq!(second["comment"]["author"], "claude-code@laptop");

    // get_task lists them oldest first; the description is untouched.
    let got = mcp_ok(&s, &t, "get_task", json!({"task_id": id})).await;
    assert_eq!(got["task"]["description"], "Plan");
    let comments = got["task"]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0]["id"], first["comment"]["id"]);
    assert_eq!(comments[1]["id"], second["comment"]["id"]);
    // Other views of the task carry no comments.
    let listed = mcp_ok(&s, &t, "filter_tasks", json!({})).await;
    assert!(listed["tasks"][0].get("comments").is_none());

    let (code, _) = mcp_err(&s, &t, "add_comment", json!({"task_id": id, "body": "  "})).await;
    assert_eq!(code, "invalid_value");
    let (code, _) = mcp_err(&s, &t, "add_comment", json!({"task_id": id})).await;
    assert_eq!(code, "invalid_arguments");
    let (code, _) = mcp_err(
        &s,
        &t,
        "add_comment",
        json!({"task_id": id, "body": "x", "author": "a".repeat(201)}),
    )
    .await;
    assert_eq!(code, "invalid_arguments");

    // Deleting the task hides its comments; new ones are refused.
    mcp_ok(&s, &t, "delete_task", json!({"task_id": id})).await;
    let (code, _) = mcp_err(
        &s,
        &t,
        "add_comment",
        json!({"task_id": id, "body": "zu spät"}),
    )
    .await;
    assert_eq!(code, "not_found");
}

#[tokio::test]
async fn move_and_delete() {
    let (s, t) = setup().await;
    let (area, _, std_project) = structure(&s, &t).await;
    let parent = create(
        &s,
        &t,
        json!({"title": "Eltern", "location": {"kind": "project", "id": std_project}}),
    )
    .await;
    let child = create(&s, &t, json!({"title": "Kind"})).await;

    // Become a subtask: effective location is the parent's.
    let r = mcp_ok(
        &s,
        &t,
        "move_task",
        json!({"task_id": child, "parent_id": parent}),
    )
    .await;
    assert_eq!(r["task"]["parent_id"], parent.as_str());
    assert_eq!(r["task"]["location"]["id"], std_project.as_str());

    // Detach: keeps the effective location.
    let r = mcp_ok(
        &s,
        &t,
        "move_task",
        json!({"task_id": child, "parent_id": null}),
    )
    .await;
    assert_eq!(r["task"]["parent_id"], Value::Null);
    assert_eq!(r["task"]["location"]["id"], std_project.as_str());

    // Move by location (detaches from a parent).
    mcp_ok(
        &s,
        &t,
        "move_task",
        json!({"task_id": child, "parent_id": parent}),
    )
    .await;
    let r = mcp_ok(
        &s,
        &t,
        "move_task",
        json!({"task_id": child, "location": {"kind": "area", "id": area}}),
    )
    .await;
    assert_eq!(r["task"]["parent_id"], Value::Null);
    assert_eq!(r["task"]["location"]["kind"], "area");

    let (code, _) = mcp_err(
        &s,
        &t,
        "move_task",
        json!({"task_id": parent, "parent_id": parent}),
    )
    .await;
    assert_eq!(code, "cycle");
    let (code, _) = mcp_err(&s, &t, "move_task", json!({"task_id": parent})).await;
    assert_eq!(code, "invalid_arguments");

    // Delete cascades to subtasks.
    mcp_ok(
        &s,
        &t,
        "move_task",
        json!({"task_id": child, "parent_id": parent}),
    )
    .await;
    let r = mcp_ok(&s, &t, "delete_task", json!({"task_id": parent})).await;
    assert_eq!(r["count"], 2);
    let (code, message) = mcp_err(&s, &t, "get_task", json!({"task_id": child})).await;
    assert_eq!(code, "not_found");
    assert!(message.contains("gelöscht"));
    let (code, _) = mcp_err(&s, &t, "delete_task", json!({"task_id": parent})).await;
    assert_eq!(code, "not_found");
    let r = mcp_ok(&s, &t, "filter_tasks", json!({"include_closed": true})).await;
    assert_eq!(r["count"], 0);
    // History of deleted tasks stays readable.
    let history = mcp_ok(&s, &t, "get_task_history", json!({"task_id": parent})).await;
    assert!(history["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["field"] == "deleted" && e["value"] == true));
}

#[tokio::test]
async fn mcp_changes_reach_sync_clients() {
    let (s, t) = setup().await;
    let before = s.app.cursor();
    let mut events = s.app.subscribe();
    let id = create(&s, &t, json!({"title": "Neu"})).await;
    assert!(events.try_recv().unwrap() > before);
    let pulled = s
        .get(
            &format!("/api/sync/pull?since={before}"),
            Auth::Bearer(BOOTSTRAP),
        )
        .await
        .json();
    let ops = pulled["ops"].as_array().unwrap();
    assert!(!ops.is_empty());
    assert!(ops.iter().all(|o| o["op"]["entity_id"] == id.as_str()));
    assert!(ops
        .iter()
        .all(|o| o["op"]["hlc"].as_str().unwrap().ends_with(":server")));
}

#[tokio::test]
async fn tool_errors_do_not_break_batches() {
    let (s, t) = setup().await;
    let r = mcp_post(
        &s,
        &t,
        json!([
            rpc(
                1,
                "tools/call",
                json!({"name": "get_task", "arguments": {}})
            ),
            rpc(
                2,
                "tools/call",
                json!({"name": "create_task", "arguments": {"title": "Ok"}})
            ),
        ]),
    )
    .await;
    let replies = r.json();
    let replies = replies.as_array().unwrap();
    tool_err(&replies[0]["result"]);
    assert_eq!(replies[1]["result"]["isError"], false);
    let result = mcp_call(&s, &t, "list_structure", json!({})).await;
    assert_eq!(result["isError"], false);
}

/// Pushes an imported task with a ClickUp ID like zunder-import does.
async fn imported_task(s: &TestServer, clickup_id: &str, title: &str) -> String {
    let mut clock = Clock::new(NodeId::new("import").unwrap());
    let id = zunder_core::ids::clickup_entity_id(zunder_core::EntityKind::Task, clickup_id);
    let pushed = ops(&mut clock, |b| {
        b.create_task(NewTask {
            id: Some(id),
            clickup_id: Some(clickup_id.into()),
            ..NewTask::new(title)
        });
    });
    assert_eq!(
        s.push(Auth::Bearer(BOOTSTRAP), &pushed).await.status,
        StatusCode::OK
    );
    id.to_string()
}

#[tokio::test]
async fn parent_id_accepts_clickup_ids() {
    let (s, t) = setup().await;
    let parent = imported_task(&s, "86abc1", "Importierte Eltern").await;

    let child = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "Kind", "parent_id": "86abc1"}),
    )
    .await;
    assert_eq!(child["task"]["parent_id"], parent.as_str());

    let loose = create(&s, &t, json!({"title": "Lose"})).await;
    let moved = mcp_ok(
        &s,
        &t,
        "move_task",
        json!({"task_id": loose, "parent_id": "86abc1"}),
    )
    .await;
    assert_eq!(moved["task"]["parent_id"], parent.as_str());

    let r = mcp_ok(&s, &t, "filter_tasks", json!({"parent_id": "86abc1"})).await;
    let mut got = titles(&r);
    got.sort();
    assert_eq!(got, ["Kind", "Lose"]);
    // The UUID still works and an unknown ClickUp ID simply matches nothing.
    let r = mcp_ok(&s, &t, "filter_tasks", json!({"parent_id": parent})).await;
    assert_eq!(r["count"], 2);
    let r = mcp_ok(&s, &t, "filter_tasks", json!({"parent_id": "unbekannt"})).await;
    assert_eq!(r["count"], 0);

    let (code, message) = mcp_err(
        &s,
        &t,
        "create_task",
        json!({"title": "X", "parent_id": "unbekannt"}),
    )
    .await;
    assert_eq!(code, "not_found");
    assert!(message.contains("unbekannt"), "{message}");
    let (code, _) = mcp_err(
        &s,
        &t,
        "move_task",
        json!({"task_id": loose, "parent_id": "unbekannt"}),
    )
    .await;
    assert_eq!(code, "not_found");
    // A missing parent given as UUID is still checked by the core.
    let (code, _) = mcp_err(
        &s,
        &t,
        "create_task",
        json!({"title": "X", "parent_id": "01929c3e-0000-7000-8000-000000000001"}),
    )
    .await;
    assert_eq!(code, "invalid_reference");
    let (code, _) = mcp_err(
        &s,
        &t,
        "create_task",
        json!({"title": "X", "parent_id": " "}),
    )
    .await;
    assert_eq!(code, "invalid_arguments");
}

#[tokio::test]
async fn task_reference_errors_are_invalid_arguments() {
    let (s, t) = setup().await;
    let id = create(&s, &t, json!({"title": "Ziel"})).await;
    for (tool, args) in [
        ("get_task", json!({})),
        ("get_task", json!({"task_id": id, "clickup_id": "86abc1"})),
        ("get_task", json!({"clickup_id": ""})),
        ("update_task", json!({"title": "Neu"})),
        ("start_flame", json!({})),
    ] {
        let (code, message) = mcp_err(&s, &t, tool, args.clone()).await;
        assert_eq!(code, "invalid_arguments", "{tool} {args}: {message}");
    }
}

#[tokio::test]
async fn filter_tasks_pages_orders_and_filters_by_closing_date() {
    let (s, t) = setup().await;
    for days in [10, 9, 8, 2, 1] {
        closed_task(&s, &format!("D{days}"), TaskStatus::Done, days).await;
    }
    create(&s, &t, json!({"title": "Offen"})).await;
    let base = json!({"include_closed": true, "status": ["done"], "limit": 2});
    let page = |offset: u64| {
        let mut args = base.clone();
        args["offset"] = json!(offset);
        let (s, t) = (&s, &t);
        async move { mcp_ok(s, t, "filter_tasks", args).await }
    };
    let r = page(0).await;
    assert_eq!(titles(&r), ["D10", "D9"]);
    assert_eq!(r["total"], 5);
    assert_eq!(r["truncated"], true);
    assert_eq!(r["next_offset"], 2);
    let r = page(2).await;
    assert_eq!(titles(&r), ["D8", "D2"]);
    assert_eq!(r["next_offset"], 4);
    let r = page(4).await;
    assert_eq!(titles(&r), ["D1"]);
    assert_eq!(r["truncated"], false);
    assert_eq!(r["next_offset"], Value::Null);
    assert!(titles(&page(10).await).is_empty());

    // Newest completions first.
    let r = mcp_ok(
        &s,
        &t,
        "filter_tasks",
        json!({"include_closed": true, "status": ["done"], "order_by": "status_changed", "reverse": true, "limit": 3}),
    )
    .await;
    assert_eq!(titles(&r), ["D1", "D2", "D8"]);

    // Closing dates imply include_closed and only match closed tasks.
    let now = zunder_core::hlc::system_time_ms();
    let ago = |days: u64| zunder_server::app::format_ms((now - days * DAY_MS) as i64);
    let f = |args: Value| {
        let (s, t) = (&s, &t);
        async move { titles(&mcp_ok(s, t, "filter_tasks", args).await) }
    };
    assert_eq!(f(json!({"closed_after": ago(3)})).await, ["D2", "D1"]);
    assert_eq!(
        f(json!({"closed_before": ago(5)})).await,
        ["D10", "D9", "D8"]
    );
    assert_eq!(
        f(json!({"closed_after": ago(9), "closed_before": ago(5)})).await,
        ["D8"]
    );
    assert_eq!(f(json!({"closed_after": "2000-01-01"})).await.len(), 5);
    assert!(f(json!({"closed_before": "2000-01-01"})).await.is_empty());

    for bad in [
        json!({"offset": -1}),
        json!({"order_by": "titel"}),
        json!({"reverse": "ja"}),
        json!({"closed_after": "neulich"}),
    ] {
        let (code, _) = mcp_err(&s, &t, "filter_tasks", bad.clone()).await;
        assert_eq!(code, "invalid_arguments", "{bad}");
    }
}

#[tokio::test]
async fn running_flame_can_be_found_and_stopped_without_its_task() {
    let (s, t) = setup().await;
    let (code, message) = mcp_err(&s, &t, "stop_flame", json!({})).await;
    assert_eq!(code, "conflict");
    assert!(message.contains("keine Flamme"), "{message}");

    let a = create(&s, &t, json!({"title": "A"})).await;
    create(&s, &t, json!({"title": "B"})).await;
    mcp_ok(&s, &t, "start_flame", json!({"task_id": a})).await;
    // Closing the task does not hide its running flame.
    mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": a, "status": "done"}),
    )
    .await;
    let r = mcp_ok(&s, &t, "filter_tasks", json!({"flame_running": true})).await;
    assert_eq!(titles(&r), ["A"]);
    let r = mcp_ok(&s, &t, "filter_tasks", json!({"flame_running": false})).await;
    assert_eq!(titles(&r), ["B"]);

    let r = mcp_ok(&s, &t, "stop_flame", json!({})).await;
    assert_eq!(r["running"], false);
    assert_eq!(r["task"]["id"], a.as_str());
    let r = mcp_ok(&s, &t, "filter_tasks", json!({"flame_running": true})).await;
    assert_eq!(r["count"], 0);
    let (code, _) = mcp_err(&s, &t, "stop_flame", json!({})).await;
    assert_eq!(code, "conflict");
}

#[tokio::test]
async fn undated_tasks_can_be_acknowledged() {
    let (s, t) = setup().await;
    let open = create(&s, &t, json!({"title": "A"})).await;
    let marked = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "B", "undated_ack": true}),
    )
    .await;
    assert!(marked["task"]["undated_ack"].is_string());
    create(&s, &t, json!({"title": "C", "due": "2026-10-20"})).await;
    // A mark from 20 weeks ago (client clock and value in the past).
    let old = create(&s, &t, json!({"title": "D"})).await;
    let long_ago = zunder_core::hlc::system_time_ms() - 20 * 7 * DAY_MS;
    let mut clock = Clock::new(NodeId::new("web-alt").unwrap());
    let mut batch = OpBatch::new(&mut clock, long_ago);
    batch.set_task(
        zunder_core::ids::parse_id(&old).unwrap(),
        "undated_ack",
        zunder_core::model::format_rfc3339(
            time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(long_ago) * 1_000_000)
                .unwrap(),
        ),
    );
    let r = s.push(Auth::Bearer(BOOTSTRAP), &batch.into_ops()).await;
    assert!(r.json()["rejected"].as_array().unwrap().is_empty());

    let f = |args: Value| {
        let s = &s;
        let t = &t;
        // Sorted: the old mark makes D the oldest task (smallest HLC).
        async move {
            let mut got = titles(&mcp_ok(s, t, "filter_tasks", args).await);
            got.sort();
            got
        }
    };
    assert_eq!(f(json!({"has_due": true})).await, ["C"]);
    assert_eq!(f(json!({"has_due": false})).await, ["A", "B", "D"]);
    assert_eq!(f(json!({"undated_ack": true})).await, ["B", "D"]);
    assert_eq!(
        f(json!({"has_due": false, "undated_ack": false})).await,
        ["A"]
    );
    // Wiedervorlage: an old mark counts as expired.
    assert_eq!(
        f(json!({"has_due": false, "undated_ack": false, "undated_ack_weeks": 12})).await,
        ["A", "D"]
    );
    assert_eq!(
        f(json!({"undated_ack": true, "undated_ack_weeks": 12})).await,
        ["B"]
    );
    assert_eq!(
        f(json!({"undated_ack": true, "undated_ack_weeks": 21})).await,
        ["B", "D"]
    );

    // true marks now (and renews an old mark), false and null remove it.
    let r = mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": open, "undated_ack": true}),
    )
    .await;
    assert!(r["task"]["undated_ack"].is_string());
    let r = mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": old, "undated_ack": true}),
    )
    .await;
    let renewed = r["task"]["undated_ack"].as_str().unwrap();
    let renewed =
        time::OffsetDateTime::parse(renewed, &time::format_description::well_known::Rfc3339)
            .unwrap();
    assert!(renewed.unix_timestamp_nanos() / 1_000_000 > i128::from(long_ago));
    assert_eq!(
        f(json!({"undated_ack": true, "undated_ack_weeks": 1})).await,
        ["A", "B", "D"]
    );
    let r = mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": open, "undated_ack": false}),
    )
    .await;
    assert_eq!(r["task"]["undated_ack"], Value::Null);
    let r = mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": old, "undated_ack": null}),
    )
    .await;
    assert_eq!(r["task"]["undated_ack"], Value::Null);
    assert_eq!(f(json!({"undated_ack": true})).await, ["B"]);

    let (code, _) = mcp_err(
        &s,
        &t,
        "update_task",
        json!({"task_id": open, "undated_ack": "2026-10-12T09:00:00Z"}),
    )
    .await;
    assert_eq!(code, "invalid_arguments");
    for bad in [
        json!({"undated_ack_weeks": 4}),
        json!({"undated_ack": false, "undated_ack_weeks": 0}),
        json!({"undated_ack": false, "undated_ack_weeks": 521}),
        json!({"has_due": "nein"}),
    ] {
        let (code, _) = mcp_err(&s, &t, "filter_tasks", bad.clone()).await;
        assert_eq!(code, "invalid_arguments", "{bad}");
    }
}

#[tokio::test]
async fn blocked_by_in_both_directions() {
    let (s, t) = setup().await;
    let (_, _, project) = structure(&s, &t).await;
    mcp_ok(
        &s,
        &t,
        "update_project",
        json!({"project_id": project, "key": "UMZ"}),
    )
    .await;
    let offer = create(
        &s,
        &t,
        json!({"title": "Angebot einholen", "location": {"kind": "project", "id": project}}),
    )
    .await;
    let imported = imported_task(&s, "86blk1", "Kartons").await;

    // References, ClickUp IDs and IDs are resolved; repeats dropped.
    let r = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "Vertrag", "blocked_by": ["umz-1", "86blk1", offer]}),
    )
    .await;
    let contract = r["task"]["id"].as_str().unwrap().to_owned();
    let blockers = r["task"]["blocked_by"].as_array().unwrap();
    assert_eq!(blockers.len(), 2);
    assert_eq!(blockers[0]["id"], offer.as_str());
    assert_eq!(blockers[0]["ref"], "UMZ-1");
    assert_eq!(blockers[0]["title"], "Angebot einholen");
    assert_eq!(blockers[0]["status"], "open");
    assert_eq!(blockers[1]["id"], imported.as_str());

    // The reverse direction is derived.
    let r = mcp_ok(&s, &t, "get_task", json!({"task_id": "UMZ-1"})).await;
    assert_eq!(r["task"]["blocks"][0]["id"], contract.as_str());
    assert_eq!(r["task"]["blocked_by"], json!([]));
    let r = mcp_ok(&s, &t, "filter_tasks", json!({"location": "project"})).await;
    let listed = r["tasks"].as_array().unwrap();
    assert_eq!(listed[0]["blocks"][0]["title"], "Vertrag");

    // Waiting for each other is refused with the core's message.
    let (code, message) = mcp_err(
        &s,
        &t,
        "update_task",
        json!({"task_id": offer, "blocked_by": [contract]}),
    )
    .await;
    assert_eq!(code, "cycle");
    assert!(message.contains("Vertrag"), "{message}");
    let (code, _) = mcp_err(
        &s,
        &t,
        "update_task",
        json!({"task_id": offer, "blocked_by": ["UMZ-99"]}),
    )
    .await;
    assert_eq!(code, "not_found");
    let (code, _) = mcp_err(
        &s,
        &t,
        "update_task",
        json!({"task_id": offer, "blocked_by": "UMZ-1"}),
    )
    .await;
    assert_eq!(code, "invalid_arguments");

    // A deleted blocker no longer blocks; null clears the list.
    mcp_ok(&s, &t, "delete_task", json!({"clickup_id": "86blk1"})).await;
    let r = mcp_ok(&s, &t, "get_task", json!({"task_id": contract})).await;
    assert_eq!(r["task"]["blocked_by"].as_array().unwrap().len(), 1);
    let r = mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": contract, "blocked_by": null}),
    )
    .await;
    assert_eq!(r["task"]["blocked_by"], json!([]));
    let r = mcp_ok(&s, &t, "get_task", json!({"task_id": "UMZ-1"})).await;
    assert_eq!(r["task"]["blocks"], json!([]));
}

#[tokio::test]
async fn status_follows_blockers() {
    let (s, t) = setup().await;
    let first = create(&s, &t, json!({"title": "Angebot einholen"})).await;
    let second = create(&s, &t, json!({"title": "Kartons kaufen"})).await;

    // A task with open blockers starts blocked; the server writes the status.
    let r = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "Vertrag", "blocked_by": [first, second]}),
    )
    .await;
    assert_eq!(r["task"]["status"], "blocked");
    let contract = r["task"]["id"].as_str().unwrap().to_owned();
    let r = mcp_ok(&s, &t, "get_task_history", json!({"task_id": contract})).await;
    let last = r["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["field"], "status");
    assert_eq!(last["value"], "blocked");
    assert_eq!(last["source"], "server");

    // Resolving one blocker is not enough, the last one opens it.
    mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": first, "status": "done"}),
    )
    .await;
    let r = mcp_ok(&s, &t, "get_task", json!({"task_id": contract})).await;
    assert_eq!(r["task"]["status"], "blocked");
    mcp_ok(&s, &t, "delete_task", json!({"task_id": second})).await;
    let r = mcp_ok(&s, &t, "get_task", json!({"task_id": contract})).await;
    assert_eq!(r["task"]["status"], "open");

    // Reopening a blocker blocks it again.
    mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": first, "status": "open"}),
    )
    .await;
    let r = mcp_ok(&s, &t, "get_task", json!({"task_id": contract})).await;
    assert_eq!(r["task"]["status"], "blocked");
}
