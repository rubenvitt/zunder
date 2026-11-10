//! Status of GitHub pull requests linked from tasks (`pr_url`).
//!
//! The web UI and the app ask `POST /api/pr-status` for the PR links they
//! show; the server reads them in one GraphQL request per batch
//! (`POST /graphql`, one aliased `repository { pullRequest }` per PR) with
//! the token from `ZUNDER_GITHUB_TOKEN` and keeps the answers in memory for
//! a while, so a board full of cards does not hit GitHub on every render.
//!
//! What a PR reports ([`PrStatus`]):
//!
//! - `state`: `draft`, `open`, `merged` or `closed`.
//! - `review` (open PRs only): `approved`, `changes_requested`, `requested`
//!   (review required or reviewers asked) or `None`.
//! - `checks`: rollup of CI checks and statuses of the last commit:
//!   `success`, `failure`, `pending` or `None` (no checks).
//!
//! A PR GitHub does not know or the token may not read answers `None`.
//! Links that are not GitHub PR URLs are never sent to GitHub.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use url::Url;

/// GraphQL endpoint of GitHub.
pub const DEFAULT_GRAPHQL_URL: &str = "https://api.github.com/graphql";
/// Maximum number of links per `POST /api/pr-status`.
pub const MAX_PR_URLS: usize = 200;
/// PRs per GraphQL request (keeps the query cost low).
const BATCH_SIZE: usize = 25;
/// How long an open PR's status is reused.
const TTL_OPEN: Duration = Duration::from_secs(2 * 60);
/// How long a merged or closed PR's status is reused (rarely changes).
const TTL_FINAL: Duration = Duration::from_secs(30 * 60);
/// How long "not found or no access" is reused.
const TTL_MISSING: Duration = Duration::from_secs(10 * 60);
/// Maximum cached entries before old ones are dropped.
const MAX_CACHE_ENTRIES: usize = 2000;

/// A pull request as addressed by its URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u32,
}

impl PrRef {
    /// Parses `https://github.com/{owner}/{repo}/pull/{number}` (any suffix
    /// after the number, such as `/files` or `#discussion`, is ignored).
    pub fn parse(link: &str) -> Option<PrRef> {
        let url = Url::parse(link.trim()).ok()?;
        if !matches!(url.scheme(), "https" | "http") {
            return None;
        }
        let host = url.host_str()?.to_ascii_lowercase();
        if host != "github.com" && host != "www.github.com" {
            return None;
        }
        let mut segments = url.path_segments()?;
        let owner = segments.next().filter(|s| valid_name(s))?;
        let repo = segments.next().filter(|s| valid_name(s))?;
        if segments.next()? != "pull" {
            return None;
        }
        let number = segments.next()?.parse::<u32>().ok().filter(|n| *n > 0)?;
        Some(PrRef {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            number,
        })
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    Draft,
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Requested,
    Approved,
    ChangesRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecksState {
    Pending,
    Success,
    Failure,
}

/// What the UI shows for a PR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrStatus {
    pub number: u32,
    pub title: String,
    pub state: PrState,
    pub review: Option<ReviewState>,
    pub checks: Option<ChecksState>,
}

/// Errors talking to GitHub (messages are German, shown to users).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GithubError {
    #[error("GitHub ist nicht erreichbar: {0}")]
    Transport(String),
    #[error("GitHub antwortet mit Status {status}: {message}")]
    Status { status: u16, message: String },
    #[error("Unerwartete Antwort von GitHub: {0}")]
    InvalidResponse(String),
}

struct CacheEntry {
    at: Instant,
    status: Option<PrStatus>,
}

impl CacheEntry {
    fn fresh(&self, now: Instant) -> bool {
        let ttl = match self.status.as_ref().map(|s| s.state) {
            None => TTL_MISSING,
            Some(PrState::Merged | PrState::Closed) => TTL_FINAL,
            Some(PrState::Open | PrState::Draft) => TTL_OPEN,
        };
        now.duration_since(self.at) < ttl
    }
}

/// GraphQL client with a small in-memory cache.
pub struct GithubClient {
    http: reqwest::Client,
    endpoint: Url,
    token: String,
    cache: Mutex<HashMap<PrRef, CacheEntry>>,
}

impl std::fmt::Debug for GithubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubClient")
            .field("endpoint", &self.endpoint.as_str())
            .finish_non_exhaustive()
    }
}

impl GithubClient {
    pub fn new(http: reqwest::Client, endpoint: &Url, token: impl Into<String>) -> Self {
        Self {
            http,
            endpoint: endpoint.clone(),
            token: token.into(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The status of each PR (`None`: unknown to GitHub or not readable).
    /// Cached answers are reused; the rest is asked in batches. When GitHub
    /// fails, the error is returned and nothing is cached.
    pub async fn statuses(
        &self,
        refs: &[PrRef],
    ) -> Result<HashMap<PrRef, Option<PrStatus>>, GithubError> {
        let now = Instant::now();
        let mut out = HashMap::new();
        let mut missing = Vec::new();
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            for r in refs {
                if out.contains_key(r) || missing.contains(r) {
                    continue;
                }
                match cache.get(r) {
                    Some(entry) if entry.fresh(now) => {
                        out.insert(r.clone(), entry.status.clone());
                    }
                    _ => missing.push(r.clone()),
                }
            }
        }
        for chunk in missing.chunks(BATCH_SIZE) {
            let fetched = self.fetch(chunk).await?;
            let at = Instant::now();
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if cache.len() + fetched.len() > MAX_CACHE_ENTRIES {
                cache.retain(|_, e| e.fresh(at));
                if cache.len() + fetched.len() > MAX_CACHE_ENTRIES {
                    cache.clear();
                }
            }
            for (r, status) in fetched {
                cache.insert(
                    r.clone(),
                    CacheEntry {
                        at,
                        status: status.clone(),
                    },
                );
                out.insert(r, status);
            }
        }
        Ok(out)
    }

    async fn fetch(&self, refs: &[PrRef]) -> Result<Vec<(PrRef, Option<PrStatus>)>, GithubError> {
        let (query, variables) = build_query(refs);
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(|e| GithubError::Transport(e.without_url().to_string()))?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| GithubError::InvalidResponse(e.without_url().to_string()))?;
        if !status.is_success() {
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("ohne Meldung")
                .to_owned();
            return Err(GithubError::Status {
                status: status.as_u16(),
                message,
            });
        }
        parse_response(refs, &body)
    }
}

const PR_FIELDS: &str = "number title state isDraft reviewDecision \
reviewRequests(first: 1) { totalCount } \
commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }";

/// One aliased field per PR (`p0`, `p1`, …); owner, name and number travel
/// as variables, never inside the query text.
fn build_query(refs: &[PrRef]) -> (String, Map<String, Value>) {
    let mut params = Vec::new();
    let mut fields = String::new();
    let mut variables = Map::new();
    for (i, r) in refs.iter().enumerate() {
        params.push(format!("$o{i}: String!, $r{i}: String!, $n{i}: Int!"));
        fields.push_str(&format!(
            "p{i}: repository(owner: $o{i}, name: $r{i}) {{ pullRequest(number: $n{i}) {{ {PR_FIELDS} }} }} "
        ));
        variables.insert(format!("o{i}"), json!(r.owner));
        variables.insert(format!("r{i}"), json!(r.repo));
        variables.insert(format!("n{i}"), json!(r.number));
    }
    (
        format!("query({}) {{ {fields}}}", params.join(", ")),
        variables,
    )
}

#[derive(Deserialize)]
struct RawPr {
    number: u32,
    title: String,
    state: String,
    #[serde(rename = "isDraft", default)]
    is_draft: bool,
    #[serde(rename = "reviewDecision")]
    review_decision: Option<String>,
    #[serde(rename = "reviewRequests")]
    review_requests: Option<TotalCount>,
    commits: Option<Commits>,
}

#[derive(Deserialize)]
struct TotalCount {
    #[serde(rename = "totalCount")]
    total_count: u32,
}

#[derive(Deserialize)]
struct Commits {
    nodes: Vec<Option<CommitNode>>,
}

#[derive(Deserialize)]
struct CommitNode {
    commit: CommitInfo,
}

#[derive(Deserialize)]
struct CommitInfo {
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<Rollup>,
}

#[derive(Deserialize)]
struct Rollup {
    state: String,
}

/// Reads the GraphQL answer. Missing repositories or PRs come back as
/// `null` with an entry in `errors` (type `NOT_FOUND`); they map to `None`.
/// Other errors without any data (bad token scopes, rate limit) fail.
fn parse_response(
    refs: &[PrRef],
    body: &Value,
) -> Result<Vec<(PrRef, Option<PrStatus>)>, GithubError> {
    let data = body.get("data").filter(|d| d.is_object());
    let Some(data) = data else {
        let message = body
            .get("errors")
            .and_then(Value::as_array)
            .and_then(|e| e.first())
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("keine Daten");
        return Err(GithubError::InvalidResponse(message.to_owned()));
    };
    let mut out = Vec::with_capacity(refs.len());
    for (i, r) in refs.iter().enumerate() {
        let raw = data
            .get(format!("p{i}"))
            .and_then(|repo| repo.get("pullRequest"))
            .filter(|pr| !pr.is_null());
        let status = match raw {
            None => None,
            Some(pr) => {
                let raw: RawPr = serde_json::from_value(pr.clone())
                    .map_err(|e| GithubError::InvalidResponse(e.to_string()))?;
                Some(raw.into_status())
            }
        };
        out.push((r.clone(), status));
    }
    Ok(out)
}

impl RawPr {
    fn into_status(self) -> PrStatus {
        let state = match self.state.as_str() {
            "MERGED" => PrState::Merged,
            "CLOSED" => PrState::Closed,
            _ if self.is_draft => PrState::Draft,
            _ => PrState::Open,
        };
        let review = if state == PrState::Open || state == PrState::Draft {
            match self.review_decision.as_deref() {
                Some("APPROVED") => Some(ReviewState::Approved),
                Some("CHANGES_REQUESTED") => Some(ReviewState::ChangesRequested),
                Some("REVIEW_REQUIRED") => Some(ReviewState::Requested),
                _ if self.review_requests.is_some_and(|r| r.total_count > 0) => {
                    Some(ReviewState::Requested)
                }
                _ => None,
            }
        } else {
            None
        };
        let checks = self
            .commits
            .and_then(|c| c.nodes.into_iter().flatten().next())
            .and_then(|n| n.commit.status_check_rollup)
            .and_then(|r| match r.state.as_str() {
                "SUCCESS" => Some(ChecksState::Success),
                "FAILURE" | "ERROR" => Some(ChecksState::Failure),
                "PENDING" | "EXPECTED" => Some(ChecksState::Pending),
                _ => None,
            });
        PrStatus {
            number: self.number,
            title: self.title,
            state,
            review,
            checks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(owner: &str, repo: &str, number: u32) -> PrRef {
        PrRef {
            owner: owner.into(),
            repo: repo.into(),
            number,
        }
    }

    #[test]
    fn parses_pr_links() {
        assert_eq!(
            PrRef::parse("https://github.com/rubenvitt/zunder/pull/42"),
            Some(pr("rubenvitt", "zunder", 42))
        );
        assert_eq!(
            PrRef::parse(" https://www.github.com/a-b/c.d_e/pull/7/files#diff "),
            Some(pr("a-b", "c.d_e", 7))
        );
        for bad in [
            "",
            "github.com/a/b/pull/1",
            "https://gitlab.com/a/b/pull/1",
            "https://github.com/a/b/issues/1",
            "https://github.com/a/b/pull/",
            "https://github.com/a/b/pull/0",
            "https://github.com/a/b/pull/x",
            "https://github.com/a/b",
            "ftp://github.com/a/b/pull/1",
            "https://github.com/a%22/b/pull/1",
        ] {
            assert_eq!(PrRef::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn query_uses_variables() {
        let (query, vars) = build_query(&[pr("o", "r", 1), pr("x", "y", 2)]);
        assert!(query.starts_with("query($o0: String!, $r0: String!, $n0: Int!, $o1:"));
        assert!(query.contains("p1: repository(owner: $o1, name: $r1)"));
        assert_eq!(vars["o1"], json!("x"));
        assert_eq!(vars["n1"], json!(2));
    }

    #[test]
    fn maps_states() {
        let refs = [
            pr("o", "r", 1),
            pr("o", "r", 2),
            pr("o", "r", 3),
            pr("o", "r", 4),
            pr("o", "r", 5),
        ];
        let body = json!({
            "data": {
                "p0": { "pullRequest": {
                    "number": 1, "title": "Erster", "state": "OPEN", "isDraft": false,
                    "reviewDecision": "APPROVED", "reviewRequests": { "totalCount": 0 },
                    "commits": { "nodes": [ { "commit": { "statusCheckRollup": { "state": "FAILURE" } } } ] }
                } },
                "p1": { "pullRequest": {
                    "number": 2, "title": "Entwurf", "state": "OPEN", "isDraft": true,
                    "reviewDecision": null, "reviewRequests": { "totalCount": 2 },
                    "commits": { "nodes": [ { "commit": { "statusCheckRollup": null } } ] }
                } },
                "p2": { "pullRequest": {
                    "number": 3, "title": "Fertig", "state": "MERGED", "isDraft": false,
                    "reviewDecision": "APPROVED", "reviewRequests": { "totalCount": 0 },
                    "commits": { "nodes": [ { "commit": { "statusCheckRollup": { "state": "SUCCESS" } } } ] }
                } },
                "p3": null,
                "p4": { "pullRequest": null }
            },
            "errors": [ { "type": "NOT_FOUND", "message": "Could not resolve" } ]
        });
        let got = parse_response(&refs, &body).unwrap();
        assert_eq!(
            got[0].1,
            Some(PrStatus {
                number: 1,
                title: "Erster".into(),
                state: PrState::Open,
                review: Some(ReviewState::Approved),
                checks: Some(ChecksState::Failure),
            })
        );
        let draft = got[1].1.as_ref().unwrap();
        assert_eq!(draft.state, PrState::Draft);
        assert_eq!(draft.review, Some(ReviewState::Requested));
        assert_eq!(draft.checks, None);
        let merged = got[2].1.as_ref().unwrap();
        assert_eq!(merged.state, PrState::Merged);
        assert_eq!(merged.review, None);
        assert_eq!(merged.checks, Some(ChecksState::Success));
        assert_eq!(got[3].1, None);
        assert_eq!(got[4].1, None);
    }

    #[test]
    fn errors_without_data_fail() {
        let body = json!({ "errors": [ { "message": "Bad credentials" } ] });
        assert_eq!(
            parse_response(&[pr("o", "r", 1)], &body),
            Err(GithubError::InvalidResponse("Bad credentials".into()))
        );
    }

    #[test]
    fn serializes_snake_case() {
        let status = PrStatus {
            number: 9,
            title: "T".into(),
            state: PrState::Open,
            review: Some(ReviewState::ChangesRequested),
            checks: Some(ChecksState::Pending),
        };
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            json!({"number": 9, "title": "T", "state": "open", "review": "changes_requested", "checks": "pending"})
        );
    }
}
