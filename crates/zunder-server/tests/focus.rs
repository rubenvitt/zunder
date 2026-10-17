//! "Erst für dich" in Heute: `/api/settings/focus` (day clicked away, task
//! picked for a day), browser sessions only like the other settings.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use common::{api_token, Auth, TestResponse, TestServer};

const TASK: &str = "t0000000000000000000000a";

async fn put(s: &TestServer, session: &str, body: Value) -> TestResponse {
    s.request(
        Method::PUT,
        "/api/settings/focus",
        Auth::Web(session),
        Some(body),
    )
    .await
}

async fn get(s: &TestServer, session: &str) -> Value {
    let r = s.get("/api/settings/focus", Auth::Web(session)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    r.json()
}

#[tokio::test]
async fn focus_settings_store_the_day_and_the_pick() {
    let s = TestServer::new().await;
    let session = s.session().await;
    assert_eq!(
        get(&s, &session).await,
        json!({"dismissed_on": null, "chosen": null})
    );

    // Clicked away today; the answer is the new state.
    let r = put(&s, &session, json!({"dismissed_on": "2026-10-17"})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({"dismissed_on": "2026-10-17", "chosen": null})
    );

    // A missing field stays, another one changes.
    let r = put(
        &s,
        &session,
        json!({"chosen": {"on": "2026-10-18", "task_id": TASK}}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let both =
        json!({"dismissed_on": "2026-10-17", "chosen": {"on": "2026-10-18", "task_id": TASK}});
    assert_eq!(r.json(), both);
    assert_eq!(put(&s, &session, json!({})).await.json(), both);

    // Kept across a restart (SQLite settings).
    let s = s.restart().await;
    let session = s.session().await;
    assert_eq!(get(&s, &session).await, both);

    // null removes.
    let r = put(&s, &session, json!({"dismissed_on": null, "chosen": null})).await;
    assert_eq!(r.json(), json!({"dismissed_on": null, "chosen": null}));
}

#[tokio::test]
async fn focus_settings_refuse_bad_values() {
    let s = TestServer::new().await;
    let session = s.session().await;
    for body in [
        json!({"dismissed_on": "17.10.2026"}),
        json!({"dismissed_on": "2026-02-30"}),
        json!({"dismissed_on": 20261017}),
        json!({"dismissed": "2026-10-17"}),
        json!({"chosen": {"on": "2026-10-17"}}),
        json!({"chosen": {"on": "2026-10-17", "task_id": "keine-id"}}),
        json!({"chosen": "ZUN-1"}),
    ] {
        let r = put(&s, &session, body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}: {}", r.text());
    }
    assert_eq!(
        get(&s, &session).await,
        json!({"dismissed_on": null, "chosen": null})
    );
}

#[tokio::test]
async fn focus_settings_need_a_browser_session() {
    let s = TestServer::new().await;
    let t = api_token(&s, "claude").await;
    let session = s.session().await;
    let body = json!({"dismissed_on": "2026-10-17"});

    let r = s.get("/api/settings/focus", Auth::None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = s.get("/api/settings/focus", Auth::Bearer(&t)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = s
        .request(
            Method::PUT,
            "/api/settings/focus",
            Auth::Bearer(&t),
            Some(body.clone()),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Writes need the CSRF header.
    let r = s
        .request(
            Method::PUT,
            "/api/settings/focus",
            Auth::Cookie(&session),
            Some(body),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(get(&s, &session).await["dismissed_on"], Value::Null);
}
