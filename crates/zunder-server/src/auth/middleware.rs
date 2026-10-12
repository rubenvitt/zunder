//! Request authentication for `/api` (and other protected routes).

use axum::extract::{FromRequestParts, Request, State};
use std::net::IpAddr;

use axum::http::header::{AUTHORIZATION, COOKIE, HOST, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::uri::Authority;
use axum::http::{HeaderMap, HeaderValue, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use cookie::{Cookie, SameSite};

use super::{has_client_header, missing_client_header, Principal, PrincipalKind, SESSION_COOKIE};
use crate::app::App;
use crate::error::AppError;

/// The bearer token of the `Authorization` header: `Ok(None)` without the
/// header, an error for any other scheme.
pub fn bearer_token(headers: &HeaderMap) -> Result<Option<&str>, AppError> {
    let Some(value) = headers.get(AUTHORIZATION) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| AppError::Unauthorized)?;
    match value.split_once(' ') {
        Some((scheme, token))
            if scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty() =>
        {
            Ok(Some(token.trim()))
        }
        _ => Err(AppError::Unauthorized),
    }
}

/// Authenticates a bearer token if one is present (`Ok(None)` without
/// `Authorization` header). Invalid tokens are an error, never a fallback.
pub async fn bearer_principal(
    app: &App,
    headers: &HeaderMap,
) -> Result<Option<Principal>, AppError> {
    match bearer_token(headers)? {
        Some(token) => match app.authenticate_token(token).await? {
            Some(principal) => Ok(Some(principal)),
            None => Err(AppError::Unauthorized),
        },
        None => Ok(None),
    }
}

/// The value of a cookie in the request.
pub fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.trim())
}

/// The `Set-Cookie` value for a session.
pub fn session_cookie(app: &App, id: &str, max_age_ms: i64) -> HeaderValue {
    let cookie = Cookie::build((SESSION_COOKIE, id.to_owned()))
        .http_only(true)
        .secure(app.config().secure_cookies())
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::milliseconds(max_age_ms.max(0)))
        .build();
    HeaderValue::from_str(&cookie.to_string()).expect("cookie is a valid header value")
}

/// The `Set-Cookie` value that removes the session cookie.
pub fn clear_session_cookie(app: &App) -> HeaderValue {
    session_cookie(app, "", 0)
}

/// Whether the `Host` header names a loopback host (`localhost`,
/// `127.0.0.0/8`, `[::1]`) on any port.
fn is_loopback_host(headers: &HeaderMap) -> bool {
    let Some(authority) = headers
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.contains('@'))
        .and_then(|v| v.parse::<Authority>().ok())
    else {
        return false;
    };
    let host = authority.host();
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Whether dev auth may stand in for credentials on this request: only with
/// `ZUNDER_DEV_AUTH=true` and only for requests addressed to a loopback host,
/// so neither another machine nor a DNS-rebinding website (whose requests
/// carry its own host name) acts as the owner.
pub fn dev_auth_applies(app: &App, headers: &HeaderMap) -> bool {
    app.config().dev_auth && is_loopback_host(headers)
}

fn is_safe_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Requires a principal (bearer token, session cookie or dev auth), enforces
/// the CSRF header for browser principals on state-changing requests, and
/// renews the session cookie when its sliding expiry was extended.
pub async fn require_auth(State(app): State<App>, mut req: Request, next: Next) -> Response {
    let mut renewed: Option<HeaderValue> = None;
    let principal = match authenticate(&app, req.headers(), &mut renewed).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if principal.is_browser() && !is_safe_method(req.method()) && !has_client_header(req.headers())
    {
        return missing_client_header().into_response();
    }
    req.extensions_mut().insert(principal);
    let mut response = next.run(req).await;
    if let Some(cookie) = renewed {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

async fn authenticate(
    app: &App,
    headers: &HeaderMap,
    renewed: &mut Option<HeaderValue>,
) -> Result<Principal, AppError> {
    if let Some(principal) = bearer_principal(app, headers).await? {
        return Ok(principal);
    }
    if let Some(id) = cookie_value(headers, SESSION_COOKIE).filter(|v| !v.is_empty()) {
        if let Some(auth) = app.authenticate_session(id).await? {
            if let Some(expires_at) = auth.renewed_until {
                let max_age = expires_at - zunder_core::hlc::system_time_ms() as i64;
                *renewed = Some(session_cookie(app, id, max_age));
            }
            return Ok(auth.principal);
        }
    }
    if dev_auth_applies(app, headers) {
        return Ok(Principal {
            user_id: app.owner_id(),
            kind: PrincipalKind::DevAuto,
        });
    }
    Err(AppError::Unauthorized)
}

/// The browser principal of a request (session cookie, or dev auth without
/// a valid cookie), ignoring bearer tokens, together with a binding string
/// that identifies the session (to tie in-memory state such as a pending
/// OAuth consent to it). Does not renew the session cookie.
pub async fn browser_principal(
    app: &App,
    headers: &HeaderMap,
) -> Result<Option<(Principal, String)>, AppError> {
    if let Some(id) = cookie_value(headers, SESSION_COOKIE).filter(|v| !v.is_empty()) {
        if let Some(auth) = app.authenticate_session(id).await? {
            return Ok(Some((auth.principal, super::sha256_hex(id))));
        }
    }
    if dev_auth_applies(app, headers) {
        return Ok(Some((
            Principal {
                user_id: app.owner_id(),
                kind: PrincipalKind::DevAuto,
            },
            "dev-auth".to_owned(),
        )));
    }
    Ok(None)
}

/// Handlers behind [`require_auth`] take the principal as an extractor.
impl<S: Send + Sync> FromRequestParts<S> for Principal {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Principal>()
            .cloned()
            .ok_or(AppError::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bearer_and_cookies() {
        let mut h = HeaderMap::new();
        assert_eq!(bearer_token(&h).unwrap(), None);
        h.insert(AUTHORIZATION, HeaderValue::from_static("Bearer zt_abc"));
        assert_eq!(bearer_token(&h).unwrap(), Some("zt_abc"));
        h.insert(AUTHORIZATION, HeaderValue::from_static("Basic Zm9v"));
        assert!(bearer_token(&h).is_err());

        let mut h = HeaderMap::new();
        h.append(COOKIE, HeaderValue::from_static("a=1; zunder_session=xyz"));
        h.append(COOKIE, HeaderValue::from_static("b=2"));
        assert_eq!(cookie_value(&h, "zunder_session"), Some("xyz"));
        assert_eq!(cookie_value(&h, "b"), Some("2"));
        assert_eq!(cookie_value(&h, "c"), None);
    }
}
