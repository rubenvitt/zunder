//! Login through the server's built-in OAuth authorization server
//! (authorization code with PKCE for native apps, RFC 8252):
//!
//! 1. [`start`]: read the server metadata, register the app as a public
//!    client named after the device with the redirect URI [`REDIRECT_URI`],
//!    and build the authorization URL for the resource `<issuer>/api`.
//! 2. The system browser shows Zunder's login and consent page; the server
//!    redirects to [`REDIRECT_URI`], which the OS hands to the app.
//! 3. [`code_from_callback`] checks `state`, [`exchange`] trades the code for
//!    a device token (`zd_…`, no expiry), which becomes the API token.
//!
//! The started login ([`PendingLogin`]) is stored in the database, so it
//! survives the OS killing the app while the browser is in front.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::client::build_http;
use crate::error::{Error, Result};
use crate::store::normalize_server_url;

/// Where the server sends the browser back to (private-use scheme of the
/// app, registered in `tauri.conf.json`).
pub const REDIRECT_URI: &str = "dev.rubeen.zunder:/oauth";
/// A started login is valid this long (the server's consent page lives ten
/// minutes, its codes five).
pub const LOGIN_TTL_MS: i64 = 15 * 60 * 1000;

/// A login waiting for the browser to come back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingLogin {
    pub server_url: String,
    pub client_id: String,
    pub token_endpoint: String,
    pub resource: String,
    pub verifier: String,
    pub state: String,
    /// Switching servers drops pending operations (asked before the login).
    pub discard_pending: bool,
    pub started_at_ms: i64,
}

impl PendingLogin {
    pub fn expired(&self, now_ms: i64) -> bool {
        now_ms - self.started_at_ms > LOGIN_TTL_MS
    }
}

#[derive(Deserialize)]
struct Metadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
}

#[derive(Deserialize)]
struct Registered {
    client_id: String,
}

#[derive(Deserialize)]
struct OAuthErrorBody {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct TokenBody {
    access_token: String,
}

fn random(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("operating system RNG is available");
    URL_SAFE_NO_PAD.encode(buf)
}

/// PKCE S256 challenge of a verifier (RFC 7636).
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn login_error(message: impl Into<String>) -> Error {
    Error::Login(message.into())
}

async fn oauth_response<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    if !status.is_success() {
        return Err(match serde_json::from_slice::<OAuthErrorBody>(&bytes) {
            Ok(body) => login_error(body.error_description.unwrap_or(body.error)),
            Err(_) => Error::Server {
                status: status.as_u16(),
                code: "unknown".into(),
                message: String::from_utf8_lossy(&bytes).chars().take(200).collect(),
            },
        });
    }
    serde_json::from_slice(&bytes).map_err(|e| Error::Protocol(e.to_string()))
}

/// Registers the app as client `device_name` and returns the pending login
/// and the URL to open in the browser.
pub async fn start(
    server_url: &str,
    device_name: &str,
    discard_pending: bool,
    now_ms: i64,
) -> Result<(PendingLogin, String)> {
    let server_url = normalize_server_url(server_url)?;
    let device_name = device_name.trim();
    if device_name.is_empty() || device_name.chars().count() > 100 {
        return Err(Error::InvalidConfig("Gerätename: 1 bis 100 Zeichen".into()));
    }
    let base =
        Url::parse(&format!("{server_url}/")).map_err(|e| Error::InvalidConfig(e.to_string()))?;
    let http = build_http(&base)?;
    let metadata_url = base
        .join(".well-known/oauth-authorization-server")
        .map_err(|e| Error::InvalidConfig(e.to_string()))?;
    let metadata: Metadata = oauth_response(
        http.get(metadata_url)
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?,
    )
    .await?;
    // The app opens the authorization endpoint in the OS and sends the code
    // to the token endpoint: both must belong to the server the user named.
    for endpoint in [
        &metadata.authorization_endpoint,
        &metadata.token_endpoint,
        &metadata.registration_endpoint,
    ] {
        same_origin(&base, endpoint)?;
    }
    let registered: Registered = oauth_response(
        http.post(&metadata.registration_endpoint)
            .json(&serde_json::json!({
                "client_name": device_name,
                "redirect_uris": [REDIRECT_URI],
                "grant_types": ["authorization_code"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            }))
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?,
    )
    .await?;

    let pending = PendingLogin {
        server_url,
        client_id: registered.client_id,
        token_endpoint: metadata.token_endpoint,
        resource: format!("{}/api", metadata.issuer.trim_end_matches('/')),
        verifier: random(32),
        state: random(16),
        discard_pending,
        started_at_ms: now_ms,
    };
    let mut url = Url::parse(&metadata.authorization_endpoint)
        .map_err(|e| Error::Protocol(format!("authorization_endpoint: {e}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &pending.client_id)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("code_challenge", &pkce_challenge(&pending.verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &pending.state)
        .append_pair("resource", &pending.resource);
    Ok((pending, url.into()))
}

/// Fails unless `endpoint` is on the same origin (scheme, host, port) as
/// the server.
fn same_origin(base: &Url, endpoint: &str) -> Result<()> {
    match Url::parse(endpoint) {
        Ok(url) if url.origin() == base.origin() => Ok(()),
        _ => Err(Error::Protocol(format!(
            "OAuth-Endpunkt {endpoint:?} gehört nicht zum Server {}",
            base.origin().ascii_serialization()
        ))),
    }
}

/// Whether a URL the OS handed to the app is the login callback:
/// `dev.rubeen.zunder:/oauth?…`, also in the form some browsers normalize it
/// to (`dev.rubeen.zunder://oauth/?…`).
pub fn is_callback(url: &Url) -> bool {
    let Ok(expected) = Url::parse(REDIRECT_URI) else {
        return false;
    };
    url.scheme() == expected.scheme()
        && match url.host_str() {
            None => url.path() == expected.path(),
            Some(host) => {
                host == expected.path().trim_start_matches('/') && matches!(url.path(), "" | "/")
            }
        }
}

/// The authorization code from the callback URL, after checking `state`.
pub fn code_from_callback(pending: &PendingLogin, callback: &Url) -> Result<String> {
    if !is_callback(callback) {
        return Err(login_error("Unerwartete Rücksprung-Adresse"));
    }
    let param = |name: &str| {
        callback
            .query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    if param("state").as_deref() != Some(pending.state.as_str()) {
        return Err(login_error(
            "Die Antwort gehört nicht zur laufenden Anmeldung; bitte erneut anmelden",
        ));
    }
    if let Some(error) = param("error") {
        return Err(login_error(match error.as_str() {
            "access_denied" => "Die Anmeldung wurde abgelehnt".to_owned(),
            _ => param("error_description").unwrap_or(error),
        }));
    }
    param("code").ok_or_else(|| login_error("Die Antwort enthält keinen Code"))
}

/// Trades the code for the device token.
pub async fn exchange(pending: &PendingLogin, code: &str) -> Result<String> {
    let base = Url::parse(&format!("{}/", pending.server_url))
        .map_err(|e| Error::InvalidConfig(e.to_string()))?;
    let http = build_http(&base)?;
    let body: TokenBody = oauth_response(
        http.post(&pending.token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", REDIRECT_URI),
                ("client_id", pending.client_id.as_str()),
                ("code_verifier", pending.verifier.as_str()),
                ("resource", pending.resource.as_str()),
            ])
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?,
    )
    .await?;
    Ok(body.access_token)
}

/// Revokes a device token (`POST /oauth/revoke`, RFC 7009), e.g. the one a
/// new login replaces. Best effort: the device can also be removed on the
/// token page.
pub async fn revoke(server_url: &str, token: &str) -> Result<()> {
    let base =
        Url::parse(&format!("{server_url}/")).map_err(|e| Error::InvalidConfig(e.to_string()))?;
    let url = base
        .join("oauth/revoke")
        .map_err(|e| Error::InvalidConfig(e.to_string()))?;
    build_http(&base)?
        .post(url)
        .form(&[("token", token)])
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending() -> PendingLogin {
        PendingLogin {
            server_url: "https://zunder.example.org".into(),
            client_id: "c".into(),
            token_endpoint: "https://zunder.example.org/oauth/token".into(),
            resource: "https://zunder.example.org/api".into(),
            verifier: "v".into(),
            state: "s1".into(),
            discard_pending: false,
            started_at_ms: 1_000,
        }
    }

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn pkce_matches_rfc_7636() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(random(32).len(), 43);
        assert_ne!(random(32), random(32));
    }

    #[test]
    fn callbacks_are_checked() {
        let p = pending();
        let ok = url("dev.rubeen.zunder:/oauth?code=zoc_1&state=s1&iss=x");
        assert!(is_callback(&ok));
        assert_eq!(code_from_callback(&p, &ok).unwrap(), "zoc_1");

        let foreign = url("dev.rubeen.zunder:/oauth?code=zoc_1&state=other");
        assert_eq!(
            code_from_callback(&p, &foreign).unwrap_err().code(),
            "login"
        );
        let denied = url("dev.rubeen.zunder:/oauth?error=access_denied&state=s1");
        assert!(code_from_callback(&p, &denied)
            .unwrap_err()
            .to_string()
            .contains("abgelehnt"));
        let normalized = url("dev.rubeen.zunder://oauth/?code=zoc_2&state=s1");
        assert_eq!(code_from_callback(&p, &normalized).unwrap(), "zoc_2");
        let other_path = url("dev.rubeen.zunder:/task?code=zoc_1&state=s1");
        assert!(!is_callback(&other_path));
        assert!(code_from_callback(&p, &other_path).is_err());
        let missing = url("dev.rubeen.zunder:/oauth?state=s1");
        assert!(code_from_callback(&p, &missing).is_err());
    }

    #[test]
    fn endpoints_must_belong_to_the_server() {
        let base = url("https://zunder.example.org/");
        assert!(same_origin(&base, "https://zunder.example.org/oauth/authorize").is_ok());
        assert!(same_origin(&base, "https://evil.example/oauth/authorize").is_err());
        assert!(same_origin(&base, "http://zunder.example.org/oauth/authorize").is_err());
        assert!(same_origin(&base, "file:///etc/passwd").is_err());
        assert!(same_origin(&base, "javascript:alert(1)").is_err());
    }

    #[test]
    fn logins_expire() {
        let p = pending();
        assert!(!p.expired(1_000 + LOGIN_TTL_MS));
        assert!(p.expired(1_001 + LOGIN_TTL_MS));
    }
}
