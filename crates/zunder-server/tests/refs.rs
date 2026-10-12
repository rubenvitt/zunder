//! Project keys and task references (`ZUN-42`): the server assigns them after
//! every push, MCP tools address tasks by them, clients may not write them.

mod common;

use serde_json::{json, Value};
use zunder_core::model::fields;
use zunder_core::{EntityKind, Location, NewTask};

use common::{api_token, client_clock, mcp_err, mcp_ok, ops, Auth, TestServer};

async fn setup() -> (TestServer, String, String, String) {
    let s = TestServer::new().await;
    let t = api_token(&s, "claude").await;
    let area = mcp_ok(&s, &t, "create_area", json!({"name": "Arbeit"})).await;
    let area_id = area["area"]["id"].as_str().unwrap().to_owned();
    let project = mcp_ok(
        &s,
        &t,
        "create_project",
        json!({"name": "Zunder", "area_id": area_id, "kind": "dev", "key": "ZUN"}),
    )
    .await;
    assert_eq!(project["project"]["key"], "ZUN");
    let project_id = project["project"]["id"].as_str().unwrap().to_owned();
    (s, t, area_id, project_id)
}

fn reference(result: &Value) -> &str {
    result["task"]["ref"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn tasks_in_keyed_projects_get_references() {
    let (s, t, area, project) = setup().await;
    let in_project = json!({"kind": "project", "id": project});

    let first = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "Login", "location": in_project}),
    )
    .await;
    assert_eq!(reference(&first), "ZUN-1");
    let first_id = first["task"]["id"].as_str().unwrap().to_owned();

    // Subtasks count as well; references address tasks in every tool.
    let split = mcp_ok(
        &s,
        &t,
        "decompose_task",
        json!({"task_id": "zun-1", "subtasks": [{"title": "Formular"}, {"title": "Fehlertext"}]}),
    )
    .await;
    let subs: Vec<&str> = split["task"]["subtasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["ref"].as_str().unwrap())
        .collect();
    assert_eq!(subs, ["ZUN-2", "ZUN-3"]);
    let child = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "Noch eine", "parent_id": "ZUN-1"}),
    )
    .await;
    assert_eq!(reference(&child), "ZUN-4");
    let got = mcp_ok(&s, &t, "get_task", json!({"task_id": " ZUN-1 "})).await;
    assert_eq!(got["task"]["id"], first_id.as_str());

    // An inbox task gets its reference when it moves in, and keeps it when
    // it moves on.
    let inbox = mcp_ok(&s, &t, "create_task", json!({"title": "Idee"})).await;
    assert_eq!(inbox["task"]["ref"], Value::Null);
    let id = inbox["task"]["id"].as_str().unwrap();
    let moved = mcp_ok(
        &s,
        &t,
        "move_task",
        json!({"task_id": id, "location": in_project}),
    )
    .await;
    assert_eq!(reference(&moved), "ZUN-5");
    let away = mcp_ok(
        &s,
        &t,
        "move_task",
        json!({"task_id": "ZUN-5", "location": {"kind": "area", "id": area}}),
    )
    .await;
    assert_eq!(reference(&away), "ZUN-5");

    // Search finds a reference first, in any case.
    let hits = mcp_ok(&s, &t, "search", json!({"query": "zun-4"})).await;
    assert_eq!(hits["hits"][0]["ref"], "ZUN-4");

    let (code, _) = mcp_err(&s, &t, "get_task", json!({"task_id": "ZUN-99"})).await;
    assert_eq!(code, "not_found");
    let (code, _) = mcp_err(&s, &t, "get_task", json!({"task_id": "Zahnarzt"})).await;
    assert_eq!(code, "invalid_arguments");

    // The reference was written by the server.
    let history = mcp_ok(&s, &t, "get_task_history", json!({"task_id": "ZUN-1"})).await;
    let entry = history["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["field"] == "ref")
        .unwrap()
        .clone();
    assert_eq!(entry["source"], "server");
    assert_eq!(entry["value"], "ZUN-1");
}

#[tokio::test]
async fn setting_a_key_numbers_existing_tasks_in_creation_order() {
    let s = TestServer::new().await;
    let t = api_token(&s, "claude").await;
    let area = mcp_ok(&s, &t, "create_area", json!({"name": "Arbeit"})).await;
    let area_id = area["area"]["id"].as_str().unwrap();
    let project = mcp_ok(
        &s,
        &t,
        "create_project",
        json!({"name": "App", "area_id": area_id}),
    )
    .await;
    let project_id = project["project"]["id"].as_str().unwrap();
    let mut ids = Vec::new();
    for title in ["a", "b", "c"] {
        let r = mcp_ok(
            &s,
            &t,
            "create_task",
            json!({"title": title, "location": {"kind": "project", "id": project_id}}),
        )
        .await;
        assert_eq!(r["task"]["ref"], Value::Null);
        ids.push(r["task"]["id"].as_str().unwrap().to_owned());
    }
    mcp_ok(
        &s,
        &t,
        "update_project",
        json!({"project_id": project_id, "key": "APP"}),
    )
    .await;
    for (i, id) in ids.iter().enumerate() {
        let got = mcp_ok(&s, &t, "get_task", json!({"task_id": id})).await;
        assert_eq!(reference(&got), format!("APP-{}", i + 1));
    }
    let list = mcp_ok(&s, &t, "list_structure", json!({})).await;
    assert_eq!(list["areas"][0]["projects"][0]["key"], "APP");

    // A new key keeps the references already given.
    mcp_ok(
        &s,
        &t,
        "update_project",
        json!({"project_id": project_id, "key": "NEU"}),
    )
    .await;
    let got = mcp_ok(&s, &t, "get_task", json!({"task_id": "APP-2"})).await;
    assert_eq!(got["task"]["id"], ids[1].as_str());
    let new = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "d", "location": {"kind": "project", "id": project_id}}),
    )
    .await;
    assert_eq!(reference(&new), "NEU-1");

    // References survive a restart and are not handed out again.
    let s = s.restart().await;
    let new = mcp_ok(
        &s,
        &t,
        "create_task",
        json!({"title": "e", "location": {"kind": "project", "id": project_id}}),
    )
    .await;
    assert_eq!(reference(&new), "NEU-2");
}

#[tokio::test]
async fn clients_push_tasks_and_the_server_numbers_them() {
    let (s, t, area, project) = setup().await;
    let project_id = project.parse().unwrap();
    let mut clock = client_clock();
    let mut task = None;
    let pushed = ops(&mut clock, |b| {
        task = Some(
            b.create_task(NewTask::new("Offline erfasst").at(Location::Project { id: project_id })),
        );
    });
    let r = s.push(Auth::Bearer(&t), &pushed).await;
    assert!(r.json()["rejected"].as_array().unwrap().is_empty());
    let task = task.unwrap();
    let assigned = s.app.read(|state| state.task(task).unwrap().reference);
    assert_eq!(assigned.as_deref(), Some("ZUN-1"));
    // The server's op is in the log for clients to pull.
    let pulled = s
        .get("/api/sync/pull?since=0", Auth::Bearer(&t))
        .await
        .json();
    assert!(pulled["ops"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["op"]["field"] == "ref" && o["op"]["value"] == "ZUN-1"));

    // Clients may neither set nor reset a reference, nor reuse a key.
    let forged = ops(&mut clock, |b| {
        b.set_task(task, fields::task::REF, "ZUN-99");
    });
    let r = s.push(Auth::Bearer(&t), &forged).await.json();
    assert_eq!(r["rejected"][0]["code"], "invalid_value");
    let area_id = area.parse().unwrap();
    let mut other = None;
    let duplicate = ops(&mut clock, |b| {
        other = Some(b.create_project(zunder_core::NewProject {
            key: Some("ZUN".into()),
            ..zunder_core::NewProject::new("Doppelt", area_id)
        }));
    });
    let r = s.push(Auth::Bearer(&t), &duplicate).await.json();
    let rejected = r["rejected"].as_array().unwrap();
    assert_eq!(rejected.len(), duplicate.len());
    assert!(rejected[0]["message"].as_str().unwrap().contains("Zunder"));
    assert!(s
        .app
        .read(|state| state.entity(EntityKind::Project, other.unwrap()).is_none()));
}
