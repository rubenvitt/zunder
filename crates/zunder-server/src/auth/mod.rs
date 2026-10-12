//! Authentication: principals, API tokens, sessions, CSRF rule and OIDC.
//!
//! - **API tokens**: `Authorization: Bearer zt_<32 bytes base64url>`, stored
//!   as SHA-256 hex. `ZUNDER_BOOTSTRAP_TOKEN` is accepted as an additional
//!   fixed token.
//! - **Device tokens**: `zd_…`, issued to the Tauri app by the built-in
//!   authorization server (resource `<public>/api`), stored as SHA-256 hex,
//!   only accepted by `/api`.
//! - **Sessions**: cookie [`SESSION_COOKIE`] with a random 32-byte ID, stored
//!   as HMAC-SHA256 (key `ZUNDER_SESSION_SECRET`), 30 days sliding expiry.
//! - **CSRF**: cookie-authenticated requests that change state must carry the
//!   header [`CLIENT_HEADER`] (`X-Zunder-Client: web`). Browsers cannot add
//!   custom headers cross-site without a CORS preflight, which Zunder never
//!   grants.

pub mod middleware;
pub mod oidc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::app::Source;

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "zunder_session";
/// Header that cookie-authenticated, state-changing requests must send.
pub const CLIENT_HEADER: &str = "x-zunder-client";
/// Prefix of API tokens.
pub const TOKEN_PREFIX: &str = "zt_";
/// Prefix of device tokens (Tauri app, see [`crate::oauth`]).
pub const DEVICE_TOKEN_PREFIX: &str = "zd_";
/// Session lifetime (sliding).
pub const SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// A session is extended at most this often.
pub const SESSION_RENEW_AFTER_MS: i64 = 24 * 60 * 60 * 1000;
/// `last_used_at` of a token is written at most this often.
pub const TOKEN_TOUCH_INTERVAL_MS: i64 = 60 * 1000;

/// Who is making a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub user_id: Uuid,
    pub kind: PrincipalKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrincipalKind {
    /// Browser session (cookie).
    Session,
    /// `ZUNDER_DEV_AUTH=true` without any credentials.
    DevAuto,
    /// API token (bearer).
    Token { id: Uuid, name: String },
    /// `ZUNDER_BOOTSTRAP_TOKEN` (bearer).
    Bootstrap,
    /// OAuth access token of the built-in authorization server (bearer,
    /// only accepted by `/mcp`). `source_name` is `oauth:<client_name>`.
    OAuth {
        client_id: String,
        source_name: String,
    },
    /// Device token of the Tauri app (bearer, only accepted by `/api`).
    Device { id: Uuid, name: String },
}

/// Name used for the bootstrap token in sources and logs.
pub const BOOTSTRAP_TOKEN_NAME: &str = "bootstrap";

impl Principal {
    /// Authenticated like a browser (cookie or dev auth): subject to the CSRF
    /// rule and allowed to manage tokens.
    pub fn is_browser(&self) -> bool {
        matches!(self.kind, PrincipalKind::Session | PrincipalKind::DevAuto)
    }

    /// The token name for bearer principals (`oauth:<client_name>` for
    /// OAuth access tokens).
    pub fn token_name(&self) -> Option<&str> {
        match &self.kind {
            PrincipalKind::Token { name, .. } => Some(name),
            PrincipalKind::Bootstrap => Some(BOOTSTRAP_TOKEN_NAME),
            PrincipalKind::OAuth { source_name, .. } => Some(source_name),
            PrincipalKind::Session | PrincipalKind::DevAuto | PrincipalKind::Device { .. } => None,
        }
    }

    /// The source recorded for operations pushed via the sync API:
    /// `web` for browsers, `app:<device-name>` for devices,
    /// `api:<token-name>` for tokens.
    pub fn sync_source(&self) -> Source {
        if let PrincipalKind::Device { name, .. } = &self.kind {
            return Source::App(name.clone());
        }
        match self.token_name() {
            Some(name) => Source::Api(name.to_owned()),
            None => Source::Web,
        }
    }

    /// The source recorded for operations created through MCP tools.
    pub fn mcp_source(&self) -> Source {
        Source::Mcp(self.token_name().unwrap_or("web").to_owned())
    }
}

/// Whether a request carries the CSRF header ([`CLIENT_HEADER`], non-empty).
pub fn has_client_header(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(CLIENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.trim().is_empty())
}

/// The error for a missing CSRF header.
pub fn missing_client_header() -> crate::error::AppError {
    crate::error::AppError::Forbidden(
        "Header X-Zunder-Client fehlt (Schutz vor Cross-Site-Anfragen)".into(),
    )
}

/// 32 random bytes as base64url without padding (43 characters).
pub fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("operating system RNG is available");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A fresh API token (`zt_…`).
pub fn new_api_token() -> String {
    format!("{TOKEN_PREFIX}{}", random_secret())
}

/// SHA-256 hex digest (storage form of API tokens).
pub fn sha256_hex(input: &str) -> String {
    hex(&Sha256::digest(input.as_bytes()))
}

/// Keyed hash of a session ID (storage form of sessions).
pub fn session_hash(key: &[u8], session_id: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(session_id.as_bytes());
    hex(&mac.finalize().into_bytes())
}

/// PKCE S256 code challenge for a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Constant-time equality for secrets of equal length.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_format() {
        let t = new_api_token();
        assert!(t.starts_with("zt_"));
        assert_eq!(t.len(), 3 + 43);
        assert_ne!(t, new_api_token());
        assert!(t[3..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn hashes() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_ne!(session_hash(b"k1", "s"), session_hash(b"k2", "s"));
        assert_eq!(session_hash(b"k1", "s").len(), 64);
        // RFC 7636 appendix B
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn sources() {
        let user_id = Uuid::nil();
        let web = Principal {
            user_id,
            kind: PrincipalKind::Session,
        };
        assert_eq!(web.sync_source().label(), "web");
        assert!(web.is_browser());
        let token = Principal {
            user_id,
            kind: PrincipalKind::Token {
                id: Uuid::nil(),
                name: "import".into(),
            },
        };
        assert_eq!(token.sync_source().label(), "api:import");
        assert_eq!(token.mcp_source().label(), "mcp:import");
        assert!(!token.is_browser());
        let boot = Principal {
            user_id,
            kind: PrincipalKind::Bootstrap,
        };
        assert_eq!(boot.sync_source().label(), "api:bootstrap");
        let oauth = Principal {
            user_id,
            kind: PrincipalKind::OAuth {
                client_id: "c".into(),
                source_name: "oauth:Claude".into(),
            },
        };
        assert_eq!(oauth.mcp_source().label(), "mcp:oauth:Claude");
        assert!(!oauth.is_browser());
        let device = Principal {
            user_id,
            kind: PrincipalKind::Device {
                id: Uuid::nil(),
                name: "RPhone".into(),
            },
        };
        assert_eq!(device.sync_source().label(), "app:RPhone");
        assert!(!device.is_browser());
    }
}
