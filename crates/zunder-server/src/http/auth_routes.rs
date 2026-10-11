//! `/auth/login`, `/auth/callback`, `/auth/logout`.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::header::{CACHE_CONTROL, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use cookie::{Cookie, SameSite};
use serde::Deserialize;

use crate::app::App;
use crate::auth::middleware::{
    clear_session_cookie, cookie_value, dev_auth_applies, session_cookie,
};
use crate::auth::oidc::{OidcClient, LOGIN_COOKIE, LOGIN_TTL};
use crate::auth::{has_client_header, missing_client_header, SESSION_COOKIE};
use crate::error::AppError;

#[derive(Clone)]
pub struct AuthState {
    pub app: App,
    pub oidc: Option<Arc<OidcClient>>,
}

pub fn routes<S>(state: AuthState) -> Router<S> {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/logout", post(logout))
        .with_state(state)
}

/// Only local paths are allowed as return targets (no open redirect).
///
/// Browsers strip TAB, CR and LF anywhere in a URL before parsing it, so
/// `/\t/evil.example` would become the protocol-relative `//evil.example`.
/// Any control or whitespace character therefore rejects the value outright.
fn safe_return_to(raw: Option<&str>) -> String {
    match raw {
        Some(p)
            if p.starts_with('/')
                && !p.starts_with("//")
                && !p.contains('\\')
                && !p.chars().any(|c| c.is_control() || c.is_whitespace()) =>
        {
            p.to_owned()
        }
        _ => "/".to_owned(),
    }
}

fn redirect(to: &str) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(to) {
        headers.insert(LOCATION, v);
    }
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn login_cookie(app: &App, value: &str, max_age: time::Duration) -> HeaderValue {
    let cookie = Cookie::build((LOGIN_COOKIE, value.to_owned()))
        .http_only(true)
        .secure(app.config().secure_cookies())
        .same_site(SameSite::Lax)
        .path("/auth")
        .max_age(max_age)
        .build();
    HeaderValue::from_str(&cookie.to_string()).expect("cookie is a valid header value")
}

/// A minimal German error page for the browser login flow.
fn error_page(status: StatusCode, message: &str) -> Response {
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let body = format!(
        "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Anmeldung – Zunder</title></head>\
         <body style=\"font-family: system-ui, sans-serif; max-width: 32rem; margin: 4rem auto; padding: 0 1rem\">\
         <h1>Anmeldung fehlgeschlagen</h1><p>{escaped}</p>\
         <p><a href=\"/auth/login\">Erneut anmelden</a></p></body></html>"
    );
    (status, Html(body)).into_response()
}

#[derive(Deserialize)]
struct LoginQuery {
    return_to: Option<String>,
}

async fn login(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(q): Query<LoginQuery>,
) -> Response {
    let return_to = safe_return_to(q.return_to.as_deref());
    let app = &state.app;
    if app.config().dev_auth {
        if !dev_auth_applies(app, &headers) {
            return error_page(
                StatusCode::FORBIDDEN,
                "Die Anmeldung ohne OIDC (ZUNDER_DEV_AUTH) gilt nur über localhost.",
            );
        }
        return match dev_login(app, &return_to).await {
            Ok(response) => response,
            Err(e) => error_page(e.status(), &e.to_string()),
        };
    }
    let Some(oidc) = &state.oidc else {
        return error_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "Es ist kein Anmeldedienst konfiguriert (ZUNDER_OIDC_ISSUER).",
        );
    };
    match oidc.start_login(return_to).await {
        Ok(start) => {
            let mut response = redirect(start.redirect.as_str());
            let max_age = time::Duration::seconds(LOGIN_TTL.as_secs() as i64);
            response
                .headers_mut()
                .append(SET_COOKIE, login_cookie(app, &start.state, max_age));
            response
        }
        Err(e) => {
            tracing::error!(error = %e, "OIDC login could not start");
            error_page(StatusCode::BAD_GATEWAY, &e.to_string())
        }
    }
}

async fn dev_login(app: &App, return_to: &str) -> Result<Response, AppError> {
    let user = app.owner().await?;
    let session = app.create_session(user.id).await?;
    tracing::warn!("dev auth: session created without OIDC");
    let mut response = redirect(return_to);
    response.headers_mut().append(
        SET_COOKIE,
        session_cookie(app, &session.id, crate::auth::SESSION_TTL_MS),
    );
    Ok(response)
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn callback(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let app = &state.app;
    let clear_login = login_cookie(app, "", time::Duration::ZERO);
    let fail = |status: StatusCode, message: &str| {
        let mut response = error_page(status, message);
        response
            .headers_mut()
            .append(SET_COOKIE, clear_login.clone());
        response
    };
    let Some(oidc) = &state.oidc else {
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Es ist kein Anmeldedienst konfiguriert.",
        );
    };
    if let Some(error) = q.error {
        let detail = q.error_description.unwrap_or_default();
        return fail(
            StatusCode::UNAUTHORIZED,
            &format!("Der Anmeldedienst meldet „{error}“. {detail}"),
        );
    }
    let (Some(code), Some(login_state)) = (q.code, q.state) else {
        return fail(StatusCode::BAD_REQUEST, "code oder state fehlt.");
    };
    // The state must belong to this browser (prevents login CSRF).
    if cookie_value(&headers, LOGIN_COOKIE) != Some(login_state.as_str()) {
        return fail(
            StatusCode::BAD_REQUEST,
            "Die Anmeldung wurde in einem anderen Browser begonnen oder ist abgelaufen.",
        );
    }
    let done = match oidc.finish_login(&login_state, &code).await {
        Ok(done) => done,
        Err(e) => {
            tracing::warn!(error = %e, "OIDC login failed");
            let status = match e {
                crate::auth::oidc::OidcError::NotAllowed => StatusCode::FORBIDDEN,
                crate::auth::oidc::OidcError::Provider(_) => StatusCode::BAD_GATEWAY,
                _ => StatusCode::UNAUTHORIZED,
            };
            return fail(status, &e.to_string());
        }
    };
    let result = async {
        let user = app.sign_in(done.identity).await?;
        app.create_session(user.id).await
    }
    .await;
    match result {
        Ok(session) => {
            tracing::info!("OIDC login successful");
            // Checked again: the pending login may predate a stricter rule.
            let mut response = redirect(&safe_return_to(Some(&done.return_to)));
            let headers = response.headers_mut();
            headers.append(SET_COOKIE, clear_login);
            headers.append(
                SET_COOKIE,
                session_cookie(app, &session.id, crate::auth::SESSION_TTL_MS),
            );
            response
        }
        Err(e) => fail(e.status(), &e.to_string()),
    }
}

/// Ends the session. Requires the CSRF header like every cookie-authenticated
/// state change.
async fn logout(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let app = &state.app;
    if !has_client_header(&headers) {
        return missing_client_header().into_response();
    }
    if let Some(id) = cookie_value(&headers, SESSION_COOKIE).filter(|v| !v.is_empty()) {
        if let Err(e) = app.end_session(id).await {
            return e.into_response();
        }
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .append(SET_COOKIE, clear_session_cookie(app));
    response
}

#[cfg(test)]
mod tests {
    use super::safe_return_to;

    #[test]
    fn return_to_is_local_only() {
        assert_eq!(safe_return_to(None), "/");
        assert_eq!(safe_return_to(Some("/inbox?x=1")), "/inbox?x=1");
        assert_eq!(safe_return_to(Some("//evil.example")), "/");
        assert_eq!(safe_return_to(Some("https://evil.example")), "/");
        assert_eq!(safe_return_to(Some("/\\evil.example")), "/");
        assert_eq!(safe_return_to(Some("/inbox?x=1#y")), "/inbox?x=1#y");
        for evil in [
            "/\t/evil.example",
            "/\n/evil.example",
            "/\r/evil.example",
            "/ /evil.example",
            "\t//evil.example",
            "/\u{0}/evil.example",
            "/\u{a0}/evil.example",
        ] {
            assert_eq!(safe_return_to(Some(evil)), "/", "{evil:?}");
        }
    }
}
