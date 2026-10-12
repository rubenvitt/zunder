//! Calendar feed: settings (`/api/settings/calendar`), the public feed with
//! its own token (`/calendar/<token>.ics`) and `/api/calendar.ics`.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::json;
use time::{Duration, OffsetDateTime, UtcOffset};
use zunder_core::{EntityKind, Location, NewArea, NewProject, NewTask, TaskStatus};

use common::{api_token, client_clock, ops, Auth, TestServer, BOOTSTRAP};

/// A timestamp `offset` from now in +02:00, whole seconds.
fn at(offset: Duration) -> OffsetDateTime {
    (OffsetDateTime::now_utc() + offset)
        .replace_nanosecond(0)
        .unwrap()
        .to_offset(UtcOffset::from_hms(2, 0, 0).unwrap())
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

fn ics_utc(t: OffsetDateTime) -> String {
    let t = t.to_offset(UtcOffset::UTC);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// The feed with folded lines joined again.
fn unfold(body: &str) -> String {
    body.replace("\r\n ", "")
}

/// The `VEVENT` blocks of a feed.
fn events(body: &str) -> Vec<String> {
    unfold(body)
        .split("BEGIN:VEVENT\r\n")
        .skip(1)
        .map(|e| e.split("END:VEVENT").next().unwrap().to_owned())
        .collect()
}

fn event_for<'a>(events: &'a [String], title: &str) -> &'a str {
    events
        .iter()
        .find(|e| e.contains(title))
        .unwrap_or_else(|| panic!("no event for {title:?} in {events:#?}"))
}

/// The path of a feed URL (`/calendar/…`).
fn path_of(url: &str) -> String {
    let url = url::Url::parse(url).unwrap();
    match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_owned(),
    }
}

struct Seeded {
    flame_start: OffsetDateTime,
    flame: zunder_core::ids::Id,
    timed: zunder_core::ids::Id,
}

async fn seed(s: &TestServer) -> Seeded {
    let flame_start = at(Duration::days(1));
    let mut clock = client_clock();
    let mut flame = None;
    let mut timed = None;
    let batch = ops(&mut clock, |b| {
        let area = b.create_area(NewArea::new("Programmieren"));
        let mut project = NewProject::new("Zunder", area);
        project.key = Some("ZUN".into());
        let project = b.create_project(project);
        let in_project = Location::Project { id: project };
        let phoenix = json!({"role": "flamme", "fire": "2026-W42"});

        let id = b.create_task(NewTask::new("Kalender-Feed bauen").at(in_project));
        b.set_task(id, "due", rfc3339(flame_start));
        b.set_task(id, "phoenix", phoenix.clone());
        flame = Some(id);

        let id = b.create_task(NewTask::new("Erledigte Flamme").with_status(TaskStatus::Done));
        b.set_task(id, "due", rfc3339(at(-Duration::days(2))));
        b.set_task(id, "phoenix", phoenix.clone());

        let id = b.create_task(NewTask::new("Verworfene Flamme").with_status(TaskStatus::Dropped));
        b.set_task(id, "due", rfc3339(at(Duration::days(2))));
        b.set_task(id, "phoenix", phoenix.clone());

        let id = b.create_task(NewTask::new("Alte Flamme"));
        b.set_task(id, "due", rfc3339(at(-Duration::days(100))));
        b.set_task(id, "phoenix", phoenix.clone());

        let id = b.create_task(NewTask::new("Flamme nur mit Datum"));
        b.set_task(id, "due", "2026-12-01");
        b.set_task(id, "phoenix", phoenix.clone());

        let id = b.create_task(NewTask::new("Gelöschte Flamme"));
        b.set_task(id, "due", rfc3339(at(Duration::days(3))));
        b.set_task(id, "phoenix", phoenix);
        b.set_task(id, "deleted", true);

        let id = b.create_task(NewTask::new("Zahnarzt, 2. Termin; mitbringen"));
        b.set_task(id, "due", rfc3339(at(Duration::hours(5))));
        timed = Some(id);

        let id = b.create_task(NewTask::new("Nur ein Datum"));
        b.set_task(id, "due", "2026-12-02");
        b.create_task(NewTask::new("Ohne Fälligkeit"));
    });
    let r = s.push(Auth::Bearer(BOOTSTRAP), &batch).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(
        r.json()["rejected"].as_array().unwrap().is_empty(),
        "{}",
        r.text()
    );
    Seeded {
        flame_start,
        flame: flame.unwrap(),
        timed: timed.unwrap(),
    }
}

async fn create_feed(s: &TestServer, session: &str) -> serde_json::Value {
    let r = s
        .request(
            Method::POST,
            "/api/settings/calendar",
            Auth::Web(session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    r.json()
}

#[tokio::test]
async fn feed_settings_need_a_browser_session() {
    let s = TestServer::new().await;
    let session = s.session().await;
    let t = api_token(&s, "claude").await;

    let r = s.get("/api/settings/calendar", Auth::Web(&session)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json(), json!({"enabled": false, "created_at": null}));

    let r = s.get("/api/settings/calendar", Auth::Bearer(&t)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = s
        .request(
            Method::POST,
            "/api/settings/calendar",
            Auth::Cookie(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = s
        .request(
            Method::POST,
            "/api/settings/calendar",
            Auth::Bearer(&t),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    let created = create_feed(&s, &session).await;
    assert_eq!(created["enabled"], true);
    let url = created["url"].as_str().unwrap();
    let url_all = created["url_all"].as_str().unwrap();
    assert!(
        url.starts_with("http://localhost:8080/calendar/zc_") && url.ends_with(".ics"),
        "{url}"
    );
    assert_eq!(url_all, format!("{url}?scope=all"));
    let r = s.get("/api/settings/calendar", Auth::Web(&session)).await;
    assert_eq!(r.json()["enabled"], true);
    assert_eq!(r.json()["created_at"], created["created_at"]);
    assert!(r.json().get("url").is_none(), "the URL is shown only once");
}

#[tokio::test]
async fn public_feed_lists_flames_and_timed_tasks() {
    let s = TestServer::new().await;
    let seeded = seed(&s).await;
    let session = s.session().await;
    let created = create_feed(&s, &session).await;

    let r = s
        .get(&path_of(created["url"].as_str().unwrap()), Auth::None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(
        r.header("content-type"),
        Some("text/calendar; charset=utf-8")
    );
    let body = r.text();
    assert!(
        body.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"),
        "{body}"
    );
    assert!(body.ends_with("END:VCALENDAR\r\n"));
    for line in body.split("\r\n") {
        assert!(line.len() <= 75, "line too long: {line:?}");
    }
    assert!(unfold(&body).contains("X-WR-CALNAME:Zunder – Flammen\r\n"));

    let list = events(&body);
    assert_eq!(list.len(), 2, "{list:#?}");
    let flame = event_for(&list, "Kalender-Feed bauen");
    assert!(flame.contains(&format!("UID:{}@zunder\r\n", seeded.flame)));
    assert!(
        flame.contains("SUMMARY:🔥 Kalender-Feed bauen\r\n"),
        "{flame}"
    );
    assert!(flame.contains(&format!("DTSTART:{}\r\n", ics_utc(seeded.flame_start))));
    assert!(flame.contains(&format!(
        "DTEND:{}\r\n",
        ics_utc(seeded.flame_start + Duration::hours(2))
    )));
    let link = format!("http://localhost:8080/#/aufgabe/{}", seeded.flame);
    assert!(
        flame.contains(&format!(
            "DESCRIPTION:ZUN-1 · Programmieren › Zunder\\n{link}\r\n"
        )),
        "{flame}"
    );
    assert!(flame.contains(&format!("URL:{link}\r\n")));
    assert!(!flame.contains("TRANSP"));
    let done = event_for(&list, "Erledigte Flamme");
    assert!(done.contains("SUMMARY:✓ 🔥 Erledigte Flamme\r\n"), "{done}");

    // Everything with a due time, other tasks for 30 minutes and free.
    let r = s
        .get(&path_of(created["url_all"].as_str().unwrap()), Auth::None)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(unfold(&r.text()).contains("X-WR-CALNAME:Zunder\r\n"));
    let list = events(&r.text());
    assert_eq!(list.len(), 3, "{list:#?}");
    let timed = event_for(&list, "Zahnarzt");
    assert!(timed.contains(&format!("UID:{}@zunder\r\n", seeded.timed)));
    assert!(timed.contains("SUMMARY:Zahnarzt\\, 2. Termin\\; mitbringen\r\n"));
    assert!(timed.contains("TRANSP:TRANSPARENT\r\n"));
    let start = timed.split("DTSTART:").nth(1).unwrap().get(..16).unwrap();
    let end = timed.split("DTEND:").nth(1).unwrap().get(..16).unwrap();
    let parse = |s: &str| {
        time::PrimitiveDateTime::parse(
            s,
            time::macros::format_description!("[year][month][day]T[hour][minute][second]Z"),
        )
        .unwrap()
    };
    assert_eq!(parse(end) - parse(start), Duration::minutes(30));

    // Rescheduling keeps the UID and moves the event.
    let later = at(Duration::days(4));
    let mut clock = client_clock();
    let batch = ops(&mut clock, |b| {
        b.set(EntityKind::Task, seeded.flame, "due", rfc3339(later));
    });
    s.push(Auth::Bearer(BOOTSTRAP), &batch).await;
    let r = s
        .get(&path_of(created["url"].as_str().unwrap()), Auth::None)
        .await;
    let list = events(&r.text());
    let flame = event_for(&list, "Kalender-Feed bauen");
    assert!(flame.contains(&format!("UID:{}@zunder\r\n", seeded.flame)));
    assert!(flame.contains(&format!("DTSTART:{}\r\n", ics_utc(later))));
    assert_eq!(r.text().matches("Kalender-Feed bauen").count(), 1);
}

#[tokio::test]
async fn feed_token_is_checked_and_replaceable() {
    let s = TestServer::new().await;
    let session = s.session().await;
    let t = api_token(&s, "claude").await;

    // Without a feed nothing answers, not even an API token in the path.
    let r = s.get("/calendar/zc_nothing.ics", Auth::None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = s.get(&format!("/calendar/{t}.ics"), Auth::None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    let first = create_feed(&s, &session).await;
    let first_path = path_of(first["url"].as_str().unwrap());
    let token = first_path
        .strip_prefix("/calendar/")
        .unwrap()
        .strip_suffix(".ics")
        .unwrap()
        .to_owned();
    assert_eq!(s.get(&first_path, Auth::None).await.status, StatusCode::OK);
    // Without the extension, too.
    let r = s.get(&format!("/calendar/{token}"), Auth::None).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = s.get(&format!("/calendar/{token}x.ics"), Auth::None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = s.get(&format!("{first_path}?scope=alle"), Auth::None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = s
        .get(&format!("{first_path}?from=2026-01-01"), Auth::None)
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // The feed token is no bearer token.
    let r = s.get("/api/calendar.ics", Auth::Bearer(&token)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // A new token replaces the old one.
    let second = create_feed(&s, &session).await;
    assert_ne!(first["url"], second["url"]);
    assert_eq!(
        s.get(&first_path, Auth::None).await.status,
        StatusCode::NOT_FOUND
    );
    let second_path = path_of(second["url"].as_str().unwrap());
    assert_eq!(s.get(&second_path, Auth::None).await.status, StatusCode::OK);

    // Switching the feed off; it survives a restart in between.
    let s = s.restart().await;
    assert_eq!(s.get(&second_path, Auth::None).await.status, StatusCode::OK);
    let session = s.session().await;
    let r = s
        .request(
            Method::DELETE,
            "/api/settings/calendar",
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json(), json!({"enabled": false, "created_at": null}));
    assert_eq!(
        s.get(&second_path, Auth::None).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn api_feed_uses_the_normal_auth() {
    let s = TestServer::new().await;
    seed(&s).await;
    let t = api_token(&s, "skripte").await;

    let r = s.get("/api/calendar.ics", Auth::None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = s.get("/api/calendar.ics", Auth::Bearer(&t)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(events(&r.text()).len(), 2);
    let r = s.get("/api/calendar.ics?scope=all", Auth::Bearer(&t)).await;
    assert_eq!(events(&r.text()).len(), 3);
}
