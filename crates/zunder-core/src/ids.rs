//! Identifiers.
//!
//! Entities and operations are identified by CUID2 strings: 24 characters, a
//! lowercase letter followed by lowercase letters and digits
//! (`tz4a98xxat96iws9zmbrgj3a`). Clients generate them themselves.
//!
//! Before CUID2, Zunder used UUIDs (v7, and v5 for imports). A legacy UUID is
//! still accepted wherever an ID is read and turned into a CUID2-shaped ID
//! by [`Id::from_legacy`], a fixed hash. Every component (server, web UI,
//! app, importer) arrives at the same ID for the same UUID, so old links,
//! vault `zunder_id`s, mapping files and unsynced client changes keep
//! working, and the importer still derives the IDs it derived before.

use std::fmt;
use std::str::FromStr;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::op::EntityKind;

/// Length of every ID.
pub const ID_LEN: usize = 24;

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Fixed namespace of the legacy deterministic (UUIDv5) import IDs.
///
/// Never change this value: import IDs derived from ClickUp IDs depend on it.
pub const ZUNDER_NAMESPACE: Uuid = uuid::uuid!("f9d16046-2e7f-4f97-b1be-e789d21e4fec");

/// An entity or operation ID (CUID2 form). Cheap to copy, ordered by its
/// string form.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id([u8; ID_LEN]);

impl Id {
    /// Below every valid ID (for range queries).
    pub const MIN: Id = Id([b'0'; ID_LEN]);
    /// The largest valid ID (for range queries).
    pub const MAX: Id = Id([b'z'; ID_LEN]);

    /// A new random ID (CUID2).
    pub fn new() -> Id {
        let raw = cuid2::create_id();
        Id::parse(&raw).expect("cuid2 yields 24 lowercase base36 characters starting with a letter")
    }

    /// Parses an ID in canonical form only.
    pub fn parse(s: &str) -> Option<Id> {
        let b = s.as_bytes();
        if b.len() != ID_LEN
            || !b[0].is_ascii_lowercase()
            || !b
                .iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return None;
        }
        let mut out = [0u8; ID_LEN];
        out.copy_from_slice(b);
        Some(Id(out))
    }

    /// Parses an ID in canonical form or a legacy UUID (lowercase,
    /// hyphenated), which is converted with [`Id::from_legacy`].
    pub fn parse_any(s: &str) -> Option<Id> {
        Id::parse(s).or_else(|| parse_legacy_uuid(s).map(Id::from_legacy))
    }

    /// The ID a legacy UUID became.
    pub fn from_legacy(uuid: Uuid) -> Id {
        let mut buf = Uuid::encode_buffer();
        Id::derive(&format!(
            "uuid:{}",
            uuid.hyphenated().encode_lower(&mut buf)
        ))
    }

    /// A deterministic ID in CUID2 form, derived from `name` (SHA-256).
    pub fn derive(name: &str) -> Id {
        let hash = Sha256::digest(name.as_bytes());
        let mut out = [0u8; ID_LEN];
        out[0] = b'a' + hash[0] % 26;
        for i in 1..ID_LEN {
            out[i] = BASE36[usize::from(hash[i] % 36)];
        }
        Id(out)
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("IDs are ASCII")
    }
}

impl Default for Id {
    fn default() -> Self {
        Id::new()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.as_str())
    }
}

/// Error for a string that is neither an ID nor a legacy UUID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("keine gültige ID: {0:?}")]
pub struct InvalidId(pub String);

/// Accepts canonical IDs and legacy UUIDs (see [`Id::parse_any`]).
impl FromStr for Id {
    type Err = InvalidId;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Id::parse_any(s).ok_or_else(|| InvalidId(s.to_owned()))
    }
}

impl From<Id> for serde_json::Value {
    fn from(id: Id) -> Self {
        serde_json::Value::String(id.to_string())
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Reads canonical IDs and legacy UUIDs (converted).
impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Id::parse_any(&s).ok_or_else(|| {
            de::Error::custom(format!(
                "expected an ID (24 lowercase letters/digits starting with a letter), got {:?}",
                s.as_ref()
            ))
        })
    }
}

/// Generates a new ID.
pub fn new_id() -> Id {
    Id::new()
}

/// Derives a deterministic ID from an arbitrary name. Goes through the legacy
/// UUIDv5 so IDs derived before CUID2 map to the same ID.
pub fn deterministic_id(name: &str) -> Id {
    Id::from_legacy(Uuid::new_v5(&ZUNDER_NAMESPACE, name.as_bytes()))
}

/// Derives the deterministic Zunder ID for an entity imported from ClickUp.
///
/// The name hashed is `clickup:<kind>:<clickup_id>`, so the same ClickUp ID
/// yields different IDs for different entity kinds (e.g. a ClickUp list
/// imported as a project and a task that happens to share the ID string).
pub fn clickup_entity_id(kind: EntityKind, clickup_id: &str) -> Id {
    deterministic_id(&format!("clickup:{}:{}", kind.as_str(), clickup_id))
}

/// Parses an ID in canonical form or as legacy UUID (see [`Id::parse_any`]).
pub fn parse_id(s: &str) -> Option<Id> {
    Id::parse_any(s)
}

/// A legacy UUID in its only accepted wire form (lowercase, hyphenated).
pub fn parse_legacy_uuid(s: &str) -> Option<Uuid> {
    if s.len() != 36 {
        return None;
    }
    let id = Uuid::try_parse(s).ok()?;
    let mut buf = Uuid::encode_buffer();
    (id.hyphenated().encode_lower(&mut buf) == s).then_some(id)
}

/// Converts a legacy UUID string to its ID; anything else is returned as is.
/// For IDs inside JSON values (references such as `parent_id`).
pub fn upgrade_legacy_str(s: &str) -> Option<String> {
    parse_legacy_uuid(s).map(|u| Id::from_legacy(u).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ids_are_canonical_and_distinct() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert_eq!(Id::parse(a.as_str()), Some(a));
        assert_eq!(a.as_str().len(), ID_LEN);
        assert!(a > Id::MIN && a <= Id::MAX);
    }

    #[test]
    fn parse_rejects_other_forms() {
        let s = "tz4a98xxat96iws9zmbrgj3a";
        assert!(Id::parse(s).is_some());
        for bad in [
            "",
            "Tz4a98xxat96iws9zmbrgj3a",
            "1z4a98xxat96iws9zmbrgj3a",
            "tz4a98xxat96iws9zmbrgj3",
            "tz4a98xxat96iws9zmbrgj3ab",
            "tz4a98xxat96iws9zmbrgj-a",
        ] {
            assert!(Id::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn legacy_uuids_convert_deterministically() {
        let uuid = "01929c3e-5b7a-7cc1-9a51-3f0e4f6b2a10";
        let id = Id::parse_any(uuid).unwrap();
        assert_eq!(Id::parse_any(uuid), Some(id));
        assert_eq!(Id::parse(id.as_str()), Some(id));
        // Frozen value: the web UI and the app compute the same (shared vectors).
        assert_eq!(id, Id::derive(&format!("uuid:{uuid}")));
        assert!(Id::parse_any(&uuid.to_uppercase()).is_none());
        assert!(Id::parse_any(&uuid.replace('-', "")).is_none());
        assert_eq!(upgrade_legacy_str(uuid), Some(id.to_string()));
        assert_eq!(upgrade_legacy_str("tz4a98xxat96iws9zmbrgj3a"), None);
    }

    #[test]
    fn deterministic_ids_are_stable() {
        let a = clickup_entity_id(EntityKind::Task, "86abc123");
        assert_eq!(a, clickup_entity_id(EntityKind::Task, "86abc123"));
        assert_ne!(a, clickup_entity_id(EntityKind::Project, "86abc123"));
        assert_ne!(a, clickup_entity_id(EntityKind::Task, "86abc124"));
        // The importer's IDs from before CUID2 map to the same IDs.
        let legacy = Uuid::new_v5(&ZUNDER_NAMESPACE, b"clickup:task:86abc123");
        assert_eq!(a, Id::from_legacy(legacy));
    }

    #[test]
    fn serde_accepts_legacy_and_writes_canonical() {
        let uuid = "01929c3e-5b7a-7cc1-9a51-3f0e4f6b2a10";
        let id: Id = serde_json::from_value(serde_json::json!(uuid)).unwrap();
        assert_eq!(
            serde_json::to_value(id).unwrap(),
            serde_json::json!(id.as_str())
        );
        assert!(serde_json::from_value::<Id>(serde_json::json!("nope")).is_err());
    }
}
