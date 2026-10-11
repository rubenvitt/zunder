//! Typed access to tool arguments with German error messages.
//!
//! Field values that the core validates (dates, locations, links …) are
//! passed through as raw JSON, so the core's messages reach the caller.

use std::str::FromStr;

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::views::TaskRef;

fn invalid(message: impl Into<String>) -> AppError {
    AppError::BadRequest {
        code: "invalid_arguments",
        message: message.into(),
    }
}

/// The arguments of one tool call (or one nested object).
#[derive(Debug, Clone, Default)]
pub struct Args(Map<String, Value>);

impl Args {
    pub fn new(map: Map<String, Value>) -> Self {
        Self(map)
    }

    /// Rejects unknown keys and missing required keys.
    pub fn check<'a>(
        &self,
        allowed: impl IntoIterator<Item = &'a str>,
        required: impl IntoIterator<Item = &'a str>,
    ) -> AppResult<()> {
        let allowed: Vec<&str> = allowed.into_iter().collect();
        if let Some(unknown) = self.0.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(invalid(format!(
                "Unbekannter Parameter „{unknown}“. Erlaubt: {}.",
                allowed.join(", ")
            )));
        }
        for key in required {
            if !self.0.contains_key(key) {
                return Err(invalid(format!("Parameter „{key}“ fehlt.")));
            }
        }
        Ok(())
    }

    /// The raw value, `Some(Null)` if explicitly `null`.
    pub fn raw(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    /// Present and not `null`.
    pub fn value(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    pub fn str(&self, key: &str) -> AppResult<Option<&str>> {
        match self.value(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(invalid(format!("„{key}“ muss ein Text sein."))),
        }
    }

    /// A non-empty (after trimming) text.
    pub fn non_empty_str(&self, key: &str) -> AppResult<Option<&str>> {
        match self.str(key)? {
            Some(s) if s.trim().is_empty() => {
                Err(invalid(format!("„{key}“ darf nicht leer sein.")))
            }
            other => Ok(other),
        }
    }

    pub fn bool(&self, key: &str) -> AppResult<Option<bool>> {
        match self.value(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(invalid(format!("„{key}“ muss true oder false sein."))),
        }
    }

    /// An integer in `min..=max`, `default` when absent.
    pub fn int(&self, key: &str, min: u64, max: u64, default: u64) -> AppResult<u64> {
        match self.value(key) {
            None => Ok(default),
            Some(v) => match v.as_u64() {
                Some(n) if (min..=max).contains(&n) => Ok(n),
                _ => Err(invalid(format!(
                    "„{key}“ muss eine ganze Zahl von {min} bis {max} sein."
                ))),
            },
        }
    }

    /// A canonical UUID.
    pub fn id(&self, key: &str) -> AppResult<Option<Uuid>> {
        match self.str(key)? {
            None => Ok(None),
            Some(s) => zunder_core::ids::parse_canonical(s)
                .map(Some)
                .ok_or_else(|| {
                    invalid(format!(
                        "„{key}“ muss eine UUID (klein, mit Bindestrichen) sein."
                    ))
                }),
        }
    }

    pub fn required_id(&self, key: &str) -> AppResult<Uuid> {
        self.id(key)?
            .ok_or_else(|| invalid(format!("Parameter „{key}“ fehlt.")))
    }

    /// An enum code (e.g. a status) parsed with `FromStr`.
    pub fn code<T: FromStr>(&self, key: &str, allowed: &[&str]) -> AppResult<Option<T>> {
        match self.str(key)? {
            None => Ok(None),
            Some(s) => s.parse().map(Some).map_err(|_| {
                invalid(format!(
                    "Ungültiger Wert „{s}“ für „{key}“. Erlaubt: {}.",
                    allowed.join(", ")
                ))
            }),
        }
    }

    /// A list of enum codes.
    pub fn codes<T: FromStr>(&self, key: &str, allowed: &[&str]) -> AppResult<Option<Vec<T>>> {
        let Some(value) = self.value(key) else {
            return Ok(None);
        };
        let items = value
            .as_array()
            .ok_or_else(|| invalid(format!("„{key}“ muss eine Liste sein.")))?;
        items
            .iter()
            .map(|item| {
                let s = item
                    .as_str()
                    .ok_or_else(|| invalid(format!("„{key}“ darf nur Texte enthalten.")))?;
                s.parse().map_err(|_| {
                    invalid(format!(
                        "Ungültiger Wert „{s}“ für „{key}“. Erlaubt: {}.",
                        allowed.join(", ")
                    ))
                })
            })
            .collect::<AppResult<Vec<T>>>()
            .map(Some)
    }

    /// A list of objects (e.g. the subtasks of decompose_task).
    pub fn objects(&self, key: &str) -> AppResult<Vec<Args>> {
        let Some(value) = self.value(key) else {
            return Ok(Vec::new());
        };
        let items = value
            .as_array()
            .ok_or_else(|| invalid(format!("„{key}“ muss eine Liste sein.")))?;
        items
            .iter()
            .enumerate()
            .map(|(i, item)| match item {
                Value::Object(map) => Ok(Args(map.clone())),
                _ => Err(invalid(format!(
                    "Eintrag {} von „{key}“ muss ein Objekt sein.",
                    i + 1
                ))),
            })
            .collect()
    }

    /// `task_id` or `clickup_id`.
    pub fn task_ref(&self) -> AppResult<TaskRef> {
        let clickup = self.non_empty_str("clickup_id")?.map(str::to_owned);
        TaskRef::from_args(self.id("task_id")?, clickup)
    }
}
