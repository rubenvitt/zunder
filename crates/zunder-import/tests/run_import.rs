//! `run` against the mock ClickUp API and a mock Zunder server that applies
//! the real `zunder-core` validation.

mod common;

use common::{edited_mapping, ClickUpMock, ZunderMock, ZUNDER_TOKEN};
use serde_json::json;
use uuid::Uuid;
use zunder_core::hlc::{Clock, NodeId};
use zunder_core::ids::clickup_entity_id;
use zunder_core::{
    AreaState, EntityKind, Location, NewArea, NewProject, NewTask, OpBatch, ProjectKind,
    ProjectStatus, TaskStatus, TaskType,
};
use zunder_import::convert::Conversion;
use zunder_import::mapping::Mapping;
use zunder_import::run::{self, RunOptions, RunSummary};
use zunder_import::zunder::ZunderClient;
use zunder_import::{plan, report};

fn task_id(clickup: &str) -> Uuid {
    clickup_entity_id(EntityKind::Task, clickup)
}

async fn mapping(clickup: &ClickUpMock) -> Mapping {
    let proposal = plan::plan(&clickup.client(), None, "2026-10-11")
        .await
        .unwrap();
    edited_mapping(proposal.mapping)
}

async fn conversion(clickup: &ClickUpMock, mapping: &Mapping) -> Conversion {
    report::load(&clickup.client(), &mapping.resolve().unwrap())
        .await
        .unwrap()
        .conversion
}

async fn import(
    clickup: &ClickUpMock,
    zunder: &ZunderMock,
    mapping: &Mapping,
    options: &RunOptions,
) -> anyhow::Result<RunSummary> {
    let conv = conversion(clickup, mapping).await;
    let client = ZunderClient::new(&zunder.url, ZUNDER_TOKEN);
    run::run(&conv, Some(&client), options).await
}

fn opts(batch_size: usize) -> RunOptions {
    RunOptions {
        batch_size,
        ..RunOptions::default()
    }
}

#[tokio::test]
async fn imports_everything_with_correct_fields() {
    let clickup = ClickUpMock::start().await;
    let zunder = ZunderMock::start().await;
    let mapping = mapping(&clickup).await;
    let summary = import(&clickup, &zunder, &mapping, &opts(500))
        .await
        .unwrap();

    assert!(summary.rejected.is_empty(), "{:?}", summary.rejected);
    assert!(summary.predicted_rejections.is_empty());
    assert_eq!(summary.entities_created, 3 + 4 + 18);
    assert_eq!(summary.accepted, summary.ops);
    assert_eq!(zunder.log_len(), summary.ops);
    let clients = zunder.inner.clients.lock().unwrap().clone();
    assert!(clients
        .iter()
        .all(|(id, header)| id == "import" && header == "import"));
    assert!(zunder
        .inner
        .log
        .lock()
        .unwrap()
        .iter()
        .all(|op| op.hlc.node().as_str() == "import"));

    let state = zunder.state();
    assert_eq!(state.live_areas().count(), 3);
    assert_eq!(state.live_projects().count(), 4);
    assert_eq!(state.live_tasks().count(), 18);

    // Structure.
    let home = clickup_entity_id(EntityKind::Area, "90010");
    assert_eq!(state.area(home).unwrap().name, "Zuhause");
    let old = state
        .area(clickup_entity_id(EntityKind::Area, "90012"))
        .unwrap();
    assert_eq!(old.state, AreaState::Archived);
    let tools = state
        .project(clickup_entity_id(EntityKind::Project, "folder-90021"))
        .unwrap();
    assert_eq!(tools.kind, ProjectKind::Dev);
    assert_eq!(tools.repo.as_deref(), Some("beispiel/werkzeugkiste"));
    let garden_id = clickup_entity_id(EntityKind::Project, "9001003");
    assert_eq!(
        state.project(garden_id).unwrap().status,
        ProjectStatus::Completed
    );

    // A task with all fields.
    let tax = state.task(task_id("86a002")).unwrap();
    assert_eq!(tax.title, "Steuererklärung vorbereiten");
    assert_eq!(tax.description.as_deref(), Some("**Belege** sammeln"));
    assert_eq!(tax.status, TaskStatus::InProgress);
    assert_eq!(
        tax.task_type,
        TaskType::Aufgabe,
        "bug outside a dev project"
    );
    assert_eq!(tax.location, Location::Area { id: home });
    assert_eq!(
        tax.due.unwrap().to_wire(),
        "2026-10-12T09:30:00+02:00",
        "due_date_time = true keeps the time in the mapping's time zone"
    );
    assert_eq!(tax.links.len(), 1);
    assert_eq!(tax.links[0].url, "https://app.clickup.com/t/86a002");
    assert_eq!(tax.links[0].title.as_deref(), Some("ClickUp"));
    assert_eq!(tax.clickup_id.as_deref(), Some("86a002"));
    assert_eq!(tax.rank.as_deref(), Some("0000000000002.500000"));
    assert_eq!(state.task_by_clickup_id("86a002").unwrap().id, tax.id);

    // Subtasks in any depth.
    let scan = state.task(task_id("86a004")).unwrap();
    assert_eq!(scan.parent_id, Some(tax.id));
    assert_eq!(scan.due.unwrap().to_wire(), "2026-12-15", "local date");
    let scanner = state.task(task_id("86a005")).unwrap();
    assert_eq!(scanner.parent_id, Some(scan.id));
    assert_eq!(scanner.status, TaskStatus::Done);
    assert_eq!(
        state.effective_location(scanner.id),
        Some(Location::Area { id: home })
    );
    assert_eq!(state.progress(tax.id).total, 1);

    // Orphan and archived parent.
    let orphan = state.task(task_id("86a006")).unwrap();
    assert_eq!(orphan.parent_id, None);
    assert_eq!(orphan.location, Location::Area { id: home });
    let archived_parent = state.task(task_id("86a900")).unwrap();
    assert_eq!(archived_parent.status, TaskStatus::Done);
    assert_eq!(
        archived_parent.description.as_deref(),
        Some("Schon lange erledigt")
    );
    assert_eq!(
        state.task(task_id("86a007")).unwrap().parent_id,
        Some(archived_parent.id)
    );

    // Date-only due without flag; empty description strings are no description.
    let windows = state.task(task_id("86a001")).unwrap();
    assert_eq!(windows.due.unwrap().to_wire(), "2026-10-15");
    assert_eq!(windows.description, None);

    // Dev project keeps feature/review; mapped and closed statuses.
    let feature = state.task(task_id("86f001")).unwrap();
    assert_eq!(feature.task_type, TaskType::Feature);
    assert_eq!(feature.status, TaskStatus::Review);
    assert_eq!(feature.location, Location::Project { id: tools.id });
    assert_eq!(
        state.task(task_id("86f003")).unwrap().task_type,
        TaskType::Bug
    );
    assert_eq!(
        state.task(task_id("86f002")).unwrap().status,
        TaskStatus::Blocked
    );
    assert_eq!(
        state.task(task_id("86d002")).unwrap().status,
        TaskStatus::Dropped
    );
    assert_eq!(state.task(task_id("86e002")).unwrap().title, "(ohne Titel)");
    assert_eq!(
        state.task(task_id("86c001")).unwrap().location,
        Location::Project { id: garden_id }
    );
    assert_eq!(
        state.task(task_id("86b001")).unwrap().location,
        Location::Inbox
    );
    assert_eq!(state.task(task_id("86b001")).unwrap().rank, None);

    // ClickUp history: a task closed in ClickUp days ago is dated to its
    // date_closed and stays out of the default views (3-day rule); the
    // creation is dated to date_created.
    let now = zunder_core::hlc::system_time_ms();
    let paper = task_id("86a003");
    assert_eq!(
        state.status_changed_at(paper).unwrap().wall_ms(),
        1_790_900_000_000
    );
    assert!(!state.is_visible_by_default(paper, now));
    assert_eq!(
        state
            .created_at(EntityKind::Task, windows.id)
            .unwrap()
            .wall_ms(),
        1_790_000_000_000
    );
    // Without ClickUp dates the import time applies.
    assert!(state.is_visible_by_default(scanner.id, now));

    // The closed archived parent has an open subtask: a warning, no rejection.
    assert_eq!(summary.warnings.len(), 1);
    assert_eq!(summary.warnings[0].code, "open_subtasks");
    assert_eq!(
        summary.warnings[0].entity_id,
        archived_parent.id.to_string()
    );
}

#[tokio::test]
async fn pushes_in_batches_without_splitting_entities() {
    let clickup = ClickUpMock::start().await;
    let zunder = ZunderMock::start().await;
    let mapping = mapping(&clickup).await;
    let summary = import(&clickup, &zunder, &mapping, &opts(25))
        .await
        .unwrap();
    let pushes = zunder.pushes();
    assert!(pushes.len() > 3, "{pushes:?}");
    assert_eq!(pushes.len(), summary.batches);
    assert!(pushes.iter().all(|n| *n <= 25), "{pushes:?}");
    assert_eq!(pushes.iter().sum::<usize>(), summary.ops);
    // Every push was fully accepted: no entity arrived without its
    // required fields, no subtask before its parent.
    assert!(summary.rejected.is_empty(), "{:?}", summary.rejected);
    assert_eq!(zunder.state().live_tasks().count(), 18);
}

#[tokio::test]
async fn rerun_is_idempotent_and_updates_changes() {
    let clickup = ClickUpMock::start().await;
    let zunder = ZunderMock::start().await;
    let mapping = mapping(&clickup).await;
    import(&clickup, &zunder, &mapping, &opts(500))
        .await
        .unwrap();
    let state_after_first = zunder.state();
    let pushes_after_first = zunder.pushes().len();

    // Unchanged ClickUp data: nothing to write.
    let summary = import(&clickup, &zunder, &mapping, &opts(500))
        .await
        .unwrap();
    assert_eq!(summary.ops, 0);
    assert_eq!(summary.entities_unchanged, 25);
    assert_eq!(zunder.pushes().len(), pushes_after_first, "no empty push");
    assert_eq!(zunder.state(), state_after_first);

    // Edits in Zunder: a field the import does not own survives, an owned
    // field is reset to the ClickUp value.
    let mut web = Clock::new(NodeId::new("web-test").unwrap());
    let mut batch = OpBatch::new(&mut web, zunder_core::hlc::system_time_ms());
    batch.set_task(task_id("86b001"), "remind_at", "2026-10-20T09:00:00+02:00");
    batch.set_task(task_id("86b001"), "status", "done");
    zunder.apply(batch.ops());

    // Changes in ClickUp: new title, due date removed, one new task.
    clickup.edit_task("/list/9001001/task?page=0", "86a001", |t| {
        t.insert("name".into(), json!("Fenster putzen (alle)"));
        t.insert("due_date".into(), json!(null));
    });
    {
        let mut routes = clickup.state.routes.lock().unwrap();
        routes["/list/9001101/task?page=0"]["tasks"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id": "86g002", "name": "Nistkasten aufhängen",
                "status": {"status": "Idee", "type": "open"},
                "orderindex": "2", "list": {"id": "9001101"},
                "url": "https://app.clickup.com/t/86g002"
            }));
    }

    let summary = import(&clickup, &zunder, &mapping, &opts(500))
        .await
        .unwrap();
    assert!(summary.rejected.is_empty(), "{:?}", summary.rejected);
    assert_eq!(summary.entities_created, 1);
    assert_eq!(summary.entities_updated, 2);
    let state = zunder.state();
    let windows = state.task(task_id("86a001")).unwrap();
    assert_eq!(windows.title, "Fenster putzen (alle)");
    assert_eq!(windows.due, None);
    let idea = state.task(task_id("86b001")).unwrap();
    assert_eq!(
        idea.status,
        TaskStatus::Open,
        "ClickUp wins for owned fields"
    );
    assert!(idea.remind_at.is_some(), "foreign fields stay");
    assert_eq!(state.live_tasks().count(), 19);
    assert_eq!(
        state.task(task_id("86g002")).unwrap().title,
        "Nistkasten aufhängen"
    );
}

#[tokio::test]
async fn dry_run_writes_nothing() {
    let clickup = ClickUpMock::start().await;
    let zunder = ZunderMock::start().await;
    let mapping = mapping(&clickup).await;
    let conv = conversion(&clickup, &mapping).await;

    // Without server: everything counts.
    let dry = RunOptions {
        dry_run: true,
        ..opts(100)
    };
    let offline = run::run(&conv, None, &dry).await.unwrap();
    assert!(offline.dry_run);
    assert!(offline.ops > 100);
    assert!(offline.batches >= offline.ops.div_ceil(100));
    assert!(offline.predicted_rejections.is_empty());
    assert!(run::render_summary(&offline).starts_with("Probelauf:"));

    // With server: diff against the current state, still no push.
    let client = ZunderClient::new(&zunder.url, ZUNDER_TOKEN);
    let online = run::run(&conv, Some(&client), &dry).await.unwrap();
    assert_eq!(online.ops, offline.ops);
    assert!(zunder.pushes().is_empty());
    assert_eq!(zunder.log_len(), 0);

    // A real run is refused without server.
    let err = run::run(&conv, None, &opts(100)).await.unwrap_err();
    assert!(err.to_string().contains("--dry-run"), "{err}");
}

#[tokio::test]
async fn incomplete_mapping_blocks_the_run() {
    let clickup = ClickUpMock::start().await;
    let zunder = ZunderMock::start().await;
    let proposal = plan::plan(&clickup.client(), None, "2026-10-11")
        .await
        .unwrap();
    let err = import(&clickup, &zunder, &proposal.mapping, &opts(500))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("1 nicht zugeordnete Status"),
        "{err}"
    );
    assert!(zunder.pushes().is_empty());
}

#[tokio::test]
async fn predicted_rejections_stop_before_writing_unless_keep_going() {
    let clickup = ClickUpMock::start().await;
    let zunder = ZunderMock::start().await;
    // A task in Zunder already uses the ID the archived area would get.
    let collision = clickup_entity_id(EntityKind::Area, "90012");
    let mut web = Clock::new(NodeId::new("web-test").unwrap());
    let mut batch = OpBatch::new(&mut web, zunder_core::hlc::system_time_ms());
    batch.create_task(NewTask {
        id: Some(collision),
        ..NewTask::new("Zufällige Kollision")
    });
    zunder.apply(batch.ops());
    let mapping = mapping(&clickup).await;

    let err = import(&clickup, &zunder, &mapping, &opts(500))
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("es wurde nichts geschrieben"), "{text}");
    assert!(text.contains("Bereich „Altlasten“"), "{text}");
    assert!(text.contains("kind_mismatch"), "{text}");
    assert!(zunder.pushes().is_empty());

    let keep_going = RunOptions {
        keep_going: true,
        ..opts(500)
    };
    let summary = import(&clickup, &zunder, &mapping, &keep_going)
        .await
        .unwrap();
    assert_eq!(summary.predicted_rejections.len(), summary.rejected.len());
    assert!(summary
        .rejected
        .iter()
        .all(|r| r.entity == "Bereich „Altlasten“"));
    assert_eq!(zunder.state().live_tasks().count(), 18 + 1);
    assert!(run::render_summary(&summary).contains("Abgelehnt:"));
}

#[tokio::test]
async fn existing_ids_map_onto_existing_entities() {
    let clickup = ClickUpMock::start().await;
    let zunder = ZunderMock::start().await;
    let mut web = Clock::new(NodeId::new("web-test").unwrap());
    let mut batch = OpBatch::new(&mut web, zunder_core::hlc::system_time_ms());
    let existing = batch.create_area(NewArea::new("Haushalt"));
    zunder.apply(batch.ops());

    let mut mapping = mapping(&clickup).await;
    mapping.areas.get_mut("zuhause").unwrap().existing_id = Some(existing.to_string());
    let summary = import(&clickup, &zunder, &mapping, &opts(500))
        .await
        .unwrap();
    assert!(summary.rejected.is_empty(), "{:?}", summary.rejected);
    let state = zunder.state();
    assert_eq!(state.area(existing).unwrap().name, "Haushalt", "untouched");
    assert!(state
        .area(clickup_entity_id(EntityKind::Area, "90010"))
        .is_none());
    assert_eq!(
        state.task(task_id("86a001")).unwrap().location,
        Location::Area { id: existing }
    );
    assert_eq!(state.live_areas().count(), 3);

    // An existing_id Zunder does not know is refused up front.
    let mut mapping = mapping.clone();
    mapping.areas.get_mut("werkstatt").unwrap().existing_id =
        Some("01929c3e-0000-7000-8000-000000000001".into());
    let err = import(&clickup, &zunder, &mapping, &opts(500))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("existing_id"), "{err}");
}

#[tokio::test]
async fn existing_projects_must_declare_their_real_kind() {
    let clickup = ClickUpMock::start().await;
    let zunder = ZunderMock::start().await;
    let mut web = Clock::new(NodeId::new("web-test").unwrap());
    let mut batch = OpBatch::new(&mut web, zunder_core::hlc::system_time_ms());
    let area = batch.create_area(NewArea::new("Werkstatt"));
    let dev = batch.create_project(NewProject {
        kind: ProjectKind::Dev,
        ..NewProject::new("Werkzeugkiste", area)
    });
    zunder.apply(batch.ops());
    let before = zunder.log_len();

    // The proposal says kind = "standard"; with existing_id nothing is
    // written, so the dev rules would silently turn bug/feature/review
    // into aufgabe/in_progress although the real project is a dev project.
    let mut mapping = mapping(&clickup).await;
    let tools = mapping.projects.get_mut("werkzeugkiste").unwrap();
    tools.existing_id = Some(dev.to_string());
    tools.kind = Some("standard".into());
    tools.repo = None;
    let err = import(&clickup, &zunder, &mapping, &opts(500))
        .await
        .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("existing_id"), "{message}");
    assert!(message.contains("kind = \"dev\""), "{message}");
    assert_eq!(zunder.log_len(), before, "nothing written");

    // With the real kind the dev types and statuses survive.
    let tools = mapping.projects.get_mut("werkzeugkiste").unwrap();
    tools.kind = Some("dev".into());
    let summary = import(&clickup, &zunder, &mapping, &opts(500))
        .await
        .unwrap();
    assert!(summary.rejected.is_empty(), "{:?}", summary.rejected);
    let state = zunder.state();
    let feature = state.task(task_id("86f001")).unwrap();
    assert_eq!(feature.location, Location::Project { id: dev });
    assert_eq!(feature.task_type, TaskType::Feature);
    assert_eq!(feature.status, TaskStatus::Review);
    assert_eq!(
        state.task(task_id("86f003")).unwrap().task_type,
        TaskType::Bug
    );
}

#[tokio::test]
async fn dev_project_becoming_standard_survives_small_batches() {
    let clickup = ClickUpMock::start().await;
    let zunder = ZunderMock::start().await;
    let mut mapping = mapping(&clickup).await;
    import(&clickup, &zunder, &mapping, &opts(500))
        .await
        .unwrap();

    // Werkzeugkiste stops being a dev project: its bug/feature/review tasks
    // are downgraded. The server checks every batch on its own, so the
    // downgrades must not arrive after the project's kind change.
    let tools = mapping.projects.get_mut("werkzeugkiste").unwrap();
    tools.kind = Some("standard".into());
    tools.repo = None;
    let summary = import(&clickup, &zunder, &mapping, &opts(2)).await.unwrap();
    assert!(summary.predicted_rejections.is_empty());
    assert!(summary.rejected.is_empty(), "{:?}", summary.rejected);
    assert!(summary.batches > 1);
    let state = zunder.state();
    let project = state
        .project(clickup_entity_id(EntityKind::Project, "folder-90021"))
        .unwrap();
    assert_eq!(project.kind, ProjectKind::Standard);
    let feature = state.task(task_id("86f001")).unwrap();
    assert_eq!(feature.task_type, TaskType::Aufgabe);
    assert_eq!(feature.status, TaskStatus::InProgress);
    assert_eq!(
        state.task(task_id("86f003")).unwrap().task_type,
        TaskType::Aufgabe
    );
}

#[tokio::test]
async fn oversized_entities_are_never_sent() {
    let clickup = ClickUpMock::start().await;
    let zunder = ZunderMock::start().await;
    clickup.edit_task("/list/9001001/task?page=0", "86a002", |task| {
        task.insert(
            "markdown_description".into(),
            json!("Protokoll ".repeat(800 * 1024)),
        );
    });
    let mapping = mapping(&clickup).await;

    // The dry run names the entity and, because the check runs on the real
    // batches, the subtasks that depend on it.
    let dry = import(
        &clickup,
        &zunder,
        &mapping,
        &RunOptions {
            dry_run: true,
            ..opts(500)
        },
    )
    .await
    .unwrap();
    assert_eq!(dry.oversized.len(), 1);
    assert!(dry.oversized[0].contains("86a002"), "{:?}", dry.oversized);
    assert!(dry
        .predicted_rejections
        .iter()
        .any(|r| r.entity.contains("86a004")));
    assert!(run::render_summary(&dry).contains("Zu groß für eine Anfrage"));

    let err = import(&clickup, &zunder, &mapping, &opts(500))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("zu groß"), "{err}");
    assert_eq!(zunder.log_len(), 0, "nothing written");

    let summary = import(
        &clickup,
        &zunder,
        &mapping,
        &RunOptions {
            keep_going: true,
            ..opts(500)
        },
    )
    .await
    .unwrap();
    assert_eq!(summary.oversized.len(), 1);
    let state = zunder.state();
    assert!(state.task(task_id("86a002")).is_none());
    assert!(state.task(task_id("86a001")).is_some());
}

#[tokio::test]
async fn cli_run_dry_run_and_real_run() {
    let clickup = ClickUpMock::start().await;
    let zunder = ZunderMock::start().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mapping.toml");
    let mapping = mapping(&clickup).await;
    std::fs::write(
        &path,
        zunder_import::mapping::render(&mapping, "Muster-Workspace", "2026-10-11"),
    )
    .unwrap();

    let run_cli = |extra: Vec<String>| {
        let clickup_url = clickup.url.clone();
        let path = path.clone();
        tokio::task::spawn_blocking(move || {
            std::process::Command::new(env!("CARGO_BIN_EXE_zunder-import"))
                .arg("run")
                .arg("--mapping")
                .arg(&path)
                .args(extra)
                .env("CLICKUP_TOKEN", common::CLICKUP_TOKEN)
                .env("CLICKUP_API_URL", clickup_url)
                .env_remove("ZUNDER_URL")
                .env_remove("ZUNDER_TOKEN")
                .output()
                .unwrap()
        })
    };

    let out = run_cli(vec!["--dry-run".into()]).await.unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("Probelauf:"));

    let out = run_cli(vec![]).await.unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--server"));

    let out = run_cli(vec![
        "--server".into(),
        zunder.url.clone(),
        "--token".into(),
        ZUNDER_TOKEN.into(),
        "--batch-size".into(),
        "50".into(),
    ])
    .await
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Import abgeschlossen"), "{stdout}");
    assert!(stdout.contains("25 neu"), "{stdout}");
    assert_eq!(zunder.state().live_tasks().count(), 18);
}
