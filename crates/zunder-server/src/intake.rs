//! Intake channels: a write-only way for a foreign server to create tasks in
//! exactly one project (contract 3.3).
//!
//! A channel has its own token (`zi_…`, stored as SHA-256 hex) in its own
//! table. `/api` and `/mcp` only look at API tokens, so a channel token never
//! reads or changes anything there; `/intake` in turn only looks at channels
//! and never accepts an API token. The target project belongs to the channel,
//! never to the request. A submission can only create an open task there;
//! repeating its `external_id` returns the task created first.

use std::str::FromStr;

use rusqlite::Connection;
use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;
use zunder_core::ids::{self, Id};
use zunder_core::model::{fields, Location, ProjectStatus, TaskStatus, TaskType};
use zunder_core::State;

use crate::app::{format_ms, now_ms, App, Source};
use crate::auth;
use crate::db::{self, IntakeChannelRow};
use crate::error::{AppError, AppResult};

/// Maximum length of a channel name.
pub const MAX_CHANNEL_NAME_CHARS: usize = 64;
/// Maximum length of `external_id`.
pub const MAX_EXTERNAL_ID_CHARS: usize = 128;
/// Maximum length of the trimmed `title`.
pub const MAX_TITLE_CHARS: usize = 200;
/// Maximum length of `description`.
pub const MAX_DESCRIPTION_CHARS: usize = 20_000;
/// Maximum number of `links`.
pub const MAX_LINKS: usize = 5;
/// At most this many tasks per channel within [`RATE_WINDOW_MS`] (sliding).
pub const MAX_SUBMISSIONS_PER_WINDOW: i64 = 30;
/// The window of the rate limit: one hour.
pub const RATE_WINDOW_MS: i64 = 60 * 60 * 1000;

/// The fields a submission may carry.
const SUBMISSION_FIELDS: &[&str] = &["external_id", "title", "description", "type", "links"];

/// An authenticated intake channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntakeChannel {
    pub id: Id,
    pub name: String,
    pub project_id: Id,
}

/// A channel without its token (`GET /api/intake/channels`).
#[derive(Debug, Clone, Serialize)]
pub struct ChannelInfo {
    pub id: Id,
    pub name: String,
    pub project_id: Id,
    /// Name of the target project (also when it was deleted since), `null`
    /// when the project is unknown.
    pub project_name: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// A newly created channel; `token` is shown exactly once.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedChannel {
    #[serde(flatten)]
    pub info: ChannelInfo,
    pub token: String,
}

/// The outcome of a submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Submitted {
    pub id: Id,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// `false` when the `external_id` was known (nothing written).
    #[serde(skip)]
    pub created: bool,
}

/// A checked submission (`POST /intake/v1/tasks`).
#[derive(Debug, Clone, PartialEq)]
pub struct Submission {
    pub external_id: String,
    pub title: String,
    pub description: Option<String>,
    pub task_type: TaskType,
    pub links: Option<Value>,
}

impl Submission {
    /// Checks the request body. Every problem is a `400 invalid_request`
    /// with a German message; content rules beyond these (e.g. `bug` only in
    /// dev projects, the form of a link) are left to the core.
    pub fn parse(body: &Value) -> AppResult<Submission> {
        let Some(map) = body.as_object() else {
            return Err(AppError::bad_request("Der Body muss ein JSON-Objekt sein"));
        };
        if let Some(unknown) = map
            .keys()
            .find(|k| !SUBMISSION_FIELDS.contains(&k.as_str()))
        {
            return Err(AppError::bad_request(format!(
                "Unbekanntes Feld „{unknown}“"
            )));
        }

        let external_id = text(map, "external_id")?
            .ok_or_else(|| AppError::bad_request("„external_id“ fehlt"))?;
        let len = external_id.chars().count();
        if len == 0
            || len > MAX_EXTERNAL_ID_CHARS
            || !external_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(AppError::bad_request(format!(
                "„external_id“ muss 1 bis {MAX_EXTERNAL_ID_CHARS} Zeichen aus A–Z, a–z, 0–9, - und _ haben"
            )));
        }

        let title = text(map, "title")?
            .ok_or_else(|| AppError::bad_request("„title“ fehlt"))?
            .trim();
        let len = title.chars().count();
        if len == 0 || len > MAX_TITLE_CHARS {
            return Err(AppError::bad_request(format!(
                "„title“ muss ohne Leerraum am Rand 1 bis {MAX_TITLE_CHARS} Zeichen lang sein"
            )));
        }

        let description = text(map, "description")?.filter(|d| !d.trim().is_empty());
        if description.is_some_and(|d| d.chars().count() > MAX_DESCRIPTION_CHARS) {
            return Err(AppError::bad_request(format!(
                "„description“ darf höchstens {MAX_DESCRIPTION_CHARS} Zeichen lang sein"
            )));
        }

        let task_type = match text(map, "type")? {
            None => TaskType::Aufgabe,
            Some(code) => TaskType::from_str(code).map_err(|_| {
                AppError::bad_request("„type“ muss „aufgabe“, „bug“ oder „feature“ sein")
            })?,
        };

        let links = match map.get("links") {
            None | Some(Value::Null) => None,
            Some(Value::Array(links)) => {
                check_links(links)?;
                (!links.is_empty()).then(|| Value::Array(links.clone()))
            }
            Some(_) => return Err(AppError::bad_request("„links“ muss eine Liste sein")),
        };

        Ok(Submission {
            external_id: external_id.to_owned(),
            title: title.to_owned(),
            description: description.map(str::to_owned),
            task_type,
            links,
        })
    }
}

/// An optional text field (`null` counts as absent).
fn text<'a>(map: &'a Map<String, Value>, key: &str) -> AppResult<Option<&'a str>> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(AppError::bad_request(format!("„{key}“ muss ein Text sein"))),
    }
}

/// At most [`MAX_LINKS`] links, each URL with `http` or `https`. The shape of
/// each entry is checked by the core.
fn check_links(links: &[Value]) -> AppResult<()> {
    if links.len() > MAX_LINKS {
        return Err(AppError::bad_request(format!(
            "Höchstens {MAX_LINKS} Links erlaubt"
        )));
    }
    for link in links {
        let Some(raw) = link.get("url").and_then(Value::as_str) else {
            continue;
        };
        let web = url::Url::parse(raw).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
        if !web {
            return Err(AppError::bad_request(
                "Links müssen http- oder https-Adressen sein",
            ));
        }
    }
    Ok(())
}

/// Checks a channel name: trimmed, 1–64 characters, no control characters.
fn channel_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    let len = name.chars().count();
    if len == 0 || len > MAX_CHANNEL_NAME_CHARS {
        return Err(AppError::bad_request(format!(
            "Der Name eines Eingangs muss 1 bis {MAX_CHANNEL_NAME_CHARS} Zeichen lang sein"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(AppError::bad_request(
            "Der Name eines Eingangs darf keine Steuerzeichen enthalten",
        ));
    }
    Ok(name.to_owned())
}

/// `409` unless the project lives and is not completed.
fn require_open_project(state: &State, project_id: Id) -> AppResult<()> {
    match state.project(project_id) {
        Some(p) if !p.deleted && p.status != ProjectStatus::Completed => Ok(()),
        Some(p) if !p.deleted => Err(AppError::Conflict(format!(
            "Das Zielprojekt „{}“ ist abgeschlossen",
            p.name
        ))),
        _ => Err(AppError::Conflict(
            "Das Zielprojekt gibt es nicht (mehr)".into(),
        )),
    }
}

fn task_ref(state: &State, task_id: Id) -> Option<String> {
    state.task(task_id).and_then(|t| t.reference)
}

fn channel_info(state: &State, row: &IntakeChannelRow) -> ChannelInfo {
    ChannelInfo {
        id: row.id,
        name: row.name.clone(),
        project_id: row.project_id,
        project_name: state.project(row.project_id).map(|p| p.name),
        created_at: format_ms(row.created_at),
        last_used_at: row.last_used_at.map(format_ms),
    }
}

/// A fresh channel token (`zi_…`).
pub fn new_intake_token() -> String {
    format!("{}{}", auth::INTAKE_TOKEN_PREFIX, auth::random_secret())
}

impl App {
    /// Authenticates a channel token. Anything that is not an active
    /// channel's `zi_…` token (API tokens included) is `None`.
    pub async fn authenticate_intake(&self, raw: &str) -> AppResult<Option<IntakeChannel>> {
        if !raw.starts_with(auth::INTAKE_TOKEN_PREFIX) {
            return Ok(None);
        }
        let hash = auth::sha256_hex(raw);
        let now = now_ms();
        let found = self
            .with_reader(move |conn| Ok(db::find_intake_channel_by_hash(conn, &hash)?))
            .await?;
        let Some(row) = found else {
            return Ok(None);
        };
        if row
            .last_used_at
            .is_none_or(|t| now - t >= auth::TOKEN_TOUCH_INTERVAL_MS)
        {
            let id = row.id;
            self.with_writer(move |conn| Ok(db::touch_intake_channel(conn, id, now)?))
                .await?;
        }
        Ok(Some(IntakeChannel {
            id: row.id,
            name: row.name,
            project_id: row.project_id,
        }))
    }

    /// Active channels of the user, oldest first.
    pub async fn list_intake_channels(&self, user_id: Uuid) -> AppResult<Vec<ChannelInfo>> {
        let rows = self
            .with_reader(move |conn| Ok(db::list_intake_channels(conn, user_id)?))
            .await?;
        Ok(self.read(|state| rows.iter().map(|r| channel_info(state, r)).collect()))
    }

    /// Creates a channel into a living, not completed project (`409`
    /// otherwise). Names are unique among the user's active channels.
    pub async fn create_intake_channel(
        &self,
        user_id: Uuid,
        name: &str,
        project_id: &str,
    ) -> AppResult<CreatedChannel> {
        let name = channel_name(name)?;
        let project_id = Id::parse_any(project_id)
            .ok_or_else(|| AppError::bad_request(format!("Ungültige Projekt-ID: {project_id}")))?;
        let raw = new_intake_token();
        let hash = auth::sha256_hex(&raw);
        self.locked(move |locked| {
            locked.read(|state| require_open_project(state, project_id))?;
            let conn: &Connection = locked.conn();
            if db::list_intake_channels(conn, user_id)?
                .iter()
                .any(|c| c.name == name)
            {
                return Err(AppError::Conflict(format!(
                    "Es gibt bereits einen Eingang mit dem Namen „{name}“"
                )));
            }
            let row = IntakeChannelRow {
                id: ids::new_id(),
                user_id,
                name,
                project_id,
                created_at: now_ms(),
                last_used_at: None,
            };
            db::insert_intake_channel(conn, &row, &hash)?;
            Ok(CreatedChannel {
                info: locked.read(|state| channel_info(state, &row)),
                token: raw,
            })
        })
        .await
    }

    /// Revokes a channel of the user; `NotFound` if there is no such active
    /// channel.
    pub async fn revoke_intake_channel(&self, user_id: Uuid, id: Id) -> AppResult<()> {
        self.with_writer(move |conn| {
            if db::revoke_intake_channel(conn, user_id, id, now_ms())? {
                Ok(())
            } else {
                Err(AppError::NotFound("Eingang nicht gefunden".into()))
            }
        })
        .await
    }

    /// Creates the task of a submission, or returns the one an earlier
    /// submission with the same `external_id` created. Everything runs under
    /// the write lock: the lookup, the rate limit, the project check, the
    /// push (one batch with the server clock through the core validation)
    /// and the submission row in the push's transaction.
    pub async fn submit_intake(
        &self,
        channel: IntakeChannel,
        submission: Submission,
    ) -> AppResult<Submitted> {
        self.locked(move |locked| {
            // Revoked since authentication: the request no longer counts.
            if !db::intake_channel_active(locked.conn(), channel.id)? {
                return Err(AppError::Unauthorized);
            }
            if let Some(task_id) =
                db::intake_submission(locked.conn(), channel.id, &submission.external_id)?
            {
                return Ok(Submitted {
                    id: task_id,
                    reference: locked.read(|state| task_ref(state, task_id)),
                    created: false,
                });
            }
            let now = now_ms();
            let (recent, oldest) =
                db::intake_submissions_since(locked.conn(), channel.id, now - RATE_WINDOW_MS)?;
            if recent >= MAX_SUBMISSIONS_PER_WINDOW {
                let free_at = oldest.unwrap_or(now) + RATE_WINDOW_MS;
                // Whole seconds until the oldest one leaves the window.
                let retry_after_secs = u64::try_from((free_at - now).max(1).div_euclid(1000) + 1)
                    .unwrap_or(1);
                return Err(AppError::TooManyRequests {
                    message: format!(
                        "Mehr als {MAX_SUBMISSIONS_PER_WINDOW} Aufgaben in der letzten Stunde über diesen Eingang"
                    ),
                    retry_after_secs,
                });
            }
            locked.read(|state| require_open_project(state, channel.project_id))?;

            let task_id = ids::new_id();
            let Submission {
                external_id,
                title,
                description,
                task_type,
                links,
            } = submission;
            let record = |conn: &Connection| {
                db::insert_intake_submission(conn, channel.id, &external_id, task_id, now)
            };
            let location = Location::Project {
                id: channel.project_id,
            };
            locked
                .commit_atomic(
                    &Source::Intake(channel.name.clone()),
                    |_, batch| {
                        use fields::task as f;
                        batch.set_task(task_id, f::TITLE, title);
                        batch.set_task(task_id, f::TYPE, task_type);
                        batch.set_task(task_id, f::STATUS, TaskStatus::Open);
                        batch.set_task(task_id, f::LOCATION, location);
                        if let Some(description) = description {
                            batch.set_task(task_id, f::DESCRIPTION, description);
                        }
                        if let Some(links) = links {
                            batch.set_task(task_id, f::LINKS, links);
                        }
                        Ok(())
                    },
                    Some(&record),
                )
                // A rejection of the core is a bad request of the sender,
                // whatever the core's code.
                .map_err(|e| match e {
                    AppError::BadRequest { message, .. } => AppError::bad_request(message),
                    other => other,
                })?;
            Ok(Submitted {
                id: task_id,
                reference: locked.read(|state| task_ref(state, task_id)),
                created: true,
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(body: Value) -> AppResult<Submission> {
        Submission::parse(&body)
    }

    #[test]
    fn token_format() {
        let t = new_intake_token();
        assert!(t.starts_with("zi_"));
        assert_eq!(t.len(), 3 + 43);
    }

    #[test]
    fn parses_a_full_submission() {
        let s = parse(json!({
            "external_id": "rm-42_a",
            "title": "  Knopf klemmt  ",
            "description": "**fett**",
            "type": "bug",
            "links": [{"url": "https://example.org/a", "title": "A"}],
        }))
        .unwrap();
        assert_eq!(s.external_id, "rm-42_a");
        assert_eq!(s.title, "Knopf klemmt");
        assert_eq!(s.description.as_deref(), Some("**fett**"));
        assert_eq!(s.task_type, TaskType::Bug);
        assert!(s.links.is_some());

        let s =
            parse(json!({"external_id": "x", "title": "t", "type": null, "links": []})).unwrap();
        assert_eq!(s.task_type, TaskType::Aufgabe);
        assert_eq!(s.links, None);
        assert_eq!(s.description, None);
    }

    #[test]
    fn rejects_bad_fields() {
        let long = |n: usize| "a".repeat(n);
        let bad = [
            json!([]),
            json!({"external_id": "x", "title": "t", "status": "done"}),
            json!({"title": "t"}),
            json!({"external_id": "", "title": "t"}),
            json!({"external_id": "a b", "title": "t"}),
            json!({"external_id": "ä", "title": "t"}),
            json!({"external_id": long(129), "title": "t"}),
            json!({"external_id": "x"}),
            json!({"external_id": "x", "title": "   "}),
            json!({"external_id": "x", "title": long(201)}),
            json!({"external_id": "x", "title": 3}),
            json!({"external_id": "x", "title": "t", "description": long(20_001)}),
            json!({"external_id": "x", "title": "t", "type": "epic"}),
            json!({"external_id": "x", "title": "t", "links": {}}),
            json!({"external_id": "x", "title": "t", "links": [{"url": "ftp://example.org"}]}),
            json!({"external_id": "x", "title": "t", "links": [{"url": "javascript:alert(1)"}]}),
            json!({"external_id": "x", "title": "t", "links": vec![json!({"url": "https://a.example"}); 6]}),
        ];
        for body in bad {
            let e = parse(body.clone()).expect_err(&body.to_string());
            assert_eq!(e.code(), "invalid_request", "{body}");
        }
        assert!(parse(json!({"external_id": long(128), "title": long(200)})).is_ok());
        assert!(
            parse(json!({"external_id": "x", "title": "t", "description": long(20_000)})).is_ok()
        );
    }
}
