//! ClickUp client and fetching against the mock API.

mod common;

use common::ClickUpMock;
use zunder_import::clickup::{ClickUpApi, ClickUpError, HttpClickUp};
use zunder_import::fetch;

#[tokio::test]
async fn paginates_until_last_page_or_empty_page() {
    let mock = ClickUpMock::start().await;
    let api = mock.client();

    // Ends with last_page = true on page 1.
    let tasks = fetch::fetch_list_tasks_raw(&api, "9001001", false)
        .await
        .unwrap();
    assert_eq!(tasks.len(), 7);
    let pages = mock.requests_to("/list/9001001/task");
    assert_eq!(pages.len(), 2);
    for (n, request) in pages.iter().enumerate() {
        assert!(request.contains(&format!("page={n}")), "{request}");
        assert!(request.contains("include_closed=true"), "{request}");
        assert!(request.contains("subtasks=true"), "{request}");
        assert!(request.contains("archived=false"), "{request}");
    }

    // No last_page flag: ends with the first empty page.
    let tasks = fetch::fetch_list_tasks_raw(&api, "9002001", false)
        .await
        .unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(mock.requests_to("/list/9002001/task").len(), 2);
}

#[tokio::test]
async fn retries_after_rate_limit() {
    let mock = ClickUpMock::start().await;
    mock.rate_limit("/team", 2);
    let body = mock.client().teams().await.unwrap();
    assert_eq!(body["teams"][0]["id"], "9000001");
    assert_eq!(mock.requests_to("/team").len(), 3);
}

#[tokio::test]
async fn gives_up_after_max_attempts() {
    let mock = ClickUpMock::start().await;
    mock.rate_limit("/team", 10);
    let err = mock.client().teams().await.unwrap_err();
    assert!(
        matches!(err, ClickUpError::RateLimited { attempts: 4, .. }),
        "{err}"
    );
}

#[tokio::test]
async fn sends_token_and_reports_rejection() {
    let mock = ClickUpMock::start().await;
    let err = HttpClickUp::new("falsch", &mock.url)
        .teams()
        .await
        .unwrap_err();
    assert!(matches!(err, ClickUpError::Unauthorized(_)), "{err}");
    assert!(mock.client().teams().await.is_ok());
}

#[tokio::test]
async fn unknown_task_is_none() {
    let mock = ClickUpMock::start().await;
    let api = mock.client();
    assert!(api.task("86zz404").await.unwrap().is_none());
    assert!(api.task("86a900").await.unwrap().is_some());
}

#[tokio::test]
async fn hierarchy_includes_archived_items_and_details() {
    let mock = ClickUpMock::start().await;
    let api = mock.client();
    let team = fetch::select_team(&api, None).await.unwrap();
    let hierarchy = fetch::fetch_hierarchy(&api, team, true).await.unwrap();
    let names: Vec<String> = hierarchy.lists().iter().map(|l| l.path()).collect();
    assert_eq!(
        names,
        [
            "Zuhause › Renovierung › Küche",
            "Zuhause › Renovierung › Bad",
            "Zuhause › Allgemein",
            "Zuhause › Inbox",
            "Zuhause › Garten 2025",
            "Werkstatt › Werkzeugkiste › Backlog",
            "Werkstatt › Ideen",
        ]
    );
    assert_eq!(hierarchy.spaces.len(), 3);
    assert!(hierarchy.spaces[2].space.item.archived);
    let garden = hierarchy.list("9001003").unwrap();
    assert!(
        garden.is_archived(),
        "archived flag survives the detail call"
    );
    assert_eq!(garden.list.item.statuses.len(), 2);
}

#[tokio::test]
async fn fetch_tasks_resolves_parents_outside_the_lists() {
    let mock = ClickUpMock::start().await;
    let api = mock.client();
    let lists = vec![("9001001".to_owned(), "Allgemein".to_owned())];
    let set = fetch::fetch_tasks(&api, &lists, false).await.unwrap();
    let ids: Vec<&str> = set.tasks.iter().map(|t| t.item.id.as_str()).collect();
    assert_eq!(
        ids,
        ["86a001", "86a002", "86a003", "86a004", "86a005", "86a006", "86a007", "86a900"]
    );
    assert_eq!(
        set.missing_parents.into_iter().collect::<Vec<_>>(),
        ["86zz404"]
    );
}

#[tokio::test]
async fn select_team_rejects_unknown_id() {
    let mock = ClickUpMock::start().await;
    let err = fetch::select_team(&mock.client(), Some("123"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("123"), "{err}");
}
