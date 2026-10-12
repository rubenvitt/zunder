//! Field operations, the unit of change and sync.

use std::fmt;
use std::str::FromStr;

use crate::ids::Id;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hlc::Hlc;

/// The kind of entity an operation targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityKind {
    Area,
    Project,
    Task,
}

impl EntityKind {
    pub const ALL: [EntityKind; 3] = [EntityKind::Area, EntityKind::Project, EntityKind::Task];

    pub fn as_str(self) -> &'static str {
        match self {
            EntityKind::Area => "area",
            EntityKind::Project => "project",
            EntityKind::Task => "task",
        }
    }

    /// German singular noun for user-facing messages.
    pub fn label_de(self) -> &'static str {
        match self {
            EntityKind::Area => "Bereich",
            EntityKind::Project => "Projekt",
            EntityKind::Task => "Aufgabe",
        }
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error for an unknown entity kind string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown entity kind {0:?}")]
pub struct UnknownEntityKind(pub String);

impl FromStr for EntityKind {
    type Err = UnknownEntityKind;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "area" => Ok(EntityKind::Area),
            "project" => Ok(EntityKind::Project),
            "task" => Ok(EntityKind::Task),
            other => Err(UnknownEntityKind(other.to_owned())),
        }
    }
}

/// A single field assignment: the unit of change and of sync.
///
/// JSON form (contract section 2.3):
///
/// ```json
/// {"op_id":"…","entity":"task","entity_id":"…","field":"title","value":"…","hlc":"…"}
/// ```
///
/// IDs are written in canonical form ([`Id`]); legacy UUIDs are accepted when
/// reading and converted, also inside ID-valued fields (see
/// [`upgrade_legacy_value`]). `value` is required (`null` resets an optional
/// field).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "WireOp")]
pub struct Op {
    pub op_id: Id,
    pub entity: EntityKind,
    pub entity_id: Id,
    pub field: String,
    #[serde(deserialize_with = "present_value")]
    pub value: Value,
    pub hlc: Hlc,
}

/// The JSON form as read, before legacy IDs in values are converted.
#[derive(Deserialize)]
struct WireOp {
    op_id: Id,
    entity: EntityKind,
    entity_id: Id,
    field: String,
    #[serde(deserialize_with = "present_value")]
    value: Value,
    hlc: Hlc,
}

impl From<WireOp> for Op {
    fn from(w: WireOp) -> Op {
        let mut value = w.value;
        upgrade_legacy_value(w.entity, &w.field, &mut value);
        Op {
            op_id: w.op_id,
            entity: w.entity,
            entity_id: w.entity_id,
            field: w.field,
            value,
            hlc: w.hlc,
        }
    }
}

/// Converts legacy UUIDs in the ID-valued fields (`area_id`, `parent_id`,
/// the `id` of a `location`) to their IDs ([`crate::ids::Id::from_legacy`]).
pub fn upgrade_legacy_value(entity: EntityKind, field: &str, value: &mut Value) {
    use crate::model::fields::{project, task};
    fn upgrade(v: &mut Value) {
        if let Some(id) = v.as_str().and_then(crate::ids::upgrade_legacy_str) {
            *v = Value::String(id);
        }
    }
    match (entity, field) {
        (EntityKind::Project, project::AREA_ID) | (EntityKind::Task, task::PARENT_ID) => {
            upgrade(value)
        }
        (EntityKind::Task, task::LOCATION) => {
            if let Some(id) = value.get_mut("id") {
                upgrade(id);
            }
        }
        _ => {}
    }
}

/// Like `Value::deserialize`, but used via `deserialize_with` (without
/// `#[serde(default)]`), so a missing `value` key is an error while an
/// explicit `null` is accepted.
pub(crate) fn present_value<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
    Value::deserialize(d)
}

impl Op {
    /// The key used in test vectors and snapshots: `"<entity>/<entity_id>"`.
    pub fn entity_key(&self) -> String {
        format!("{}/{}", self.entity, self.entity_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const OP_ID: &str = "o0000000000000000000000a";
    const ENTITY_ID: &str = "tz4a98xxat96iws9zmbrgj3a";

    #[test]
    fn json_matches_contract() {
        let raw = json!({
            "op_id": OP_ID,
            "entity": "task",
            "entity_id": ENTITY_ID,
            "field": "title",
            "value": "Zahnarzt anrufen",
            "hlc": "1728650000000:00000:web-3f9a"
        });
        let op: Op = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(op.entity, EntityKind::Task);
        assert_eq!(op.field, "title");
        assert_eq!(op.value, json!("Zahnarzt anrufen"));
        assert_eq!(serde_json::to_value(&op).unwrap(), raw);
        assert_eq!(op.entity_key(), format!("task/{ENTITY_ID}"));
    }

    #[test]
    fn null_value_is_kept_but_missing_value_is_rejected() {
        let with_null = json!({
            "op_id": OP_ID, "entity": "task", "entity_id": ENTITY_ID,
            "field": "due", "value": null, "hlc": "1728650000000:00000:a"
        });
        let op: Op = serde_json::from_value(with_null.clone()).unwrap();
        assert_eq!(op.value, Value::Null);
        assert_eq!(serde_json::to_value(&op).unwrap(), with_null);

        let missing = json!({
            "op_id": OP_ID, "entity": "task", "entity_id": ENTITY_ID,
            "field": "due", "hlc": "1728650000000:00000:a"
        });
        assert!(serde_json::from_value::<Op>(missing).is_err());
    }

    #[test]
    fn rejects_bad_entity_and_ids() {
        let bad_entity = json!({
            "op_id": OP_ID, "entity": "Task", "entity_id": ENTITY_ID,
            "field": "title", "value": "x", "hlc": "1728650000000:00000:a"
        });
        assert!(serde_json::from_value::<Op>(bad_entity).is_err());
        let bad_id = json!({
            "op_id": OP_ID.to_uppercase(), "entity": "task", "entity_id": ENTITY_ID,
            "field": "title", "value": "x", "hlc": "1728650000000:00000:a"
        });
        assert!(serde_json::from_value::<Op>(bad_id).is_err());
    }

    #[test]
    fn entity_kind_parse() {
        for kind in EntityKind::ALL {
            assert_eq!(kind.as_str().parse::<EntityKind>().unwrap(), kind);
        }
        assert!("tasks".parse::<EntityKind>().is_err());
    }

    #[test]
    fn legacy_uuids_are_converted_when_reading() {
        let legacy = "01929c3e-0000-7000-8000-0000000000aa";
        let parent = "01929c3e-0000-7000-8000-0000000000bb";
        let id = |s: &str| crate::ids::Id::parse_any(s).unwrap().to_string();
        let op: Op = serde_json::from_value(json!({
            "op_id": "01929c3e-0000-7000-8000-000000000001", "entity": "task",
            "entity_id": legacy, "field": "parent_id", "value": parent,
            "hlc": "1728650000000:00000:a"
        }))
        .unwrap();
        assert_eq!(op.entity_id.to_string(), id(legacy));
        assert_eq!(op.value, json!(id(parent)));
        let op: Op = serde_json::from_value(json!({
            "op_id": OP_ID, "entity": "task", "entity_id": ENTITY_ID,
            "field": "location", "value": {"kind": "project", "id": legacy},
            "hlc": "1728650000000:00000:a"
        }))
        .unwrap();
        assert_eq!(op.value, json!({"kind": "project", "id": id(legacy)}));
        // Other text fields stay as they are.
        let op: Op = serde_json::from_value(json!({
            "op_id": OP_ID, "entity": "task", "entity_id": ENTITY_ID,
            "field": "title", "value": legacy, "hlc": "1728650000000:00000:a"
        }))
        .unwrap();
        assert_eq!(op.value, json!(legacy));
    }
}
