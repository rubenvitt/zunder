//! `zunder-import run` (the real CLI) against the real `zunder-server` binary
//! and the synthetic ClickUp mock.
//!
//! Ignored by default because it needs a built server:
//!
//! ```sh
//! cargo build -p zunder-server -p zunder-import
//! ZUNDER_SERVER_BIN=$PWD/target/debug/zunder-server \
//!   cargo test -p zunder-import --test server_e2e -- --ignored
//! ```

mod common;

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::{edited_mapping, ClickUpMock, CLICKUP_TOKEN};
use serde_json::{json, Value};
use zunder_core::ids::clickup_entity_id;
use zunder_core::state::SnapshotEntity;
use zunder_core::{EntityKind, State, TaskStatus};
use zunder_import::{mapping, plan};

const BOOTSTRAP: &str = "zt_import-e2e-bootstrap-token-0123456789abc";

struct Server {
    child: Child,
    url: String,
    _dir: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Starts the server with a bootstrap token and dev auth (dev auth is only
/// used to create a named API token like a browser session would).
async fn start_server() -> Server {
    let bin = std::env::var("ZUNDER_SERVER_BIN")
        .expect("ZUNDER_SERVER_BIN must point to a built zunder-server binary");
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let url = format!("http://127.0.0.1:{port}");
    let child = Command::new(bin)
        .env("ZUNDER_BIND", format!("127.0.0.1:{port}"))
        .env("ZUNDER_PUBLIC_URL", &url)
        .env("ZUNDER_DATA_DIR", dir.path().join("data"))
        .env("ZUNDER_WEB_DIR", dir.path().join("web"))
        .env("ZUNDER_BOOTSTRAP_TOKEN", BOOTSTRAP)
        .env("ZUNDER_DEV_AUTH", "true")
        .env("RUST_LOG", "error")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let server = Server {
        child,
        url,
        _dir: dir,
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let ok = reqwest::get(format!("{}/healthz", server.url))
            .await
            .is_ok_and(|r| r.status().is_success());
        if ok {
            return server;
        }
        assert!(Instant::now() < deadline, "server did not come up");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn get_json(server: &Server, token: &str, path: &str) -> Value {
    let res = reqwest::Client::new()
        .get(format!("{}{path}", server.url))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{path}: {}", res.status());
    res.json().await.unwrap()
}

async fn snapshot(server: &Server, token: &str) -> (i64, State) {
    let body = get_json(server, token, "/api/snapshot").await;
    let entities: Vec<SnapshotEntity> = serde_json::from_value(body["entities"].clone()).unwrap();
    (
        body["cursor"].as_i64().unwrap(),
        State::from_snapshot(&entities),
    )
}

/// Runs the CLI binary off the runtime thread, so the ClickUp mock keeps serving.
async fn run_cli(args: Vec<String>) -> (bool, String) {
    tokio::task::spawn_blocking(move || {
        let out = Command::new(env!("CARGO_BIN_EXE_zunder-import"))
            .args(&args)
            .env_remove("ZUNDER_URL")
            .env_remove("ZUNDER_TOKEN")
            .env_remove("CLICKUP_TOKEN")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs ZUNDER_SERVER_BIN (built zunder-server)"]
async fn import_cli_against_real_server() {
    let server = start_server().await;
    let clickup = ClickUpMock::start().await;
    let dir = tempfile::tempdir().unwrap();

    // A named token, created like the web UI does (session/dev auth + CSRF header).
    let created: Value = reqwest::Client::new()
        .post(format!("{}/api/tokens", server.url))
        .header("X-Zunder-Client", "web")
        .json(&json!({"name": "import"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = created["token"]
        .as_str()
        .expect("token in response")
        .to_owned();

    // Mapping as a user would have edited it, written as TOML.
    let proposal = plan::plan(&clickup.client(), None, "2026-10-11")
        .await
        .unwrap();
    let mapping_path = dir.path().join("mapping.toml");
    let toml = mapping::render(
        &edited_mapping(proposal.mapping),
        "Synthetisch",
        "2026-10-11",
    );
    std::fs::write(&mapping_path, toml).unwrap();

    let args = vec![
        "run".to_owned(),
        "--mapping".to_owned(),
        mapping_path.display().to_string(),
        "--server".to_owned(),
        server.url.clone(),
        "--token".to_owned(),
        token.clone(),
        "--clickup-token".to_owned(),
        CLICKUP_TOKEN.to_owned(),
        "--clickup-url".to_owned(),
        clickup.url.clone(),
        "--batch-size".to_owned(),
        "40".to_owned(),
    ];
    let (ok, output) = run_cli(args.clone()).await;
    assert!(ok, "first run failed:\n{output}");
    assert!(output.contains("0 abgelehnt"), "{output}");

    let (cursor, state) = snapshot(&server, &token).await;
    assert!(cursor > 0);
    assert_eq!(state.live_areas().count(), 3);
    assert_eq!(state.live_projects().count(), 4);
    assert_eq!(state.live_tasks().count(), 18);
    let tax = state
        .task(clickup_entity_id(EntityKind::Task, "86a002"))
        .unwrap();
    assert_eq!(tax.status, TaskStatus::InProgress);
    assert_eq!(tax.rank.as_deref(), Some("0000000000002.500000"));

    // Source label of token pushes: api:<token-name>, node `import`.
    let history = get_json(&server, &token, &format!("/api/tasks/{}/history", tax.id)).await;
    let entries = history.as_array().unwrap();
    assert!(!entries.is_empty());
    for entry in entries {
        assert_eq!(entry["source"], "api:import");
        assert!(entry["op"]["hlc"].as_str().unwrap().ends_with(":import"));
        assert!(entry["received_at"].as_str().is_some());
    }

    // Idempotent: a second run writes nothing.
    let (ok, output) = run_cli(args.clone()).await;
    assert!(ok, "second run failed:\n{output}");
    let (cursor_after, _) = snapshot(&server, &token).await;
    assert_eq!(cursor_after, cursor, "second run must not write:\n{output}");

    // The bootstrap token works as well and is labelled api:bootstrap.
    let body = json!({"client_id": "e2e", "ops": [{
        "op_id": zunder_core::ids::new_id().to_string(),
        "entity": "task",
        "entity_id": tax.id.to_string(),
        "field": "plan",
        "value": "Belege scannen",
        "hlc": format!("{:013}:00000:e2e", zunder_core::hlc::system_time_ms()),
    }]});
    let res: Value = reqwest::Client::new()
        .post(format!("{}/api/sync/push", server.url))
        .bearer_auth(BOOTSTRAP)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["rejected"], json!([]), "{res}");
    let history = get_json(
        &server,
        BOOTSTRAP,
        &format!("/api/tasks/{}/history", tax.id),
    )
    .await;
    assert_eq!(
        history.as_array().unwrap().last().unwrap()["source"],
        "api:bootstrap"
    );

    // Made recurring in Zunder and completed: the server writes the
    // follow-up instance, and a later run leaves the completed instance
    // alone even though ClickUp has it open and without a due date.
    let windows = clickup_entity_id(EntityKind::Task, "86a001");
    let hlc = zunder_core::hlc::system_time_ms();
    let op = |field: &str, value: Value, n: u32| {
        json!({
            "op_id": zunder_core::ids::new_id().to_string(),
            "entity": "task",
            "entity_id": windows.to_string(),
            "field": field,
            "value": value,
            "hlc": format!("{hlc:013}:{n:05}:e2e"),
        })
    };
    let body = json!({"client_id": "e2e", "ops": [
        op("recurrence", json!({"every": 1, "unit": "month", "day": "last", "from": "due"}), 0),
        op("status", json!("done"), 1),
    ]});
    let res: Value = reqwest::Client::new()
        .post(format!("{}/api/sync/push", server.url))
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["rejected"], json!([]), "{res}");
    let (_, state) = snapshot(&server, &token).await;
    let completed = state.task(windows).unwrap();
    assert_eq!(state.live_tasks().count(), 18 + 1, "follow-up instance");

    clickup.edit_task("/list/9001001/task?page=0", "86a001", |t| {
        t.insert("due_date".into(), json!(null));
    });
    let (ok, output) = run_cli(args).await;
    assert!(ok, "run with a recurring task failed:\n{output}");
    assert!(
        output.contains("In Zunder wiederkehrend, übersprungen (1):"),
        "{output}"
    );
    let (_, state) = snapshot(&server, &token).await;
    assert_eq!(state.task(windows).unwrap(), completed);
    assert_eq!(state.live_tasks().count(), 18 + 1);
}
