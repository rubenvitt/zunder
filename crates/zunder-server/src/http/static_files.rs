//! Serves the web UI from `ZUNDER_WEB_DIR` with SPA fallback.
//!
//! - `/assets/*` (Vite output with content hashes):
//!   `Cache-Control: public, max-age=31536000, immutable`; missing files are
//!   a plain 404.
//! - Everything else: `Cache-Control: no-cache` (revalidate). Paths without
//!   a file extension that do not exist fall back to `index.html`, so client
//!   side routes work on reload.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use crate::app::App;
use crate::error::AppError;

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const NO_CACHE: &str = "no-cache";

pub async fn serve(State(app): State<App>, req: Request) -> Response {
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        return AppError::NotFound("Nicht gefunden".into()).into_response();
    }
    let web_dir = app.config().web_dir.clone();
    let path = req.uri().path().to_owned();
    let is_asset = path.starts_with("/assets/");
    let method = req.method().clone();
    let fallback_headers = req.headers().clone();

    let response = match ServeDir::new(&web_dir).oneshot(req).await {
        Ok(r) => r.map(Body::new),
        Err(never) => match never {},
    };
    let status = response.status();
    if status.is_success() || status == StatusCode::NOT_MODIFIED {
        return with_cache(response, if is_asset { IMMUTABLE } else { NO_CACHE });
    }
    if status != StatusCode::NOT_FOUND || is_asset || has_extension(&path) {
        return response;
    }

    // SPA fallback.
    let index = web_dir.join("index.html");
    if !index.is_file() {
        return (
            StatusCode::NOT_FOUND,
            "Web-UI nicht gefunden (ZUNDER_WEB_DIR prüfen)",
        )
            .into_response();
    }
    let mut fallback = Request::builder()
        .method(method)
        .uri("/index.html")
        .body(Body::empty())
        .expect("static request is valid");
    *fallback.headers_mut() = fallback_headers;
    let response = match ServeFile::new(index).oneshot(fallback).await {
        Ok(r) => r.map(Body::new),
        Err(never) => match never {},
    };
    with_cache(response, NO_CACHE)
}

fn has_extension(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'))
}

fn with_cache(mut response: Response, value: &'static str) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(value));
    response
}
