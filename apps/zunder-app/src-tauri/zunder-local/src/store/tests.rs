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

fn title(store: &LocalStore, id: Id) -> Option<String> {
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
                task_id: Id::parse(&format!("t{:023}", 1)).unwrap(),
                kind: ReminderKind::Remind,
                at_ms: 2_000,
                title: "A".into(),
                body: "Erinnerung".into(),
            },
        },
        ScheduledReminder {
            id: 3,
            reminder: Reminder {
                task_id: Id::parse(&format!("t{:023}", 1)).unwrap(),
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

#[test]
fn clearing_the_url_does_not_hide_a_server_switch() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let set_url = |url: Option<&str>, discard_pending: bool| ConfigUpdate {
        server_url: Some(url.map(Into::into)),
        discard_pending,
        ..ConfigUpdate::default()
    };
    store
        .set_config(&set_url(Some("https://a.example.org"), false))
        .unwrap();
    let mut web = Author::new("web-a");
    let (_, synced) = web.create(|b| b.create_task(NewTask::new("Synchron")));
    store.apply_pull_page(&page(&synced, 1, false)).unwrap();
    let (_, pending) = web.create(|b| b.create_task(NewTask::new("Offen")));
    store.apply_local(&pending, now()).unwrap();

    // Clearing the address keeps the data (it still belongs to server A).
    store.set_config(&set_url(None, false)).unwrap();
    assert_eq!(store.cursor(), synced.len() as i64);
    assert_eq!(store.pending_count(), pending.len());

    let err = store
        .set_config(&set_url(Some("https://b.example.org"), false))
        .unwrap_err();
    assert_eq!(err.code(), "pending_ops");
    assert_eq!(store.config().unwrap().server_url, None);

    let config = store
        .set_config(&set_url(Some("https://b.example.org"), true))
        .unwrap();
    assert_eq!(config.cursor, 0);
    assert_eq!(config.pending, 0);
    assert!(store.state().is_empty());
}

#[test]
fn data_created_before_any_server_goes_to_the_first_one() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let mut web = Author::new("web-a");
    let (id, pending) = web.create(|b| b.create_task(NewTask::new("Offline zuerst")));
    store.apply_local(&pending, now()).unwrap();
    store
        .set_config(&ConfigUpdate {
            server_url: Some(Some("https://a.example.org".into())),
            ..ConfigUpdate::default()
        })
        .unwrap();
    assert_eq!(store.pending_count(), pending.len());
    assert!(store.state().task(id).is_some());
}

#[test]
fn switching_servers_bumps_the_generation() {
    let mut store = LocalStore::open_in_memory().unwrap();
    let set = |url: &str| ConfigUpdate {
        server_url: Some(Some(url.into())),
        ..ConfigUpdate::default()
    };
    let start = store.generation();
    store.set_config(&set("https://a.example.org")).unwrap();
    assert_eq!(store.generation(), start, "first server: no switch");
    store
        .set_config(&ConfigUpdate {
            api_token: Some(Some("zt_neu".into())),
            ..ConfigUpdate::default()
        })
        .unwrap();
    assert_eq!(store.generation(), start, "new token, same server");
    store.set_config(&set("https://b.example.org")).unwrap();
    assert!(store.generation() > start);
}

/// Makes every write to `entities` fail, like a full disk would.
fn break_entity_writes(store: &LocalStore) {
    store
        .conn
        .execute_batch(
            "CREATE TRIGGER fail_insert BEFORE INSERT ON entities
             BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END;
             CREATE TRIGGER fail_update BEFORE UPDATE ON entities
             BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END;",
        )
        .unwrap();
}

fn repair_entity_writes(store: &LocalStore) {
    store
        .conn
        .execute_batch("DROP TRIGGER fail_insert; DROP TRIGGER fail_update;")
        .unwrap();
}

#[test]
fn a_failed_pull_commit_is_retried_completely() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zunder.db");
    let mut other = Author::new("web-b");
    let (id, ops) = other.create(|b| b.create_task(NewTask::new("Von woanders")));
    {
        let mut store = LocalStore::open(&path).unwrap();
        break_entity_writes(&store);
        assert!(store.apply_pull_page(&page(&ops, 1, false)).is_err());
        assert_eq!(store.cursor(), 0);
        assert!(store.state().task(id).is_none(), "memory follows the disk");
        repair_entity_writes(&store);
        assert!(store.apply_pull_page(&page(&ops, 1, false)).unwrap());
    }
    let store = LocalStore::open(&path).unwrap();
    assert_eq!(store.cursor(), ops.len() as i64);
    assert_eq!(title(&store, id).as_deref(), Some("Von woanders"));
    assert!(store.confirmed_state().task(id).is_some());
}

#[test]
fn a_failed_push_commit_is_retried_completely() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zunder.db");
    let mut web = Author::new("web-a");
    let (id, ops) = web.create(|b| b.create_task(NewTask::new("Eigene")));
    {
        let mut store = LocalStore::open(&path).unwrap();
        store.apply_local(&ops, now()).unwrap();
        break_entity_writes(&store);
        assert!(store
            .apply_push_response(&ops, &accept_all(&ops, 3))
            .is_err());
        assert!(store.confirmed_state().task(id).is_none());
        assert_eq!(store.pending_count(), ops.len());
        assert_eq!(title(&store, id).as_deref(), Some("Eigene"));
        repair_entity_writes(&store);
        store
            .apply_push_response(&ops, &accept_all(&ops, 3))
            .unwrap();
    }
    let store = LocalStore::open(&path).unwrap();
    assert_eq!(store.pending_count(), 0);
    assert!(store.confirmed_state().task(id).is_some());
}

#[cfg(unix)]
#[test]
fn database_files_are_private() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zunder.db");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    {
        let mut store = LocalStore::open(&path).unwrap();
        store
            .set_config(&ConfigUpdate {
                api_token: Some(Some("zt_geheim".into())),
                ..ConfigUpdate::default()
            })
            .unwrap();
        for p in [path.clone(), dir.path().join("zunder.db-wal")] {
            assert_eq!(mode(&p) & 0o077, 0, "{}: {:o}", p.display(), mode(&p));
        }
    }
    // An existing, too open file is tightened on the next start.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    drop(LocalStore::open(&path).unwrap());
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn plain_http_only_for_local_hosts() {
    for url in [
        "http://localhost:8080",
        "http://zunder.localhost",
        "http://127.0.0.1:8080",
        "http://[::1]:8080",
        "http://192.168.1.5",
        "http://10.0.0.2:8080/zunder",
        "http://172.16.4.1",
        "http://100.101.102.103",
        "http://169.254.1.1",
        "http://[fd00::5]",
        "http://nas:8080",
        "http://nas.local",
        "http://zunder.home.arpa",
        "https://zunder.example.org",
        "https://8.8.8.8",
    ] {
        assert!(normalize_server_url(url).is_ok(), "{url}");
    }
    for url in [
        "http://zunder.example.org",
        "http://8.8.8.8",
        "http://[2001:db8::1]",
        "http://zunder.rubeen.dev",
    ] {
        let err = normalize_server_url(url).unwrap_err();
        assert_eq!(err.code(), "invalid_config", "{url}");
        assert!(err.to_string().contains("https"), "{err}");
    }
}

#[test]
fn legacy_uuids_are_rewritten_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zunder.db");
    drop(LocalStore::open(&path).unwrap());
    let parent = "0192a000-0000-7000-8000-0c0000000001";
    let child = "0192a000-0000-7000-8000-0c0000000002";
    let new = |s: &str| Id::parse_any(s).unwrap();
    {
        // A store from before CUID2: confirmed parent, pending subtask.
        let conn = Connection::open(&path).unwrap();
        let insert = |op_id: &str,
                      entity_id: &str,
                      field: &str,
                      value: &str,
                      status: &str,
                      seq: Option<i64>| {
            conn.execute(
                "INSERT INTO ops (op_id, entity, entity_id, field, value_json, hlc, status, server_seq)
                 VALUES (?1, 'task', ?2, ?3, ?4, '1760000000000:00000:web-a', ?5, ?6)",
                params![op_id, entity_id, field, value, status, seq],
            )
            .unwrap();
        };
        insert(
            "0192a000-0000-7000-8000-0f0000000001",
            parent,
            "title",
            "\"Eltern\"",
            "confirmed",
            Some(1),
        );
        insert(
            "0192a000-0000-7000-8000-0f0000000002",
            child,
            "title",
            "\"Kind\"",
            "pending",
            None,
        );
        insert(
            "0192a000-0000-7000-8000-0f0000000003",
            child,
            "parent_id",
            &format!("\"{parent}\""),
            "pending",
            None,
        );
        let entity = json!({
            "entity": "task", "entity_id": parent, "created_hlc": "1760000000000:00000:web-a",
            "fields": {"title": {"value": "Eltern", "hlc": "1760000000000:00000:web-a",
                                 "op_id": "0192a000-0000-7000-8000-0f0000000001"}}
        });
        conn.execute(
            "INSERT INTO entities (entity, entity_id, data_json) VALUES ('task', ?1, ?2)",
            params![parent, entity.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO reminders (notification_id, task_id, kind, at_ms, title, body)
             VALUES (7, ?1, 'due', 0, 'Kind', '')",
            [child],
        )
        .unwrap();
        set_meta(&conn, META_CURSOR, "1").unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
    }

    let store = LocalStore::open(&path).unwrap();
    assert_eq!(store.cursor(), 1, "the server keeps its seqs");
    assert_eq!(title(&store, new(parent)).as_deref(), Some("Eltern"));
    assert_eq!(title(&store, new(child)).as_deref(), Some("Kind"));
    assert_eq!(
        store.view.task(new(child)).unwrap().parent_id,
        Some(new(parent))
    );
    let pending = store.pending_batch(usize::MAX);
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().all(|op| op.entity_id == new(child)));
    assert_eq!(
        store.cursor_anchor().unwrap(),
        Some(new("0192a000-0000-7000-8000-0f0000000001"))
    );
    let task_id: String = store
        .conn
        .query_row("SELECT task_id FROM reminders", [], |r| r.get(0))
        .unwrap();
    assert_eq!(task_id, new(child).to_string());
}

#[test]
fn reminders_checked_at_is_stored() {
    let store = LocalStore::open_in_memory().unwrap();
    assert_eq!(store.reminders_checked_at().unwrap(), None);
    store.set_reminders_checked_at(1_790_000_000_000).unwrap();
    store.set_reminders_checked_at(1_790_000_030_000).unwrap();
    assert_eq!(
        store.reminders_checked_at().unwrap(),
        Some(1_790_000_030_000)
    );
}

#[test]
fn recurring_series_round_trips_and_follows_the_core_rules() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("zunder.db");
    let mut web = Author::new("web-a");
    let rhythm = json!({"every": 1, "unit": "month", "day": "last", "from": "due"});

    // The app creates a recurring task and completes it; the server
    // confirms and later sends the follow-up instance (source `server`).
    let (first, mut ops) = web.create(|b| {
        let id = b.create_task(NewTask::new("Odoo absenden und Stunden bestätigen"));
        b.set_task(id, f::DUE, "2030-01-31T09:00:00+01:00");
        b.set_task(id, f::RECURRENCE, rhythm.clone());
        id
    });
    ops.extend(web.ops(|b| {
        b.set_task(first, f::STATUS, "done");
    }));
    let mut server = Author::new("server");
    server.receive(&ops);
    let (next, follow_up) = server.create(|b| {
        let id = b.create_task(NewTask::new("Odoo absenden und Stunden bestätigen"));
        b.set_task(id, f::DUE, "2030-02-28T09:00:00+01:00");
        b.set_task(id, f::RECURRENCE, rhythm.clone());
        b.set_task(id, f::RECURRENCE_PREV, first.as_str());
        id
    });
    {
        let mut store = LocalStore::open(&path).unwrap();
        let result = store.apply_local(&ops, now()).unwrap();
        assert!(result.rejected.is_empty(), "{:?}", result.rejected);
        store
            .apply_push_response(&ops, &accept_all(&ops, 0))
            .unwrap();
        let mut log = ops.clone();
        log.extend(follow_up.iter().cloned());
        assert!(store.apply_pull_page(&page(&log, 1, false)).unwrap());
    }

    // Both fields survive a restart (entities table and op log).
    let mut store = LocalStore::open(&path).unwrap();
    for state in [store.state(), store.confirmed_state()] {
        let task = state.task(next).unwrap();
        assert_eq!(task.recurrence_prev, Some(first));
        let day = task.recurrence.as_ref().and_then(|r| r.day);
        assert_eq!(day, Some(zunder_core::MonthDay::Last));
        assert_eq!(state.task(first).unwrap().recurrence, task.recurrence);
    }
    let snapshot = serde_json::to_value(store.snapshot()).unwrap();
    let entity = snapshot["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entity_id"] == next.as_str())
        .unwrap();
    assert_eq!(entity["fields"][f::RECURRENCE]["value"], rhythm);
    assert_eq!(
        entity["fields"][f::RECURRENCE_PREV]["value"],
        first.as_str()
    );

    // Local validation applies the core's rules before anything is queued:
    // the series needs its due date, recurrence_prev is the server's.
    web.receive(&follow_up);
    let without_due = web.ops(|b| {
        b.set_task(next, f::DUE, serde_json::Value::Null);
    });
    let own_prev = web.ops(|b| {
        b.set_task(next, f::RECURRENCE_PREV, serde_json::Value::Null);
    });
    for ops in [without_due, own_prev] {
        let result = store.apply_local(&ops, now()).unwrap();
        assert_eq!(result.rejected.len(), 1, "{:?}", result.rejected);
        assert_eq!(result.rejected[0].code, RejectCode::InvalidValue);
        assert_eq!(result.pending, 0);
    }
    // Ending the series together with the due date is fine.
    let end = web.ops(|b| {
        b.set_task(next, f::DUE, serde_json::Value::Null);
        b.set_task(next, f::RECURRENCE, serde_json::Value::Null);
    });
    let result = store.apply_local(&end, now()).unwrap();
    assert!(result.rejected.is_empty(), "{:?}", result.rejected);
    assert!(store.state().task(next).unwrap().recurrence.is_none());
}
