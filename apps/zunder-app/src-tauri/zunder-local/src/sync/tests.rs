use std::sync::Mutex;

use zunder_core::hlc::system_time_ms;
use zunder_core::model::fields::task as f;
use zunder_core::{Location, NewArea, NewTask, RejectCode};

use super::*;
use crate::testutil::{Author, FakeServer};

fn store() -> Mutex<LocalStore> {
    Mutex::new(LocalStore::open_in_memory().unwrap())
}

fn now() -> i64 {
    system_time_ms() as i64
}

fn apply(store: &Mutex<LocalStore>, ops: &[zunder_core::Op]) {
    let result = lock(store).apply_local(ops, system_time_ms()).unwrap();
    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
}

#[tokio::test]
async fn two_devices_converge_through_the_server() {
    let server = FakeServer::new();
    let phone = store();
    let laptop = store();
    let mut phone_ui = Author::new("web-phone");
    let mut laptop_ui = Author::new("web-laptop");

    let (task, create) = phone_ui.create(|b| b.create_task(NewTask::new("Zahnarzt anrufen")));
    apply(&phone, &create);
    let report = sync_once(&phone, &server, now()).await.unwrap();
    assert_eq!(report.pushed, create.len());
    assert_eq!(report.pending, 0);
    assert!(report.rejected.is_empty());
    assert_eq!(report.cursor, server.cursor());

    let report = sync_once(&laptop, &server, now()).await.unwrap();
    assert!(report.changed);
    assert_eq!(report.cursor, server.cursor());

    // The laptop UI has seen the task (its clock received the HLCs).
    laptop_ui.receive(&create);
    let rename = laptop_ui.ops(|b| {
        b.set_task(task, f::TITLE, "Zahnarzt anrufen (dringend)");
    });
    apply(&laptop, &rename);
    sync_once(&laptop, &server, now()).await.unwrap();
    let report = sync_once(&phone, &server, now()).await.unwrap();
    assert!(report.changed);
    assert_eq!(report.pulled, rename.len());

    for device in [&phone, &laptop] {
        let s = lock(device);
        assert_eq!(s.state(), &server.state());
        assert_eq!(s.confirmed_state(), &server.state());
        assert_eq!(s.cursor(), server.cursor());
    }
}

#[tokio::test]
async fn first_sync_uses_the_snapshot_and_then_pulls_in_pages() {
    let server = FakeServer::with_page_size(2);
    let mut other = Author::new("web-other");
    let (_, early) = other.create(|b| b.create_task(NewTask::new("Früh")));
    server.accept(&early);

    let device = store();
    let report = sync_once(&device, &server, now()).await.unwrap();
    assert!(report.changed);
    assert_eq!(server.counts().1, 1, "one snapshot");
    assert_eq!(report.pulled, 0);

    let (_, late) = other.create(|b| b.create_task(NewTask::new("Spät")));
    assert!(late.len() > 2, "needs several pages");
    server.accept(&late);
    let report = sync_once(&device, &server, now()).await.unwrap();
    assert_eq!(report.pulled, late.len());
    assert_eq!(server.counts().1, 1, "no second snapshot");
    assert_eq!(lock(&device).state(), &server.state());
}

#[tokio::test]
async fn offline_changes_wait_and_go_out_later() {
    let server = FakeServer::new();
    let device = store();
    let mut ui = Author::new("web-a");
    let (task, ops) = ui.create(|b| b.create_task(NewTask::new("Offline erfasst")));
    apply(&device, &ops);

    server.set_offline(true);
    let err = sync_once(&device, &server, 1_000).await.unwrap_err();
    assert!(err.is_network());
    {
        let s = lock(&device);
        assert_eq!(s.pending_count(), ops.len());
        assert!(s.state().task(task).is_some());
        let last = s.last_sync().unwrap().unwrap();
        assert_eq!(last.at_ms, 1_000);
        assert!(!last.ok);
        assert!(last.error.unwrap().contains("nicht erreichbar"));
    }

    server.set_offline(false);
    let report = sync_once(&device, &server, 2_000).await.unwrap();
    assert_eq!(report.pushed, ops.len());
    assert!(server.state().task(task).is_some());
    let last = lock(&device).last_sync().unwrap().unwrap();
    assert_eq!(
        last,
        LastSync {
            at_ms: 2_000,
            ok: true,
            error: None
        }
    );
}

#[tokio::test]
async fn server_rejections_are_rolled_back_and_reported() {
    let server = FakeServer::new();
    let device = store();
    let mut ui = Author::new("web-a");
    let mut other = Author::new("web-b");

    // Both know an area; the other client deletes it on the server.
    let (area, create_area) = ui.create(|b| b.create_area(NewArea::new("Haushalt")));
    apply(&device, &create_area);
    sync_once(&device, &server, now()).await.unwrap();
    server.accept(&other.ops(|b| {
        b.delete(zunder_core::EntityKind::Area, area);
    }));

    // Offline, the device files a new task into the (locally live) area.
    let (task, create_task) = ui
        .create(|b| b.create_task(NewTask::new("Fenster putzen").at(Location::Area { id: area })));
    apply(&device, &create_task);

    let report = sync_once(&device, &server, now()).await.unwrap();
    assert_eq!(report.pushed, 0);
    assert_eq!(report.rejected.len(), create_task.len());
    assert!(report
        .rejected
        .iter()
        .any(|r| r.code == RejectCode::InvalidReference));
    assert!(report.changed);
    let s = lock(&device);
    assert_eq!(s.pending_count(), 0);
    assert!(s.state().task(task).is_none(), "rolled back");
    assert_eq!(s.state(), &server.state());
}

#[tokio::test]
async fn a_lost_push_response_is_harmless() {
    let server = FakeServer::new();
    let device = store();
    let mut ui = Author::new("web-a");
    let (_, ops) = ui.create(|b| b.create_task(NewTask::new("A")));
    apply(&device, &ops);

    server.lose_next_push_response();
    assert!(sync_once(&device, &server, now()).await.is_err());
    assert_eq!(lock(&device).pending_count(), ops.len());

    // The retry pushes again; the server knows the op IDs and accepts them.
    let report = sync_once(&device, &server, now()).await.unwrap();
    assert_eq!(report.pushed, ops.len());
    assert_eq!(report.pending, 0);
    assert_eq!(server.cursor(), ops.len() as i64, "stored once");
    assert_eq!(lock(&device).state(), &server.state());
}

#[tokio::test]
async fn syncer_requires_a_configured_server() {
    let device = store();
    let err = Syncer::new().sync(&device, now()).await.unwrap_err();
    assert_eq!(err.code(), "not_configured");
    assert!(
        lock(&device).last_sync().unwrap().is_none(),
        "nothing was attempted"
    );
}

#[test]
fn syncer_client_follows_the_configuration() {
    let device = store();
    let syncer = Syncer::new();
    assert_eq!(syncer.client(&device).unwrap_err().code(), "not_configured");

    let configure = |url: &str, token: &str| {
        lock(&device)
            .set_config(&crate::ConfigUpdate {
                server_url: Some(Some(url.into())),
                api_token: Some(Some(token.into())),
                discard_pending: false,
            })
            .unwrap();
    };
    configure("https://zunder.example.org/", "zt_one");
    let first = syncer.client(&device).unwrap();
    assert_eq!(first.remote().server_url, "https://zunder.example.org");
    assert_eq!(first.remote().api_token, "zt_one");
    assert_eq!(syncer.client(&device).unwrap().remote(), first.remote());

    // A new token takes effect for the next call without a restart.
    configure("https://zunder.example.org", "zt_two");
    assert_eq!(syncer.client(&device).unwrap().remote().api_token, "zt_two");
}

#[test]
fn retry_delays_back_off_to_the_regular_interval() {
    let secs: Vec<u64> = (0..6).map(|n| retry_delay(n).as_secs()).collect();
    assert_eq!(secs, vec![30, 5, 10, 20, 30, 30]);
    assert_eq!(retry_delay(u32::MAX), SYNC_INTERVAL);
}
