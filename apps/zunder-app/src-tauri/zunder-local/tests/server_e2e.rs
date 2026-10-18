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

use serde_json::json;
use time::format_description::well_known::Rfc3339;
use time::macros::{offset, time};
use time::{Date, Month, OffsetDateTime, Time, UtcOffset, Weekday};
use zunder_core::hlc::system_time_ms;
use zunder_core::ids::Id;
use zunder_core::model::fields::task as f;
use zunder_core::{
    Clock, Due, EntityKind, Location, NewArea, NewTask, NodeId, Op, OpBatch, Recurrence,
    RejectCode, TaskStatus,
};
use zunder_local::reminders::ReminderKind;
use zunder_local::sync::lock;
use zunder_local::{
    login, upcoming_reminders, ConfigUpdate, Error, FlameAction, LocalStore, Syncer,
};

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

    let err = client
        .task_history(zunder_core::ids::new_id())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404"), "{err}");

    // "Erst für dich" needs a device token (login), not an API token.
    let err = client.focus_settings().await.unwrap_err();
    assert_eq!(err.code(), "unsupported", "{err:?}");
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

    // "Erst für dich" (commands `focus_get`, `focus_set`) with the device token.
    let client = syncer.client(&phone).unwrap();
    let update = serde_json::json!({"dismissed_on": "2026-10-18"});
    let settings = client.update_focus_settings(&update).await.unwrap();
    assert_eq!(
        settings,
        serde_json::json!({"dismissed_on": "2026-10-18", "chosen": null})
    );
    let update = serde_json::json!({"dismissed_on": null});
    client.update_focus_settings(&update).await.unwrap();
    let settings = client.focus_settings().await.unwrap();
    assert_eq!(settings["dismissed_on"], serde_json::Value::Null);

    // Revoking (new login, logout) cuts the device off.
    login::revoke(&server.url, &token).await.unwrap();
    let failure = syncer.sync(&phone, now()).await.unwrap_err();
    assert!(
        matches!(failure.error, Error::Unauthorized),
        "{:?}",
        failure.error
    );
}

/// Live tasks whose `recurrence_prev` names `prev` (the follow-up instances).
fn follow_ups(store: &Mutex<LocalStore>, prev: Id) -> Vec<Id> {
    lock(store)
        .state()
        .live_tasks()
        .filter(|t| t.recurrence_prev == Some(prev))
        .map(|t| t.id)
        .collect()
}

/// The first year after the current one whose 31.03. is a Sunday (2030,
/// 2041, …): Berlin switches to summer time in that very night, so a
/// month-end series due at the end of February crosses the switch while its
/// reminder the day before does not. Being ahead of today, the series never
/// adjusts to it, whatever the date the test runs on.
fn year_switching_on_march_31() -> i32 {
    let mut year = OffsetDateTime::now_utc().year() + 1;
    while Date::from_calendar_date(year, Month::March, 31)
        .unwrap()
        .weekday()
        != Weekday::Sunday
    {
        year += 1;
    }
    year
}

#[tokio::test]
#[ignore = "braucht ZUNDER_SERVER_BIN (gebautes zunder-server-Binary)"]
async fn recurring_tasks_continue_through_the_real_server() {
    // The server computes follow-ups in its process time zone (contract 3.1).
    let server = start_server_with(&[("TZ", "Europe/Berlin")]).await;
    let dir = tempfile::tempdir().unwrap();
    let phone = device(&dir, "phone", &server, TOKEN);
    let laptop = device(&dir, "laptop", &server, TOKEN);
    let syncer_phone = Syncer::new();
    let syncer_laptop = Syncer::new();
    let mut phone_ui = Ui::new("web-phone");
    let mut laptop_ui = Ui::new("web-laptop");
    let rhythm = json!({"every": 1, "unit": "month", "day": "last", "from": "due"});
    let year = year_switching_on_march_31();
    let feb_last = Month::February.length(year);
    let at = |month: Month, day: u8, time: Time, offset: UtcOffset| {
        Date::from_calendar_date(year, month, day)
            .unwrap()
            .with_time(time)
            .assume_offset(offset)
    };
    let due = at(Month::February, feb_last, time!(09:00), offset!(+1));
    let remind = at(Month::February, feb_last - 1, time!(18:00), offset!(+1));

    // Offline on the phone: create the series and tick it off right away.
    // Both land in one push (pending_batch), which still completes it.
    let ((area, first), create) = phone_ui.ops(|b| {
        let area = b.create_area(NewArea::new("Arbeit"));
        let task = b.create_task(
            NewTask::new("Odoo absenden und Stunden bestätigen").at(Location::Area { id: area }),
        );
        b.set_task(task, f::DUE, due.format(&Rfc3339).unwrap());
        b.set_task(task, f::REMIND_AT, remind.format(&Rfc3339).unwrap());
        b.set_task(task, f::RECURRENCE, rhythm.clone());
        (area, task)
    });
    apply(&phone, &create);
    let (_, done) = phone_ui.ops(|b| {
        b.set_status(first, TaskStatus::Done);
    });
    apply(&phone, &done);
    let report = syncer_phone.sync(&phone, now()).await.unwrap();
    assert_eq!(report.pushed, create.len() + done.len());
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    assert!(report.changed);

    // The follow-up came back with the same round's pull.
    let next = match follow_ups(&phone, first)[..] {
        [next] => next,
        ref other => panic!("expected one follow-up, got {other:?}"),
    };
    {
        let p = lock(&phone);
        let t = p.state().task(next).unwrap();
        assert_eq!(t.title, "Odoo absenden und Stunden bestätigen");
        assert_eq!(t.status, TaskStatus::Open);
        assert_eq!(t.location, Location::Area { id: area });
        assert_eq!(t.recurrence, Some(Recurrence::parse(&rhythm).unwrap()));
        // Last day of the next month, same local time across the switch
        // to summer time (31.03.); the reminder keeps "the day before at
        // 18:00".
        assert_eq!(
            t.due,
            Some(Due::DateTime(at(
                Month::March,
                31,
                time!(09:00),
                offset!(+2)
            )))
        );
        assert_eq!(
            t.remind_at,
            Some(at(Month::March, 30, time!(18:00), offset!(+1)))
        );
        assert_eq!(p.state().task(first).unwrap().status, TaskStatus::Done);

        // Reminders move to the new instance.
        let reminders = upcoming_reminders(p.state(), now(), 64);
        let keys: Vec<_> = reminders.iter().map(|r| (r.task_id, r.kind)).collect();
        assert_eq!(
            keys,
            vec![(next, ReminderKind::Remind), (next, ReminderKind::Due)]
        );
        assert_eq!(reminders[0].body, "Erinnerung · Arbeit");
    }

    // The laptop gets the series with its snapshot. Reopening and
    // completing the old instance again (one push) adds no second one.
    syncer_laptop.sync(&laptop, now()).await.unwrap();
    assert_eq!(lock(&laptop).state(), lock(&phone).state());
    laptop_ui.see(&laptop);
    let (_, reopen) = laptop_ui.ops(|b| {
        b.set_status(first, TaskStatus::Open);
    });
    apply(&laptop, &reopen);
    let (_, again) = laptop_ui.ops(|b| {
        b.set_status(first, TaskStatus::Done);
    });
    apply(&laptop, &again);
    let report = syncer_laptop.sync(&laptop, now()).await.unwrap();
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    assert_eq!(follow_ups(&laptop, first), vec![next]);

    // Local validation knows the series rules: no due date, no series.
    phone_ui.see(&phone);
    let (_, no_due) = phone_ui.ops(|b| {
        b.set_task(next, f::DUE, serde_json::Value::Null);
    });
    let result = lock(&phone).apply_local(&no_due, now() as u64).unwrap();
    assert_eq!(result.rejected.len(), 1);
    assert_eq!(result.rejected[0].code, RejectCode::InvalidValue);

    // Dropping with "end series" (recurrence: null in the same action)
    // creates nothing further.
    let (_, end) = phone_ui.ops(|b| {
        b.set_status(next, TaskStatus::Dropped);
        b.set_task(next, f::RECURRENCE, serde_json::Value::Null);
    });
    apply(&phone, &end);
    let report = syncer_phone.sync(&phone, now()).await.unwrap();
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    assert!(follow_ups(&phone, next).is_empty());
    syncer_laptop.sync(&laptop, now()).await.unwrap();
    assert_eq!(lock(&laptop).state(), lock(&phone).state());
}
