//! Errors of the local store and the sync client.
//!
//! Serializes as `{"code": "...", "message": "..."}` so that Tauri commands
//! can hand it to the web UI unchanged (codes are stable, messages German).

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Lokale Datenbank: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("Lokale Daten sind beschädigt: {0}")]
    Corrupt(String),
    #[error("Die lokale Datenbank stammt von einer neueren App-Version (Schema {0})")]
    SchemaTooNew(i32),
    #[error("Es ist noch kein Server eingerichtet")]
    NotConfigured,
    #[error("Ungültige Einstellung: {0}")]
    InvalidConfig(String),
    #[error(
        "{0} lokale Änderungen sind noch nicht übertragen; ein Serverwechsel würde sie verwerfen"
    )]
    PendingOps(usize),
    #[error("Server nicht erreichbar: {0}")]
    Network(String),
    #[error("Der Server lehnt den API-Token ab")]
    Unauthorized,
    #[error("Server-Fehler ({status}): {message}")]
    Server {
        status: u16,
        code: String,
        message: String,
    },
    #[error("Unerwartete Antwort vom Server: {0}")]
    Protocol(String),
    #[error("Benachrichtigung: {0}")]
    Notification(String),
}

impl Error {
    /// Stable, machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Db(_) => "db",
            Error::Corrupt(_) => "corrupt",
            Error::SchemaTooNew(_) => "schema_too_new",
            Error::NotConfigured => "not_configured",
            Error::InvalidConfig(_) => "invalid_config",
            Error::PendingOps(_) => "pending_ops",
            Error::Network(_) => "network",
            Error::Unauthorized => "unauthorized",
            Error::Server { .. } => "server",
            Error::Protocol(_) => "protocol",
            Error::Notification(_) => "notification",
        }
    }

    /// Whether the error means "no connection" (retry soon) rather than a
    /// problem that a retry will not fix.
    pub fn is_network(&self) -> bool {
        matches!(self, Error::Network(_))
    }
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut s = serializer.serialize_struct("Error", 2)?;
        s.serialize_field("code", self.code())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_code_and_message() {
        let json = serde_json::to_value(Error::Unauthorized).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"code": "unauthorized", "message": "Der Server lehnt den API-Token ab"})
        );
    }
}
