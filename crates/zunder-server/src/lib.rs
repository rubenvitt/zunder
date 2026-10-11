//! Zunder server: sync API, login, persistence and delivery of the web UI.
//!
//! - [`config`]: configuration from environment variables
//! - [`db`]: SQLite schema, migrations and queries
//! - [`app`]: application service (the only way to read or change data)
//! - [`auth`]: principals, API tokens, sessions, CSRF rule, OIDC
//! - [`http`]: router and handlers
//! - [`mcp`]: MCP endpoint (`/mcp`, Streamable HTTP, JSON-RPC 2.0)
//! - [`views`]: task, area and project representations for tools
//! - [`flame`]: starting and stopping flames (shared by HTTP and MCP)
//! - [`timing`]: client for the Timing web API
//! - [`backup`]: nightly `VACUUM INTO` backups
//! - [`healthcheck`]: client for the `healthcheck` subcommand

pub mod app;
pub mod auth;
pub mod backup;
pub mod config;
pub mod db;
pub mod error;
pub mod flame;
pub mod healthcheck;
pub mod http;
pub mod mcp;
pub mod timing;
pub mod views;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::Router;

pub use app::{App, Source};
pub use config::Config;
pub use error::{AppError, AppResult};

use auth::oidc::OidcClient;

/// The default HTTP client for outgoing requests (OIDC provider, Timing).
pub fn default_http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("zunder/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("HTTP-Client lässt sich nicht bauen")
}

/// Opens the app and builds the router.
pub async fn build(config: Config) -> anyhow::Result<(App, Router)> {
    build_with_http_client(config, default_http_client()?).await
}

/// Like [`build`], with a custom HTTP client for outgoing requests.
pub async fn build_with_http_client(
    config: Config,
    http: reqwest::Client,
) -> anyhow::Result<(App, Router)> {
    let timing = timing::TimingClient::from_config(config.timing.as_ref(), http.clone());
    let oidc = config
        .oidc
        .clone()
        .map(|c| Arc::new(OidcClient::new(c, config.oidc_redirect_uri(), http)));
    let app = App::open_with_timing(config, timing).await?;
    let router = http::router(app.clone(), oidc);
    Ok((app, router))
}

fn log_startup_warnings(config: &Config) {
    if config.dev_auth {
        for _ in 0..3 {
            tracing::warn!(
                "!!! ZUNDER_DEV_AUTH=true: Anmeldung OHNE OIDC aktiv. Jede Anfrage gilt als angemeldet. Nur lokal verwenden !!!"
            );
        }
    } else if config.oidc.is_none() {
        tracing::warn!(
            "no OIDC provider configured: browser login is disabled, only API tokens work"
        );
    }
    if config
        .bootstrap_token
        .as_ref()
        .is_some_and(|t| t.len() < 32)
    {
        tracing::warn!("ZUNDER_BOOTSTRAP_TOKEN is shorter than 32 characters");
    }
    if !config.web_dir.join("index.html").is_file() {
        tracing::warn!(dir = %config.web_dir.display(), "web UI not found (ZUNDER_WEB_DIR)");
    }
}

/// Runs the server until SIGTERM/SIGINT, then shuts down gracefully.
pub async fn run(config: Config) -> anyhow::Result<()> {
    log_startup_warnings(&config);
    let bind = config.bind;
    let (app, router) = build(config).await?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("{bind} lässt sich nicht binden"))?;
    tracing::info!(addr = %listener.local_addr()?, "zunder-server listening");

    let backup = tokio::spawn(backup::run_nightly(app.clone()));
    let shutdown = app.shutdown_token();
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            tracing::info!("shutting down");
            // Ends SSE streams and background tasks so connections drain.
            shutdown.cancel();
        })
        .await?;
    backup.await?;
    tracing::info!("stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "cannot listen for SIGINT");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::error!(error = %e, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
