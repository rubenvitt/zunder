//! HTTP endpoints of the built-in authorization server:
//!
//! | Route | Purpose |
//! |---|---|
//! | `GET /.well-known/oauth-protected-resource[/mcp]` | protected resource metadata (RFC 9728) |
//! | `GET /.well-known/oauth-authorization-server` | authorization server metadata (RFC 8414) |
//! | `POST /oauth/register` | dynamic client registration (RFC 7591) |
//! | `GET /oauth/authorize` | checks the request, sends to login if needed, shows the consent page |
//! | `POST /oauth/authorize` | consent decision (form, bound to the session) |
//! | `POST /oauth/token` | `authorization_code` and `refresh_token` grants |
//! | `POST /oauth/revoke` | token revocation (RFC 7009) |

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::rejection::FormRejection;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Query, RawQuery, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_SECURITY_POLICY, LOCATION, ORIGIN,
};
use axum::http::{Extensions, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use super::{
    active_client, exchange_code, issue_code, issuer, mcp_resource, no_store, normalize_resource,
    redirect_uri_matches, refresh, register, CodeExchange, CodeGrant, OAuthError, RefreshRequest,
    MAX_SCOPE_LEN, MAX_STATE_LEN,
};
use crate::app::App;
use crate::auth;
use crate::auth::middleware::browser_principal;

/// Body limit of the `/oauth/*` endpoints.
pub const OAUTH_BODY_LIMIT: usize = 16 * 1024;
/// How long a consent page stays valid.
pub const PENDING_TTL: Duration = Duration::from_secs(10 * 60);
/// At most this many consent pages are pending at once.
const MAX_PENDING: usize = 64;

/// A consent page waiting for the owner's decision.
#[derive(Debug, Clone)]
struct Pending {
    grant: CodeGrant,
    state: Option<String>,
    /// Identifies the browser session that saw the consent page.
    binding: String,
    created: Instant,
}

/// State of the OAuth routes: the app plus pending consent requests (in
/// memory; a restart only means the owner has to start over).
#[derive(Clone)]
pub struct OAuthState {
    app: App,
    pending: Arc<Mutex<HashMap<String, Pending>>>,
    registrations: Arc<super::RegistrationLimiter>,
}

impl OAuthState {
    pub fn new(app: App) -> Self {
        Self {
            app,
            pending: Arc::new(Mutex::new(HashMap::new())),
            registrations: Arc::default(),
        }
    }

    fn insert_pending(&self, pending: Pending) -> String {
        let id = auth::random_secret();
        let mut map = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        map.retain(|_, p| p.created.elapsed() < PENDING_TTL);
        while map.len() >= MAX_PENDING {
            let oldest = map
                .iter()
                .min_by_key(|(_, p)| p.created)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => map.remove(&k),
                None => break,
            };
        }
        map.insert(id.clone(), pending);
        id
    }

    fn take_pending(&self, id: &str) -> Option<Pending> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
            .filter(|p| p.created.elapsed() < PENDING_TTL)
    }
}

pub fn routes<S>(state: OAuthState) -> Router<S> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server),
        )
        .route("/oauth/register", post(register_client))
        .route("/oauth/authorize", get(authorize).post(consent))
        .route("/oauth/token", post(token))
        .route("/oauth/revoke", post(revoke_token))
        .layer(DefaultBodyLimit::max(OAUTH_BODY_LIMIT))
        .with_state(state)
}

// ----- metadata -----------------------------------------------------------------

/// Public metadata documents may be read cross-origin (browser-based MCP
/// clients); everything else stays without CORS.
fn public_json(body: Value) -> Response {
    let mut response = Json(body).into_response();
    response
        .headers_mut()
        .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("max-age=300"));
    response
}

async fn protected_resource(State(state): State<OAuthState>) -> Response {
    let config = state.app.config();
    public_json(json!({
        "resource": mcp_resource(config),
        "authorization_servers": [issuer(config)],
        "bearer_methods_supported": ["header"],
        "resource_name": "Zunder",
    }))
}

async fn authorization_server(State(state): State<OAuthState>) -> Response {
    let issuer = issuer(state.app.config());
    public_json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "registration_endpoint": format!("{issuer}/oauth/register"),
        "revocation_endpoint": format!("{issuer}/oauth/revoke"),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "revocation_endpoint_auth_methods_supported": ["none"],
        "authorization_response_iss_parameter_supported": true,
    }))
}

// ----- registration -------------------------------------------------------------

/// The address a request comes from: the peer, or behind a reverse proxy on
/// a loopback or private address (Traefik in the same Docker network) the
/// right-most `X-Forwarded-For` entry, which the proxy itself appended.
fn client_source(peer: Option<SocketAddr>, headers: &HeaderMap) -> Option<IpAddr> {
    let peer = peer?.ip();
    let behind_proxy = match peer {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    };
    if !behind_proxy {
        return Some(peer);
    }
    let forwarded = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .last()
        .and_then(|ip| ip.trim().parse::<IpAddr>().ok());
    Some(forwarded.unwrap_or(peer))
}

async fn register_client(
    State(state): State<OAuthState>,
    extensions: Extensions,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let peer = extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let source = client_source(peer, &headers);
    if !state.registrations.allows(source, crate::app::now_ms()) {
        return crate::oauth::too_many_registrations().into_response();
    }
    let metadata: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return OAuthError::invalid_client_metadata("Die Registrierung ist kein gültiges JSON")
                .into_response()
        }
    };
    match register(&state.app, metadata).await {
        Ok(client) => {
            state.registrations.record(source, crate::app::now_ms());
            no_store((StatusCode::CREATED, Json(client)).into_response())
        }
        Err(e) => e.into_response(),
    }
}

// ----- pages --------------------------------------------------------------------

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// A minimal German HTML page (no scripts, not frameable, not cached).
/// The global `Referrer-Policy: same-origin` keeps the page URL from
/// leaking to the client; a stricter `no-referrer` would make browsers send
/// `Origin: null` with the consent form, which the origin check rejects.
fn page(status: StatusCode, title: &str, body: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title} – Zunder</title>\
         <style>\
         body{{font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem;line-height:1.5;color:#1d1d1f;background:#fff}}\
         @media (prefers-color-scheme: dark){{body{{color:#ececec;background:#18181b}}code{{background:#27272a}}}}\
         code{{background:#f2f2f2;padding:.1rem .3rem;border-radius:4px;word-break:break-all}}\
         .warn{{border-left:4px solid #d97706;padding:.5rem .75rem;background:rgba(217,119,6,.1)}}\
         .actions{{display:flex;gap:.75rem;margin-top:1.5rem}}\
         button{{font:inherit;padding:.5rem 1.25rem;border-radius:6px;border:1px solid #888;cursor:pointer;background:transparent;color:inherit}}\
         button.primary{{background:#e8590c;border-color:#e8590c;color:#fff}}\
         .muted{{opacity:.7;font-size:.9rem}}\
         </style></head><body><h1>{title}</h1>{body}</body></html>",
        title = escape(title),
    );
    let mut response = (status, Html(html)).into_response();
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    response
}

fn error_page(status: StatusCode, message: &str) -> Response {
    page(
        status,
        "Verbindung nicht möglich",
        &format!(
            "<p>{}</p><p class=\"muted\">Starte die Verbindung in der App, die Zugriff auf Zunder \
             möchte, erneut.</p>",
            escape(message)
        ),
    )
}

fn see_other(location: &str) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    match HeaderValue::from_str(location) {
        Ok(v) => {
            headers.insert(LOCATION, v);
        }
        Err(_) => return error_page(StatusCode::BAD_REQUEST, "Ungültige Weiterleitung."),
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Redirects back to the client with `params`, `state` and `iss`
/// (RFC 9207) appended to the registered redirect URI.
fn redirect_to_client(
    app: &App,
    redirect_uri: &str,
    params: &[(&str, &str)],
    state: Option<&str>,
) -> Response {
    let Ok(mut url) = Url::parse(redirect_uri) else {
        return error_page(StatusCode::BAD_REQUEST, "Ungültige Redirect-URI.");
    };
    {
        let mut query = url.query_pairs_mut();
        for (k, v) in params {
            query.append_pair(k, v);
        }
        if let Some(state) = state {
            query.append_pair("state", state);
        }
        query.append_pair("iss", &issuer(app.config()));
    }
    see_other(url.as_str())
}

fn redirect_error(
    app: &App,
    redirect_uri: &str,
    error: &str,
    description: &str,
    state: Option<&str>,
) -> Response {
    redirect_to_client(
        app,
        redirect_uri,
        &[("error", error), ("error_description", description)],
        state,
    )
}

// ----- authorization ------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AuthorizeQuery {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    state: Option<String>,
    resource: Option<String>,
    scope: Option<String>,
}

/// `GET /oauth/authorize`. Problems with client or redirect URI are shown
/// as a page (never redirected, RFC 6749 4.1.2.1); all others go back to
/// the client. Without a browser session the owner is sent to the login
/// and comes back here.
async fn authorize(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    query: Result<Query<AuthorizeQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let app = &state.app;
    let Ok(Query(q)) = query else {
        return error_page(StatusCode::BAD_REQUEST, "Die Anfrage ist ungültig.");
    };
    let client = match active_client(app, q.client_id.as_deref().unwrap_or("")).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "Dieser Client ist bei Zunder nicht (mehr) registriert.",
            )
        }
        Err(e) => return error_page(e.status(), &e.to_string()),
    };
    let redirect_uri = match q.redirect_uri.as_deref() {
        Some(uri)
            if client
                .redirect_uris
                .iter()
                .any(|r| redirect_uri_matches(r, uri)) =>
        {
            uri.to_owned()
        }
        Some(_) => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "Die Redirect-URI ist für diesen Client nicht registriert.",
            )
        }
        None if client.redirect_uris.len() == 1 => client.redirect_uris[0].clone(),
        None => return error_page(StatusCode::BAD_REQUEST, "redirect_uri fehlt."),
    };
    if q.state.as_ref().is_some_and(|s| s.len() > MAX_STATE_LEN) {
        return error_page(StatusCode::BAD_REQUEST, "state ist zu lang.");
    }
    let client_state = q.state.as_deref();
    let fail = |error: &str, description: &str| {
        redirect_error(app, &redirect_uri, error, description, client_state)
    };
    if q.response_type.as_deref() != Some("code") {
        return fail(
            "unsupported_response_type",
            "Unterstützt wird nur response_type=code",
        );
    }
    let challenge = match (
        q.code_challenge.as_deref(),
        q.code_challenge_method.as_deref(),
    ) {
        (Some(c), Some("S256")) if super::is_pkce_value(c) => c.to_owned(),
        _ => {
            return fail(
                "invalid_request",
                "PKCE mit code_challenge_method=S256 ist erforderlich",
            )
        }
    };
    let expected =
        normalize_resource(&mcp_resource(app.config())).expect("public URL is a valid http(s) URL");
    let resource = match q.resource.as_deref() {
        None => expected,
        Some(raw) if normalize_resource(raw).as_deref() == Some(expected.as_str()) => expected,
        Some(_) => {
            return fail(
                "invalid_target",
                "Zunder stellt Tokens nur für den eigenen MCP-Endpunkt aus",
            )
        }
    };
    if q.scope.as_ref().is_some_and(|s| s.len() > MAX_SCOPE_LEN) {
        return fail("invalid_scope", "scope ist zu lang");
    }

    let (principal, binding) = match browser_principal(app, &headers).await {
        Ok(Some(found)) => found,
        Ok(None) => {
            let return_to = format!("/oauth/authorize?{}", raw_query.unwrap_or_default());
            let encoded: String =
                url::form_urlencoded::byte_serialize(return_to.as_bytes()).collect();
            return see_other(&format!("/auth/login?return_to={encoded}"));
        }
        Err(e) => return error_page(e.status(), &e.to_string()),
    };
    let owner = app.user(principal.user_id).await.ok().flatten();
    let pending = Pending {
        grant: CodeGrant {
            client_id: client.id.clone(),
            user_id: principal.user_id,
            redirect_uri: redirect_uri.clone(),
            code_challenge: challenge,
            resource,
            scope: q.scope.clone().filter(|s| !s.trim().is_empty()),
        },
        state: q.state.clone(),
        binding,
        created: Instant::now(),
    };
    let request_id = state.insert_pending(pending);
    consent_page(
        &client.name,
        &redirect_uri,
        &client.redirect_uris,
        owner
            .as_ref()
            .and_then(|u| u.email.as_deref().or(u.name.as_deref())),
        &request_id,
    )
}

fn consent_page(
    client_name: &str,
    redirect_uri: &str,
    registered: &[String],
    owner: Option<&str>,
    request_id: &str,
) -> Response {
    let host = Url::parse(redirect_uri)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| redirect_uri.to_owned());
    let account = owner
        .map(|o| format!(" als <strong>{}</strong>", escape(o)))
        .unwrap_or_default();
    let loopback_warning = if registered.iter().all(|r| super::is_loopback_redirect(r)) {
        "<p class=\"warn\">Achtung: Die Anmeldung geht an ein Programm auf diesem Rechner \
         (localhost). Erlaube den Zugriff nur, wenn du die Verbindung gerade selbst gestartet hast.</p>"
    } else {
        ""
    };
    let body = format!(
        "<p><strong>{name}</strong> möchte{account} auf Zunder zugreifen: Aufgaben, Projekte und \
         Bereiche über MCP lesen und ändern.</p>\
         <p>Nach der Freigabe geht es weiter zu <code>{host}</code>.</p>{loopback_warning}\
         <form method=\"post\" action=\"/oauth/authorize\">\
         <input type=\"hidden\" name=\"request_id\" value=\"{request_id}\">\
         <div class=\"actions\">\
         <button class=\"primary\" type=\"submit\" name=\"decision\" value=\"allow\">Erlauben</button>\
         <button type=\"submit\" name=\"decision\" value=\"deny\">Ablehnen</button>\
         </div></form>\
         <p class=\"muted\">Erlaube den Zugriff nur, wenn du die Verbindung gerade selbst \
         gestartet hast. Widerrufen lässt er sich jederzeit in Zunder unter „Tokens“.</p>",
        name = escape(client_name),
        host = escape(&host),
        request_id = escape(request_id),
    );
    page(StatusCode::OK, "Zugriff erlauben?", &body)
}

#[derive(Debug, Deserialize)]
struct ConsentForm {
    request_id: String,
    decision: String,
}

/// `POST /oauth/authorize`: the owner's decision. The request ID is an
/// unguessable, single-use secret bound to the session that saw the page
/// (CSRF protection); a present `Origin` must be Zunder's own.
async fn consent(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    form: Result<Form<ConsentForm>, FormRejection>,
) -> Response {
    let app = &state.app;
    if let Some(origin) = headers.get(ORIGIN) {
        if origin.to_str().ok() != Some(app.config().public_origin().as_str()) {
            return error_page(StatusCode::FORBIDDEN, "Fremde Herkunft (Origin).");
        }
    }
    let Ok(Form(form)) = form else {
        return error_page(StatusCode::BAD_REQUEST, "Die Anfrage ist ungültig.");
    };
    let Some(pending) = state.take_pending(&form.request_id) else {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Die Freigabe ist abgelaufen oder wurde schon beantwortet.",
        );
    };
    match browser_principal(app, &headers).await {
        Ok(Some((principal, binding)))
            if binding == pending.binding && principal.user_id == pending.grant.user_id => {}
        Ok(_) => {
            return error_page(
                StatusCode::FORBIDDEN,
                "Die Freigabe gehört zu einer anderen Anmeldung.",
            )
        }
        Err(e) => return error_page(e.status(), &e.to_string()),
    }
    let redirect_uri = pending.grant.redirect_uri.clone();
    let client_state = pending.state.as_deref();
    if form.decision != "allow" {
        return redirect_error(
            app,
            &redirect_uri,
            "access_denied",
            "Der Zugriff wurde abgelehnt",
            client_state,
        );
    }
    // The client might have been revoked while the page was open.
    match active_client(app, &pending.grant.client_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "Dieser Client ist bei Zunder nicht (mehr) registriert.",
            )
        }
        Err(e) => return error_page(e.status(), &e.to_string()),
    }
    let client_id = pending.grant.client_id.clone();
    match issue_code(app, pending.grant.clone()).await {
        Ok(code) => {
            tracing::info!(client_id = %client_id, "OAuth authorization granted");
            redirect_to_client(app, &redirect_uri, &[("code", &code)], client_state)
        }
        Err(e) => error_page(e.status(), &e.to_string()),
    }
}

// ----- token --------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct TokenForm {
    grant_type: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    resource: Option<String>,
}

fn form_error(rejection: FormRejection) -> OAuthError {
    OAuthError::invalid_request(format!(
        "Erwartet wird application/x-www-form-urlencoded: {}",
        rejection.body_text()
    ))
}

async fn token(
    State(state): State<OAuthState>,
    form: Result<Form<TokenForm>, FormRejection>,
) -> Response {
    let form = match form {
        Ok(Form(f)) => f,
        Err(rejection) => return form_error(rejection).into_response(),
    };
    let result = match form.grant_type.as_deref() {
        Some("authorization_code") => {
            let (Some(client_id), Some(code)) = (form.client_id, form.code) else {
                return OAuthError::invalid_request("client_id und code sind erforderlich")
                    .into_response();
            };
            match active_client(&state.app, &client_id).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return OAuthError::invalid_client("Unbekannter Client").into_response()
                }
                Err(e) => return OAuthError::from(e).into_response(),
            }
            exchange_code(
                &state.app,
                CodeExchange {
                    client_id,
                    code,
                    redirect_uri: form.redirect_uri,
                    code_verifier: form.code_verifier.unwrap_or_default(),
                    resource: form.resource,
                },
            )
            .await
        }
        Some("refresh_token") => {
            let Some(refresh_token) = form.refresh_token else {
                return OAuthError::invalid_request("refresh_token fehlt").into_response();
            };
            refresh(
                &state.app,
                RefreshRequest {
                    client_id: form.client_id,
                    refresh_token,
                    resource: form.resource,
                },
            )
            .await
        }
        Some(_) => Err(OAuthError::new(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "Unterstützt werden authorization_code und refresh_token",
        )),
        None => Err(OAuthError::invalid_request("grant_type fehlt")),
    };
    match result {
        Ok(tokens) => no_store(Json(tokens).into_response()),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct RevokeForm {
    token: Option<String>,
    client_id: Option<String>,
}

/// `POST /oauth/revoke` (RFC 7009): always 200, also for unknown tokens.
async fn revoke_token(
    State(state): State<OAuthState>,
    form: Result<Form<RevokeForm>, FormRejection>,
) -> Response {
    let form = match form {
        Ok(Form(f)) => f,
        Err(rejection) => return form_error(rejection).into_response(),
    };
    let Some(token) = form.token.filter(|t| !t.is_empty()) else {
        return OAuthError::invalid_request("token fehlt").into_response();
    };
    match super::revoke(&state.app, &token, form.client_id.as_deref()).await {
        Ok(()) => no_store(StatusCode::OK.into_response()),
        Err(e) => OAuthError::from(e).into_response(),
    }
}
