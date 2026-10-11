//! Shared helpers for the HTTP integration tests.
#![allow(dead_code)]

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;
use zunder_core::{Clock, NodeId, Op, OpBatch};
use zunder_server::{App, Config};

pub const BOOTSTRAP: &str = "zt_bootstrap-token-for-tests-0123456789abcdef";

pub struct TestServer {
    pub app: App,
    pub router: Router,
    pub dir: TempDir,
}

/// How a request authenticates.
#[derive(Clone, Copy)]
pub enum Auth<'a> {
    None,
    Bearer(&'a str),
    /// Session cookie without the CSRF header.
    Cookie(&'a str),
    /// Session cookie with `X-Zunder-Client: web`.
    Web(&'a str),
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl TestResponse {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "body is not JSON ({e}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect()
    }

    /// The value of a cookie set by this response.
    pub fn cookie(&self, name: &str) -> Option<String> {
        self.set_cookies().iter().find_map(|c| {
            let first = c.split(';').next()?;
            let (k, v) = first.split_once('=')?;
            (k == name).then(|| v.to_owned())
        })
    }

    pub fn error_code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap().to_owned()
    }
}

pub fn test_config(dir: &TempDir) -> Config {
    let mut config = Config::for_data_dir(dir.path().join("data"));
    config.web_dir = dir.path().join("web");
    config.bootstrap_token = Some(BOOTSTRAP.to_owned());
    config
}

impl TestServer {
    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    pub async fn with_config(adjust: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = test_config(&dir);
        adjust(&mut config);
        let (app, router) = zunder_server::build(config).await.unwrap();
        Self { app, router, dir }
    }

    /// Reopens the server on the same data directory.
    pub async fn restart(self) -> Self {
        let TestServer { app, router, dir } = self;
        let config = app.config().clone();
        drop(router);
        drop(app);
        let (app, router) = zunder_server::build(config).await.unwrap();
        Self { app, router, dir }
    }

    pub async fn send(&self, req: Request<Body>) -> TestResponse {
        let response = self.router.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        TestResponse {
            status,
            headers,
            body,
        }
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        auth: Auth<'_>,
        body: Option<Value>,
    ) -> TestResponse {
        let mut builder = Request::builder().method(method).uri(path);
        builder = authorize(builder, auth);
        let req = match body {
            Some(json) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        self.send(req).await
    }

    pub async fn get(&self, path: &str, auth: Auth<'_>) -> TestResponse {
        self.request(Method::GET, path, auth, None).await
    }

    pub async fn post(&self, path: &str, auth: Auth<'_>, body: Value) -> TestResponse {
        self.request(Method::POST, path, auth, Some(body)).await
    }

    pub async fn push(&self, auth: Auth<'_>, ops: &[Op]) -> TestResponse {
        self.post(
            "/api/sync/push",
            auth,
            serde_json::json!({"client_id": "test", "ops": ops}),
        )
        .await
    }

    /// A fresh browser session of the owner.
    pub async fn session(&self) -> String {
        self.app
            .create_session(self.app.owner_id())
            .await
            .unwrap()
            .id
    }
}

pub fn authorize(
    builder: axum::http::request::Builder,
    auth: Auth<'_>,
) -> axum::http::request::Builder {
    match auth {
        Auth::None => builder,
        Auth::Bearer(token) => builder.header(header::AUTHORIZATION, format!("Bearer {token}")),
        Auth::Cookie(id) => builder.header(header::COOKIE, format!("zunder_session={id}")),
        Auth::Web(id) => builder
            .header(header::COOKIE, format!("zunder_session={id}"))
            .header("x-zunder-client", "web"),
    }
}

/// A client clock like the web UI's.
pub fn client_clock() -> Clock {
    Clock::new(NodeId::new("web-test").unwrap())
}

/// Builds operations with a client clock at the current time.
pub fn ops(clock: &mut Clock, build: impl FnOnce(&mut OpBatch<'_>)) -> Vec<Op> {
    let mut batch = OpBatch::new(clock, zunder_core::hlc::system_time_ms());
    build(&mut batch);
    batch.into_ops()
}
