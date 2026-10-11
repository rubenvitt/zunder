//! Typed views of ClickUp API v2 objects.
//!
//! Only the fields the importer uses are modelled; parsing is lenient
//! (IDs may be strings or numbers, `null` is accepted wherever ClickUp
//! sends it).

use serde::de::{DeserializeOwned, Deserializer};
use serde::Deserialize;
use serde_json::Value;

/// A workspace (called "team" in API v2).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Team {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Space {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Folder {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct List {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub archived: bool,
    /// Only present in `GET /list/{id}` (and some list responses).
    #[serde(default, deserialize_with = "null_default")]
    pub statuses: Vec<Status>,
}

/// A status as defined on a list or as carried by a task.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Status {
    #[serde(default, deserialize_with = "null_default")]
    pub status: String,
    /// `open`, `custom`, `done` or `closed`.
    #[serde(rename = "type", default, deserialize_with = "null_default")]
    pub kind: String,
}

impl Status {
    /// The lowercase status name, the key of the status mapping.
    pub fn key(&self) -> String {
        self.status.trim().to_lowercase()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IdRef {
    #[serde(deserialize_with = "id")]
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Tag {
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Task {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default)]
    pub text_content: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Only present when requested with `include_markdown_description=true`.
    #[serde(default)]
    pub markdown_description: Option<String>,
    pub status: Status,
    #[serde(default, deserialize_with = "opt_string")]
    pub orderindex: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub archived: bool,
    #[serde(default, deserialize_with = "opt_string")]
    pub parent: Option<String>,
    pub list: IdRef,
    #[serde(default)]
    pub url: Option<String>,
    /// Unix milliseconds.
    #[serde(default, deserialize_with = "opt_millis")]
    pub due_date: Option<i64>,
    /// Whether `due_date` carries a time of day. Not every response has it.
    #[serde(default)]
    pub due_date_time: Option<bool>,
    #[serde(default, deserialize_with = "null_default")]
    pub tags: Vec<Tag>,
    /// Unix milliseconds.
    #[serde(default, deserialize_with = "opt_millis")]
    pub date_created: Option<i64>,
    /// Unix milliseconds; set while the status has type `closed`.
    #[serde(default, deserialize_with = "opt_millis")]
    pub date_closed: Option<i64>,
    /// Unix milliseconds; set while the status has type `done`.
    #[serde(default, deserialize_with = "opt_millis")]
    pub date_done: Option<i64>,
    /// Unix milliseconds.
    #[serde(default, deserialize_with = "opt_millis")]
    pub date_updated: Option<i64>,
}

/// One item of a collection response together with its raw JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct Raw<T> {
    pub raw: Value,
    pub item: T,
}

/// Parses `body[key]` (an array) into typed items, keeping the raw JSON.
pub fn items<T: DeserializeOwned>(body: &Value, key: &str) -> Result<Vec<Raw<T>>, String> {
    let array = match body.get(key) {
        Some(Value::Array(a)) => a,
        Some(Value::Null) | None => return Ok(Vec::new()),
        Some(_) => return Err(format!("Feld {key:?} ist keine Liste")),
    };
    array.iter().map(|raw| item(raw.clone())).collect()
}

/// Parses a single object, keeping the raw JSON.
pub fn item<T: DeserializeOwned>(raw: Value) -> Result<Raw<T>, String> {
    let item = T::deserialize(&raw).map_err(|e| {
        let id = raw.get("id").map(|v| v.to_string()).unwrap_or_default();
        format!("Objekt {id} nicht lesbar: {e}")
    })?;
    Ok(Raw { raw, item })
}

fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

fn scalar_to_string(v: Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn id<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    scalar_to_string(Value::deserialize(d)?)
        .ok_or_else(|| serde::de::Error::custom("ID ist weder String noch Zahl"))
}

fn opt_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(scalar_to_string(Value::deserialize(d)?).filter(|s| !s.is_empty()))
}

fn opt_millis<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    match Value::deserialize(d)? {
        Value::Null => Ok(None),
        Value::Number(n) => Ok(n.as_i64()),
        Value::String(s) if s.trim().is_empty() => Ok(None),
        Value::String(s) => s
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| serde::de::Error::custom(format!("ungültiger Zeitstempel {s:?}"))),
        other => Err(serde::de::Error::custom(format!(
            "ungültiger Zeitstempel {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn task_parses_leniently() {
        let raw = json!({
            "id": "86aa001",
            "name": "Beispiel",
            "description": null,
            "status": {"status": "In Progress", "type": "custom"},
            "orderindex": 12.5,
            "parent": null,
            "list": {"id": 9001},
            "due_date": "1760000000000",
            "tags": null,
            "date_created": "1750000000000",
            "date_closed": null
        });
        let task: Task = item(raw).unwrap().item;
        assert_eq!(task.list.id, "9001");
        assert_eq!(task.orderindex.as_deref(), Some("12.5"));
        assert_eq!(task.due_date, Some(1_760_000_000_000));
        assert_eq!(task.due_date_time, None);
        assert_eq!(task.status.key(), "in progress");
        assert!(task.tags.is_empty());
        assert!(task.parent.is_none());
        assert_eq!(task.date_created, Some(1_750_000_000_000));
        assert_eq!(task.date_closed, None);
        assert_eq!(task.date_done, None);
    }

    #[test]
    fn items_handles_missing_key() {
        let parsed: Vec<Raw<Space>> = items(&json!({}), "spaces").unwrap();
        assert!(parsed.is_empty());
        assert!(items::<Space>(&json!({"spaces": 3}), "spaces").is_err());
    }
}
