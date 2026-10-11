//! HTTP implementation of [`ClickUpApi`] with rate-limit handling.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderMap, AUTHORIZATION};
use reqwest::StatusCode;
use serde_json::Value;

use super::{ClickUpApi, ClickUpError, Result};

/// Production base URL of the ClickUp API v2.
pub const DEFAULT_BASE_URL: &str = "https://api.clickup.com/api/v2";

/// Retry behaviour for rate limits (HTTP 429) and transient failures.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Attempts per request in total (first try included).
    pub max_attempts: u32,
    /// Upper bound for a single wait.
    pub max_wait: Duration,
    /// Wait when a 429 response carries no usable reset header.
    pub fallback_wait: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            max_wait: Duration::from_secs(120),
            fallback_wait: Duration::from_secs(15),
        }
    }
}

/// ClickUp client over HTTP. Authenticates with a personal API token
/// (`Authorization: <token>`, no `Bearer` prefix).
#[derive(Debug, Clone)]
pub struct HttpClickUp {
    http: reqwest::Client,
    base: String,
    token: String,
    retry: RetryPolicy,
}

impl HttpClickUp {
    /// A client for `base_url` (e.g. [`DEFAULT_BASE_URL`]).
    pub fn new(token: impl Into<String>, base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("zunder-import/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(120))
            .build()
            .expect("HTTP client builds");
        Self {
            http,
            base: base_url.trim_end_matches('/').to_owned(),
            token: token.into(),
            retry: RetryPolicy::default(),
        }
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Performs a GET request and returns the JSON body.
    ///
    /// - 429: waits until `X-RateLimit-Reset` (Unix seconds) or `Retry-After`
    ///   and retries.
    /// - 5xx and network errors: retried with exponential backoff.
    /// - 2xx with `X-RateLimit-Remaining: 0`: waits for the reset before
    ///   returning, so the next request does not run into the limit.
    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let url = format!("{}{}", self.base, path);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let response = self
                .http
                .get(&url)
                .query(query)
                .header(AUTHORIZATION, &self.token)
                .send()
                .await;
            let response = match response {
                Ok(r) => r,
                Err(source) if attempt < self.retry.max_attempts => {
                    let wait = backoff(attempt, &self.retry);
                    eprintln!(
                        "Netzwerkfehler bei {path} ({source}), neuer Versuch in {} s …",
                        wait.as_secs()
                    );
                    tokio::time::sleep(wait).await;
                    continue;
                }
                Err(source) => {
                    return Err(ClickUpError::Network {
                        path: path.to_owned(),
                        source,
                    })
                }
            };
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                if attempt >= self.retry.max_attempts {
                    return Err(ClickUpError::RateLimited {
                        path: path.to_owned(),
                        attempts: attempt,
                    });
                }
                let wait = rate_limit_wait(response.headers(), unix_now(), &self.retry);
                eprintln!(
                    "ClickUp-Rate-Limit erreicht, warte {} s …",
                    wait.as_secs_f32().ceil()
                );
                tokio::time::sleep(wait).await;
                continue;
            }
            if status.is_server_error() && attempt < self.retry.max_attempts {
                let wait = backoff(attempt, &self.retry);
                eprintln!(
                    "ClickUp meldet HTTP {} für {path}, neuer Versuch in {} s …",
                    status.as_u16(),
                    wait.as_secs()
                );
                tokio::time::sleep(wait).await;
                continue;
            }
            let pause = exhausted_wait(response.headers(), unix_now(), &self.retry);
            let body = response
                .text()
                .await
                .map_err(|source| ClickUpError::Network {
                    path: path.to_owned(),
                    source,
                })?;
            if status == StatusCode::UNAUTHORIZED {
                return Err(ClickUpError::Unauthorized(body));
            }
            if !status.is_success() {
                return Err(ClickUpError::Status {
                    status: status.as_u16(),
                    path: path.to_owned(),
                    body,
                });
            }
            if let Some(pause) = pause {
                tokio::time::sleep(pause).await;
            }
            return serde_json::from_str(&body).map_err(|e| ClickUpError::InvalidResponse {
                path: path.to_owned(),
                message: e.to_string(),
            });
        }
    }
}

fn archived_query(archived: bool) -> (&'static str, String) {
    ("archived", archived.to_string())
}

impl ClickUpApi for HttpClickUp {
    async fn teams(&self) -> Result<Value> {
        self.get("/team", &[]).await
    }

    async fn spaces(&self, team_id: &str, archived: bool) -> Result<Value> {
        self.get(
            &format!("/team/{team_id}/space"),
            &[archived_query(archived)],
        )
        .await
    }

    async fn folders(&self, space_id: &str, archived: bool) -> Result<Value> {
        self.get(
            &format!("/space/{space_id}/folder"),
            &[archived_query(archived)],
        )
        .await
    }

    async fn folder_lists(&self, folder_id: &str, archived: bool) -> Result<Value> {
        self.get(
            &format!("/folder/{folder_id}/list"),
            &[archived_query(archived)],
        )
        .await
    }

    async fn folderless_lists(&self, space_id: &str, archived: bool) -> Result<Value> {
        self.get(
            &format!("/space/{space_id}/list"),
            &[archived_query(archived)],
        )
        .await
    }

    async fn list(&self, list_id: &str) -> Result<Value> {
        self.get(&format!("/list/{list_id}"), &[]).await
    }

    async fn tasks_page(&self, list_id: &str, page: u32, archived: bool) -> Result<Value> {
        self.get(
            &format!("/list/{list_id}/task"),
            &[
                archived_query(archived),
                ("include_closed", "true".into()),
                ("subtasks", "true".into()),
                ("include_markdown_description", "true".into()),
                ("page", page.to_string()),
            ],
        )
        .await
    }

    async fn task(&self, task_id: &str) -> Result<Option<Value>> {
        match self
            .get(
                &format!("/task/{task_id}"),
                &[("include_markdown_description", "true".into())],
            )
            .await
        {
            Ok(v) => Ok(Some(v)),
            Err(ClickUpError::Status { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn time_entries(&self, team_id: &str, start_ms: i64, end_ms: i64) -> Result<Value> {
        self.get(
            &format!("/team/{team_id}/time_entries"),
            &[
                ("start_date", start_ms.to_string()),
                ("end_date", end_ms.to_string()),
                ("include_task_tags", "true".into()),
                ("include_location_names", "true".into()),
            ],
        )
        .await
    }
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn header_f64(headers: &HeaderMap, name: &str) -> Option<f64> {
    headers
        .get(name)?
        .to_str()
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

/// Time until `X-RateLimit-Reset` (Unix seconds), clamped to
/// `[0, max_wait]`; `None` without a usable header.
fn until_reset(headers: &HeaderMap, now: f64, policy: &RetryPolicy) -> Option<Duration> {
    let reset = header_f64(headers, "x-ratelimit-reset")?;
    let secs = (reset - now).clamp(0.0, policy.max_wait.as_secs_f64());
    Some(Duration::from_secs_f64(secs))
}

/// How long to wait after a 429 response.
fn rate_limit_wait(headers: &HeaderMap, now: f64, policy: &RetryPolicy) -> Duration {
    until_reset(headers, now, policy)
        .or_else(|| {
            header_f64(headers, "retry-after")
                .map(|s| Duration::from_secs_f64(s.clamp(0.0, policy.max_wait.as_secs_f64())))
        })
        .unwrap_or(policy.fallback_wait)
}

/// Pause after a successful response that used up the remaining quota.
fn exhausted_wait(headers: &HeaderMap, now: f64, policy: &RetryPolicy) -> Option<Duration> {
    let remaining = header_f64(headers, "x-ratelimit-remaining")?;
    if remaining > 0.0 {
        return None;
    }
    until_reset(headers, now, policy).filter(|d| !d.is_zero())
}

fn backoff(attempt: u32, policy: &RetryPolicy) -> Duration {
    let secs = 1u64 << attempt.saturating_sub(1).min(6);
    Duration::from_secs(secs).min(policy.max_wait)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn rate_limit_wait_uses_reset_header() {
        let policy = RetryPolicy::default();
        let h = headers(&[("x-ratelimit-reset", "1010")]);
        assert_eq!(
            rate_limit_wait(&h, 1000.0, &policy),
            Duration::from_secs(10)
        );
        // Reset in the past: retry immediately.
        assert_eq!(rate_limit_wait(&h, 2000.0, &policy), Duration::ZERO);
        // Far future: capped.
        let h = headers(&[("x-ratelimit-reset", "999999")]);
        assert_eq!(rate_limit_wait(&h, 0.0, &policy), policy.max_wait);
    }

    #[test]
    fn rate_limit_wait_falls_back() {
        let policy = RetryPolicy::default();
        let h = headers(&[("retry-after", "3")]);
        assert_eq!(rate_limit_wait(&h, 0.0, &policy), Duration::from_secs(3));
        assert_eq!(
            rate_limit_wait(&HeaderMap::new(), 0.0, &policy),
            policy.fallback_wait
        );
    }

    #[test]
    fn exhausted_quota_pauses_until_reset() {
        let policy = RetryPolicy::default();
        let h = headers(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "105")]);
        assert_eq!(
            exhausted_wait(&h, 100.0, &policy),
            Some(Duration::from_secs(5))
        );
        let h = headers(&[
            ("x-ratelimit-remaining", "12"),
            ("x-ratelimit-reset", "105"),
        ]);
        assert_eq!(exhausted_wait(&h, 100.0, &policy), None);
    }
}
