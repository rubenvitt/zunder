//! Switching IDs to CUID2: a database from before (UUIDs in the op log,
//! schema 3) is backed up and rewritten on start; legacy IDs keep working as
//! input.

mod common;

use serde_json::{json, Value};
use zunder_core::ids::Id;

use common::{api_token, mcp_ok, Auth, TestServer, BOOTSTRAP};

const AREA: &str = "0192a000-0000-7000-8000-0a0000000001";
const PROJECT: &str = "0192a000-0000-7000-8000-0b0000000001";
const PARENT: &str = "0192a000-0000-7000-8000-0c0000000001";
const CHILD: &str = "0192a000-0000-7000-8000-0c0000000002";

fn new(legacy: &str) -> String {
    Id::parse_any(legacy).unwrap().to_string()
}

/// Turns the server's database into one from before CUID2.
fn write_legacy_data(s: &TestServer) {
    let db = rusqlite::Connection::open(s.app.config().db_path()).unwrap();
    let owner = s.app.owner_id().to_string();
    let ops: [(&str, &str, &str, Value); 8] = [
        ("area", AREA, "name", json!("Arbeit")),
        ("project", PROJECT, "name", json!("Zunder")),
        ("project", PROJECT, "area_id", json!(AREA)),
        ("project", PROJECT, "kind", json!("dev")),
        ("task", PARENT, "title", json!("Elternaufgabe")),
        (
            "task",
            PARENT,
            "location",
            json!({"kind": "project", "id": PROJECT}),
        ),
        ("task", CHILD, "title", json!("Kind")),
        ("task", CHILD, "parent_id", json!(PARENT)),
    ];
    for (i, (entity, id, field, value)) in ops.iter().enumerate() {
        db.execute(
            "INSERT INTO ops (op_id, owner_id, entity, entity_id, field, value_json, hlc, source, client_id, received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'web', NULL, 0)",
            rusqlite::params![
                format!("0192a000-0000-7000-8000-0f00000000{i:02x}"),
                owner,
                entity,
                id,
                field,
                value.to_string(),
                format!("17600000000{i:02}:00000:web-alt"),
            ],
        )
        .unwrap();
    }
    db.execute("DELETE FROM entities", []).unwrap();
    db.pragma_update(None, "user_version", 3).unwrap();
}

#[tokio::test]
async fn legacy_database_is_backed_up_and_rewritten() {
    let s = TestServer::new().await;
    write_legacy_data(&s);
    let s = s.restart().await;
    let data = s.app.config().data_dir.clone();

    // Rewritten state: new IDs, references included.
    let (child, location) = s.app.read(|state| {
        let child = state.task(Id::parse(&new(CHILD)).unwrap()).unwrap();
        let parent = state.task(Id::parse(&new(PARENT)).unwrap()).unwrap();
        (child, parent.location)
    });
    assert_eq!(child.parent_id.unwrap().to_string(), new(PARENT));
    assert_eq!(
        serde_json::to_value(location).unwrap(),
        json!({"kind": "project", "id": new(PROJECT)})
    );
    let pulled = s
        .get("/api/sync/pull", Auth::Bearer(BOOTSTRAP))
        .await
        .json();
    let ops = pulled["ops"].as_array().unwrap();
    assert_eq!(ops.len(), 8);
    assert_eq!(ops[0]["seq"], 1, "seq stays, clients keep their cursor");
    assert!(ops
        .iter()
        .all(|o| o["op"]["op_id"].as_str().unwrap().len() == 24));
    assert_eq!(ops[2]["op"]["value"], new(AREA).as_str());

    // Backup with the old data, outside the nightly rotation.
    let backups: Vec<_> = std::fs::read_dir(data.join("backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("vor-cuid2-")
        })
        .collect();
    assert_eq!(backups.len(), 1);
    let old = rusqlite::Connection::open(&backups[0]).unwrap();
    let version: i64 = old
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
    let legacy_rows: i64 = old
        .query_row(
            "SELECT COUNT(*) FROM ops WHERE length(entity_id) = 36",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy_rows, 8);

    // Mapping for the vault.
    let mapping = std::fs::read_dir(&data)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("id-zuordnung-")
        })
        .unwrap();
    let list: Vec<Value> = serde_json::from_slice(&std::fs::read(mapping).unwrap()).unwrap();
    let area = list.iter().find(|e| e["old_id"] == AREA).unwrap();
    assert_eq!(area["new_id"], new(AREA).as_str());
    assert_eq!(area["entity"], "area");
    assert_eq!(area["name"], "Arbeit");
    assert_eq!(list.len(), 4);

    // Legacy IDs keep working as input (MCP, history, pushes of old clients).
    let t = api_token(&s, "claude").await;
    let got = mcp_ok(&s, &t, "get_task", json!({"task_id": CHILD})).await;
    assert_eq!(got["task"]["id"], new(CHILD).as_str());
    assert_eq!(got["task"]["parent_id"], new(PARENT).as_str());
    let structure = mcp_ok(&s, &t, "list_structure", json!({})).await;
    assert_eq!(structure["areas"][0]["id"], new(AREA).as_str());
    let r = s
        .get(
            &format!("/api/tasks/{PARENT}/history"),
            Auth::Bearer(BOOTSTRAP),
        )
        .await;
    assert_eq!(r.json().as_array().unwrap().len(), 2);
    let old_client = json!({"client_id": "alt", "ops": [{
        "op_id": "0192a000-0000-7000-8000-0f0000000100",
        "entity": "task", "entity_id": CHILD, "field": "parent_id",
        "value": null, "hlc": "1760000000100:00000:web-alt"
    }, {
        "op_id": "0192a000-0000-7000-8000-0f0000000101",
        "entity": "task", "entity_id": CHILD, "field": "location",
        "value": {"kind": "project", "id": PROJECT}, "hlc": "1760000000101:00000:web-alt"
    }]});
    let body = s
        .post("/api/sync/push", Auth::Bearer(BOOTSTRAP), old_client)
        .await
        .json();
    assert_eq!(body["rejected"], json!([]), "{body}");
    // The answer names the operations the way the old client sent them.
    assert_eq!(
        body["accepted"],
        json!([
            "0192a000-0000-7000-8000-0f0000000100",
            "0192a000-0000-7000-8000-0f0000000101"
        ])
    );
    let moved = s.app.read(|state| {
        state
            .effective_project(Id::parse(&new(CHILD)).unwrap())
            .map(|p| p.to_string())
    });
    assert_eq!(moved, Some(new(PROJECT)));

    // A second start changes nothing and takes no second backup.
    let s = s.restart().await;
    let count = std::fs::read_dir(data.join("backups"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("vor-cuid2-")
        })
        .count();
    assert_eq!(count, 1);
    assert!(s
        .app
        .read(|state| state.task(Id::parse(&new(CHILD)).unwrap()).is_some()));
}
