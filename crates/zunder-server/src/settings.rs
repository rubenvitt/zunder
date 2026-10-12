//! Instance settings changed in the web UI. For now: the Timing bridge.
//!
//! Stored settings override the environment field by field:
//!
//! - `timing.token`: API token of the Timing web app (overrides
//!   `ZUNDER_TIMING_TOKEN`).
//! - `timing.project`: project for new time entries (overrides
//!   `ZUNDER_TIMING_PROJECT`; an empty value means "no project", even if the
//!   environment names one).
//!
//! Resetting removes both, so the environment applies again. The token
//! never leaves the server; the UI only sees its last characters.

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
