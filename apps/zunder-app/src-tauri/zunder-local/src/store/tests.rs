use serde_json::json;
use zunder_core::hlc::system_time_ms;
use zunder_core::model::fields::task as f;
use zunder_core::{NewTask, RejectCode};

use super::*;
use crate::testutil::Author;
use crate::wire::PulledOp;

fn now() -> u64 {
    system_time_ms()
}

fn title(store: &LocalStore, id: Uuid) -> Option<String> {
    store.state().task(id).map(|t| t.title)
}

fn accept_all(ops: &[Op], cursor: i64) -> PushResponse {
    PushResponse {
        accepted: ops.iter().map(|o| o.op_id).collect(),
        cursor,
        ..PushResponse::default()
    }
}

fn page(ops: &[Op], first_seq: i64, has_more: bool) -> PullPage {
    let ops: Vec<PulledOp> = ops
        .iter()
        .enumerate()
        .map(|(i, op)| PulledOp {
            seq: first_seq + i as i64,
            op: op.clone(),
        })
        .collect();
    PullPage {
        cursor: ops.last().map_or(first_seq - 1, |p| p.seq),
        ops,
        has_more,
    }
}

#[test]
fn local_ops_are_applied_and_queued() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let mut web = Author::new("web-a");
    let (id, ops) = web.create(|b| b.create_task(NewTask::new("Zahnarzt anrufen")));

    let result = store.apply_local(&ops, now()).unwrap();
    assert_eq!(result.accepted.len(), ops.len());
    assert!(result.rejected.is_empty());
    assert_eq!(result.pending, ops.len());
    assert_eq!(title(&store, id).as_deref(), Some("Zahnarzt anrufen"));
    assert!(store.confirmed_state().is_empty());
    assert_eq!(store.pending_batch(2), ops[..2].to_vec());

    let snapshot = store.snapshot();
    assert_eq!(snapshot.cursor, 0);
    assert_eq!(snapshot.pending, ops.len());
    assert_eq!(snapshot.entities.len(), 1);
    assert_eq!(snapshot.entities[0].entity_id, id);
}

#[test]
fn invalid_local_ops_are_rejected_and_not_queued() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let mut web = Author::new("web-a");
    // A task without title.
    let ops = web.ops(|b| {
        b.set_task(zunder_core::ids::new_id(), f::STATUS, "open");
    });
    let result = store.apply_local(&ops, now()).unwrap();
    assert!(result.accepted.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert_eq!(result.rejected[0].code, RejectCode::MissingRequired);
    assert_eq!(result.pending, 0);
    assert!(store.state().is_empty());
}

#[test]
fn repeated_local_ops_are_accepted_once() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let mut web = Author::new("web-a");
    let (_, ops) = web.create(|b| b.create_task(NewTask::new("A")));
    store.apply_local(&ops, now()).unwrap();
    let again = store.apply_local(&ops, now()).unwrap();
    assert_eq!(again.accepted.len(), ops.len());
    assert_eq!(again.pending, ops.len());
}

#[test]
fn everything_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zunder.db");
    let mut web = Author::new("web-a");
    let (confirmed_id, confirmed_ops) = web.create(|b| b.create_task(NewTask::new("Bestätigt")));
    let (pending_id, pending_ops) = web.create(|b| b.create_task(NewTask::new("Offen")));

    let node_id = {
        let mut store = LocalStore::open(&path).unwrap();
        store.apply_local(&confirmed_ops, now()).unwrap();
        store
            .apply_push_response(&confirmed_ops, &accept_all(&confirmed_ops, 3))
            .unwrap();
        store
            .apply_pull_page(&page(&confirmed_ops, 1, false))
            .unwrap();
        store.apply_local(&pending_ops, now()).unwrap();
        store.node_id().clone()
    };

    let store = LocalStore::open(&path).unwrap();
    assert_eq!(store.node_id(), &node_id);
    assert!(store.node_id().as_str().starts_with("app-"));
    assert_eq!(store.cursor(), confirmed_ops.len() as i64);
    assert_eq!(store.pending_batch(usize::MAX), pending_ops);
    assert!(store.confirmed_state().task(confirmed_id).is_some());
    assert!(store.confirmed_state().task(pending_id).is_none());
    assert_eq!(title(&store, pending_id).as_deref(), Some("Offen"));
    // Confirmed op IDs are known again, so a duplicate is not re-queued.
    let mut store = store;
    let again = store.apply_local(&confirmed_ops, now()).unwrap();
    assert_eq!(again.pending, pending_ops.len());
}

#[test]
fn accepted_ops_become_confirmed() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let mut web = Author::new("web-a");
    let (id, ops) = web.create(|b| b.create_task(NewTask::new("A")));
    store.apply_local(&ops, now()).unwrap();

    let applied = store
        .apply_push_response(&ops, &accept_all(&ops, 3))
        .unwrap();
    assert_eq!(applied.accepted, ops.len());
    assert!(!applied.view_changed);
    assert_eq!(store.pending_count(), 0);
    assert_eq!(
        store.confirmed_state().task(id).map(|t| t.title).as_deref(),
        Some("A")
    );
    // The push cursor is not taken over: other clients' ops may lie between.
    assert_eq!(store.cursor(), 0);
}

#[test]
fn rejected_ops_are_rolled_back() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let mut web = Author::new("web-a");
    let (id, create) = web.create(|b| b.create_task(NewTask::new("Alt")));
    store.apply_local(&create, now()).unwrap();
    store
        .apply_push_response(&create, &accept_all(&create, 3))
        .unwrap();

    let rename = web.ops(|b| {
        b.set_task(id, f::TITLE, "Neu");
    });
    store.apply_local(&rename, now()).unwrap();
    assert_eq!(title(&store, id).as_deref(), Some("Neu"));

    let rejection = Rejection {
        op_id: rename[0].op_id,
        code: RejectCode::InvalidValue,
        message: "abgelehnt".into(),
    };
    let applied = store
        .apply_push_response(
            &rename,
            &PushResponse {
                rejected: vec![rejection.clone()],
                ..PushResponse::default()
            },
        )
        .unwrap();
    assert_eq!(applied.rejected, vec![rejection]);
    assert!(applied.view_changed);
    assert_eq!(store.pending_count(), 0);
    assert_eq!(title(&store, id).as_deref(), Some("Alt"));
}

#[test]
fn rollback_keeps_other_pending_ops() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let mut web = Author::new("web-a");
    let (a, create_a) = web.create(|b| b.create_task(NewTask::new("A")));
    let (b_id, create_b) = web.create(|b| b.create_task(NewTask::new("B")));
    store.apply_local(&create_a, now()).unwrap();
    store.apply_local(&create_b, now()).unwrap();

    // The server only judged A's ops (rejecting them), B stays pending.
    let response = PushResponse {
        rejected: create_a
            .iter()
            .map(|op| Rejection {
                op_id: op.op_id,
                code: RejectCode::EntityRejected,
                message: "x".into(),
            })
            .collect(),
        ..PushResponse::default()
    };
    store.apply_push_response(&create_a, &response).unwrap();
    assert!(store.state().task(a).is_none());
    assert_eq!(title(&store, b_id).as_deref(), Some("B"));
    assert_eq!(store.pending_batch(usize::MAX), create_b);
}

#[test]
fn pulled_ops_merge_and_advance_the_cursor() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let mut other = Author::new("web-b");
    let (id, ops) = other.create(|b| b.create_task(NewTask::new("Von woanders")));

    let changed = store.apply_pull_page(&page(&ops, 1, false)).unwrap();
    assert!(changed);
    assert_eq!(store.cursor(), ops.len() as i64);
    assert_eq!(title(&store, id).as_deref(), Some("Von woanders"));
    assert!(store.confirmed_state().task(id).is_some());

    // Pulling the same page again changes nothing.
    assert!(!store.apply_pull_page(&page(&ops, 1, false)).unwrap());
}

#[test]
fn newer_pending_ops_win_over_older_pulled_ones() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let mut other = Author::new("web-b");
    let mut me = Author::new("web-a");
    let (id, create) = other.create(|b| b.create_task(NewTask::new("A")));
    store.apply_pull_page(&page(&create, 1, false)).unwrap();

    let old_remote = other.ops(|b| {
        b.set_task(id, f::TITLE, "Älter");
    });
    me.receive(&old_remote);
    let mine = me.ops(|b| {
        b.set_task(id, f::TITLE, "Neuer");
    });
    store.apply_local(&mine, now()).unwrap();
    store
        .apply_pull_page(&page(&old_remote, create.len() as i64 + 1, false))
        .unwrap();
    assert_eq!(title(&store, id).as_deref(), Some("Neuer"));
}

#[test]
fn a_pending_op_that_comes_back_through_pull_is_confirmed() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let mut web = Author::new("web-a");
    let (_, ops) = web.create(|b| b.create_task(NewTask::new("A")));
    store.apply_local(&ops, now()).unwrap();
    // The push response was lost, but the server stored the ops.
    store.apply_pull_page(&page(&ops, 1, false)).unwrap();
    assert_eq!(store.pending_count(), 0);
    let pending = load_ops(&store.conn, OpStatus::Pending).unwrap();
    assert!(pending.is_empty());
    let confirmed = load_ops(&store.conn, OpStatus::Confirmed).unwrap();
    assert_eq!(confirmed, ops);
}

#[test]
fn server_snapshot_initializes_the_store() {
    let mut store = LocalStore::open_in_memory().unwrap();
    assert!(store.needs_snapshot());
    let mut other = Author::new("web-b");
    let (id, ops) = other.create(|b| b.create_task(NewTask::new("Aus dem Snapshot")));
    let server = State::from_ops(&ops);

    let changed = store
        .apply_server_snapshot(&ServerSnapshot {
            cursor: 42,
            entities: server.to_snapshot(),
        })
        .unwrap();
    assert!(changed);
    assert!(!store.needs_snapshot());
    assert_eq!(store.cursor(), 42);
    assert_eq!(title(&store, id).as_deref(), Some("Aus dem Snapshot"));
    assert_eq!(store.confirmed_state(), &server);
}

#[test]
fn config_defaults_and_updates() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let config = store.config().unwrap();
    assert_eq!(config.server_url, None);
    assert!(!config.has_api_token);
    assert!(config.last_sync.is_none());
    assert!(store.remote().unwrap().is_none());

    let config = store
        .set_config(&ConfigUpdate {
            server_url: Some(Some(" https://zunder.example.org/ ".into())),
            ..ConfigUpdate::default()
        })
        .unwrap();
    assert_eq!(
        config.server_url.as_deref(),
        Some("https://zunder.example.org")
    );
    assert!(store.remote().unwrap().is_none(), "token still missing");

    let config = store
        .set_config(&ConfigUpdate {
            api_token: Some(Some("zt_geheim".into())),
            ..ConfigUpdate::default()
        })
        .unwrap();
    assert!(config.has_api_token);
    assert_eq!(
        config.server_url.as_deref(),
        Some("https://zunder.example.org")
    );
    assert_eq!(
        store.remote().unwrap(),
        Some(Remote {
            server_url: "https://zunder.example.org".into(),
            api_token: "zt_geheim".into(),
        })
    );
    // The token is never part of the config view.
    assert!(!serde_json::to_string(&config).unwrap().contains("geheim"));

    let config = store
        .set_config(&ConfigUpdate {
            api_token: Some(None),
            ..ConfigUpdate::default()
        })
        .unwrap();
    assert!(!config.has_api_token);
}

#[test]
fn config_update_distinguishes_null_from_absent() {
    let update: ConfigUpdate = serde_json::from_value(json!({"api_token": null})).unwrap();
    assert_eq!(update.server_url, None);
    assert_eq!(update.api_token, Some(None));
    assert!(serde_json::from_value::<ConfigUpdate>(json!({"token": "x"})).is_err());
}

#[test]
fn invalid_config_values_are_rejected() {
    let mut store = LocalStore::open_in_memory().unwrap();
    for url in [
        "zunder.example.org",
        "ftp://zunder.example.org",
        "https://zunder.example.org/?a=1",
        "https://user:pw@zunder.example.org",
    ] {
        let err = store
            .set_config(&ConfigUpdate {
                server_url: Some(Some(url.into())),
                ..ConfigUpdate::default()
            })
            .unwrap_err();
        assert_eq!(err.code(), "invalid_config", "{url}");
    }
    for token in ["", "   ", "zt_a b"] {
        let err = store
            .set_config(&ConfigUpdate {
                api_token: Some(Some(token.into())),
                ..ConfigUpdate::default()
            })
            .unwrap_err();
        assert_eq!(err.code(), "invalid_config", "{token:?}");
    }
    assert_eq!(store.config().unwrap().server_url, None);
}

#[test]
fn switching_servers_resets_local_data() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let set_url = |url: &str, discard_pending: bool| ConfigUpdate {
        server_url: Some(Some(url.into())),
        discard_pending,
        ..ConfigUpdate::default()
    };
    store
        .set_config(&set_url("https://a.example.org", false))
        .unwrap();
    let mut web = Author::new("web-a");
    let (_, synced) = web.create(|b| b.create_task(NewTask::new("Synchron")));
    store.apply_pull_page(&page(&synced, 1, false)).unwrap();
    let (_, pending) = web.create(|b| b.create_task(NewTask::new("Offen")));
    store.apply_local(&pending, now()).unwrap();

    // Same server (normalized): nothing happens.
    store
        .set_config(&set_url("https://a.example.org/", false))
        .unwrap();
    assert_eq!(store.pending_count(), pending.len());

    let err = store
        .set_config(&set_url("https://b.example.org", false))
        .unwrap_err();
    assert_eq!(err.code(), "pending_ops");
    assert_eq!(
        store.config().unwrap().server_url.as_deref(),
        Some("https://a.example.org")
    );

    let config = store
        .set_config(&set_url("https://b.example.org", true))
        .unwrap();
    assert_eq!(config.server_url.as_deref(), Some("https://b.example.org"));
    assert_eq!(config.cursor, 0);
    assert_eq!(config.pending, 0);
    assert!(store.state().is_empty());
    assert!(store.confirmed_state().is_empty());
    assert!(load_ops(&store.conn, OpStatus::Confirmed)
        .unwrap()
        .is_empty());
}

#[test]
fn last_sync_is_recorded() {
    let store = LocalStore::open_in_memory().unwrap();
    let last = LastSync {
        at_ms: 1_000,
        ok: false,
        error: Some("Server nicht erreichbar".into()),
    };
    store.record_sync(&last).unwrap();
    assert_eq!(store.config().unwrap().last_sync, Some(last));
}

#[test]
fn scheduled_reminders_round_trip() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let scheduled = vec![
        ScheduledReminder {
            id: 7,
            reminder: Reminder {
                task_id: Uuid::from_u128(1),
                kind: ReminderKind::Remind,
                at_ms: 2_000,
                title: "A".into(),
                body: "Erinnerung".into(),
            },
        },
        ScheduledReminder {
            id: 3,
            reminder: Reminder {
                task_id: Uuid::from_u128(1),
                kind: ReminderKind::Due,
                at_ms: 1_000,
                title: "A".into(),
                body: "Jetzt fällig".into(),
            },
        },
    ];
    store.save_scheduled_reminders(&scheduled).unwrap();
    let loaded = store.scheduled_reminders().unwrap();
    assert_eq!(loaded, vec![scheduled[1].clone(), scheduled[0].clone()]);
    store.save_scheduled_reminders(&[]).unwrap();
    assert!(store.scheduled_reminders().unwrap().is_empty());
}

#[test]
fn a_newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zunder.db");
    drop(LocalStore::open(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    drop(conn);
    let err = LocalStore::open(&path).err().unwrap();
    assert_eq!(err.code(), "schema_too_new");
}
