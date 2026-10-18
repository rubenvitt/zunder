use std::sync::Mutex;

use zunder_core::hlc::system_time_ms;
use zunder_core::ids::Id;
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
    assert!(err.error.is_network());
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

/// One local change with at least `min_ops` operations (new tasks).
fn change(ui: &mut Author, prefix: &str, min_ops: usize) -> Vec<zunder_core::Op> {
    let per_task = ui
        .ops(|b| {
            b.create_task(NewTask::new("Probe"));
        })
        .len();
    ui.ops(|b| {
        for i in 0..min_ops.div_ceil(per_task) {
            b.create_task(NewTask::new(format!("{prefix} {i}")));
        }
    })
}

#[tokio::test]
async fn a_change_larger_than_a_push_batch_goes_into_one_push() {
    let server = FakeServer::new();
    let device = store();
    let mut ui = Author::new("web-a");
    let big = change(&mut ui, "Groß", PUSH_BATCH + 1);
    let small = change(&mut ui, "Klein", 1);
    apply(&device, &big);
    apply(&device, &small);

    let report = sync_once(&device, &server, now()).await.unwrap();
    assert_eq!(server.push_sizes(), vec![big.len(), small.len()]);
    assert_eq!(report.pushed, big.len() + small.len());
    assert_eq!(report.pending, 0);
    assert_eq!(lock(&device).state(), &server.state());
}

#[tokio::test]
async fn several_changes_share_a_push_without_being_split() {
    let server = FakeServer::new();
    let device = store();
    let mut ui = Author::new("web-a");
    // Three changes of 200+ ops: two fit into a push, the third would
    // overflow it and waits for the next one instead of being cut.
    let changes: Vec<_> = (0..3)
        .map(|i| change(&mut ui, &format!("Änderung {i}"), 200))
        .collect();
    let [a, b, c] = [&changes[0], &changes[1], &changes[2]].map(Vec::len);
    assert!(a + b <= PUSH_BATCH && a + b + c > PUSH_BATCH);
    for ops in &changes {
        apply(&device, ops);
    }

    let report = sync_once(&device, &server, now()).await.unwrap();
    assert_eq!(server.push_sizes(), vec![a + b, c]);
    assert_eq!(report.pushed, a + b + c);
    assert_eq!(lock(&device).state(), &server.state());
}

#[tokio::test]
async fn an_invalid_later_change_does_not_cost_an_earlier_one() {
    let server = FakeServer::new();
    let device = store();
    let mut ui = Author::new("web-a");
    let mut other = Author::new("web-b");

    let (area, create_area) = ui.create(|b| b.create_area(NewArea::new("Haushalt")));
    let (task, create_task) = ui.create(|b| b.create_task(NewTask::new("Fenster")));
    apply(&device, &[create_area, create_task].concat());
    sync_once(&device, &server, now()).await.unwrap();
    server.accept(&other.ops(|b| {
        b.delete(zunder_core::EntityKind::Area, area);
    }));

    // Offline: rename the task, then file it into the (locally live) area.
    let rename = ui.ops(|b| {
        b.set_task(task, f::TITLE, "Fenster putzen");
    });
    let file = ui.ops(|b| {
        b.move_task(task, Location::Area { id: area });
    });
    apply(&device, &rename);
    apply(&device, &file);

    // Both touch the task: two pushes, so only the move is rejected.
    let report = sync_once(&device, &server, now()).await.unwrap();
    assert_eq!(&server.push_sizes()[1..], [rename.len(), file.len()]);
    assert_eq!(report.pushed, rename.len());
    assert_eq!(report.rejected.len(), file.len());
    assert!(report
        .rejected
        .iter()
        .all(|r| r.code == RejectCode::InvalidReference));
    let s = lock(&device);
    assert_eq!(s.pending_count(), 0);
    assert_eq!(s.state().task(task).unwrap().title, "Fenster putzen");
    assert_eq!(s.state(), &server.state());
}

#[tokio::test]
async fn a_push_refused_as_a_whole_is_retried_change_by_change() {
    for status in [400, 413] {
        let server = FakeServer::new();
        let device = store();
        let mut ui = Author::new("web-a");
        let changes: Vec<_> = ["A", "B", "C"]
            .map(|title| ui.create(|b| b.create_task(NewTask::new(title))))
            .into();
        for (_, ops) in &changes {
            apply(&device, ops);
        }
        let [(a, a_ops), (b, b_ops), (c, c_ops)] = &changes[..] else {
            unreachable!()
        };
        // Something in B makes the server refuse every request holding it.
        server.refuse_pushes_with(status, b_ops.iter().map(|op| op.op_id));

        let report = sync_once(&device, &server, now()).await.unwrap();
        assert_eq!(
            server.push_sizes(),
            [
                a_ops.len() + b_ops.len() + c_ops.len(),
                a_ops.len(),
                b_ops.len(),
                c_ops.len()
            ],
            "status {status}"
        );
        assert_eq!(report.pushed, a_ops.len() + c_ops.len());
        let rejected: Vec<_> = report.rejected.iter().map(|r| r.op_id).collect();
        let b_ids: Vec<_> = b_ops.iter().map(|op| op.op_id).collect();
        assert_eq!(rejected, b_ids);
        assert!(report.changed);
        assert_eq!(report.pending, 0);
        let s = lock(&device);
        assert!(s.state().task(*a).is_some());
        assert!(s.state().task(*b).is_none(), "rolled back");
        assert!(s.state().task(*c).is_some());
        assert_eq!(s.state(), &server.state());
    }
}

#[tokio::test]
async fn other_server_errors_keep_the_changes_pending() {
    let server = FakeServer::new();
    let device = store();
    let mut ui = Author::new("web-a");
    let (_, ops) = ui.create(|b| b.create_task(NewTask::new("A")));
    apply(&device, &ops);
    server.refuse_pushes_with(503, ops.iter().map(|op| op.op_id));

    let failure = sync_once(&device, &server, now()).await.unwrap_err();
    assert!(matches!(failure.error, Error::Server { status: 503, .. }));
    assert!(failure.report.rejected.is_empty());
    assert_eq!(failure.report.pending, ops.len());
    assert_eq!(server.push_sizes(), [ops.len()]);
}

#[tokio::test]
async fn syncer_requires_a_configured_server() {
    let device = store();
    let err = Syncer::new().sync(&device, now()).await.unwrap_err();
    assert_eq!(err.error.code(), "not_configured");
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

#[tokio::test]
async fn a_round_that_fails_after_the_push_keeps_its_rejections() {
    let server = FakeServer::new();
    let device = store();
    let mut ui = Author::new("web-a");
    let mut other = Author::new("web-b");
    let (area, create_area) = ui.create(|b| b.create_area(NewArea::new("Haushalt")));
    apply(&device, &create_area);
    sync_once(&device, &server, now()).await.unwrap();
    server.accept(&other.ops(|b| {
        b.delete(zunder_core::EntityKind::Area, area);
    }));
    let (task, create_task) = ui
        .create(|b| b.create_task(NewTask::new("Fenster putzen").at(Location::Area { id: area })));
    apply(&device, &create_task);

    // The push is rejected, then the pull fails.
    server.limit_pulls(Some(0));
    let failure = sync_once(&device, &server, now()).await.unwrap_err();
    assert!(failure.error.is_network());
    assert_eq!(failure.report.rejected.len(), create_task.len());
    assert!(failure.report.changed, "the rollback changed the view");
    assert_eq!(failure.report.pending, 0);
    assert!(lock(&device).state().task(task).is_none());
    assert!(!lock(&device).last_sync().unwrap().unwrap().ok);
}

#[tokio::test]
async fn a_round_that_fails_on_a_later_page_reports_the_earlier_ones() {
    let server = FakeServer::with_page_size(3);
    let mut other = Author::new("web-b");
    let (_, early) = other.create(|b| b.create_task(NewTask::new("Früh")));
    server.accept(&early);
    let device = store();
    sync_once(&device, &server, now()).await.unwrap();

    let (task, ops) = other.create(|b| b.create_task(NewTask::new("Seitenweise")));
    assert!(ops.len() > 2, "needs several pages (2 new ops each)");
    server.accept(&ops);
    server.limit_pulls(Some(1));
    let failure = sync_once(&device, &server, now()).await.unwrap_err();
    assert!(failure.error.is_network());
    assert_eq!(failure.report.pulled, 2);
    assert!(failure.report.changed);
    assert_eq!(failure.report.cursor, early.len() as i64 + 2);
    assert!(lock(&device).state().task(task).is_some());

    server.limit_pulls(None);
    let report = sync_once(&device, &server, now()).await.unwrap();
    assert_eq!(report.pulled, ops.len() - 2);
    assert_eq!(lock(&device).state(), &server.state());
}

/// Two devices that both pulled the first `ops` from `server`.
async fn synced_device(server: &FakeServer, ui: &mut Author) -> (Mutex<LocalStore>, Id) {
    let device = store();
    let (task, ops) = ui.create(|b| b.create_task(NewTask::new("Vor dem Backup")));
    apply(&device, &ops);
    sync_once(&device, server, now()).await.unwrap();
    // A second round pulls the device's own ops back (cursor anchor).
    sync_once(&device, server, now()).await.unwrap();
    (device, task)
}

#[tokio::test]
async fn a_restored_server_with_new_ops_at_reused_seqs_is_detected() {
    let server = FakeServer::new();
    let mut ui = Author::new("web-a");
    let (device, kept) = synced_device(&server, &mut ui).await;
    let backup = server.cursor() as usize;
    let (lost, after_backup) = ui.create(|b| b.create_task(NewTask::new("Nach dem Backup")));
    apply(&device, &after_backup);
    sync_once(&device, &server, now()).await.unwrap();
    let device_cursor = lock(&device).cursor();

    // Restore, then another client writes at least as many ops as were
    // lost: the server's cursor is back at (or beyond) the device's.
    server.restore(backup);
    let mut other = Author::new("web-b");
    let mut fresh = Vec::new();
    while (server.cursor()) < device_cursor {
        let (id, ops) = other.create(|b| b.create_task(NewTask::new("Neu auf dem Server")));
        server.accept(&ops);
        fresh.push(id);
    }

    let report = sync_once(&device, &server, now()).await.unwrap();
    assert!(report.resynced);
    assert!(report.changed);
    let s = lock(&device);
    assert_eq!(s.state(), &server.state());
    assert!(s.state().task(kept).is_some());
    assert!(s.state().task(lost).is_none(), "gone with the restore");
    assert!(fresh.iter().all(|id| s.state().task(*id).is_some()));
    assert_eq!(s.cursor(), server.cursor());
}

#[tokio::test]
async fn a_restored_server_behind_the_cursor_is_detected_and_pending_ops_survive() {
    let server = FakeServer::new();
    let mut ui = Author::new("web-a");
    let (device, kept) = synced_device(&server, &mut ui).await;
    let backup = server.cursor() as usize;
    let (_, after_backup) = ui.create(|b| b.create_task(NewTask::new("Nach dem Backup")));
    apply(&device, &after_backup);
    sync_once(&device, &server, now()).await.unwrap();
    server.restore(backup);

    // Pending while the server is being restored.
    let rename = ui.ops(|b| {
        b.set_task(kept, f::TITLE, "Umbenannt");
    });
    apply(&device, &rename);

    let report = sync_once(&device, &server, now()).await.unwrap();
    assert!(report.resynced);
    assert_eq!(report.pushed, rename.len());
    let s = lock(&device);
    assert_eq!(s.state(), &server.state());
    assert_eq!(s.state().task(kept).unwrap().title, "Umbenannt");
    assert_eq!(s.cursor(), server.cursor());
}

#[tokio::test]
async fn a_restore_right_after_the_first_snapshot_is_detected() {
    let server = FakeServer::new();
    let mut other = Author::new("web-b");
    server.accept(&other.create(|b| b.create_task(NewTask::new("A"))).1);
    let backup = server.cursor() as usize;
    let (lost, ops) = other.create(|b| b.create_task(NewTask::new("B")));
    server.accept(&ops);
    let device = store();
    let report = sync_once(&device, &server, now()).await.unwrap();
    assert_eq!(report.pulled, 0, "the overlap is not counted");
    assert!(lock(&device).state().task(lost).is_some());

    server.restore(backup);
    while server.cursor() <= lock(&device).cursor() {
        server.accept(&other.create(|b| b.create_task(NewTask::new("Neu"))).1);
    }
    let report = sync_once(&device, &server, now()).await.unwrap();
    assert!(report.resynced);
    assert!(lock(&device).state().task(lost).is_none());
    assert_eq!(lock(&device).state(), &server.state());
}

#[tokio::test]
async fn an_unchanged_server_is_not_resynced() {
    let server = FakeServer::with_page_size(2);
    let mut ui = Author::new("web-a");
    let (device, _) = synced_device(&server, &mut ui).await;
    let mut other = Author::new("web-b");
    let (_, ops) = other.create(|b| b.create_task(NewTask::new("Weiter")));
    server.accept(&ops);
    let report = sync_once(&device, &server, now()).await.unwrap();
    assert!(!report.resynced);
    assert_eq!(report.pulled, ops.len(), "the overlap is not counted");
    assert_eq!(server.counts().1, 1, "no second snapshot");
    assert_eq!(lock(&device).state(), &server.state());
}

/// Switches the store to another server while answering one request.
struct Switching<'a> {
    server: &'a FakeServer,
    store: &'a Mutex<LocalStore>,
    during: &'static str,
}

impl Switching<'_> {
    fn switch_if(&self, step: &str) {
        if self.during == step {
            lock(self.store)
                .set_config(&crate::ConfigUpdate {
                    server_url: Some(Some("https://b.example.org".into())),
                    api_token: None,
                    discard_pending: true,
                })
                .unwrap();
        }
    }
}

impl SyncApi for Switching<'_> {
    async fn snapshot(&self) -> Result<crate::wire::ServerSnapshot> {
        let response = self.server.snapshot().await;
        self.switch_if("snapshot");
        response
    }

    async fn push(
        &self,
        client_id: &str,
        ops: &[zunder_core::Op],
    ) -> Result<crate::wire::PushResponse> {
        let response = self.server.push(client_id, ops).await;
        self.switch_if("push");
        response
    }

    async fn pull(&self, since: i64, limit: u32) -> Result<crate::wire::PullPage> {
        let response = self.server.pull(since, limit).await;
        self.switch_if("pull");
        response
    }
}

#[tokio::test]
async fn a_server_switch_during_a_round_discards_its_results() {
    for during in ["push", "snapshot", "pull"] {
        let server = FakeServer::new();
        let mut other = Author::new("web-b");
        server.accept(&other.create(|b| b.create_task(NewTask::new("Alt"))).1);
        let device = store();
        lock(&device)
            .set_config(&crate::ConfigUpdate {
                server_url: Some(Some("https://a.example.org".into())),
                ..crate::ConfigUpdate::default()
            })
            .unwrap();
        if during == "pull" {
            sync_once(&device, &server, now()).await.unwrap();
            server.accept(&other.create(|b| b.create_task(NewTask::new("Neu"))).1);
        }
        let mut ui = Author::new("web-a");
        apply(
            &device,
            &ui.create(|b| b.create_task(NewTask::new("Mein"))).1,
        );

        let api = Switching {
            server: &server,
            store: &device,
            during,
        };
        let failure = sync_once(&device, &api, now()).await.unwrap_err();
        assert_eq!(failure.error.code(), "reconfigured", "{during}");
        let s = lock(&device);
        assert_eq!(
            s.config().unwrap().server_url.as_deref(),
            Some("https://b.example.org")
        );
        assert_eq!(s.cursor(), 0, "{during}");
        assert!(s.needs_snapshot(), "{during}");
        assert!(s.state().is_empty(), "{during}");
        assert!(s.confirmed_state().is_empty(), "{during}");
        assert!(s.last_sync().unwrap().is_none(), "{during}");
    }
}

#[test]
fn retry_delays_back_off_to_the_regular_interval() {
    let secs: Vec<u64> = (0..6).map(|n| retry_delay(n).as_secs()).collect();
    assert_eq!(secs, vec![30, 5, 10, 20, 30, 30]);
    assert_eq!(retry_delay(u32::MAX), SYNC_INTERVAL);
}
