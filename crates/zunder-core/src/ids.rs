//! Identifier helpers.
//!
//! All identifiers travel as lowercase, hyphenated UUID strings (36 chars).
//! Clients generate UUIDv7 themselves; the importer derives deterministic
//! UUIDv5 values from ClickUp IDs so that repeated runs update instead of
//! duplicating.

use uuid::Uuid;

use crate::op::EntityKind;

/// Fixed namespace for all deterministic (UUIDv5) Zunder identifiers.
///
/// Never change this value: import IDs derived from ClickUp IDs depend on it.
pub const ZUNDER_NAMESPACE: Uuid = uuid::uuid!("f9d16046-2e7f-4f97-b1be-e789d21e4fec");

/// Generates a new time-ordered identifier (UUIDv7).
pub fn new_id() -> Uuid {
    Uuid::now_v7()
}

/// Derives a deterministic identifier (UUIDv5 in [`ZUNDER_NAMESPACE`]) from an
/// arbitrary name.
pub fn deterministic_id(name: &str) -> Uuid {
    Uuid::new_v5(&ZUNDER_NAMESPACE, name.as_bytes())
}

/// Derives the deterministic Zunder ID for an entity imported from ClickUp.
///
/// The name hashed is `clickup:<kind>:<clickup_id>`, so the same ClickUp ID
/// yields different IDs for different entity kinds (e.g. a ClickUp list
/// imported as a project and a task that happens to share the ID string).
pub fn clickup_entity_id(kind: EntityKind, clickup_id: &str) -> Uuid {
    deterministic_id(&format!("clickup:{}:{}", kind.as_str(), clickup_id))
}

/// Parses a UUID only if it is in canonical wire form (lowercase, hyphenated).
pub fn parse_canonical(s: &str) -> Option<Uuid> {
    if s.len() != 36 {
        return None;
    }
    let id = Uuid::try_parse(s).ok()?;
    let mut buf = Uuid::encode_buffer();
    (id.hyphenated().encode_lower(&mut buf) == s).then_some(id)
}

/// Serde adapter that writes canonical UUID strings and only accepts
/// canonical UUID strings when reading.
pub mod canonical {
    use serde::{de, Deserialize, Deserializer, Serializer};
    use uuid::Uuid;

    pub fn serialize<S: Serializer>(id: &Uuid, serializer: S) -> Result<S::Ok, S::Error> {
        let mut buf = Uuid::encode_buffer();
        serializer.serialize_str(id.hyphenated().encode_lower(&mut buf))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Uuid, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        super::parse_canonical(&s).ok_or_else(|| {
            de::Error::custom(format!(
                "expected lowercase hyphenated UUID, got {:?}",
                s.as_ref()
            ))
        })
    }

    /// Same as the parent module, for `Option<Uuid>` (`null` = `None`).
    pub mod option {
        use serde::{Deserialize, Deserializer, Serializer};
        use uuid::Uuid;

        pub fn serialize<S: Serializer>(
            id: &Option<Uuid>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            match id {
                Some(id) => super::serialize(id, serializer),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<Uuid>, D::Error> {
            #[derive(Deserialize)]
            struct Wrapper(#[serde(with = "super")] Uuid);
            Ok(Option::<Wrapper>::deserialize(deserializer)?.map(|w| w.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_id_is_v7_and_canonical() {
        let id = new_id();
        assert_eq!(id.get_version_num(), 7);
        assert_eq!(parse_canonical(&id.to_string()), Some(id));
    }

    #[test]
    fn deterministic_ids_are_stable() {
        let a = clickup_entity_id(EntityKind::Task, "86abc123");
        let b = clickup_entity_id(EntityKind::Task, "86abc123");
        assert_eq!(a, b);
        assert_eq!(a.get_version_num(), 5);
        assert_ne!(a, clickup_entity_id(EntityKind::Project, "86abc123"));
        assert_ne!(a, clickup_entity_id(EntityKind::Task, "86abc124"));
        // Frozen value: guards against accidental namespace or format changes.
        assert_eq!(
            deterministic_id("clickup:task:86abc123"),
            Uuid::new_v5(&ZUNDER_NAMESPACE, b"clickup:task:86abc123")
        );
    }

    #[test]
    fn parse_canonical_rejects_other_forms() {
        let s = "01929c3e-5b7a-7cc1-9a51-3f0e4f6b2a10";
        assert!(parse_canonical(s).is_some());
        assert!(parse_canonical(&s.to_uppercase()).is_none());
        assert!(parse_canonical(&s.replace('-', "")).is_none());
        assert!(parse_canonical(&format!("{{{s}}}")).is_none());
        assert!(parse_canonical("not-a-uuid").is_none());
        assert!(parse_canonical("").is_none());
    }

    #[test]
    fn serde_adapter_is_strict() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        struct W {
            #[serde(with = "canonical")]
            id: Uuid,
            #[serde(with = "canonical::option", default)]
            opt: Option<Uuid>,
        }
        let s = "01929c3e-5b7a-7cc1-9a51-3f0e4f6b2a10";
        let w: W = serde_json::from_str(&format!(r#"{{"id":"{s}","opt":null}}"#)).unwrap();
        assert_eq!(w.opt, None);
        assert_eq!(
            serde_json::to_string(&w).unwrap(),
            format!(r#"{{"id":"{s}","opt":null}}"#)
        );
        let upper = s.to_uppercase();
        assert!(serde_json::from_str::<W>(&format!(r#"{{"id":"{upper}"}}"#)).is_err());
        assert!(serde_json::from_str::<W>(&format!(r#"{{"id":"{s}","opt":"{upper}"}}"#)).is_err());
    }
}
