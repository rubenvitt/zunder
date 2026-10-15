//! Recurring tasks: completing one makes the server create the next
//! instance of its series, once, as its own push (contract 3.1,
//! "Folgeinstanzen").

mod common;

use serde_json::{json, Value};
use time::{Duration, OffsetDateTime};
use zunder_core::ids::Id;
use zunder_core::model::fields;
use zunder_core::{Clock, Due, EntityKind, Location, NewTask, NodeId, Task, TaskStatus};
use zunder_server::calendar::FeedScope;

use common::{api_token, client_clock, mcp_ok, ops, Auth, TestServer, BOOTSTRAP};

const BOOT: Auth<'static> = Auth::Bearer(BOOTSTRAP);

/// Monthly on the last day; far in the future so "today" does not matter.
fn month_end() -> Value {
    json!({"every": 1, "unit": "month", "day": "last", "from": "due"})
}

fn instant(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).unwrap()
}

/// Pushes `ops` with the bootstrap token and checks that all were accepted.
async fn push(s: &TestServer, ops: &[zunder_core::Op]) {
    let r = s.push(BOOT, ops).await;
    assert_eq!(r.status, axum::http::StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["rejected"], json!([]), "{}", r.text());
}

/// Creates a recurring task (due 2099-01-31, month end) at `location`.
async fn recurring(s: &TestServer, clock: &mut Clock, title: &str, location: Location) -> Id {
    let mut id = None;
    let ops = ops(clock, |b| {
        let t = b.create_task(NewTask {
            due: Due::parse("2099-01-31"),
            ..NewTask::new(title).at(location)
        });
        b.set_task(t, fields::task::RECURRENCE, month_end());
        id = Some(t);
    });
    push(s, &ops).await;
    id.unwrap()
}

async fn set_status(s: &TestServer, clock: &mut Clock, id: Id, status: TaskStatus) {
    let ops = ops(clock, |b| {
        b.set_status(id, status);
    });
    push(s, &ops).await;
}

/// Live tasks naming `id` as their predecessor.
fn successors(s: &TestServer, id: Id) -> Vec<Task> {
    s.app.read(|st| {
        st.live_tasks()
            .filter(|t| t.recurrence_prev == Some(id))
            .collect()
    })
}

fn only_successor(s: &TestServer, id: Id) -> Task {
    let next = successors(s, id);
    assert_eq!(next.len(), 1, "{next:?}");
    next.into_iter().next().unwrap()
}

#[tokio::test]
async fn completing_creates_exactly_one_instance() {
    let s = TestServer::new().await;
    let mut clock = client_clock();
    let id = recurring(&s, &mut clock, "Odoo absenden", Location::Inbox).await;
    assert!(successors(&s, id).is_empty());

    set_status(&s, &mut clock, id, TaskStatus::Done).await;
    let next = only_successor(&s, id);
    assert_eq!(next.title, "Odoo absenden");
    assert_eq!(next.status, TaskStatus::Open);
    assert_eq!(next.location, Location::Inbox);
    assert_eq!(next.due, Due::parse("2099-02-28"));
    assert_eq!(
        next.recurrence,
        s.app.read(|st| st.task(id).unwrap().recurrence)
    );
    // The predecessor stays as it is.
    let before = s.app.read(|st| st.task(id).unwrap());
    assert_eq!(before.status, TaskStatus::Done);
    assert_eq!(before.due, Due::parse("2099-01-31"));

    // The instance is the server's change, in the history of the new task
    // and in pull.
    let history = s
        .get(&format!("/api/tasks/{}/history", next.id), BOOT)
        .await
        .json();
    let entries = history.as_array().unwrap();
    assert!(!entries.is_empty());
    assert!(entries.iter().all(|e| e["source"] == "server"), "{history}");
    assert!(entries
        .iter()
        .any(|e| e["op"]["field"] == "recurrence_prev" && e["op"]["value"] == id.to_string()));
    let pulled = s.get("/api/sync/pull?since=0", BOOT).await.json();
    assert!(pulled["ops"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["op"]["entity_id"] == next.id.to_string()));

    // Completing the instance continues the series.
    set_status(&s, &mut clock, next.id, TaskStatus::Done).await;
    assert_eq!(only_successor(&s, next.id).due, Due::parse("2099-03-31"));
}

#[tokio::test]
async fn dropping_creates_the_next_instance_too() {
    let s = TestServer::new().await;
    let mut clock = client_clock();
    let id = recurring(&s, &mut clock, "Miete", Location::Loose).await;
    set_status(&s, &mut clock, id, TaskStatus::Dropped).await;
    let next = only_successor(&s, id);
    assert_eq!(next.location, Location::Loose);
    assert_eq!(next.status, TaskStatus::Open);
}

#[tokio::test]
async fn done_open_done_creates_no_second_instance() {
    let s = TestServer::new().await;
    let mut clock = client_clock();
    let id = recurring(&s, &mut clock, "Miete", Location::Inbox).await;
    set_status(&s, &mut clock, id, TaskStatus::Done).await;
    let first = only_successor(&s, id);
    set_status(&s, &mut clock, id, TaskStatus::Open).await;
    // Reopening leaves the instance alone.
    assert_eq!(only_successor(&s, id).id, first.id);
    set_status(&s, &mut clock, id, TaskStatus::Done).await;
    assert_eq!(only_successor(&s, id).id, first.id);

    // Undo: once the instance is deleted, the next completion creates a
    // new one.
    let undo = ops(&mut clock, |b| {
        b.delete(EntityKind::Task, first.id);
        b.set_status(id, TaskStatus::Open);
    });
    push(&s, &undo).await;
    assert!(successors(&s, id).is_empty());
    set_status(&s, &mut clock, id, TaskStatus::Done).await;
    assert_ne!(only_successor(&s, id).id, first.id);
}

#[tokio::test]
async fn two_clients_completing_offline_create_one_instance() {
    let s = TestServer::new().await;
    let mut clock = client_clock();
    let id = recurring(&s, &mut clock, "Miete", Location::Inbox).await;
    // Both complete offline, then sync one after the other.
    let mut phone = Clock::new(NodeId::new("phone").unwrap());
    let mut laptop = Clock::new(NodeId::new("laptop").unwrap());
    let from_phone = ops(&mut phone, |b| {
        b.set_status(id, TaskStatus::Done);
    });
    let from_laptop = ops(&mut laptop, |b| {
        b.set_status(id, TaskStatus::Done);
    });
    push(&s, &from_laptop).await;
    push(&s, &from_phone).await;
    only_successor(&s, id);
    let all = s
        .app
        .read(|st| st.live_tasks().filter(|t| t.title == "Miete").count());
    assert_eq!(all, 2);
}

#[tokio::test]
async fn ending_or_deleting_the_series_stops_it() {
    let s = TestServer::new().await;
    let mut clock = client_clock();

    // recurrence: null together with the completion: no instance.
    let ended = recurring(&s, &mut clock, "Abo", Location::Inbox).await;
    let end = ops(&mut clock, |b| {
        b.set_status(ended, TaskStatus::Dropped);
        b.clear(EntityKind::Task, ended, fields::task::RECURRENCE);
    });
    push(&s, &end).await;
    assert!(successors(&s, ended).is_empty());

    // recurrence: null first, completion later: no instance either.
    let stopped = recurring(&s, &mut clock, "Zeitung", Location::Inbox).await;
    let stop = ops(&mut clock, |b| {
        b.clear(EntityKind::Task, stopped, fields::task::RECURRENCE);
    });
    push(&s, &stop).await;
    set_status(&s, &mut clock, stopped, TaskStatus::Done).await;
    assert!(successors(&s, stopped).is_empty());

    // Deleting the series' current instance: nothing follows.
    let deleted = recurring(&s, &mut clock, "Gelöscht", Location::Inbox).await;
    let delete = ops(&mut clock, |b| {
        b.set_status(deleted, TaskStatus::Done);
        b.delete(EntityKind::Task, deleted);
    });
    push(&s, &delete).await;
    assert!(successors(&s, deleted).is_empty());
    assert_eq!(
        s.app
            .read(|st| st.live_tasks().filter(|t| t.recurrence.is_some()).count()),
        0
    );
}

#[tokio::test]
async fn subtasks_comments_phoenix_and_blockers_stay_behind() {
    let s = TestServer::new().await;
    let mut clock = client_clock();
    let id = recurring(&s, &mut clock, "Wochenplanung", Location::Inbox).await;
    let mut blocker = None;
    let mut waiting = None;
    let extra = ops(&mut clock, |b| {
        b.create_task(NewTask::subtask("Unteraufgabe", id));
        b.add_comment(id, "Notiz", None);
        b.set_task(
            id,
            fields::task::PHOENIX,
            json!({"role": "flamme", "fire": "2099-W05"}),
        );
        let x = b.create_task(NewTask::new("Vorher"));
        b.set_task(id, fields::task::BLOCKED_BY, json!([x.to_string()]));
        let w = b.create_task(NewTask::new("Wartet"));
        b.set_task(w, fields::task::BLOCKED_BY, json!([id.to_string()]));
        b.set_task(id, fields::task::PLAN, "Plan");
        b.set_task(id, fields::task::DESCRIPTION, "Beschreibung");
        b.set_task(
            id,
            fields::task::LINKS,
            json!([{"url": "https://example.org", "title": "Beispiel"}]),
        );
        b.set_task(id, fields::task::VAULT_LINKS, json!(["Notizen/Plan.md"]));
        blocker = Some(x);
        waiting = Some(w);
    });
    push(&s, &extra).await;
    let ops_done = ops(&mut clock, |b| {
        b.set_status(blocker.unwrap(), TaskStatus::Done);
        b.set_status(id, TaskStatus::Done);
    });
    push(&s, &ops_done).await;

    let next = only_successor(&s, id);
    let (children, comments, waiting_blockers) = s.app.read(|st| {
        (
            st.children(next.id),
            st.comments_of(next.id),
            st.task(waiting.unwrap()).unwrap().blocked_by,
        )
    });
    assert!(children.is_empty());
    assert!(comments.is_empty());
    assert_eq!(next.phoenix, None);
    assert!(next.blocked_by.is_empty());
    assert_eq!(next.plan, None);
    assert_eq!(waiting_blockers, [id], "waiting tasks keep their blocker");
    // Carried over.
    assert_eq!(next.description.as_deref(), Some("Beschreibung"));
    assert_eq!(next.links.len(), 1);
    assert_eq!(next.vault_links, ["Notizen/Plan.md"]);
}

#[tokio::test]
async fn reminder_keeps_its_distance_to_the_due_time() {
    let s = TestServer::new().await;
    let mut clock = client_clock();
    let due = instant("2099-01-31T17:00:00Z");
    let remind = instant("2099-01-30T08:00:00Z");
    let mut id = None;
    let create = ops(&mut clock, |b| {
        let t = b.create_task(NewTask {
            due: Some(Due::DateTime(due)),
            remind_at: Some(remind),
            ..NewTask::new("Odoo absenden")
        });
        b.set_task(t, fields::task::RECURRENCE, month_end());
        id = Some(t);
    });
    push(&s, &create).await;
    let id = id.unwrap();
    set_status(&s, &mut clock, id, TaskStatus::Done).await;

    // 31.01. → 28.02.: 28 days later at the same local time (no daylight
    // saving change between January and February in the test's zone).
    let next = only_successor(&s, id);
    assert_eq!(next.due, Some(Due::DateTime(due + Duration::days(28))));
    assert_eq!(next.remind_at, Some(remind + Duration::days(28)));

    // The calendar feed shows the instance by itself.
    let feed = s.app.calendar_feed(FeedScope::All).await.unwrap();
    let unfolded = feed.replace("\r\n ", "");
    assert!(unfolded.contains(&format!("UID:{}", next.id)), "{feed}");
    assert!(unfolded.contains("DTSTART:20990228T170000Z"), "{feed}");
}

#[tokio::test]
async fn instances_in_keyed_projects_get_the_next_reference() {
    let s = TestServer::new().await;
    let t = api_token(&s, "claude").await;
    let area = mcp_ok(&s, &t, "create_area", json!({"name": "Arbeit"})).await;
    let area_id = area["area"]["id"].as_str().unwrap();
    let project = mcp_ok(
        &s,
        &t,
        "create_project",
        json!({"name": "Büro", "area_id": area_id, "key": "BUE"}),
    )
    .await;
    let project_id: Id = project["project"]["id"].as_str().unwrap().parse().unwrap();
    let mut clock = client_clock();
    let id = recurring(
        &s,
        &mut clock,
        "Odoo absenden",
        Location::Project { id: project_id },
    )
    .await;
    assert_eq!(
        s.app.read(|st| st.task(id).unwrap().reference),
        Some("BUE-1".into())
    );

    // Completed by an agent over MCP: the instance follows all the same.
    let done = mcp_ok(
        &s,
        &t,
        "update_task",
        json!({"task_id": "BUE-1", "status": "done"}),
    )
    .await;
    assert_eq!(done["task"]["status"], "done");
    let next = only_successor(&s, id);
    assert_eq!(next.reference.as_deref(), Some("BUE-2"));
    assert_eq!(next.location, Location::Project { id: project_id });

    // The agent log shows only what the agent did, not the server's
    // instance.
    let log = s.get("/api/agent-log", BOOT).await.json();
    let touched: Vec<&str> = log["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["changes"].as_array().unwrap())
        .map(|c| c["entity_id"].as_str().unwrap())
        .collect();
    assert!(!touched.contains(&next.id.to_string().as_str()), "{log}");
    assert!(touched.contains(&id.to_string().as_str()), "{log}");
}
