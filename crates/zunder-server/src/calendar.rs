//! Subscribable calendar feed (iCalendar, RFC 5545) of planned flames and
//! other tasks with a due time.
//!
//! - Only live tasks whose `due` is a timestamp become events; a plain date
//!   is a deadline, not an appointment, and stays out.
//! - [`FeedScope::Flames`]: tasks marked `phoenix.role = flamme`, 2 hours
//!   each. [`FeedScope::All`] adds every other task with a due time,
//!   30 minutes each and marked free (`TRANSP:TRANSPARENT`), so they do not
//!   block the calendar like a flame does.
//! - `dropped` tasks leave the feed, `done` tasks stay with a "✓ " in front
//!   of the title. Events that started more than [`PAST_DAYS`] days ago are
//!   left out, so the feed does not grow forever.
//! - The UID is the task ID, so moving a task moves its event instead of
//!   duplicating it. `DTSTAMP` and `LAST-MODIFIED` come from the task's
//!   `updated_at`; the output only changes when the data does.
//! - Times are written in UTC, which needs no `VTIMEZONE`; calendar apps
//!   show them in their own time zone.
//!
//! Access: the feed URL carries its own read-only token (`zc_…`, see
//! [`crate::auth::CALENDAR_TOKEN_PREFIX`]), because calendar apps cannot
//! send an `Authorization` header. There is at most one; it is stored as
//! SHA-256 in the settings ([`CALENDAR_TOKEN_KEY`]) and shown once when it
//! is created. It grants nothing but the feed.

use std::collections::BTreeMap;
use std::fmt::Write;

use serde::Serialize;
use time::{Duration, OffsetDateTime, UtcOffset};
use url::Url;
use zunder_core::{Due, EntityKind, PhoenixRole, State, Task, TaskStatus};

use crate::error::{AppError, AppResult};
use crate::views::LocationView;

/// Key prefix of the calendar feed settings.
pub const CALENDAR_PREFIX: &str = "calendar.";
/// SHA-256 (hex) of the feed token.
pub const CALENDAR_TOKEN_KEY: &str = "calendar.token_hash";
/// When the feed token was created (Unix milliseconds).
pub const CALENDAR_CREATED_KEY: &str = "calendar.created_at";

/// Length of a flame event.
pub const FLAME_MINUTES: i64 = 120;
/// Length of an event for any other task.
pub const TASK_MINUTES: i64 = 30;
/// Events that started more than this many days ago are left out.
pub const PAST_DAYS: i64 = 90;
/// How often calendar apps should refresh the feed.
const REFRESH: &str = "PT15M";
/// Maximum length of a content line in octets, without the line break.
const MAX_LINE_OCTETS: usize = 75;

/// Which tasks the feed contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeedScope {
    /// Flames only (default).
    #[default]
    Flames,
    /// Flames and every other task with a due time.
    All,
}

impl FeedScope {
    /// Parses the `scope` query parameter (`flames` or `all`; missing =
    /// flames).
    pub fn parse(value: Option<&str>) -> AppResult<Self> {
        match value {
            None | Some("") | Some("flames") => Ok(FeedScope::Flames),
            Some("all") => Ok(FeedScope::All),
            Some(other) => Err(AppError::bad_request(format!(
                "Unbekannter Umfang „{other}“ (erlaubt: flames, all)"
            ))),
        }
    }

    fn calendar_name(self) -> &'static str {
        match self {
            FeedScope::Flames => "Zunder – Flammen",
            FeedScope::All => "Zunder",
        }
    }
}

/// State of the feed for the settings view (`GET /api/settings/calendar`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CalendarSettings {
    /// Whether a feed token exists.
    pub enabled: bool,
    /// When the token was created (RFC 3339, UTC).
    pub created_at: Option<String>,
}

impl CalendarSettings {
    /// Reads the settings from the rows of [`crate::db::settings`].
    pub fn from_rows(rows: &BTreeMap<String, String>) -> Self {
        let enabled = rows.get(CALENDAR_TOKEN_KEY).is_some_and(|h| !h.is_empty());
        let created_at = rows
            .get(CALENDAR_CREATED_KEY)
            .filter(|_| enabled)
            .and_then(|v| v.parse::<i64>().ok())
            .map(crate::app::format_ms);
        Self {
            enabled,
            created_at,
        }
    }
}

/// A newly created feed; the URLs contain the token and are shown once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreatedCalendarFeed {
    #[serde(flatten)]
    pub settings: CalendarSettings,
    /// Flames only.
    pub url: String,
    /// Flames and every other task with a due time.
    pub url_all: String,
}

/// The subscription URL of the feed: `<public>/calendar/<token>.ics`, with
/// `?scope=all` for [`FeedScope::All`].
pub fn feed_url(public_url: &Url, token: &str, scope: FeedScope) -> String {
    let base = public_url.as_str().trim_end_matches('/');
    match scope {
        FeedScope::Flames => format!("{base}/calendar/{token}.ics"),
        FeedScope::All => format!("{base}/calendar/{token}.ics?scope=all"),
    }
}

/// One event of the feed.
struct Event {
    task: Task,
    start: OffsetDateTime,
    flame: bool,
}

/// Renders the feed for `scope` from the state. `public_url` is the base of
/// the links back to the tasks, `now` decides which past events are left
/// out.
pub fn render(state: &State, public_url: &Url, scope: FeedScope, now: OffsetDateTime) -> String {
    let oldest = now - Duration::days(PAST_DAYS);
    let mut events: Vec<Event> = state
        .live_tasks()
        .filter(|t| t.status != TaskStatus::Dropped)
        .filter_map(|task| {
            let Some(Due::DateTime(start)) = task.due else {
                return None;
            };
            let flame = task.phoenix.is_some_and(|p| p.role == PhoenixRole::Flamme);
            (start >= oldest && (flame || scope == FeedScope::All)).then_some(Event {
                task,
                start,
                flame,
            })
        })
        .collect();
    events.sort_by(|a, b| a.start.cmp(&b.start).then(a.task.id.cmp(&b.task.id)));

    let base = public_url.as_str().trim_end_matches('/');
    let mut out = String::new();
    for line in [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Zunder//Kalender-Feed//DE",
        "CALSCALE:GREGORIAN",
        "METHOD:PUBLISH",
    ] {
        push_line(&mut out, line);
    }
    push_line(
        &mut out,
        &format!("X-WR-CALNAME:{}", escape_text(scope.calendar_name())),
    );
    push_line(
        &mut out,
        &format!("REFRESH-INTERVAL;VALUE=DURATION:{REFRESH}"),
    );
    push_line(&mut out, &format!("X-PUBLISHED-TTL:{REFRESH}"));
    for event in &events {
        write_event(&mut out, state, base, event);
    }
    push_line(&mut out, "END:VCALENDAR");
    out
}

fn write_event(out: &mut String, state: &State, base: &str, event: &Event) {
    let task = &event.task;
    let minutes = if event.flame {
        FLAME_MINUTES
    } else {
        TASK_MINUTES
    };
    let end = event.start + Duration::minutes(minutes);
    let modified = state
        .updated_at(EntityKind::Task, task.id)
        .and_then(|h| i64::try_from(h.wall_ms()).ok())
        .and_then(|ms| OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok())
        .unwrap_or(event.start);
    let link = format!("{base}/#/aufgabe/{}", task.id);

    let mut summary = String::new();
    if task.status == TaskStatus::Done {
        summary.push_str("✓ ");
    }
    if event.flame {
        summary.push_str("🔥 ");
    }
    summary.push_str(&task.title);

    let mut description = Vec::new();
    let place = place(state, task);
    match (&task.reference, place) {
        (Some(r), Some(p)) => description.push(format!("{r} · {p}")),
        (Some(r), None) => description.push(r.clone()),
        (None, Some(p)) => description.push(p),
        (None, None) => {}
    }
    description.push(link.clone());

    push_line(out, "BEGIN:VEVENT");
    push_line(out, &format!("UID:{}@zunder", task.id));
    push_line(out, &format!("DTSTAMP:{}", utc(modified)));
    push_line(out, &format!("LAST-MODIFIED:{}", utc(modified)));
    push_line(out, &format!("DTSTART:{}", utc(event.start)));
    push_line(out, &format!("DTEND:{}", utc(end)));
    push_line(out, &format!("SUMMARY:{}", escape_text(&summary)));
    push_line(
        out,
        &format!("DESCRIPTION:{}", escape_text(&description.join("\n"))),
    );
    push_line(out, &format!("URL:{link}"));
    if !event.flame {
        push_line(out, "TRANSP:TRANSPARENT");
    }
    push_line(out, "END:VEVENT");
}

/// Where the task lives, for the event description: "Bereich › Projekt",
/// the area, or `None` for inbox and loose tasks.
fn place(state: &State, task: &Task) -> Option<String> {
    let location = state.effective_location(task.id)?;
    let view = LocationView::of(state, location);
    match (view.area_name, view.name) {
        (Some(area), Some(project)) => Some(format!("{area} › {project}")),
        (None, Some(name)) => Some(name),
        _ => None,
    }
}

/// A timestamp in UTC as iCalendar `DATE-TIME` (`20261012T140000Z`).
fn utc(t: OffsetDateTime) -> String {
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

/// Escapes a `TEXT` value (RFC 5545, 3.3.11). Line breaks become `\n`,
/// other control characters are dropped.
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Appends a content line with CRLF, folded after at most 75 octets
/// without splitting a character (RFC 5545, 3.1).
fn push_line(out: &mut String, line: &str) {
    let mut used = 0;
    for c in line.chars() {
        let len = c.len_utf8();
        if used + len > MAX_LINE_OCTETS {
            out.push_str("\r\n ");
            // The leading space counts towards the next line.
            used = 1;
        }
        out.push(c);
        used += len;
    }
    let _ = write!(out, "\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_text() {
        assert_eq!(escape_text("a,b;c\\d\ne\r\u{7}"), "a\\,b\\;c\\\\d\\ne");
    }

    #[test]
    fn folds_long_lines_on_character_boundaries() {
        let mut out = String::new();
        let line = format!("SUMMARY:{}", "ä".repeat(60));
        push_line(&mut out, &line);
        assert!(out.ends_with("\r\n"));
        let lines: Vec<&str> = out.trim_end_matches("\r\n").split("\r\n").collect();
        assert!(lines.len() > 1);
        for l in &lines {
            assert!(l.len() <= MAX_LINE_OCTETS, "{l:?} is too long");
        }
        let unfolded: String = lines
            .iter()
            .enumerate()
            .map(|(i, l)| if i == 0 { *l } else { &l[1..] })
            .collect();
        assert_eq!(unfolded, line);
    }

    #[test]
    fn formats_utc() {
        let t = OffsetDateTime::parse(
            "2026-10-12T16:30:05+02:00",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        assert_eq!(utc(t), "20261012T143005Z");
    }

    #[test]
    fn parses_scope() {
        assert_eq!(FeedScope::parse(None).unwrap(), FeedScope::Flames);
        assert_eq!(FeedScope::parse(Some("flames")).unwrap(), FeedScope::Flames);
        assert_eq!(FeedScope::parse(Some("all")).unwrap(), FeedScope::All);
        assert!(FeedScope::parse(Some("alle")).is_err());
    }
}
