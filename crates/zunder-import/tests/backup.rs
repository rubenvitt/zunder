//! `backup` against the mock ClickUp API.

mod common;

use std::path::Path;

use common::{ClickUpMock, CLICKUP_TOKEN};
use serde_json::Value;
use zunder_import::backup::{self, BackupOptions};
use zunder_import::fetch::{self, CommentParent};

/// 2024-01-01T00:00:00Z
const SINCE: i64 = 1_704_067_200_000;
/// 2026-10-11T00:00:00Z
const UNTIL: i64 = 1_791_676_800_000;

fn options(base: &Path) -> BackupOptions {
    BackupOptions {
        base_dir: base.to_path_buf(),
        force: false,
        since_ms: SINCE,
        until_ms: UNTIL,
        window_days: 90,
        stamp: "2026-10-11_12-00-00".into(),
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn writes_complete_raw_backup() {
    let mock = ClickUpMock::start().await;
    let dir = tempfile::tempdir().unwrap();
    let root = backup::backup(&mock.client(), &options(&dir.path().join("sicherung")))
        .await
        .unwrap();
    assert_eq!(root, dir.path().join("sicherung/2026-10-11_12-00-00"));

    let manifest = read(&root.join("manifest.json"));
    let team = &manifest["teams"][0];
    assert_eq!(team["id"], "9000001");
    assert_eq!(team["spaces"], 3);
    assert_eq!(team["folders"], 2);
    assert_eq!(team["lists"], 7);
    assert_eq!(team["tasks"], 18, "closed, subtasks and archived included");
    assert_eq!(team["time_entries"], 3);
    assert_eq!(manifest["parents_fetched"], 1);
    assert_eq!(manifest["missing_parents"], serde_json::json!(["86zz404"]));

    assert_eq!(
        read(&root.join("teams.json"))["teams"][0]["name"],
        "Muster-Workspace"
    );
    assert_eq!(
        read(&root.join("teams/9000001/spaces.json"))
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let bath = read(&root.join("lists/9002002/tasks.json"));
    let ids: Vec<&str> = bath
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["86e001", "86e002", "86e900"]);
    // Raw means raw: fields the importer does not use are kept.
    let general = read(&root.join("lists/9001001/tasks.json"));
    assert_eq!(general[0]["creator"]["username"], "Beispielperson");
    assert_eq!(
        read(&root.join("lists/9002101/list.json"))["statuses"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    assert!(root.join("folders/90020/lists.json").exists());
    assert!(root.join("spaces/90010/lists.json").exists());
    assert_eq!(read(&root.join("tasks/parents.json"))[0]["id"], "86a900");
    let entries = read(&root.join("teams/9000001/time_entries.json"));
    assert_eq!(entries.as_array().unwrap().len(), 3);

    // Comments (with threaded replies) and the full task detail with its
    // attachment metadata: neither is part of GET /list/{id}/task.
    assert_eq!(manifest["task_details"], 18);
    assert_eq!(manifest["task_details_missing"], serde_json::json!([]));
    assert_eq!(manifest["attachments"], 1);
    assert_eq!(manifest["tasks_with_comments"], 1);
    assert_eq!(manifest["lists_with_comments"], 1);
    assert_eq!(manifest["comments"], 3);
    assert_eq!(manifest["comment_replies"], 1);
    let detail = read(&root.join("tasks/86a002/task.json"));
    assert_eq!(detail["attachments"][0]["title"], "beleg-muster.pdf");
    assert!(root.join("tasks/86e001/task.json").exists());
    let comments = read(&root.join("tasks/86a002/comments.json"));
    assert_eq!(comments.as_array().unwrap().len(), 2);
    assert_eq!(
        comments[0]["comment_text"],
        "Agenten-Ergebnis: Belege für 2025 vollständig"
    );
    assert_eq!(
        comments[0]["replies"][0]["comment_text"],
        "Danke, übernommen"
    );
    assert!(comments[1].get("replies").is_none(), "no replies requested");
    assert_eq!(mock.requests_to("/comment/90000000001/reply").len(), 0);
    assert!(
        !root.join("tasks/86e001/comments.json").exists(),
        "only tasks with comments get a file"
    );
    assert_eq!(
        read(&root.join("lists/9001001/comments.json"))[0]["comment_text"],
        "Notiz zur Liste"
    );
    // The parent outside the lists is a full detail already; its comments
    // are requested all the same.
    assert_eq!(mock.requests_to("/task/86a900").len(), 1);
    assert_eq!(mock.requests_to("/task/86a900/comment").len(), 1);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&root).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "backup directory is private");
    }
}

#[tokio::test]
async fn time_entries_are_requested_in_windows_covering_everything() {
    let mock = ClickUpMock::start().await;
    let opts = options(Path::new("/unused"));
    let entries = backup::time_entries(&mock.client(), "9000001", &opts)
        .await
        .unwrap();
    assert_eq!(entries.len(), 3);
    let windows: Vec<(i64, i64)> = mock
        .requests_to("/team/9000001/time_entries")
        .iter()
        .map(|r| {
            let query = r.split_once('?').unwrap().1;
            let params: std::collections::HashMap<String, String> =
                url::form_urlencoded::parse(query.as_bytes())
                    .into_owned()
                    .collect();
            (
                params["start_date"].parse().unwrap(),
                params["end_date"].parse().unwrap(),
            )
        })
        .collect();
    assert_eq!(windows.first().unwrap().0, SINCE);
    assert_eq!(windows.last().unwrap().1, UNTIL);
    for pair in windows.windows(2) {
        assert_eq!(pair[1].0, pair[0].1 + 1, "windows are contiguous");
    }
    let max = 90 * 24 * 60 * 60 * 1000;
    assert!(windows.iter().all(|(s, e)| e - s < max));
    assert_eq!(windows.len(), 12, "1014 days in 90-day windows");
}

#[tokio::test]
async fn refuses_to_write_into_a_git_repository() {
    let mock = ClickUpMock::start().await;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("projekt");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let target = repo.join("backups");

    let err = backup::backup(&mock.client(), &options(&target))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Git-Repository"), "{err}");
    assert!(!target.exists(), "nothing written");
    assert!(mock.requests().is_empty(), "nothing fetched");

    let forced = BackupOptions {
        force: true,
        ..options(&target)
    };
    let root = backup::backup(&mock.client(), &forced).await.unwrap();
    assert!(root.join("manifest.json").exists());

    // The same stamp twice is refused instead of mixing two backups.
    let err = backup::backup(&mock.client(), &forced).await.unwrap_err();
    assert!(err.to_string().contains("existiert bereits"), "{err}");
}

#[tokio::test]
async fn cli_backup_defaults_to_home_and_refuses_repo_paths() {
    let mock = ClickUpMock::start().await;
    let home = tempfile::tempdir().unwrap();
    let repo = home.path().join("code");
    std::fs::create_dir_all(repo.join(".git")).unwrap();

    let cli = |args: Vec<String>| {
        let url = mock.url.clone();
        let home = home.path().to_path_buf();
        tokio::task::spawn_blocking(move || {
            std::process::Command::new(env!("CARGO_BIN_EXE_zunder-import"))
                .arg("backup")
                .args(args)
                .args(["--since", "2026-01-01", "--window-days", "365"])
                .env("CLICKUP_TOKEN", CLICKUP_TOKEN)
                .env("CLICKUP_API_URL", url)
                .env("HOME", home)
                .output()
                .unwrap()
        })
    };

    let out = cli(vec![
        "--out".into(),
        repo.join("dump").display().to_string(),
    ])
    .await
    .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("Git-Repository"));

    let out = cli(vec![]).await.unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let base = home.path().join("zunder-import-backup");
    let stamps: Vec<_> = std::fs::read_dir(&base).unwrap().collect();
    assert_eq!(stamps.len(), 1);
    let root = stamps[0].as_ref().unwrap().path();
    assert!(root.join("manifest.json").exists());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Sicherung geschrieben"));
}

fn comments(prefix: &str, range: std::ops::RangeInclusive<u32>) -> Value {
    let list: Vec<Value> = range
        .map(|n| {
            serde_json::json!({
                "id": format!("{prefix}{n:02}"),
                "comment_text": format!("Kommentar {n}"),
                // Newest first: the date falls with n.
                "date": (1_790_000_100_000u64 - u64::from(n) * 1000).to_string(),
                "reply_count": 0
            })
        })
        .collect();
    serde_json::json!({ "comments": list })
}

#[tokio::test]
async fn comments_are_paged_from_the_oldest_comment() {
    let mock = ClickUpMock::start().await;
    {
        let mut routes = mock.state.routes.lock().unwrap();
        routes.insert("/task/86f001/comment".into(), comments("c", 1..=25));
        // ClickUp may repeat the start comment; it is kept once.
        routes.insert(
            "/task/86f001/comment?start_id=c25".into(),
            comments("c", 25..=27),
        );
        // A full page that never advances must not loop.
        routes.insert("/task/86f003/comment".into(), comments("d", 1..=25));
        routes.insert(
            "/task/86f003/comment?start_id=d25".into(),
            comments("d", 1..=25),
        );
    }
    let client = mock.client();

    let all = fetch::fetch_comments(&client, CommentParent::Task("86f001"))
        .await
        .unwrap();
    let ids: Vec<&str> = all.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 27);
    assert_eq!((ids[0], ids[26]), ("c01", "c27"));
    let requests = mock.requests_to("/task/86f001/comment");
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].contains("start=1790000075000") && requests[1].contains("start_id=c25"),
        "{requests:?}"
    );

    let stuck = fetch::fetch_comments(&client, CommentParent::Task("86f003"))
        .await
        .unwrap();
    assert_eq!(stuck.len(), 25);
    assert_eq!(mock.requests_to("/task/86f003/comment").len(), 2);

    // Unknown tasks have no comments.
    assert!(
        fetch::fetch_comments(&client, CommentParent::Task("86zz404"))
            .await
            .unwrap()
            .is_empty()
    );
}
