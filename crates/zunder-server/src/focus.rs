//! "Erst für dich" in Heute (contract 5.1): which day the owner clicked the
//! section away, and which Glut task they picked for which day when none
//! was planned. Plain settings rows (`focus.*`), not synchronized
//! entities: one value per account, last write wins, read and written over
//! `/api/settings/focus` by browser sessions and the app's devices.
//!
//! The days are calendar days of the client that writes them (its local
//! time zone); the server stores them without interpreting them, so "today"
//! always means the viewer's today. A value of another day simply no longer
//! applies, nothing has to expire.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use zunder_core::ids::Id;
use zunder_core::model::format_date;
use zunder_core::Due;

use crate::error::{AppError, AppResult};

/// Key prefix of the focus settings.
pub const FOCUS_PREFIX: &str = "focus.";
/// Day the section was clicked away (`JJJJ-MM-TT`).
pub const FOCUS_DISMISSED_KEY: &str = "focus.dismissed_on";
/// Day a task was picked for.
pub const FOCUS_CHOSEN_ON_KEY: &str = "focus.chosen_on";
/// The picked task.
pub const FOCUS_CHOSEN_TASK_KEY: &str = "focus.chosen_task_id";

/// A task picked for one day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chosen {
    /// Calendar day of the client (`JJJJ-MM-TT`).
    pub on: String,
    pub task_id: String,
}

/// `GET /api/settings/focus`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FocusSettings {
    /// Day "Erst für dich" was clicked away, if ever.
    pub dismissed_on: Option<String>,
    /// The task picked for a day, if any.
    pub chosen: Option<Chosen>,
}

impl FocusSettings {
    /// Reads the settings from the rows of [`crate::db::settings`].
    pub fn from_rows(rows: &BTreeMap<String, String>) -> Self {
        let chosen = match (
            rows.get(FOCUS_CHOSEN_ON_KEY),
            rows.get(FOCUS_CHOSEN_TASK_KEY),
        ) {
            (Some(on), Some(task_id)) => Some(Chosen {
                on: on.clone(),
                task_id: task_id.clone(),
            }),
            _ => None,
        };
        Self {
            dismissed_on: rows.get(FOCUS_DISMISSED_KEY).cloned(),
            chosen,
        }
    }
}

/// `PUT /api/settings/focus`: a missing field stays, `null` removes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusUpdate {
    #[serde(default, deserialize_with = "present")]
    pub dismissed_on: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub chosen: Option<Option<Chosen>>,
}

/// A field that is present: `Some(None)` for `null`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A strict calendar day `JJJJ-MM-TT`, normalized.
fn day(value: &str, field: &str) -> AppResult<String> {
    match Due::parse(value.trim()) {
        Some(Due::Date(d)) => Ok(format_date(d)),
        _ => Err(AppError::bad_request(format!(
            "„{field}“ muss ein Tag JJJJ-MM-TT sein"
        ))),
    }
}

impl FocusUpdate {
    /// Checks days and task ID (German messages).
    pub fn validated(self) -> AppResult<Self> {
        let dismissed_on = match self.dismissed_on {
            Some(Some(d)) => Some(Some(day(&d, "dismissed_on")?)),
            other => other,
        };
        let chosen = match self.chosen {
            Some(Some(c)) => {
                let on = day(&c.on, "chosen.on")?;
                let task_id = Id::parse(c.task_id.trim()).ok_or_else(|| {
                    AppError::bad_request("„chosen.task_id“ muss eine Aufgaben-ID sein")
                })?;
                Some(Some(Chosen {
                    on,
                    task_id: task_id.to_string(),
                }))
            }
            other => other,
        };
        Ok(Self {
            dismissed_on,
            chosen,
        })
    }

    /// The rows to write (`Some`) or remove (`None`), by key.
    pub fn rows(&self) -> Vec<(&'static str, Option<String>)> {
        let mut out = Vec::new();
        if let Some(d) = &self.dismissed_on {
            out.push((FOCUS_DISMISSED_KEY, d.clone()));
        }
        if let Some(c) = &self.chosen {
            out.push((FOCUS_CHOSEN_ON_KEY, c.as_ref().map(|c| c.on.clone())));
            out.push((FOCUS_CHOSEN_TASK_KEY, c.as_ref().map(|c| c.task_id.clone())));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(json: &str) -> Result<FocusUpdate, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn missing_null_and_values_differ() {
        assert_eq!(update("{}").unwrap(), FocusUpdate::default());
        let u = update(r#"{"dismissed_on": null}"#).unwrap();
        assert_eq!(u.dismissed_on, Some(None));
        assert_eq!(u.chosen, None);
        assert_eq!(u.rows(), [(FOCUS_DISMISSED_KEY, None)]);
        let u = update(r#"{"dismissed_on": " 2026-10-17 "}"#)
            .unwrap()
            .validated()
            .unwrap();
        assert_eq!(u.rows(), [(FOCUS_DISMISSED_KEY, Some("2026-10-17".into()))]);
        let u =
            update(r#"{"chosen": {"on": "2026-10-17", "task_id": "t0000000000000000000000a"}}"#)
                .unwrap()
                .validated()
                .unwrap();
        assert_eq!(
            u.rows(),
            [
                (FOCUS_CHOSEN_ON_KEY, Some("2026-10-17".into())),
                (
                    FOCUS_CHOSEN_TASK_KEY,
                    Some("t0000000000000000000000a".into())
                ),
            ]
        );
    }

    #[test]
    fn invalid_values_are_refused() {
        for json in [
            r#"{"dismissed_on": "17.10.2026"}"#,
            r#"{"dismissed_on": "2026-02-30"}"#,
            r#"{"dismissed_on": "2026-10-17T08:00:00+02:00"}"#,
            r#"{"chosen": {"on": "2026-10-17", "task_id": "nope"}}"#,
        ] {
            assert!(update(json).unwrap().validated().is_err(), "{json}");
        }
        for json in [
            r#"{"dismissed": "2026-10-17"}"#,
            r#"{"dismissed_on": 20261017}"#,
            r#"{"chosen": {"on": "2026-10-17"}}"#,
            r#"{"chosen": {"on": "2026-10-17", "task_id": "t0000000000000000000000a", "x": 1}}"#,
        ] {
            assert!(update(json).is_err(), "{json}");
        }
    }

    #[test]
    fn settings_from_rows() {
        assert_eq!(
            FocusSettings::from_rows(&BTreeMap::new()),
            FocusSettings::default()
        );
        let mut rows = BTreeMap::new();
        rows.insert(FOCUS_DISMISSED_KEY.to_owned(), "2026-10-17".to_owned());
        rows.insert(FOCUS_CHOSEN_ON_KEY.to_owned(), "2026-10-16".to_owned());
        let s = FocusSettings::from_rows(&rows);
        assert_eq!(s.dismissed_on.as_deref(), Some("2026-10-17"));
        // Half a choice is none.
        assert_eq!(s.chosen, None);
        rows.insert(
            FOCUS_CHOSEN_TASK_KEY.to_owned(),
            "t0000000000000000000000a".to_owned(),
        );
        let json = serde_json::to_value(FocusSettings::from_rows(&rows)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "dismissed_on": "2026-10-17",
                "chosen": {"on": "2026-10-16", "task_id": "t0000000000000000000000a"},
            })
        );
    }
}
