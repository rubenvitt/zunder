//! Instance settings changed in the web UI: the Timing bridge and the
//! GitHub token for PR status.
//!
//! Stored settings override the environment field by field:
//!
//! - `timing.token`: API token of the Timing web app (overrides
//!   `ZUNDER_TIMING_TOKEN`).
//! - `timing.project`: project for new time entries (overrides
//!   `ZUNDER_TIMING_PROJECT`; an empty value means "no project", even if the
//!   environment names one).
//!
//! - `github.token`: token for reading pull requests (overrides
//!   `ZUNDER_GITHUB_TOKEN`).
//!
//! Resetting removes a section's settings, so the environment applies
//! again. Tokens never leave the server; the UI only sees their last
//! characters.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::TimingConfig;
use crate::error::{AppError, AppResult};

/// Key prefix of all Timing settings.
pub const TIMING_PREFIX: &str = "timing.";
pub const TIMING_TOKEN_KEY: &str = "timing.token";
pub const TIMING_PROJECT_KEY: &str = "timing.project";
/// Maximum length of a Timing token.
pub const MAX_TIMING_TOKEN_CHARS: usize = 2048;
/// Maximum length of a Timing project (self link or title).
pub const MAX_TIMING_PROJECT_CHARS: usize = 500;
/// Number of trailing token characters shown as a hint.
const TOKEN_HINT_CHARS: usize = 4;

/// Timing settings stored in the database (`None`: not overridden).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimingOverrides {
    pub token: Option<String>,
    /// `Some("")`: explicitly no project.
    pub project: Option<String>,
}

impl TimingOverrides {
    /// Reads the overrides from the rows of [`crate::db::settings`].
    pub fn from_rows(rows: &BTreeMap<String, String>) -> Self {
        Self {
            token: rows
                .get(TIMING_TOKEN_KEY)
                .filter(|t| !t.is_empty())
                .cloned(),
            project: rows.get(TIMING_PROJECT_KEY).cloned(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.token.is_none() && self.project.is_none()
    }

    /// Applies a validated update.
    pub fn apply(&mut self, update: &TimingUpdate) {
        if let Some(token) = &update.token {
            self.token = Some(token.clone());
        }
        if let Some(project) = &update.project {
            self.project = Some(project.clone());
        }
    }
}

/// Where an effective value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingSource {
    /// Stored in the web UI.
    Settings,
    /// Environment variable.
    Env,
}

/// The Timing settings in effect: environment plus overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveTiming {
    pub token: Option<String>,
    pub project: Option<String>,
}

impl EffectiveTiming {
    pub fn resolve(env: Option<&TimingConfig>, overrides: &TimingOverrides) -> Self {
        let token = overrides
            .token
            .clone()
            .or_else(|| env.map(|e| e.token.clone()));
        let project = match &overrides.project {
            Some(p) if p.is_empty() => None,
            Some(p) => Some(p.clone()),
            None => env.and_then(|e| e.project.clone()),
        };
        Self { token, project }
    }
}

/// What the web UI shows (`GET /api/settings/timing`). Never contains the
/// token itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimingSettings {
    /// Whether flames use Timing (a token is in effect).
    pub configured: bool,
    pub token_source: Option<SettingSource>,
    /// Last characters of the token in effect, to recognize it.
    pub token_hint: Option<String>,
    /// Project in effect (`None`: Timing's default).
    pub project: Option<String>,
    pub project_source: Option<SettingSource>,
    /// Whether settings from the web UI override the environment.
    pub overridden: bool,
    /// Whether `ZUNDER_TIMING_TOKEN` is set (applies after a reset).
    pub env_token: bool,
    /// `ZUNDER_TIMING_PROJECT` (applies after a reset).
    pub env_project: Option<String>,
}

impl TimingSettings {
    pub fn build(env: Option<&TimingConfig>, overrides: &TimingOverrides) -> Self {
        let effective = EffectiveTiming::resolve(env, overrides);
        let token_source = match (&overrides.token, env) {
            (Some(_), _) => Some(SettingSource::Settings),
            (None, Some(_)) => Some(SettingSource::Env),
            (None, None) => None,
        };
        let project_source = match (&overrides.project, env.and_then(|e| e.project.as_ref())) {
            (Some(_), _) => Some(SettingSource::Settings),
            (None, Some(_)) => Some(SettingSource::Env),
            (None, None) => None,
        };
        Self {
            configured: effective.token.is_some(),
            token_source,
            token_hint: effective.token.as_deref().map(token_hint),
            project: effective.project,
            project_source,
            overridden: !overrides.is_empty(),
            env_token: env.is_some(),
            env_project: env.and_then(|e| e.project.clone()),
        }
    }
}

/// `…` plus the last characters; short tokens are not revealed at all.
fn token_hint(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < TOKEN_HINT_CHARS * 3 {
        return "…".into();
    }
    let tail: String = chars[chars.len() - TOKEN_HINT_CHARS..].iter().collect();
    format!("…{tail}")
}

/// Change of the Timing settings (`PUT /api/settings/timing`). Missing or
/// empty `token` keeps the stored token; missing `project` keeps the
/// project, an empty one means "no project".
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingUpdate {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
}

impl TimingUpdate {
    /// Trims and checks the values (German messages).
    pub fn validated(self) -> AppResult<Self> {
        let token = match self.token.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(t) if t.chars().count() > MAX_TIMING_TOKEN_CHARS => {
                return Err(AppError::bad_request(format!(
                    "Der Timing-Token darf höchstens {MAX_TIMING_TOKEN_CHARS} Zeichen lang sein"
                )))
            }
            Some(t) if t.chars().any(|c| c.is_whitespace() || c.is_control()) => {
                return Err(AppError::bad_request(
                    "Der Timing-Token darf keine Leer- oder Steuerzeichen enthalten",
                ))
            }
            Some(t) => Some(t.to_owned()),
        };
        let project = match self.project.as_deref().map(str::trim) {
            None => None,
            Some(p) if p.chars().count() > MAX_TIMING_PROJECT_CHARS => {
                return Err(AppError::bad_request(format!(
                    "Das Timing-Projekt darf höchstens {MAX_TIMING_PROJECT_CHARS} Zeichen lang sein"
                )))
            }
            Some(p) if p.chars().any(char::is_control) => {
                return Err(AppError::bad_request(
                    "Das Timing-Projekt darf keine Steuerzeichen enthalten",
                ))
            }
            Some(p) => Some(p.to_owned()),
        };
        Ok(Self { token, project })
    }
}

/// Key prefix of the GitHub settings.
pub const GITHUB_PREFIX: &str = "github.";
pub const GITHUB_TOKEN_KEY: &str = "github.token";
/// Maximum length of a GitHub token.
pub const MAX_GITHUB_TOKEN_CHARS: usize = 1024;

/// Page that creates a fine-grained token with the permissions Zunder
/// needs preselected: read-only *Pull requests* and *Commit statuses*
/// (*Metadata* comes with every token). Owner and repositories are chosen
/// on that page. See GitHub's "Pre-filling fine-grained personal access
/// token details using URL parameters".
pub const GITHUB_TOKEN_URL: &str = "https://github.com/settings/personal-access-tokens/new\
?name=Zunder&description=Zunder%3A+Status+verlinkter+Pull+Requests+lesen\
&expires_in=366&pull_requests=read&statuses=read";
/// Classic token with `repo` (classic tokens have no read-only scope for
/// private repositories), for PRs across owners and CI check runs.
pub const GITHUB_CLASSIC_TOKEN_URL: &str =
    "https://github.com/settings/tokens/new?scopes=repo&description=Zunder";

/// What the web UI shows (`GET /api/settings/github`). Never contains the
/// token itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubSettings {
    /// Whether PR links show their status (a token is in effect).
    pub configured: bool,
    pub token_source: Option<SettingSource>,
    /// Last characters of the token in effect, to recognize it.
    pub token_hint: Option<String>,
    /// Whether a token from the web UI overrides the environment.
    pub overridden: bool,
    /// Whether `ZUNDER_GITHUB_TOKEN` is set (applies after a reset).
    pub env_token: bool,
    /// Creates a fine-grained token with the right permissions.
    pub create_url: &'static str,
    /// Creates a classic token instead.
    pub create_classic_url: &'static str,
}

impl GithubSettings {
    pub fn build(env: Option<&str>, stored: Option<&str>) -> Self {
        let token_source = match (stored, env) {
            (Some(_), _) => Some(SettingSource::Settings),
            (None, Some(_)) => Some(SettingSource::Env),
            (None, None) => None,
        };
        let effective = stored.or(env);
        Self {
            configured: effective.is_some(),
            token_source,
            token_hint: effective.map(token_hint),
            overridden: stored.is_some(),
            env_token: env.is_some(),
            create_url: GITHUB_TOKEN_URL,
            create_classic_url: GITHUB_CLASSIC_TOKEN_URL,
        }
    }
}

/// Reads the stored GitHub token from the rows of [`crate::db::settings`].
pub fn github_token_from_rows(rows: &BTreeMap<String, String>) -> Option<String> {
    rows.get(GITHUB_TOKEN_KEY)
        .filter(|t| !t.is_empty())
        .cloned()
}

/// Change of the GitHub settings (`PUT /api/settings/github`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubUpdate {
    pub token: String,
}

impl GithubUpdate {
    /// The trimmed token, checked (German messages).
    pub fn validated(self) -> AppResult<String> {
        let token = self.token.trim();
        if token.is_empty() {
            return Err(AppError::bad_request("Bitte einen GitHub-Token eingeben"));
        }
        if token.chars().count() > MAX_GITHUB_TOKEN_CHARS {
            return Err(AppError::bad_request(format!(
                "Der GitHub-Token darf höchstens {MAX_GITHUB_TOKEN_CHARS} Zeichen lang sein"
            )));
        }
        if !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return Err(AppError::bad_request(
                "Der GitHub-Token darf nur Buchstaben, Ziffern, _ , - und . enthalten",
            ));
        }
        Ok(token.to_owned())
    }
}

/// Result of `POST /api/settings/github/check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubCheck {
    /// Whether GitHub accepted the token.
    pub ok: bool,
    /// The account the token belongs to.
    pub login: Option<String>,
    /// German error message when GitHub did not accept it.
    pub error: Option<String>,
}

/// Result of `POST /api/settings/timing/check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimingCheck {
    /// Whether Timing accepted the token.
    pub ok: bool,
    /// German error message when it did not.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(project: Option<&str>) -> TimingConfig {
        TimingConfig {
            token: "env-token-0123456789".into(),
            project: project.map(str::to_owned),
        }
    }

    #[test]
    fn environment_only() {
        let e = env(Some("/projects/1"));
        let s = TimingSettings::build(Some(&e), &TimingOverrides::default());
        assert!(s.configured);
        assert_eq!(s.token_source, Some(SettingSource::Env));
        assert_eq!(s.token_hint.as_deref(), Some("…6789"));
        assert_eq!(s.project.as_deref(), Some("/projects/1"));
        assert_eq!(s.project_source, Some(SettingSource::Env));
        assert!(!s.overridden);
        assert!(s.env_token);
    }

    #[test]
    fn nothing_configured() {
        let s = TimingSettings::build(None, &TimingOverrides::default());
        assert!(!s.configured);
        assert_eq!(s.token_source, None);
        assert_eq!(s.token_hint, None);
        assert_eq!(s.project, None);
        assert!(!s.env_token);
    }

    #[test]
    fn overrides_win_field_by_field() {
        let e = env(Some("/projects/1"));
        let o = TimingOverrides {
            token: None,
            project: Some("Deep Work".into()),
        };
        let eff = EffectiveTiming::resolve(Some(&e), &o);
        assert_eq!(eff.token.as_deref(), Some("env-token-0123456789"));
        assert_eq!(eff.project.as_deref(), Some("Deep Work"));

        // An empty project override removes the environment's project.
        let o = TimingOverrides {
            token: Some("ui-token-abcdefghij".into()),
            project: Some(String::new()),
        };
        let s = TimingSettings::build(Some(&e), &o);
        assert_eq!(s.token_source, Some(SettingSource::Settings));
        assert_eq!(s.token_hint.as_deref(), Some("…ghij"));
        assert_eq!(s.project, None);
        assert_eq!(s.project_source, Some(SettingSource::Settings));
        assert_eq!(s.env_project.as_deref(), Some("/projects/1"));
        assert!(s.overridden);
    }

    #[test]
    fn short_tokens_are_not_revealed() {
        assert_eq!(token_hint("abc"), "…");
        assert_eq!(token_hint("0123456789ab"), "…89ab");
    }

    #[test]
    fn update_validation() {
        let u = TimingUpdate {
            token: Some("  123|abc  ".into()),
            project: Some("  /projects/2 ".into()),
        }
        .validated()
        .unwrap();
        assert_eq!(u.token.as_deref(), Some("123|abc"));
        assert_eq!(u.project.as_deref(), Some("/projects/2"));

        let u = TimingUpdate {
            token: Some("   ".into()),
            project: Some(String::new()),
        }
        .validated()
        .unwrap();
        assert_eq!(u.token, None);
        assert_eq!(u.project.as_deref(), Some(""));

        for token in ["a b", "a\nb"] {
            assert!(TimingUpdate {
                token: Some(token.into()),
                project: None
            }
            .validated()
            .is_err());
        }
        assert!(TimingUpdate {
            token: None,
            project: Some("x\u{7}".into())
        }
        .validated()
        .is_err());
        assert!(TimingUpdate {
            token: Some("x".repeat(MAX_TIMING_TOKEN_CHARS + 1)),
            project: None
        }
        .validated()
        .is_err());
    }

    #[test]
    fn github_settings() {
        let s = GithubSettings::build(None, None);
        assert!(!s.configured);
        assert_eq!(s.token_source, None);
        assert_eq!(s.token_hint, None);
        assert!(!s.overridden && !s.env_token);

        let s = GithubSettings::build(Some("ghp_env0123456789"), None);
        assert!(s.configured);
        assert_eq!(s.token_source, Some(SettingSource::Env));
        assert_eq!(s.token_hint.as_deref(), Some("…6789"));

        let s = GithubSettings::build(Some("ghp_env0123456789"), Some("github_pat_ui_abcd"));
        assert_eq!(s.token_source, Some(SettingSource::Settings));
        assert_eq!(s.token_hint.as_deref(), Some("…abcd"));
        assert!(s.overridden && s.env_token);
    }

    #[test]
    fn github_token_links_preselect_read_only_permissions() {
        let url = url::Url::parse(GITHUB_TOKEN_URL).unwrap();
        assert_eq!(url.path(), "/settings/personal-access-tokens/new");
        let q: BTreeMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q["name"], "Zunder");
        assert!(q["name"].len() <= 40);
        assert_eq!(q["pull_requests"], "read");
        assert_eq!(q["statuses"], "read");
        assert_eq!(q["expires_in"], "366");
        assert!(q.values().all(|v| v != "write" && v != "admin"));
        let classic = url::Url::parse(GITHUB_CLASSIC_TOKEN_URL).unwrap();
        assert_eq!(
            classic
                .query_pairs()
                .find(|(k, _)| k == "scopes")
                .unwrap()
                .1,
            "repo"
        );
    }

    #[test]
    fn github_update_validation() {
        let t = GithubUpdate {
            token: "  github_pat_11AB_cd-9.x  ".into(),
        };
        assert_eq!(t.validated().unwrap(), "github_pat_11AB_cd-9.x");
        for bad in ["", "   ", "a b", "ghp_ä", "a\nb"] {
            assert!(
                GithubUpdate { token: bad.into() }.validated().is_err(),
                "{bad:?}"
            );
        }
        assert!(GithubUpdate {
            token: "x".repeat(MAX_GITHUB_TOKEN_CHARS + 1)
        }
        .validated()
        .is_err());
        let mut rows = BTreeMap::new();
        assert_eq!(github_token_from_rows(&rows), None);
        rows.insert(GITHUB_TOKEN_KEY.to_owned(), "ghp_x".to_owned());
        assert_eq!(github_token_from_rows(&rows).as_deref(), Some("ghp_x"));
    }

    #[test]
    fn overrides_from_rows() {
        let mut rows = BTreeMap::new();
        rows.insert(TIMING_TOKEN_KEY.to_owned(), "t".to_owned());
        rows.insert(TIMING_PROJECT_KEY.to_owned(), String::new());
        let o = TimingOverrides::from_rows(&rows);
        assert_eq!(o.token.as_deref(), Some("t"));
        assert_eq!(o.project.as_deref(), Some(""));
        assert!(TimingOverrides::from_rows(&BTreeMap::new()).is_empty());
    }
}
