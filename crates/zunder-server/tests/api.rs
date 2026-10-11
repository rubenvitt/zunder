//! Integration tests against the HTTP API (router via `oneshot`).

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use zunder_core::{EntityKind, NewTask, Op, TaskStatus};
use zunder_server::Source;

use common::{client_clock, ops, Auth, TestServer, BOOTSTRAP};

const BOOT: Auth<'static> = Auth::Bearer(BOOTSTRAP);

fn new_task_ops(title: &str) -> (uuid::Uuid, Vec<Op>) {
    let mut clock = client_clock();
    let mut id = uuid::Uuid::nil();
    let ops = ops(&mut clock, |b| id = b.create_task(NewTask::new(title)));
    (id, ops)
}

fn op_ids(ops: &[Op]) -> Value {
    json!(ops.iter().map(|o| o.op_id.to_string()).collect::<Vec<_>>())
}

#[tokio::test]
async fn healthz_without_auth() {
    let s = TestServer::new().await;
    let r = s.get("/healthz", Auth::None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["status"], "ok");
    assert_eq!(r.json()["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn api_requires_credentials() {
    let s = TestServer::new().await;
    for path in [
        "/api/me",
        "/api/snapshot",
        "/api/sync/pull",
        "/api/sync/events",
        "/api/tokens",
    ] {
        let r = s.get(path, Auth::None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(r.error_code(), "unauthorized");
        assert!(r.json()["error"]["message"].is_string());
    }
    let r = s.get("/api/snapshot", Auth::Bearer("zt_wrong")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = s
        .get("/api/snapshot", Auth::Cookie("unknown-session"))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = s.push(Auth::None, &new_task_ops("x").1).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn push_pull_snapshot() {
    let s = TestServer::new().await;
    let (task, ops) = new_task_ops("Zahnarzt anrufen");
    let r = s.push(BOOT, &ops).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let body = r.json();
    assert_eq!(body["accepted"], op_ids(&ops));
    assert_eq!(body["rejected"], json!([]));
    assert_eq!(body["warnings"], json!([]));
    assert_eq!(body["cursor"], ops.len() as i64);

    let snap = s.get("/api/snapshot", BOOT).await.json();
    assert_eq!(snap["cursor"], ops.len() as i64);
    let entities = snap["entities"].as_array().unwrap();
    assert_eq!(entities.len(), 1);
    let e = &entities[0];
    assert_eq!(e["entity"], "task");
    assert_eq!(e["entity_id"], task.to_string());
    assert_eq!(e["created_hlc"], ops[0].hlc.to_string());
    assert_eq!(e["fields"]["title"]["value"], "Zahnarzt anrufen");
    assert_eq!(e["fields"]["title"]["op_id"], ops[0].op_id.to_string());
    assert_eq!(e["fields"]["status"]["value"], "open");

    let pull = s.get("/api/sync/pull?since=0", BOOT).await.json();
    assert_eq!(pull["has_more"], false);
    assert_eq!(pull["cursor"], ops.len() as i64);
    let pulled = pull["ops"].as_array().unwrap();
    assert_eq!(pulled.len(), ops.len());
    for (i, (entry, op)) in pulled.iter().zip(&ops).enumerate() {
        assert_eq!(entry["seq"], i as i64 + 1);
        assert_eq!(entry["op"], serde_json::to_value(op).unwrap());
    }

    let pull = s
        .get(&format!("/api/sync/pull?since={}", ops.len()), BOOT)
        .await
        .json();
    assert_eq!(pull["ops"], json!([]));
    assert_eq!(pull["cursor"], ops.len() as i64);
    assert_eq!(pull["has_more"], false);
}

#[tokio::test]
async fn push_is_idempotent() {
    let s = TestServer::new().await;
    let (_, ops) = new_task_ops("Einmal");
    let first = s.push(BOOT, &ops).await.json();
    let again = s.push(BOOT, &ops).await.json();
    assert_eq!(again["accepted"], op_ids(&ops));
    assert_eq!(again["rejected"], json!([]));
    assert_eq!(again["cursor"], first["cursor"]);
    let pull = s.get("/api/sync/pull", BOOT).await.json();
    assert_eq!(pull["ops"].as_array().unwrap().len(), ops.len());
}

#[tokio::test]
async fn rejections_carry_reasons_and_nothing_is_stored() {
    let s = TestServer::new().await;
    let (task, mut ops) = new_task_ops("Kaputt");
    ops[2].value = json!("erledigt"); // status: not a valid code
    let r = s.push(BOOT, &ops).await;
    assert_eq!(r.status, StatusCode::OK);
    let body = r.json();
    assert_eq!(body["accepted"], json!([]));
    let rejected = body["rejected"].as_array().unwrap();
    assert_eq!(rejected.len(), ops.len());
    let status_rejection = rejected
        .iter()
        .find(|r| r["op_id"] == ops[2].op_id.to_string())
        .unwrap();
    assert_eq!(status_rejection["code"], "invalid_value");
    assert!(!status_rejection["message"].as_str().unwrap().is_empty());
    assert!(rejected
        .iter()
        .filter(|r| r["op_id"] != ops[2].op_id.to_string())
        .all(|r| r["code"] == "entity_rejected"));
    assert_eq!(body["cursor"], 0);
    assert!(s.app.read(|st| st.task(task).is_none()));
    let pull = s.get("/api/sync/pull", BOOT).await.json();
    assert_eq!(pull["ops"], json!([]));
}

#[tokio::test]
async fn future_hlc_is_rejected() {
    let s = TestServer::new().await;
    let mut clock = client_clock();
    let far_future = zunder_core::hlc::system_time_ms() + 2 * 24 * 60 * 60 * 1000;
    let mut batch = zunder_core::OpBatch::new(&mut clock, far_future);
    batch.create_task(NewTask::new("Zukunft"));
    let ops = batch.into_ops();
    let body = s.push(BOOT, &ops).await.json();
    assert!(body["rejected"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["code"] == "future_hlc"));
}

#[tokio::test]
async fn warnings_are_returned() {
    let s = TestServer::new().await;
    let mut clock = client_clock();
    let mut parent = uuid::Uuid::nil();
    let create = ops(&mut clock, |b| {
        parent = b.create_task(NewTask::new("Eltern"));
        b.create_task(NewTask::subtask("Kind", parent));
    });
    assert_eq!(s.push(BOOT, &create).await.json()["rejected"], json!([]));
    let done = ops(&mut clock, |b| {
        b.set_status(parent, TaskStatus::Done);
    });
    let body = s.push(BOOT, &done).await.json();
    assert_eq!(body["accepted"], op_ids(&done));
    assert_eq!(body["warnings"][0]["entity_id"], parent.to_string());
    assert_eq!(body["warnings"][0]["code"], "open_subtasks");
}

#[tokio::test]
async fn malformed_push_is_a_bad_request() {
    let s = TestServer::new().await;
    let r = s
        .post(
            "/api/sync/push",
            BOOT,
            json!({"client_id": "x", "ops": [{"op_id": "nope"}]}),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.error_code(), "invalid_request");

    let req = Request::post("/api/sync/push")
        .header(header::AUTHORIZATION, format!("Bearer {BOOTSTRAP}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{kein json"))
        .unwrap();
    let r = s.send(req).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.error_code(), "invalid_request");

    let r = s.get("/api/sync/pull?since=-1", BOOT).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = s.get("/api/sync/pull?limit=abc", BOOT).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pull_pages_with_has_more() {
    let s = TestServer::new().await;
    let (_, ops) = new_task_ops("Seiten");
    assert_eq!(ops.len(), 4);
    s.push(BOOT, &ops).await;

    let mut since = 0;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let page = s
            .get(&format!("/api/sync/pull?since={since}&limit=3"), BOOT)
            .await
            .json();
        pages += 1;
        for entry in page["ops"].as_array().unwrap() {
            seen.push(entry["seq"].as_i64().unwrap());
        }
        since = page["cursor"].as_i64().unwrap();
        if !page["has_more"].as_bool().unwrap() {
            break;
        }
        assert_eq!(since, *seen.last().unwrap());
    }
    assert_eq!(pages, 2);
    assert_eq!(seen, vec![1, 2, 3, 4]);
    assert_eq!(since, 4);

    // exactly `limit` remaining ops: has_more is false
    let page = s.get("/api/sync/pull?since=1&limit=3", BOOT).await.json();
    assert_eq!(page["has_more"], false);
    assert_eq!(page["cursor"], 4);
}

async fn next_sse_data(body: &mut Body) -> String {
    let mut buffer = String::new();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("SSE event within 5 s")
            .expect("stream open")
            .unwrap();
        if let Ok(data) = frame.into_data() {
            buffer.push_str(std::str::from_utf8(&data).unwrap());
            if buffer.ends_with("\n\n") {
                if buffer.starts_with(':') {
                    buffer.clear(); // keep-alive comment
                    continue;
                }
                return buffer;
            }
        }
    }
}

#[tokio::test]
async fn sse_emits_cursor_on_change() {
    let s = TestServer::new().await;
    let req = Request::get("/api/sync/events")
        .header(header::AUTHORIZATION, format!("Bearer {BOOTSTRAP}"))
        .header(header::ACCEPT_ENCODING, "gzip, br")
        .body(Body::empty())
        .unwrap();
    let response = s.router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
    let mut body = response.into_body();
    assert_eq!(next_sse_data(&mut body).await, "event: cursor\ndata: 0\n\n");

    let (_, ops) = new_task_ops("Live");
    s.push(BOOT, &ops).await;
    assert_eq!(
        next_sse_data(&mut body).await,
        format!("event: cursor\ndata: {}\n\n", ops.len())
    );

    // A push without new operations does not notify.
    s.push(BOOT, &ops).await;
    let (_, more) = new_task_ops("Noch eine");
    s.push(BOOT, &more).await;
    assert_eq!(
        next_sse_data(&mut body).await,
        format!("event: cursor\ndata: {}\n\n", ops.len() + more.len())
    );

    // Shutdown ends the stream.
    s.app.shutdown_token().cancel();
    let end = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .expect("stream ends on shutdown");
    assert!(end.is_none());
}

#[tokio::test]
async fn tokens_create_use_revoke() {
    let s = TestServer::new().await;
    let session = s.session().await;

    let r = s
        .post(
            "/api/tokens",
            Auth::Web(&session),
            json!({"name": "claude"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let created = r.json();
    let token = created["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("zt_"));
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["name"], "claude");
    assert!(created["last_used_at"].is_null());

    // duplicate name
    let r = s
        .post(
            "/api/tokens",
            Auth::Web(&session),
            json!({"name": "claude"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = s
        .post("/api/tokens", Auth::Web(&session), json!({"name": "  "}))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // use it
    let r = s.get("/api/snapshot", Auth::Bearer(&token)).await;
    assert_eq!(r.status, StatusCode::OK);
    let (task, ops) = new_task_ops("Per Token");
    let r = s.push(Auth::Bearer(&token), &ops).await;
    assert_eq!(r.json()["rejected"], json!([]));
    let history = s
        .get(&format!("/api/tasks/{task}/history"), Auth::Bearer(&token))
        .await
        .json();
    assert_eq!(history[0]["source"], "api:claude");

    // listing never contains secrets, last_used_at is set
    let list = s.get("/api/tokens", Auth::Cookie(&session)).await.json();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], id);
    assert!(list[0]["last_used_at"].is_string());
    assert!(list[0].get("token").is_none());
    assert!(!list.to_string().contains(&token[3..]));

    // tokens cannot manage tokens
    let r = s.get("/api/tokens", Auth::Bearer(&token)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = s
        .post("/api/tokens", Auth::Bearer(&token), json!({"name": "x"}))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // revoke
    let r = s
        .request(
            Method::DELETE,
            &format!("/api/tokens/{id}"),
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = s.get("/api/snapshot", Auth::Bearer(&token)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = s
        .request(
            Method::DELETE,
            &format!("/api/tokens/{id}"),
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let list = s.get("/api/tokens", Auth::Cookie(&session)).await.json();
    assert_eq!(list, json!([]));

    // the name is free again
    let r = s
        .post(
            "/api/tokens",
            Auth::Web(&session),
            json!({"name": "claude"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
}

#[tokio::test]
async fn csrf_header_required_for_cookie_writes() {
    let s = TestServer::new().await;
    let session = s.session().await;
    let (_, ops) = new_task_ops("CSRF");

    let r = s.push(Auth::Cookie(&session), &ops).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.error_code(), "forbidden");
    assert!(s.app.cursor() == 0);
    let r = s
        .post("/api/tokens", Auth::Cookie(&session), json!({"name": "x"}))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // reads need no header
    let r = s.get("/api/snapshot", Auth::Cookie(&session)).await;
    assert_eq!(r.status, StatusCode::OK);

    let r = s.push(Auth::Web(&session), &ops).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["accepted"], op_ids(&ops));

    // bearer tokens are not subject to the CSRF rule
    let (_, more) = new_task_ops("Bearer");
    let r = s.push(BOOT, &more).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn me_and_logout() {
    let s = TestServer::new().await;
    let session = s.session().await;
    let me = s.get("/api/me", Auth::Cookie(&session)).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json()["user_id"], s.app.owner_id().to_string());

    let r = s
        .request(Method::POST, "/auth/logout", Auth::Cookie(&session), None)
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "logout needs the header");

    let r = s
        .request(Method::POST, "/auth/logout", Auth::Web(&session), None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let cleared = r.set_cookies().join("\n");
    assert!(cleared.contains("zunder_session=;"), "{cleared}");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    let r = s.get("/api/me", Auth::Cookie(&session)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dev_auth_logs_in_without_oidc() {
    let s = TestServer::with_config(|c| c.dev_auth = true).await;
    let me = s.get("/api/me", Auth::None).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json()["name"], "Entwicklung");

    // writes without credentials still need the CSRF header
    let (_, ops) = new_task_ops("Dev");
    let r = s.push(Auth::None, &ops).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let req = common::authorize(Request::post("/api/sync/push"), Auth::None)
        .header("x-zunder-client", "web")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"ops": ops}).to_string()))
        .unwrap();
    let r = s.send(req).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["rejected"], json!([]));

    // /auth/login creates a real session and redirects
    let r = s.get("/auth/login?return_to=/inbox", Auth::None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.header("location"), Some("/inbox"));
    let cookie = r.cookie("zunder_session").unwrap();
    let set_cookie = r.set_cookies().join("\n");
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));
    assert!(set_cookie.contains("Path=/"));
    assert!(!set_cookie.contains("Secure"), "plain-http localhost");
    let r = s.get("/api/tokens", Auth::Cookie(&cookie)).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn dev_auth_only_for_loopback_hosts() {
    let s = TestServer::with_config(|c| c.dev_auth = true).await;
    let with_host = |method: Method, path: &str, host: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, host)
            .header("x-zunder-client", "web")
            .header(header::CONTENT_TYPE, "application/json")
    };
    for host in [
        "localhost",
        "localhost:5173",
        "127.0.0.1:8080",
        "[::1]:8080",
    ] {
        let r = s
            .send(
                with_host(Method::GET, "/api/me", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{host}");
    }
    // DNS rebinding or a foreign machine: the Host names someone else
    for host in [
        "rebind.attacker.example:8080",
        "192.0.2.2:8080",
        "localhost.attacker.example",
        "localhost@evil.example",
    ] {
        let r = s
            .send(
                with_host(Method::GET, "/api/me", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{host}");
        let r = s
            .send(
                with_host(Method::POST, "/api/tokens", host)
                    .body(Body::from(json!({"name": "x"}).to_string()))
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{host}");
        let r = s
            .send(
                with_host(Method::GET, "/auth/login", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{host}");
        assert!(r.cookie("zunder_session").is_none());
    }
}

#[tokio::test]
async fn login_rejects_return_to_with_control_characters() {
    let s = TestServer::with_config(|c| c.dev_auth = true).await;
    for evil in [
        "/%09/evil.example",
        "/%0A/evil.example",
        "/%0D/evil.example",
    ] {
        let r = s
            .get(&format!("/auth/login?return_to={evil}"), Auth::None)
            .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        assert_eq!(r.header("location"), Some("/"), "{evil}");
    }
}

#[tokio::test]
async fn session_cookie_is_secure_for_https() {
    let s = TestServer::with_config(|c| {
        c.dev_auth = true;
        // dev auth only runs on loopback; https on loopback still gets Secure
        c.public_url = url::Url::parse("https://localhost").unwrap();
    })
    .await;
    let r = s.get("/auth/login", Auth::None).await;
    assert!(r.set_cookies().join("\n").contains("Secure"));
}

#[tokio::test]
async fn login_without_provider_is_unavailable() {
    let s = TestServer::new().await;
    let r = s.get("/auth/login", Auth::None).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(r.text().contains("Anmeldedienst"));
}

#[tokio::test]
async fn task_history_with_sources() {
    let s = TestServer::new().await;
    let session = s.session().await;
    let (task, ops) = new_task_ops("Verlauf");
    s.push(Auth::Web(&session), &ops).await;
    let mut clock = client_clock();
    let rename = common::ops(&mut clock, |b| {
        b.set_task(task, "title", "Verlauf 2");
    });
    s.push(BOOT, &rename).await;
    let (_, result) = s
        .app
        .mutate(Source::Server, move |_state, batch| {
            batch.set_status(task, TaskStatus::InProgress);
            Ok(())
        })
        .await
        .unwrap();
    assert!(result.is_fully_accepted());

    let r = s.get(&format!("/api/tasks/{task}/history"), BOOT).await;
    assert_eq!(r.status, StatusCode::OK);
    let history = r.json();
    let entries = history.as_array().unwrap();
    assert_eq!(entries.len(), ops.len() + 2);
    let sources: Vec<&str> = entries
        .iter()
        .map(|e| e["source"].as_str().unwrap())
        .collect();
    assert!(sources[..ops.len()].iter().all(|s| *s == "web"));
    assert_eq!(sources[ops.len()], "api:bootstrap");
    assert_eq!(sources[ops.len() + 1], "server");
    let seqs: Vec<i64> = entries.iter().map(|e| e["seq"].as_i64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(entries[ops.len()]["op"]["value"], "Verlauf 2");
    assert!(entries[0]["received_at"].as_str().unwrap().ends_with('Z'));

    let r = s
        .get(
            &format!("/api/tasks/{}/history", uuid::Uuid::now_v7()),
            BOOT,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = s.get("/api/tasks/not-an-id/history", BOOT).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn server_mutations_use_server_clock_and_validation() {
    let s = TestServer::new().await;
    let (id, result) = s
        .app
        .mutate(Source::Server, |_state, batch| {
            Ok(batch.create_task(NewTask::new("Vom Server")))
        })
        .await
        .unwrap();
    assert!(result.is_fully_accepted());
    assert_eq!(result.cursor, s.app.cursor());
    let task_hlc = s.app.read(|st| {
        st.entity(EntityKind::Task, id)
            .unwrap()
            .created_at()
            .clone()
    });
    assert_eq!(task_hlc.node().as_str(), "server");

    // invalid server-side changes are rejected like any push
    let (_, result) = s
        .app
        .mutate(Source::Server, move |_state, batch| {
            batch.set_task(id, "status", "gibt-es-nicht");
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(result.rejected.len(), 1);
    assert_eq!(result.rejected[0].code.as_str(), "invalid_value");

    // the server clock moves past client timestamps it has seen
    let mut clock = zunder_core::Clock::new(zunder_core::NodeId::new("web-future").unwrap());
    let ahead = zunder_core::hlc::system_time_ms() + 60 * 60 * 1000;
    let mut batch = zunder_core::OpBatch::new(&mut clock, ahead);
    batch.set_task(id, "title", "Aus der Zukunft");
    let client_ops = batch.into_ops();
    s.push(BOOT, &client_ops).await;
    s.app
        .mutate(Source::Server, move |_s, b| {
            b.set_task(id, "title", "Server gewinnt");
            Ok(())
        })
        .await
        .unwrap();
    let title = s.app.read(|st| st.task(id).unwrap().title);
    assert_eq!(title, "Server gewinnt");
}

#[tokio::test]
async fn state_survives_restart_and_rebuilds() {
    let s = TestServer::new().await;
    let (task, ops) = new_task_ops("Bleibt");
    s.push(BOOT, &ops).await;
    let mut clock = client_clock();
    let rename = common::ops(&mut clock, |b| {
        b.set_task(task, "title", "Bleibt sicher");
    });
    s.push(BOOT, &rename).await;
    let before = s.get("/api/snapshot", BOOT).await.json();

    let s = s.restart().await;
    assert_eq!(s.get("/api/snapshot", BOOT).await.json(), before);
    // losing ops are still known after loading from the entities table
    let again = s.push(BOOT, &ops).await.json();
    assert_eq!(again["cursor"], before["cursor"]);
    assert_eq!(again["accepted"], op_ids(&ops));

    // wipe the materialized table: rebuilt from the log on startup
    let db = rusqlite::Connection::open(s.app.config().db_path()).unwrap();
    db.execute("DELETE FROM entities", []).unwrap();
    drop(db);
    let s = s.restart().await;
    assert_eq!(s.get("/api/snapshot", BOOT).await.json(), before);
}

#[tokio::test]
async fn backup_writes_a_database_copy() {
    let s = TestServer::new().await;
    let (_, ops) = new_task_ops("Gesichert");
    s.push(BOOT, &ops).await;
    let path = s.app.backup_now().await.unwrap();
    assert!(path.starts_with(s.app.config().backup_dir()));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("zunder-") && name.ends_with(".db"));
    let copy = rusqlite::Connection::open(&path).unwrap();
    let n: i64 = copy
        .query_row("SELECT COUNT(*) FROM ops", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, ops.len() as i64);
    let second = s.app.backup_now().await.unwrap();
    assert_ne!(path, second);
}

#[tokio::test]
async fn body_limits() {
    let s = TestServer::new().await;
    let session = s.session().await;
    let big_name = "x".repeat(100 * 1024);
    let r = s
        .post(
            "/api/tokens",
            Auth::Web(&session),
            json!({"name": big_name}),
        )
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(r.error_code(), "payload_too_large");

    // push allows large batches (here ~1 MB) ...
    let mut clock = client_clock();
    let many = common::ops(&mut clock, |b| {
        for i in 0..2000 {
            b.create_task(NewTask::new(format!("Aufgabe {i}")));
        }
    });
    let r = s.push(BOOT, &many).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["accepted"].as_array().unwrap().len(), many.len());
    // ... but not unlimited
    let huge = json!({"ops": [], "pad": "x".repeat(9 * 1024 * 1024)});
    let r = s.post("/api/sync/push", BOOT, huge).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn responses_are_compressed() {
    let s = TestServer::new().await;
    let (_, ops) = new_task_ops("Komprimiert");
    s.push(BOOT, &ops).await;
    for encoding in ["gzip", "br"] {
        let req = common::authorize(Request::get("/api/snapshot"), BOOT)
            .header(header::ACCEPT_ENCODING, encoding)
            .body(Body::empty())
            .unwrap();
        let r = s.send(req).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.header("content-encoding"), Some(encoding));
    }
}

#[tokio::test]
async fn unknown_api_path_is_json_404() {
    let s = TestServer::new().await;
    let r = s.get("/api/gibt-es-nicht", BOOT).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.error_code(), "not_found");
}

#[tokio::test]
async fn static_files_with_spa_fallback() {
    let s = TestServer::new().await;
    let web = s.app.config().web_dir.clone();
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(
        web.join("index.html"),
        "<!doctype html><title>Zunder</title>",
    )
    .unwrap();
    std::fs::write(web.join("assets/app-3f9a1c.js"), "console.log('zunder');").unwrap();
    std::fs::write(web.join("favicon.svg"), "<svg/>").unwrap();

    let r = s.get("/", Auth::None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.text().contains("<title>Zunder</title>"));
    assert_eq!(r.header("cache-control"), Some("no-cache"));

    let r = s.get("/assets/app-3f9a1c.js", Auth::None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert!(r.header("content-type").unwrap().contains("javascript"));

    let r = s.get("/favicon.svg", Auth::None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("cache-control"), Some("no-cache"));

    for route in ["/inbox", "/bereiche/0192abcd/projekt", "/index.html"] {
        let r = s.get(route, Auth::None).await;
        assert_eq!(r.status, StatusCode::OK, "{route}");
        assert!(r.text().contains("<title>Zunder</title>"), "{route}");
        assert_eq!(r.header("cache-control"), Some("no-cache"), "{route}");
    }

    let r = s.get("/assets/fehlt-123.js", Auth::None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = s.get("/fehlt.png", Auth::None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
}

#[tokio::test]
async fn sessions_slide_and_expire() {
    let s = TestServer::new().await;
    let session = s.session().await;
    let db = rusqlite::Connection::open(s.app.config().db_path()).unwrap();

    // fresh session: no renewal cookie
    let r = s.get("/api/me", Auth::Cookie(&session)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.cookie("zunder_session").is_none());

    // last extension two days ago: renewed for another 30 days
    let now = zunder_core::hlc::system_time_ms() as i64;
    let day = 24 * 60 * 60 * 1000;
    db.execute(
        "UPDATE sessions SET last_seen_at = ?1, expires_at = ?2",
        [now - 2 * day, now + 28 * day],
    )
    .unwrap();
    let r = s.get("/api/me", Auth::Cookie(&session)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.cookie("zunder_session").as_deref(),
        Some(session.as_str())
    );
    assert!(
        r.set_cookies()[0].contains("Max-Age=259"),
        "{:?}",
        r.set_cookies()
    );
    let expires: i64 = db
        .query_row("SELECT expires_at FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert!(expires > now + 29 * day);

    // expired sessions are refused
    db.execute("UPDATE sessions SET expires_at = ?1", [now - 1])
        .unwrap();
    let r = s.get("/api/me", Auth::Cookie(&session)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // only a hash is stored
    let stored: String = db
        .query_row("SELECT id_hash FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_ne!(stored, session);
    assert_eq!(stored.len(), 64);
}

#[tokio::test]
async fn healthcheck_against_a_real_listener() {
    let s = TestServer::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = s.router.clone();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    zunder_server::healthcheck::check(addr).await.unwrap();
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    assert!(zunder_server::healthcheck::check(closed).await.is_err());
}
