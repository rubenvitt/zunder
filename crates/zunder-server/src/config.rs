//! Server configuration from environment variables (contract section 3).

use std::net::SocketAddr;
use std::path::PathBuf;

use url::Url;

/// Minimum length of `ZUNDER_SESSION_SECRET`.
pub const MIN_SESSION_SECRET_LEN: usize = 32;

/// Key used to hash session IDs when dev auth runs without a session secret.
const DEV_SESSION_SECRET: &str = "zunder-dev-auth-session-secret-not-for-production";

/// Configuration error with a German, user-facing message.
#[derive(Debug, thiserror::Error)]
#[error("Konfigurationsfehler: {0}")]
pub struct ConfigError(pub String);

fn err<T>(msg: impl Into<String>) -> Result<T, ConfigError> {
    Err(ConfigError(msg.into()))
}

/// OpenID Connect settings (Pocket-ID).
#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer: Url,
    pub client_id: String,
    /// `None` = public client (PKCE only).
    pub client_secret: Option<String>,
    pub allowed_subject: Option<String>,
    pub allowed_email: Option<String>,
}

/// Timing web API settings (used by the Timing bridge).
#[derive(Debug, Clone)]
pub struct TimingConfig {
    pub token: String,
    pub project: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub public_url: Url,
    pub web_dir: PathBuf,
    pub oidc: Option<OidcConfig>,
    /// Key for hashing session IDs (HMAC-SHA256).
    pub session_secret: String,
    pub timing: Option<TimingConfig>,
    pub bootstrap_token: Option<String>,
    pub dev_auth: bool,
    /// Local hour (0–23) of the nightly backup.
    pub backup_hour: u8,
}

impl Config {
    /// Reads the configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Reads the configuration through `lookup` (testable). Empty values
    /// count as unset.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |key: &str| {
            lookup(key)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };

        let bind: SocketAddr = match get("ZUNDER_BIND") {
            Some(v) => v
                .parse()
                .map_err(|_| ConfigError(format!("ZUNDER_BIND ist keine Adresse: {v}")))?,
            None => SocketAddr::from(([0, 0, 0, 0], 8080)),
        };
        let data_dir = PathBuf::from(get("ZUNDER_DATA_DIR").unwrap_or_else(|| "./data".into()));
        let web_dir = PathBuf::from(get("ZUNDER_WEB_DIR").unwrap_or_else(|| "./web/dist".into()));

        let public_url = match get("ZUNDER_PUBLIC_URL") {
            Some(v) => Url::parse(&v)
                .map_err(|_| ConfigError(format!("ZUNDER_PUBLIC_URL ist keine URL: {v}")))?,
            None => Url::parse(&format!("http://localhost:{}", bind.port()))
                .expect("static URL is valid"),
        };
        if !matches!(public_url.scheme(), "http" | "https") || public_url.host_str().is_none() {
            return err("ZUNDER_PUBLIC_URL muss eine http(s)-URL sein");
        }

        let dev_auth = match get("ZUNDER_DEV_AUTH").as_deref() {
            None | Some("false") | Some("0") => false,
            Some("true") | Some("1") => true,
            Some(other) => {
                return err(format!(
                    "ZUNDER_DEV_AUTH muss true oder false sein: {other}"
                ))
            }
        };
        if dev_auth && !is_loopback_url(&public_url) {
            return err(
                "ZUNDER_DEV_AUTH=true ist nur mit einer lokalen ZUNDER_PUBLIC_URL \
                 (localhost, 127.0.0.1, ::1) erlaubt",
            );
        }

        let oidc = match (get("ZUNDER_OIDC_ISSUER"), get("ZUNDER_OIDC_CLIENT_ID")) {
            (None, None) => None,
            (Some(issuer), Some(client_id)) => {
                let issuer = Url::parse(&issuer).map_err(|_| {
                    ConfigError(format!("ZUNDER_OIDC_ISSUER ist keine URL: {issuer}"))
                })?;
                let allowed_subject = get("ZUNDER_OIDC_ALLOWED_SUBJECT");
                let allowed_email = get("ZUNDER_OIDC_ALLOWED_EMAIL").map(|e| e.to_lowercase());
                if allowed_subject.is_none() && allowed_email.is_none() {
                    return err(
                        "ZUNDER_OIDC_ALLOWED_SUBJECT oder ZUNDER_OIDC_ALLOWED_EMAIL muss gesetzt sein",
                    );
                }
                Some(OidcConfig {
                    issuer,
                    client_id,
                    client_secret: get("ZUNDER_OIDC_CLIENT_SECRET"),
                    allowed_subject,
                    allowed_email,
                })
            }
            _ => {
                return err("ZUNDER_OIDC_ISSUER und ZUNDER_OIDC_CLIENT_ID gehören zusammen");
            }
        };

        let session_secret = match get("ZUNDER_SESSION_SECRET") {
            Some(s) if s.len() >= MIN_SESSION_SECRET_LEN => s,
            Some(_) => {
                return err(format!(
                "ZUNDER_SESSION_SECRET muss mindestens {MIN_SESSION_SECRET_LEN} Zeichen lang sein"
            ))
            }
            None if oidc.is_some() && !dev_auth => {
                return err("ZUNDER_SESSION_SECRET ist für den OIDC-Login erforderlich")
            }
            None => DEV_SESSION_SECRET.to_owned(),
        };

        let timing = get("ZUNDER_TIMING_TOKEN").map(|token| TimingConfig {
            token,
            project: get("ZUNDER_TIMING_PROJECT"),
        });

        let backup_hour = match get("ZUNDER_BACKUP_HOUR") {
            None => 3,
            Some(v) => match v.parse::<u8>() {
                Ok(h) if h < 24 => h,
                _ => return err(format!("ZUNDER_BACKUP_HOUR muss 0 bis 23 sein: {v}")),
            },
        };

        Ok(Self {
            bind,
            data_dir,
            public_url,
            web_dir,
            oidc,
            session_secret,
            timing,
            bootstrap_token: get("ZUNDER_BOOTSTRAP_TOKEN"),
            dev_auth,
            backup_hour,
        })
    }

    /// A configuration for tests and local tooling: everything in `data_dir`,
    /// no OIDC, no dev auth.
    pub fn for_data_dir(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            web_dir: data_dir.join("web"),
            data_dir,
            public_url: Url::parse("http://localhost:8080").expect("static URL is valid"),
            oidc: None,
            session_secret: DEV_SESSION_SECRET.to_owned(),
            timing: None,
            bootstrap_token: None,
            dev_auth: false,
            backup_hour: 3,
        }
    }

    /// Path of the SQLite database.
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("zunder.db")
    }

    /// Directory of the nightly backups.
    pub fn backup_dir(&self) -> PathBuf {
        self.data_dir.join("backups")
    }

    /// Session cookies are `Secure` unless the public URL is plain-http
    /// loopback (local development).
    pub fn secure_cookies(&self) -> bool {
        !(self.public_url.scheme() == "http" && is_loopback_url(&self.public_url))
    }

    /// The OIDC redirect URI (`<public_url>/auth/callback`).
    pub fn oidc_redirect_uri(&self) -> Url {
        self.public_url
            .join("auth/callback")
            .expect("joining a static path is valid")
    }

    /// The origin (`scheme://host[:port]`) of the public URL.
    pub fn public_origin(&self) -> String {
        self.public_url.origin().ascii_serialization()
    }
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|k| map.get(k).cloned())
    }

    #[test]
    fn defaults() {
        let c = cfg(&[]).unwrap();
        assert_eq!(c.bind.to_string(), "0.0.0.0:8080");
        assert_eq!(c.data_dir, PathBuf::from("./data"));
        assert_eq!(c.web_dir, PathBuf::from("./web/dist"));
        assert_eq!(c.public_url.as_str(), "http://localhost:8080/");
        assert_eq!(c.backup_hour, 3);
        assert!(!c.dev_auth);
        assert!(c.oidc.is_none());
        assert!(!c.secure_cookies());
        assert_eq!(c.db_path(), PathBuf::from("./data/zunder.db"));
    }

    #[test]
    fn full_oidc_config() {
        let c = cfg(&[
            ("ZUNDER_BIND", "127.0.0.1:9000"),
            ("ZUNDER_PUBLIC_URL", "https://zunder.example.org"),
            ("ZUNDER_OIDC_ISSUER", "https://id.example.org"),
            ("ZUNDER_OIDC_CLIENT_ID", "zunder"),
            ("ZUNDER_OIDC_CLIENT_SECRET", "geheim"),
            ("ZUNDER_OIDC_ALLOWED_EMAIL", "Person@Example.org"),
            ("ZUNDER_SESSION_SECRET", "0123456789abcdef0123456789abcdef"),
            ("ZUNDER_BACKUP_HOUR", "23"),
            ("ZUNDER_TIMING_TOKEN", "t"),
        ])
        .unwrap();
        let oidc = c.oidc.as_ref().unwrap();
        assert_eq!(oidc.allowed_email.as_deref(), Some("person@example.org"));
        assert!(c.secure_cookies());
        assert_eq!(
            c.oidc_redirect_uri().as_str(),
            "https://zunder.example.org/auth/callback"
        );
        assert_eq!(c.public_origin(), "https://zunder.example.org");
        assert_eq!(c.backup_hour, 23);
        assert!(c.timing.is_some());
    }

    #[test]
    fn rejects_invalid_values() {
        assert!(cfg(&[("ZUNDER_BIND", "nope")]).is_err());
        assert!(cfg(&[("ZUNDER_BACKUP_HOUR", "24")]).is_err());
        assert!(cfg(&[("ZUNDER_DEV_AUTH", "yes")]).is_err());
        assert!(cfg(&[("ZUNDER_OIDC_ISSUER", "https://id.example.org")]).is_err());
        // OIDC without allowed person
        assert!(cfg(&[
            ("ZUNDER_OIDC_ISSUER", "https://id.example.org"),
            ("ZUNDER_OIDC_CLIENT_ID", "zunder"),
            ("ZUNDER_SESSION_SECRET", "0123456789abcdef0123456789abcdef"),
        ])
        .is_err());
        // OIDC without session secret
        assert!(cfg(&[
            ("ZUNDER_OIDC_ISSUER", "https://id.example.org"),
            ("ZUNDER_OIDC_CLIENT_ID", "zunder"),
            ("ZUNDER_OIDC_ALLOWED_SUBJECT", "abc"),
        ])
        .is_err());
        assert!(cfg(&[("ZUNDER_SESSION_SECRET", "kurz")]).is_err());
    }

    #[test]
    fn dev_auth_only_on_loopback() {
        assert!(cfg(&[("ZUNDER_DEV_AUTH", "true")]).unwrap().dev_auth);
        assert!(
            cfg(&[
                ("ZUNDER_DEV_AUTH", "true"),
                ("ZUNDER_PUBLIC_URL", "http://127.0.0.1:5173")
            ])
            .unwrap()
            .dev_auth
        );
        assert!(cfg(&[
            ("ZUNDER_DEV_AUTH", "true"),
            ("ZUNDER_PUBLIC_URL", "https://zunder.example.org")
        ])
        .is_err());
    }
}
