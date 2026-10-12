//! `plan` and `report` against the mock ClickUp API.

mod common;

use common::{edited_mapping, ClickUpMock, CLICKUP_TOKEN};
use zunder_core::ids::clickup_entity_id;
use zunder_core::EntityKind;
use zunder_import::convert::TargetCount;
use zunder_import::mapping::Mapping;
use zunder_import::{plan, report};

async fn proposal(mock: &ClickUpMock) -> plan::Proposal {
    plan::plan(&mock.client(), None, "2026-10-11")
        .await
        .unwrap()
}

#[tokio::test]
async fn plan_proposes_structure_and_statuses() {
    let mock = ClickUpMock::start().await;
    let p = proposal(&mock).await;
    let m = &p.mapping;
    assert_eq!(m.team_id, "9000001");

    let areas: Vec<(&str, &str)> = m
        .areas
        .iter()
        .map(|(k, a)| (k.as_str(), a.clickup_id.as_deref().unwrap()))
        .collect();
    assert_eq!(
        areas,
        [
            ("altlasten", "90012"),
            ("werkstatt", "90011"),
            ("zuhause", "90010")
        ]
    );
    assert_eq!(m.areas["altlasten"].state.as_deref(), Some("archived"));
    assert_eq!(m.areas["zuhause"].state, None);

    let projects: Vec<(&str, &str, &str)> = m
        .projects
        .iter()
        .map(|(k, p)| {
            (
                k.as_str(),
                p.area.as_str(),
                p.clickup_id.as_deref().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        projects,
        [
            ("garten-2025", "zuhause", "9001003"),
            ("ideen", "werkstatt", "9001101"),
            ("renovierung", "zuhause", "folder-90020"),
            ("werkzeugkiste", "werkstatt", "folder-90021"),
        ]
    );
    assert_eq!(
        m.projects["garten-2025"].status.as_deref(),
        Some("completed")
    );

    let targets: Vec<(&str, &str)> = m
        .lists
        .iter()
        .map(|(id, l)| (id.as_str(), l.target.as_str()))
        .collect();
    assert_eq!(
        targets,
        [
            ("9001001", "area:zuhause"),
            ("9001002", "inbox"),
            ("9001003", "project:garten-2025"),
            ("9001101", "project:ideen"),
            ("9002001", "project:renovierung"),
            ("9002002", "project:renovierung"),
            ("9002101", "project:werkzeugkiste"),
        ]
    );
    assert_eq!(
        m.lists["9002001"].path.as_deref(),
        Some("Zuhause › Renovierung › Küche")
    );

    let backlog = &m.lists["9002101"].statuses;
    assert_eq!(backlog["backlog"], "open");
    assert_eq!(backlog["in progress"], "in_progress");
    assert_eq!(backlog["in review"], "review");
    assert_eq!(backlog["eingefroren"], "", "unknown status stays open");
    assert_eq!(backlog["closed"], "done");
    let kitchen = &m.lists["9002001"].statuses;
    assert_eq!(kitchen["doing"], "in_progress");
    assert_eq!(kitchen["waiting"], "blocked");
    assert_eq!(kitchen["cancelled"], "dropped");
    assert_eq!(m.lists["9001101"].statuses["idee"], "open", "type fallback");
    assert_eq!(m.lists["9001003"].statuses["erledigt"], "done");
    assert_eq!(p.open_statuses, 1);
}

#[tokio::test]
async fn rendered_plan_is_commented_toml_that_roundtrips() {
    let mock = ClickUpMock::start().await;
    let p = proposal(&mock).await;
    assert!(p.toml.starts_with("# zunder-import – Zuordnungsdatei"));
    assert!(p
        .toml
        .contains("\"eingefroren\" = \"\"  # UNBEKANNT – bitte festlegen"));
    assert!(p.toml.contains("# existing_id = \"…\""));
    assert!(p.toml.contains("# repo = \"owner/name\""));
    let parsed = Mapping::parse(&p.toml).unwrap();
    assert_eq!(parsed, p.mapping);

    let resolved = parsed.resolve().unwrap();
    assert_eq!(
        resolved.areas["zuhause"].id,
        clickup_entity_id(EntityKind::Area, "90010")
    );
    assert_eq!(
        resolved.projects["renovierung"].id,
        clickup_entity_id(EntityKind::Project, "folder-90020")
    );
}

fn target<'a>(targets: &'a [TargetCount], key: &str) -> &'a TargetCount {
    targets
        .iter()
        .find(|t| t.key == key || (key == "inbox" && t.kind == "inbox"))
        .unwrap_or_else(|| panic!("target {key}"))
}

#[tokio::test]
async fn report_counts_and_flags_problems() {
    let mock = ClickUpMock::start().await;
    let mapping = proposal(&mock).await.mapping.resolve().unwrap();
    let loaded = report::load(&mock.client(), &mapping).await.unwrap();
    let r = &loaded.report;
    let d = &r.diagnostics;

    assert_eq!(r.lists_total, 7);
    assert_eq!(r.lists_skipped, 0);
    assert_eq!(d.tasks_loaded, 18, "17 from lists + 1 archived parent");
    assert_eq!(d.tasks_imported, 17);
    assert!(r.blocking);

    assert_eq!(d.unmapped_statuses.len(), 1);
    assert_eq!(d.unmapped_statuses[0].status, "eingefroren");
    assert_eq!(d.unmapped_statuses[0].tasks, 1);
    assert_eq!(
        d.unmapped_statuses[0].path,
        "Werkstatt › Werkzeugkiste › Backlog"
    );
    assert!(d.without_target.is_empty());

    let orphans: Vec<&str> = d.orphans.iter().map(|o| o.clickup_id.as_str()).collect();
    assert_eq!(orphans, ["86a006"]);
    assert!(d.orphans[0].detail.contains("86zz404"));

    let mut downgrades: Vec<&str> = d
        .dev_downgrades
        .iter()
        .map(|t| t.clickup_id.as_str())
        .collect();
    downgrades.sort();
    assert_eq!(downgrades, ["86a002", "86f001", "86f003"]);
    let f001 = d
        .dev_downgrades
        .iter()
        .find(|t| t.clickup_id == "86f001")
        .unwrap();
    assert!(f001.detail.contains("feature → aufgabe"), "{}", f001.detail);
    assert!(
        f001.detail.contains("review → in_progress"),
        "{}",
        f001.detail
    );

    assert_eq!(d.title_fixes.len(), 1);
    assert_eq!(d.title_fixes[0].clickup_id, "86e002");

    let keys: Vec<&str> = d.targets.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "zuhause",
            "garten-2025",
            "ideen",
            "renovierung",
            "werkzeugkiste",
            ""
        ]
    );
    let home = target(&d.targets, "zuhause");
    assert_eq!(
        (home.tasks, home.subtasks, home.open, home.closed),
        (5, 3, 5, 3)
    );
    assert_eq!(home.name, "Zuhause");
    let reno = target(&d.targets, "renovierung");
    assert_eq!(
        (reno.tasks, reno.subtasks, reno.open, reno.closed),
        (4, 0, 2, 2)
    );
    assert_eq!(reno.name, "Zuhause › Renovierung");
    let tools = target(&d.targets, "werkzeugkiste");
    assert_eq!((tools.tasks, tools.open, tools.closed), (2, 1, 1));
    let inbox = target(&d.targets, "inbox");
    assert_eq!((inbox.tasks, inbox.name.as_str()), (1, "Inbox"));

    let text = report::render_text(r);
    assert!(
        text.contains("Nicht zugeordnete Status – blockiert den Import (1)"),
        "{text}"
    );
    assert!(text.contains("„eingefroren“"), "{text}");
    assert!(
        text.contains("Die Zuordnung ist noch unvollständig"),
        "{text}"
    );

    let json = serde_json::to_value(r).unwrap();
    assert_eq!(json["tasks_imported"], 17);
    assert_eq!(json["blocking"], true);
    assert_eq!(json["unmapped_statuses"][0]["status"], "eingefroren");
}

#[tokio::test]
async fn report_after_editing_is_clean() {
    let mock = ClickUpMock::start().await;
    let mut mapping = edited_mapping(proposal(&mock).await.mapping);
    // A list that is skipped is not loaded at all.
    mapping.lists.get_mut("9001101").unwrap().target = "skip".into();
    // A list missing from the mapping shows up as "without target".
    mapping.lists.remove("9001002");
    let resolved = mapping.resolve().unwrap();
    let loaded = report::load(&mock.client(), &resolved).await.unwrap();
    let r = &loaded.report;
    let d = &r.diagnostics;
    assert_eq!(r.lists_skipped, 1);
    assert!(mock.requests_to("/list/9001101/task").is_empty());
    assert!(d.unmapped_statuses.is_empty());
    assert_eq!(d.without_target.len(), 1);
    assert_eq!(d.without_target[0].list_id, "9001002");
    assert_eq!(d.without_target[0].path, "Zuhause › Inbox");
    assert!(r.blocking);
    let downgrades: Vec<&str> = d
        .dev_downgrades
        .iter()
        .map(|t| t.clickup_id.as_str())
        .collect();
    assert_eq!(
        downgrades,
        ["86a002"],
        "only the bug outside the dev project"
    );

    mapping.lists.insert(
        "9001002".into(),
        zunder_import::mapping::ListMapping {
            path: None,
            target: "loose".into(),
            statuses: [("to do".into(), "open".into())].into_iter().collect(),
        },
    );
    let loaded = report::load(&mock.client(), &mapping.resolve().unwrap())
        .await
        .unwrap();
    assert!(!loaded.report.blocking);
    assert_eq!(loaded.report.diagnostics.tasks_imported, 17);
    assert!(loaded
        .report
        .diagnostics
        .targets
        .iter()
        .any(|t| t.kind == "loose" && t.tasks == 1));
}

#[tokio::test]
async fn archived_tasks_are_loaded_on_request() {
    let mock = ClickUpMock::start().await;
    let mut mapping = edited_mapping(proposal(&mock).await.mapping);
    mapping.settings.import_archived_tasks = true;
    let loaded = report::load(&mock.client(), &mapping.resolve().unwrap())
        .await
        .unwrap();
    assert_eq!(loaded.report.diagnostics.tasks_imported, 19);
    assert!(loaded
        .conversion
        .tasks
        .iter()
        .any(|t| t.label.contains("86e900")));
}

#[tokio::test]
async fn cli_plan_writes_mapping_file() {
    let mock = ClickUpMock::start().await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("mapping.toml");
    let url = mock.url.clone();
    let out_arg = out.clone();
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_zunder-import"))
            .args(["plan", "--out"])
            .arg(&out_arg)
            .env("CLICKUP_TOKEN", CLICKUP_TOKEN)
            .env("CLICKUP_API_URL", url)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("3 Bereiche, 4 Projekte, 7 Listen, 1 offene Status"),
        "{stdout}"
    );
    let mapping = Mapping::load(&out).unwrap();
    assert_eq!(mapping.lists.len(), 7);

    // A second plan does not overwrite the edited file.
    let url = mock.url.clone();
    let out_arg = out.clone();
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_zunder-import"))
            .args(["plan", "--out"])
            .arg(&out_arg)
            .env("CLICKUP_TOKEN", CLICKUP_TOKEN)
            .env("CLICKUP_API_URL", url)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--force"));
}
