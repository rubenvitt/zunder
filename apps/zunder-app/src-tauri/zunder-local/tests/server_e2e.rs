//! End-to-end sync against the real `zunder-server` binary.
//!
//! Ignored by default because it needs a built server:
//!
//! ```sh
//! cargo build -p zunder-server                      # in the repo root
//! ZUNDER_SERVER_BIN=$PWD/target/debug/zunder-server \
//!   cargo test --manifest-path apps/zunder-app/src-tauri/Cargo.toml \
//!   -p zunder-local --test server_e2e -- --ignored
//! ```

use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;
use zunder_core::hlc::system_time_ms;
use zunder_core::model::fields::task as f;
use zunder_core::{Clock, EntityKind, Location, NewArea, NewTask, NodeId, Op, OpBatch, RejectCode};
use zunder_local::sync::lock;
use zunder_local::{login, ConfigUpdate, Error, FlameAction, LocalStore, Syncer};

const TOKEN: &str = "zt_app-e2e-bootstrap-token-0123456789abcdef";

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

async fn start_server() -> Server {
    start_server_with(&[]).await
}

/// Starts the server with additional environment variables; its public URL
/// is its own address.
async fn start_server_with(env: &[(&str, &str)]) -> Server {
    let bin = std::env::var("ZUNDER_SERVER_BIN")
        .expect("ZUNDER_SERVER_BIN must point to a built zunder-server binary");
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let child = Command::new(bin)
        .envs(env.iter().copied())
        .env("ZUNDER_PUBLIC_URL", format!("http://127.0.0.1:{port}"))
        .env("ZUNDER_BIND", format!("127.0.0.1:{port}"))
        .env("ZUNDER_DATA_DIR", dir.path().join("data"))
        .env("ZUNDER_WEB_DIR", dir.path().join("web"))
        .env("ZUNDER_BOOTSTRAP_TOKEN", TOKEN)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let server = Server {
        child,
        url: format!("http://127.0.0.1:{port}"),
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

fn device(dir: &tempfile::TempDir, name: &str, server: &Server, token: &str) -> Mutex<LocalStore> {
    let mut store = LocalStore::open(dir.path().join(format!("{name}.db"))).unwrap();
    store
        .set_config(&ConfigUpdate {
            server_url: Some(Some(server.url.clone())),
            api_token: Some(Some(token.into())),
            discard_pending: false,
        })
        .unwrap();
    Mutex::new(store)
}

struct Ui {
    clock: Clock,
}

impl Ui {
    fn new(node: &str) -> Self {
        Self {
            clock: Clock::new(NodeId::new(node).unwrap()),
        }
    }

    fn see(&mut self, store: &Mutex<LocalStore>) {
        for entity in lock(store).state().entities() {
            for field in entity.fields().values() {
                self.clock.receive(&field.hlc);
            }
        }
    }

    fn ops<T>(&mut self, build: impl FnOnce(&mut OpBatch<'_>) -> T) -> (T, Vec<Op>) {
        let mut batch = OpBatch::new(&mut self.clock, system_time_ms());
        let out = build(&mut batch);
        (out, batch.into_ops())
    }
}

fn apply(store: &Mutex<LocalStore>, ops: &[Op]) {
    let result = lock(store).apply_local(ops, system_time_ms()).unwrap();
    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
}

fn now() -> i64 {
    system_time_ms() as i64
}

#[tokio::test]
#[ignore = "braucht ZUNDER_SERVER_BIN (gebautes zunder-server-Binary)"]
async fn devices_sync_through_the_real_server() {
    let server = start_server().await;
    let dir = tempfile::tempdir().unwrap();
    let phone = device(&dir, "phone", &server, TOKEN);
    let laptop = device(&dir, "laptop", &server, TOKEN);
    let syncer_phone = Syncer::new();
    let syncer_laptop = Syncer::new();
    let mut phone_ui = Ui::new("web-phone");
    let mut laptop_ui = Ui::new("web-laptop");

    // Phone creates an area and a task in it.
    let ((area, task), ops) = phone_ui.ops(|b| {
        let area = b.create_area(NewArea::new("Gesundheit"));
        let task = b.create_task(NewTask::new("Zahnarzt anrufen").at(Location::Area { id: area }));
        (area, task)
    });
    apply(&phone, &ops);
    let report = syncer_phone.sync(&phone, now()).await.unwrap();
    assert_eq!(report.pushed, ops.len());
    assert!(report.rejected.is_empty());
    assert_eq!(report.pending, 0);

    // Laptop starts empty: snapshot.
    let report = syncer_laptop.sync(&laptop, now()).await.unwrap();
    assert!(report.changed);
    assert_eq!(
        lock(&laptop).state().task(task).unwrap().title,
        "Zahnarzt anrufen"
    );

    // Laptop renames, phone pulls.
    laptop_ui.see(&laptop);
    let (_, rename) = laptop_ui.ops(|b| {
        b.set_task(task, f::TITLE, "Zahnarzt anrufen (dringend)");
    });
    apply(&laptop, &rename);
    syncer_laptop.sync(&laptop, now()).await.unwrap();
    let report = syncer_phone.sync(&phone, now()).await.unwrap();
    assert_eq!(report.pulled, rename.len());
    assert!(report.changed);
    // The overlapping pull found the phone's own last op at its cursor.
    assert!(!report.resynced);
    assert_eq!(
        lock(&phone).state().task(task).unwrap().title,
        "Zahnarzt anrufen (dringend)"
    );

    // Laptop deletes the area (with its task); phone, not yet aware, files
    // another task into it. The server rejects it, the phone rolls back.
    let (_, delete) = laptop_ui.ops(|b| {
        let state = lock(&laptop).state().clone();
        b.delete_cascade(&state, EntityKind::Area, area);
    });
    apply(&laptop, &delete);
    syncer_laptop.sync(&laptop, now()).await.unwrap();

    phone_ui.see(&phone);
    let (late, create) = phone_ui
        .ops(|b| b.create_task(NewTask::new("Rezept abholen").at(Location::Area { id: area })));
    apply(&phone, &create);
    let report = syncer_phone.sync(&phone, now()).await.unwrap();
    assert_eq!(report.pushed, 0);
    assert_eq!(report.rejected.len(), create.len());
    assert!(report
        .rejected
        .iter()
        .any(|r| r.code == RejectCode::InvalidReference));
    {
        let p = lock(&phone);
        assert!(p.state().task(late).is_none());
        assert!(p.state().live_tasks().next().is_none());
        assert_eq!(p.state(), lock(&laptop).state());
        assert_eq!(p.cursor(), lock(&laptop).cursor());
    }

    // A wrong token is reported as such.
    let stranger = device(&dir, "stranger", &server, "zt_wrong-token");
    let err = Syncer::new()
        .sync(&stranger, now())
        .await
        .unwrap_err()
        .error;
    assert_eq!(err.code(), "unauthorized");
}

#[tokio::test]
#[ignore = "braucht ZUNDER_SERVER_BIN (gebautes zunder-server-Binary)"]
async fn history_and_flames_with_the_api_token() {
    let server = start_server().await;
    let dir = tempfile::tempdir().unwrap();
    let phone = device(&dir, "phone", &server, TOKEN);
    let syncer = Syncer::new();
    let mut ui = Ui::new("web-phone");

    let (task, ops) = ui.ops(|b| b.create_task(NewTask::new("Steuererklärung")));
    apply(&phone, &ops);
    syncer.sync(&phone, now()).await.unwrap();

    // The commands `task_history`, `flame_start` and `flame_stop` use this client.
    let client = syncer.client(&phone).unwrap();
    let history = client.task_history(task).await.unwrap();
    assert_eq!(history.len(), ops.len());
    assert!(history.iter().all(|e| e.source == "api:bootstrap"));
    assert!(history.iter().any(|e| e.op.field == "title"));

    let outcome = client.flame(task, FlameAction::Start).await.unwrap();
    assert_eq!(outcome["running"], true);
    let report = syncer.sync(&phone, now()).await.unwrap();
    assert!(report.changed, "the server wrote flame_started_at");
    let local = lock(&phone).state().task(task).unwrap();
    assert!(local.flame_started_at.is_some());

    client.flame(task, FlameAction::Stop).await.unwrap();
    let err = client.flame(task, FlameAction::Stop).await.unwrap_err();
    assert_eq!(err.code(), "server");
    assert!(err.to_string().contains("409"), "{err}");

    let err = client.task_history(Uuid::now_v7()).await.unwrap_err();
    assert!(err.to_string().contains("404"), "{err}");
}

#[tokio::test]
#[ignore = "braucht ZUNDER_SERVER_BIN (gebautes zunder-server-Binary)"]
async fn login_yields_a_device_token_that_syncs() {
    // Dev auth stands in for the browser session on the consent page.
    let server = start_server_with(&[("ZUNDER_DEV_AUTH", "true")]).await;
    let (pending, authorize) = login::start(&server.url, "E2E-Gerät", false, now())
        .await
        .unwrap();
    assert!(authorize.starts_with(&format!("{}/oauth/authorize?", server.url)));

    // The browser: consent page, "Erlauben", redirect to the app.
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let page = browser
        .get(&authorize)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("als Gerät"), "{page}");
    let marker = "name=\"request_id\" value=\"";
    let start = page.find(marker).unwrap() + marker.len();
    let request_id = &page[start..start + page[start..].find('"').unwrap()];
    let r = browser
        .post(format!("{}/oauth/authorize", server.url))
        .form(&[("request_id", request_id), ("decision", "allow")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    let callback = url::Url::parse(r.headers()["location"].to_str().unwrap()).unwrap();
    assert!(login::is_callback(&callback), "{callback}");

    let code = login::code_from_callback(&pending, &callback).unwrap();
    let token = login::exchange(&pending, &code).await.unwrap();
    assert!(token.starts_with("zd_"), "{token}");
    // A code works once.
    assert_eq!(
        login::exchange(&pending, &code).await.unwrap_err().code(),
        "login"
    );

    // The replay revoked the device; a fresh login works again.
    let dir = tempfile::tempdir().unwrap();
    let phone = device(&dir, "phone", &server, &token);
    let syncer = Syncer::new();
    let failure = syncer.sync(&phone, now()).await.unwrap_err();
    assert!(
        matches!(failure.error, Error::Unauthorized),
        "{:?}",
        failure.error
    );

    let (pending, authorize) = login::start(&server.url, "E2E-Gerät", false, now())
        .await
        .unwrap();
    let page = browser
        .get(&authorize)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let start = page.find(marker).unwrap() + marker.len();
    let request_id = &page[start..start + page[start..].find('"').unwrap()];
    let r = browser
        .post(format!("{}/oauth/authorize", server.url))
        .form(&[("request_id", request_id), ("decision", "allow")])
        .send()
        .await
        .unwrap();
    let callback = url::Url::parse(r.headers()["location"].to_str().unwrap()).unwrap();
    let code = login::code_from_callback(&pending, &callback).unwrap();
    let token = login::exchange(&pending, &code).await.unwrap();
    let phone = device(&dir, "phone2", &server, &token);
    syncer.sync(&phone, now()).await.unwrap();

    // Revoking (new login, logout) cuts the device off.
    login::revoke(&server.url, &token).await.unwrap();
    let failure = syncer.sync(&phone, now()).await.unwrap_err();
    assert!(
        matches!(failure.error, Error::Unauthorized),
        "{:?}",
        failure.error
    );
}
