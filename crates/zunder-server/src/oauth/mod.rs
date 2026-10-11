//! Built-in OAuth 2.1 authorization server for `/mcp` (MCP Authorization
//! specification), so that clients that cannot send a fixed bearer header,
//! such as claude.ai custom connectors, can connect.
//!
//! - Zunder is resource server **and** authorization server. The protected
//!   resource is `<public>/mcp`, the issuer is `<public>`.
//! - Discovery: protected resource metadata (RFC 9728) and authorization
//!   server metadata (RFC 8414), see [`routes`].
//! - Clients register themselves (RFC 7591) as public clients
//!   (`token_endpoint_auth_method: none`); redirect URIs must be https or
//!   http on a loopback host (port-agnostic match, RFC 8252).
//! - The owner authenticates through the existing browser session (OIDC
//!   login, dev auth locally) and confirms a German consent page.
//! - Authorization codes are single use, live [`CODE_TTL_MS`] and are bound
//!   to client, redirect URI, PKCE challenge (S256 only) and resource
//!   (RFC 8707). Replaying a code revokes the tokens issued for it.
//! - Access tokens (`zoa_…`) are opaque, stored as SHA-256, live
//!   [`ACCESS_TOKEN_TTL_MS`] and are only accepted by `/mcp` with the
//!   matching resource (audience). Refresh tokens (`zor_…`) live
//!   [`REFRESH_TOKEN_TTL_MS`] and rotate; replaying a rotated refresh token
//!   revokes the whole family.
//! - Operations from OAuth clients carry the source `mcp:oauth:<client_name>`.
//!
//! API tokens (`zt_…`) keep working unchanged for `/mcp` and `/api`.

pub mod routes;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{json, Value};
use url::Url;
use uuid::Uuid;

use crate::app::{format_ms, now_ms, App};
use crate::auth::{self, Principal, PrincipalKind};
use crate::config::Config;
use crate::db::{self, OAuthClientRow, OAuthCodeRow, OAuthTokenKind, OAuthTokenRow};
use crate::error::{AppError, AppResult};

/// Prefix of OAuth access tokens.
pub const ACCESS_TOKEN_PREFIX: &str = "zoa_";
/// Prefix of OAuth refresh tokens.
pub const REFRESH_TOKEN_PREFIX: &str = "zor_";
/// Prefix of authorization codes.
pub const CODE_PREFIX: &str = "zoc_";

const MINUTE_MS: i64 = 60 * 1000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

/// Lifetime of an authorization code.
pub const CODE_TTL_MS: i64 = 5 * MINUTE_MS;
/// Lifetime of an access token.
pub const ACCESS_TOKEN_TTL_MS: i64 = HOUR_MS;
/// Lifetime of a refresh token (renewed with every rotation).
pub const REFRESH_TOKEN_TTL_MS: i64 = 30 * DAY_MS;
/// Registered clients without a live grant are removed after this time.
pub const STALE_CLIENT_MS: i64 = DAY_MS;
/// At most this many client registrations per hour.
pub const MAX_REGISTRATIONS_PER_HOUR: i64 = 30;
/// At most this many registered clients in total.
pub const MAX_CLIENTS: i64 = 500;
/// At most this many redirect URIs per client.
pub const MAX_REDIRECT_URIS: usize = 10;
/// Maximum length of a redirect URI.
pub const MAX_REDIRECT_URI_LEN: usize = 2000;
/// Maximum length of a client name (longer names are rejected).
pub const MAX_CLIENT_NAME_CHARS: usize = 100;
/// Name used when a client registers without `client_name`.
pub const DEFAULT_CLIENT_NAME: &str = "Unbenannter Client";
/// Maximum length of `state` and `scope`.
pub const MAX_STATE_LEN: usize = 2048;
pub const MAX_SCOPE_LEN: usize = 1000;
/// `last_used_at` of a client is written at most this often.
const CLIENT_TOUCH_INTERVAL_MS: i64 = MINUTE_MS;

// ----- identifiers --------------------------------------------------------------

/// The issuer: the public URL without trailing slash.
pub fn issuer(config: &Config) -> String {
    config.public_url.as_str().trim_end_matches('/').to_owned()
}

/// The protected resource (canonical URI of the MCP server).
pub fn mcp_resource(config: &Config) -> String {
    format!("{}/mcp", issuer(config))
}

/// URL of the protected resource metadata (named in `WWW-Authenticate`).
pub fn resource_metadata_url(config: &Config) -> String {
    format!("{}/.well-known/oauth-protected-resource", issuer(config))
}

/// Canonical form of a resource indicator for comparison: lowercase scheme
/// and host (done by the URL parser), default port dropped, no trailing
/// slash. Fragments, queries and user info are not allowed.
pub fn normalize_resource(raw: &str) -> Option<String> {
    let url = Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.fragment().is_some()
        || url.query().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let host = url.host_str()?;
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    Some(format!(
        "{}://{}{}{}",
        url.scheme(),
        host,
        port,
        url.path().trim_end_matches('/')
    ))
}

/// The canonical resource this server protects.
fn expected_resource(config: &Config) -> String {
    normalize_resource(&mcp_resource(config)).expect("public URL is a valid http(s) URL")
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Checks a redirect URI at registration: https with a host, or http on a
/// loopback host (local tools); no fragment, no user info.
pub fn validate_redirect_uri(raw: &str) -> Result<(), String> {
    if raw.len() > MAX_REDIRECT_URI_LEN {
        return Err(format!(
            "Redirect-URI ist länger als {MAX_REDIRECT_URI_LEN} Zeichen"
        ));
    }
    let url = Url::parse(raw).map_err(|_| format!("Redirect-URI ist keine URL: {raw}"))?;
    if url.fragment().is_some() {
        return Err("Redirect-URI darf kein Fragment enthalten".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Redirect-URI darf keine Zugangsdaten enthalten".into());
    }
    match url.scheme() {
        "https" if url.host_str().is_some() => Ok(()),
        "http" if is_loopback_host(&url) => Ok(()),
        _ => Err(format!(
            "Redirect-URI muss https sein (oder http auf localhost/127.0.0.1): {raw}"
        )),
    }
}

/// Whether `requested` matches a registered redirect URI: exact string
/// match, or for http loopback URIs the same host, path and query with any
/// port (RFC 8252, section 7.3).
pub fn redirect_uri_matches(registered: &str, requested: &str) -> bool {
    if registered == requested {
        return true;
    }
    let (Ok(r), Ok(q)) = (Url::parse(registered), Url::parse(requested)) else {
        return false;
    };
    r.scheme() == "http"
        && q.scheme() == "http"
        && is_loopback_host(&r)
        && r.host_str() == q.host_str()
        && r.path() == q.path()
        && r.query() == q.query()
        && q.fragment().is_none()
        && q.username().is_empty()
        && q.password().is_none()
}

/// Whether a redirect URI points at the local machine.
pub fn is_loopback_redirect(uri: &str) -> bool {
    Url::parse(uri).is_ok_and(|u| is_loopback_host(&u))
}

/// PKCE code challenge / verifier syntax (RFC 7636): 43–128 characters of
/// `[A-Za-z0-9-._~]`.
pub fn is_pkce_value(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

fn new_secret(prefix: &str) -> String {
    format!("{prefix}{}", auth::random_secret())
}

// ----- errors -------------------------------------------------------------------

/// An error of the token, registration or revocation endpoint in the OAuth
/// form `{"error","error_description"}`.
#[derive(Debug)]
pub struct OAuthError {
    pub status: StatusCode,
    pub error: &'static str,
    pub description: String,
}

impl OAuthError {
    pub fn new(status: StatusCode, error: &'static str, description: impl Into<String>) -> Self {
        Self {
            status,
            error,
            description: description.into(),
        }
    }

    pub fn invalid_request(description: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", description)
    }

    pub fn invalid_grant(description: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_grant", description)
    }

    pub fn invalid_client(description: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "invalid_client", description)
    }

    pub fn invalid_target(description: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_target", description)
    }

    pub fn invalid_client_metadata(description: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            description,
        )
    }

    pub fn invalid_redirect_uri(description: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_redirect_uri", description)
    }
}

impl From<AppError> for OAuthError {
    fn from(e: AppError) -> Self {
        if let AppError::Internal(inner) = &e {
            tracing::error!(error = ?inner, "internal error in OAuth endpoint");
        }
        OAuthError::new(e.status(), "server_error", e.to_string())
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let body = json!({"error": self.error, "error_description": self.description});
        no_store((self.status, axum::Json(body)).into_response())
    }
}

/// Adds `Cache-Control: no-store` and `Pragma: no-cache` (RFC 6749, 5.1).
pub fn no_store(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        axum::http::header::PRAGMA,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    response
}

// ----- registration -------------------------------------------------------------

/// Response of a successful registration (RFC 7591, section 3.2.1).
#[derive(Debug, Clone, Serialize)]
pub struct RegisteredClient {
    pub client_id: String,
    pub client_id_issued_at: i64,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: [&'static str; 2],
    pub response_types: [&'static str; 1],
    pub token_endpoint_auth_method: &'static str,
}

fn string_list(value: Option<&Value>, field: &str) -> Result<Option<Vec<String>>, OAuthError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str().map(str::to_owned).ok_or_else(|| {
                    OAuthError::invalid_client_metadata(format!("{field} muss Texte enthalten"))
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(OAuthError::invalid_client_metadata(format!(
            "{field} muss eine Liste sein"
        ))),
    }
}

/// Validates client metadata and registers a public client. Clients may
/// ask for another `token_endpoint_auth_method`; they are registered as
/// public clients regardless and the response says so (RFC 7591, 3.2.1).
pub async fn register(app: &App, metadata: Value) -> Result<RegisteredClient, OAuthError> {
    let Value::Object(meta) = metadata else {
        return Err(OAuthError::invalid_client_metadata(
            "Die Registrierung muss ein JSON-Objekt sein",
        ));
    };
    let redirect_uris = string_list(meta.get("redirect_uris"), "redirect_uris")?
        .filter(|uris| !uris.is_empty())
        .ok_or_else(|| OAuthError::invalid_redirect_uri("redirect_uris fehlt"))?;
    if redirect_uris.len() > MAX_REDIRECT_URIS {
        return Err(OAuthError::invalid_redirect_uri(format!(
            "Höchstens {MAX_REDIRECT_URIS} Redirect-URIs"
        )));
    }
    for uri in &redirect_uris {
        validate_redirect_uri(uri).map_err(OAuthError::invalid_redirect_uri)?;
    }
    if let Some(grants) = string_list(meta.get("grant_types"), "grant_types")? {
        if !grants.iter().any(|g| g == "authorization_code")
            || grants
                .iter()
                .any(|g| g != "authorization_code" && g != "refresh_token")
        {
            return Err(OAuthError::invalid_client_metadata(
                "Unterstützt werden nur die grant_types authorization_code und refresh_token",
            ));
        }
    }
    if let Some(types) = string_list(meta.get("response_types"), "response_types")? {
        if types.iter().any(|t| t != "code") {
            return Err(OAuthError::invalid_client_metadata(
                "Unterstützt wird nur der response_type code",
            ));
        }
    }
    let name = match meta.get("client_name") {
        None | Some(Value::Null) => DEFAULT_CLIENT_NAME.to_owned(),
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.chars().count() > MAX_CLIENT_NAME_CHARS {
                return Err(OAuthError::invalid_client_metadata(format!(
                    "client_name darf höchstens {MAX_CLIENT_NAME_CHARS} Zeichen lang sein"
                )));
            }
            if s.chars().any(char::is_control) {
                return Err(OAuthError::invalid_client_metadata(
                    "client_name darf keine Steuerzeichen enthalten",
                ));
            }
            if s.is_empty() {
                DEFAULT_CLIENT_NAME.to_owned()
            } else {
                s.to_owned()
            }
        }
        Some(_) => {
            return Err(OAuthError::invalid_client_metadata(
                "client_name muss ein Text sein",
            ))
        }
    };

    let now = now_ms();
    let row = OAuthClientRow {
        id: zunder_core::ids::new_id().to_string(),
        name,
        redirect_uris,
        created_at: now,
        last_used_at: None,
        revoked_at: None,
    };
    let stored = row.clone();
    app.with_writer(move |conn| {
        // Unused registrations do not pile up: clients without a live grant
        // disappear after a day.
        db::purge_oauth(conn, now, now - STALE_CLIENT_MS)?;
        let recent = db::count_oauth_clients_since(conn, now - HOUR_MS)?;
        let total = db::count_oauth_clients_since(conn, 0)?;
        if recent >= MAX_REGISTRATIONS_PER_HOUR || total >= MAX_CLIENTS {
            return Ok(false);
        }
        db::insert_oauth_client(conn, &stored)?;
        Ok(true)
    })
    .await
    .map_err(OAuthError::from)
    .and_then(|ok| {
        if ok {
            Ok(())
        } else {
            Err(OAuthError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "temporarily_unavailable",
                "Zu viele Client-Registrierungen, bitte später erneut versuchen",
            ))
        }
    })?;
    tracing::info!(client_id = %row.id, client_name = %row.name, "OAuth client registered");
    Ok(RegisteredClient {
        client_id: row.id,
        client_id_issued_at: now / 1000,
        client_name: row.name,
        redirect_uris: row.redirect_uris,
        grant_types: ["authorization_code", "refresh_token"],
        response_types: ["code"],
        token_endpoint_auth_method: "none",
    })
}

/// A registered, not revoked client.
pub async fn active_client(app: &App, id: &str) -> AppResult<Option<OAuthClientRow>> {
    if id.is_empty() || id.len() > 100 {
        return Ok(None);
    }
    let id = id.to_owned();
    let found = app
        .with_reader(move |conn| Ok(db::oauth_client(conn, &id)?))
        .await?;
    Ok(found.filter(|c| c.revoked_at.is_none()))
}

// ----- authorization codes --------------------------------------------------------

/// Everything an authorization code is bound to.
#[derive(Debug, Clone)]
pub struct CodeGrant {
    pub client_id: String,
    pub user_id: Uuid,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub resource: String,
    pub scope: Option<String>,
}

/// Issues a single-use authorization code (returned raw, stored hashed).
pub async fn issue_code(app: &App, grant: CodeGrant) -> AppResult<String> {
    let code = new_secret(CODE_PREFIX);
    let hash = auth::sha256_hex(&code);
    let now = now_ms();
    let row = OAuthCodeRow {
        client_id: grant.client_id,
        user_id: grant.user_id,
        redirect_uri: grant.redirect_uri,
        code_challenge: grant.code_challenge,
        resource: grant.resource,
        scope: grant.scope,
        family_id: zunder_core::ids::new_id().to_string(),
        created_at: now,
        expires_at: now + CODE_TTL_MS,
        used_at: None,
    };
    app.with_writer(move |conn| Ok(db::insert_oauth_code(conn, &hash, &row)?))
        .await?;
    Ok(code)
}

// ----- tokens ---------------------------------------------------------------------

/// Successful token response (RFC 6749, section 5.1).
#[derive(Debug, Clone, Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
    pub refresh_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

struct Family<'a> {
    id: &'a str,
    client_id: &'a str,
    user_id: Uuid,
    resource: &'a str,
    scope: Option<&'a str>,
}

/// Stores a new access/refresh token pair of a family.
fn issue_pair(
    conn: &rusqlite::Connection,
    family: &Family<'_>,
    now: i64,
) -> AppResult<TokenResponse> {
    let access = new_secret(ACCESS_TOKEN_PREFIX);
    let refresh = new_secret(REFRESH_TOKEN_PREFIX);
    for (raw, kind, ttl) in [
        (&access, OAuthTokenKind::Access, ACCESS_TOKEN_TTL_MS),
        (&refresh, OAuthTokenKind::Refresh, REFRESH_TOKEN_TTL_MS),
    ] {
        db::insert_oauth_token(
            conn,
            &auth::sha256_hex(raw),
            &OAuthTokenRow {
                id: zunder_core::ids::new_id().to_string(),
                kind,
                client_id: family.client_id.to_owned(),
                user_id: family.user_id,
                family_id: family.id.to_owned(),
                resource: family.resource.to_owned(),
                scope: family.scope.map(str::to_owned),
                created_at: now,
                expires_at: now + ttl,
                used_at: None,
                revoked_at: None,
            },
        )?;
    }
    Ok(TokenResponse {
        access_token: access,
        token_type: "Bearer",
        expires_in: ACCESS_TOKEN_TTL_MS / 1000,
        refresh_token: refresh,
        scope: family.scope.map(str::to_owned),
    })
}

/// Checks an optional `resource` parameter of a token request against the
/// resource of the grant.
fn check_resource(requested: Option<&str>, granted: &str) -> Result<(), OAuthError> {
    match requested {
        None => Ok(()),
        Some(raw) if normalize_resource(raw).as_deref() == Some(granted) => Ok(()),
        Some(_) => Err(OAuthError::invalid_target(
            "resource passt nicht zur Autorisierung",
        )),
    }
}

/// Parameters of the `authorization_code` grant.
#[derive(Debug, Clone)]
pub struct CodeExchange {
    pub client_id: String,
    pub code: String,
    pub redirect_uri: Option<String>,
    pub code_verifier: String,
    pub resource: Option<String>,
}

/// Redeems an authorization code: single use (a replay revokes the tokens
/// issued for the code), bound to client, redirect URI, PKCE and resource.
pub async fn exchange_code(app: &App, req: CodeExchange) -> Result<TokenResponse, OAuthError> {
    if !is_pkce_value(&req.code_verifier) {
        return Err(OAuthError::invalid_request(
            "code_verifier fehlt oder ist ungültig",
        ));
    }
    let hash = auth::sha256_hex(&req.code);
    let now = now_ms();
    app.with_writer(move |conn| {
        let tx = conn.transaction()?;
        let Some(code) = db::oauth_code(&tx, &hash)? else {
            return Ok(Err(OAuthError::invalid_grant(
                "Unbekannter Autorisierungscode",
            )));
        };
        // Single use: any redemption attempt consumes the code.
        if !db::mark_oauth_code_used(&tx, &hash, now)? {
            let revoked = db::revoke_oauth_family(&tx, &code.family_id, now)?;
            tx.commit()?;
            tracing::warn!(
                client_id = %code.client_id,
                revoked,
                "OAuth authorization code replayed; tokens revoked"
            );
            return Ok(Err(OAuthError::invalid_grant(
                "Der Autorisierungscode wurde bereits eingelöst",
            )));
        }
        let outcome = (|| {
            if code.expires_at <= now {
                return Err(OAuthError::invalid_grant(
                    "Der Autorisierungscode ist abgelaufen",
                ));
            }
            if code.client_id != req.client_id {
                return Err(OAuthError::invalid_grant(
                    "Der Autorisierungscode gehört zu einem anderen Client",
                ));
            }
            let client = db::oauth_client(&tx, &code.client_id).map_err(AppError::Internal)?;
            if client.is_none_or(|c| c.revoked_at.is_some()) {
                return Err(OAuthError::invalid_client("Der Client ist widerrufen"));
            }
            if req.redirect_uri.as_deref() != Some(code.redirect_uri.as_str()) {
                return Err(OAuthError::invalid_grant(
                    "redirect_uri passt nicht zur Autorisierung",
                ));
            }
            let challenge = auth::pkce_challenge(&req.code_verifier);
            if !auth::constant_time_eq(challenge.as_bytes(), code.code_challenge.as_bytes()) {
                return Err(OAuthError::invalid_grant("PKCE-Prüfung fehlgeschlagen"));
            }
            check_resource(req.resource.as_deref(), &code.resource)?;
            Ok(issue_pair(
                &tx,
                &Family {
                    id: &code.family_id,
                    client_id: &code.client_id,
                    user_id: code.user_id,
                    resource: &code.resource,
                    scope: code.scope.as_deref(),
                },
                now,
            )?)
        })();
        tx.commit()?;
        Ok(outcome)
    })
    .await
    .map_err(OAuthError::from)?
}

/// Parameters of the `refresh_token` grant.
#[derive(Debug, Clone)]
pub struct RefreshRequest {
    /// Optional for public clients; must match when present.
    pub client_id: Option<String>,
    pub refresh_token: String,
    pub resource: Option<String>,
}

/// Rotates a refresh token. Replaying an already rotated refresh token
/// revokes the whole family (all access and refresh tokens of the grant).
pub async fn refresh(app: &App, req: RefreshRequest) -> Result<TokenResponse, OAuthError> {
    if !req.refresh_token.starts_with(REFRESH_TOKEN_PREFIX) {
        return Err(OAuthError::invalid_grant("Unbekannter Refresh-Token"));
    }
    let hash = auth::sha256_hex(&req.refresh_token);
    let now = now_ms();
    app.with_writer(move |conn| {
        let tx = conn.transaction()?;
        let found = db::oauth_token(&tx, &hash)?;
        let Some((token, client)) = found.filter(|(t, _)| t.kind == OAuthTokenKind::Refresh)
        else {
            return Ok(Err(OAuthError::invalid_grant("Unbekannter Refresh-Token")));
        };
        if req.client_id.as_deref().is_some_and(|id| id != token.client_id) {
            return Ok(Err(OAuthError::invalid_grant(
                "Der Refresh-Token gehört zu einem anderen Client",
            )));
        }
        if token.used_at.is_some() {
            let revoked = db::revoke_oauth_family(&tx, &token.family_id, now)?;
            tx.commit()?;
            tracing::warn!(
                client_id = %token.client_id,
                revoked,
                "OAuth refresh token reused; token family revoked"
            );
            return Ok(Err(OAuthError::invalid_grant(
                "Der Refresh-Token wurde bereits verwendet; die Verbindung wurde aus Sicherheitsgründen getrennt",
            )));
        }
        if token.revoked_at.is_some() || client.revoked_at.is_some() {
            return Ok(Err(OAuthError::invalid_grant("Der Zugriff wurde widerrufen")));
        }
        if token.expires_at <= now {
            return Ok(Err(OAuthError::invalid_grant(
                "Der Refresh-Token ist abgelaufen",
            )));
        }
        if let Err(e) = check_resource(req.resource.as_deref(), &token.resource) {
            return Ok(Err(e));
        }
        if !db::mark_oauth_token_used(&tx, &token.id, now)? {
            return Ok(Err(OAuthError::invalid_grant(
                "Der Refresh-Token wurde bereits verwendet",
            )));
        }
        let pair = issue_pair(
            &tx,
            &Family {
                id: &token.family_id,
                client_id: &token.client_id,
                user_id: token.user_id,
                resource: &token.resource,
                scope: token.scope.as_deref(),
            },
            now,
        )?;
        tx.commit()?;
        Ok(Ok(pair))
    })
    .await
    .map_err(OAuthError::from)?
}

/// Token revocation (RFC 7009): a refresh token revokes its family, an
/// access token only itself. Unknown tokens and tokens of other clients are
/// silently ignored (the endpoint always answers 200).
pub async fn revoke(app: &App, token: &str, client_id: Option<&str>) -> AppResult<()> {
    let hash = auth::sha256_hex(token);
    let client_id = client_id.map(str::to_owned);
    let now = now_ms();
    app.with_writer(move |conn| {
        let Some((row, _)) = db::oauth_token(conn, &hash)? else {
            return Ok(());
        };
        if client_id.is_some_and(|id| id != row.client_id) {
            return Ok(());
        }
        match row.kind {
            OAuthTokenKind::Refresh => {
                db::revoke_oauth_family(conn, &row.family_id, now)?;
            }
            OAuthTokenKind::Access => db::revoke_oauth_token(conn, &row.id, now)?,
        }
        Ok(())
    })
    .await
}

/// Authenticates an OAuth access token for `/mcp`: known, not revoked, not
/// expired, client not revoked, and issued for this server's resource.
pub async fn authenticate_access(app: &App, raw: &str) -> AppResult<Option<Principal>> {
    if !raw.starts_with(ACCESS_TOKEN_PREFIX) {
        return Ok(None);
    }
    let hash = auth::sha256_hex(raw);
    let expected = expected_resource(app.config());
    let now = now_ms();
    let found = app
        .with_reader(move |conn| Ok(db::oauth_token(conn, &hash)?))
        .await?;
    let Some((token, client)) = found else {
        return Ok(None);
    };
    let valid = token.kind == OAuthTokenKind::Access
        && token.revoked_at.is_none()
        && client.revoked_at.is_none()
        && token.expires_at > now
        && token.resource == expected;
    if !valid {
        return Ok(None);
    }
    if client
        .last_used_at
        .is_none_or(|t| now - t >= CLIENT_TOUCH_INTERVAL_MS)
    {
        let id = client.id.clone();
        app.with_writer(move |conn| Ok(db::touch_oauth_client(conn, &id, now)?))
            .await?;
    }
    Ok(Some(Principal {
        user_id: token.user_id,
        kind: PrincipalKind::OAuth {
            source_name: format!("oauth:{}", client.name),
            client_id: client.id,
        },
    }))
}

// ----- management (token page) ----------------------------------------------------

/// A connected OAuth client as listed on the token page.
#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    pub id: String,
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// Clients holding a live grant of the user.
pub async fn connected_clients(app: &App, user_id: Uuid) -> AppResult<Vec<ClientInfo>> {
    let now = now_ms();
    let rows = app
        .with_reader(move |conn| Ok(db::connected_oauth_clients(conn, user_id, now)?))
        .await?;
    Ok(rows
        .into_iter()
        .map(|c| ClientInfo {
            id: c.id,
            name: c.name,
            redirect_uris: c.redirect_uris,
            created_at: format_ms(c.created_at),
            last_used_at: c.last_used_at.map(format_ms),
        })
        .collect())
}

/// Revokes a client and all its tokens; `NotFound` if the user has no such
/// client.
pub async fn revoke_client(app: &App, user_id: Uuid, id: &str) -> AppResult<()> {
    let id = id.to_owned();
    let revoked = app
        .with_writer(move |conn| Ok(db::revoke_oauth_client(conn, user_id, &id, now_ms())?))
        .await?;
    if revoked {
        Ok(())
    } else {
        Err(AppError::NotFound(
            "Verbundener Client nicht gefunden".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_are_normalized() {
        assert_eq!(
            normalize_resource("https://Zunder.Example.org/mcp/").as_deref(),
            Some("https://zunder.example.org/mcp")
        );
        assert_eq!(
            normalize_resource("HTTPS://zunder.example.org:443/mcp").as_deref(),
            Some("https://zunder.example.org/mcp")
        );
        assert_eq!(
            normalize_resource("http://localhost:8080/mcp").as_deref(),
            Some("http://localhost:8080/mcp")
        );
        assert_eq!(normalize_resource("https://x.example/mcp#f"), None);
        assert_eq!(normalize_resource("https://x.example/mcp?a=1"), None);
        assert_eq!(normalize_resource("zunder.example.org/mcp"), None);
        assert_eq!(normalize_resource("ftp://x.example/mcp"), None);
    }

    #[test]
    fn identifiers_follow_the_public_url() {
        let mut config = Config::for_data_dir("/tmp/x");
        config.public_url = Url::parse("https://zunder.example.org").unwrap();
        assert_eq!(issuer(&config), "https://zunder.example.org");
        assert_eq!(mcp_resource(&config), "https://zunder.example.org/mcp");
        assert_eq!(
            resource_metadata_url(&config),
            "https://zunder.example.org/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn redirect_uris() {
        assert!(validate_redirect_uri("https://claude.ai/api/mcp/auth_callback").is_ok());
        assert!(validate_redirect_uri("http://localhost:3118/callback").is_ok());
        assert!(validate_redirect_uri("http://127.0.0.1/callback").is_ok());
        assert!(validate_redirect_uri("http://[::1]:9/cb").is_ok());
        assert!(validate_redirect_uri("http://evil.example/cb").is_err());
        assert!(validate_redirect_uri("https://a.example/cb#x").is_err());
        assert!(validate_redirect_uri("https://u:p@a.example/cb").is_err());
        assert!(validate_redirect_uri("myapp://callback").is_err());
        assert!(validate_redirect_uri("javascript:alert(1)").is_err());
        assert!(validate_redirect_uri("nonsense").is_err());

        let claude = "https://claude.ai/api/mcp/auth_callback";
        assert!(redirect_uri_matches(claude, claude));
        assert!(!redirect_uri_matches(
            claude,
            "https://claude.ai/api/mcp/auth_callback/"
        ));
        assert!(!redirect_uri_matches(
            claude,
            "https://claude.ai:8443/api/mcp/auth_callback"
        ));
        // Loopback: any port, same host and path.
        assert!(redirect_uri_matches(
            "http://localhost/callback",
            "http://localhost:3118/callback"
        ));
        assert!(redirect_uri_matches(
            "http://127.0.0.1:1/callback",
            "http://127.0.0.1:5555/callback"
        ));
        assert!(!redirect_uri_matches(
            "http://localhost/callback",
            "http://127.0.0.1:3118/callback"
        ));
        assert!(!redirect_uri_matches(
            "http://localhost/callback",
            "http://localhost:3118/other"
        ));
        assert!(is_loopback_redirect("http://localhost:1/cb"));
        assert!(!is_loopback_redirect(claude));
    }

    #[test]
    fn pkce_syntax() {
        assert!(is_pkce_value(&"a".repeat(43)));
        assert!(is_pkce_value(&"a".repeat(128)));
        assert!(!is_pkce_value(&"a".repeat(42)));
        assert!(!is_pkce_value(&"a".repeat(129)));
        assert!(!is_pkce_value(&format!("{}+", "a".repeat(43))));
    }
}
