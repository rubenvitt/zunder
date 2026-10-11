//! HTTP client for the server's sync API (reqwest with rustls, so the same
//! code runs on desktop, iOS and Android without a system TLS library).

use std::future::Future;
use std::time::Duration;

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use url::Url;
use zunder_core::Op;

use crate::error::{Error, Result};
use crate::store::{normalize_server_url, Remote};
use crate::wire::{ErrorBody, PullPage, PushRequest, PushResponse, ServerSnapshot};

/// Page size for `GET /api/sync/pull`.
pub const PULL_LIMIT: u32 = 1000;

/// The server side of a sync, abstracted so the sync logic can be tested
/// without HTTP.
pub trait SyncApi {
    fn snapshot(&self) -> impl Future<Output = Result<ServerSnapshot>> + Send;
    fn push(
        &self,
        client_id: &str,
        ops: &[Op],
    ) -> impl Future<Output = Result<PushResponse>> + Send;
    fn pull(&self, since: i64, limit: u32) -> impl Future<Output = Result<PullPage>> + Send;
}

/// `SyncApi` over HTTP with a bearer API token.
#[derive(Debug, Clone)]
pub struct HttpClient {
    http: reqwest::Client,
    base: Url,
    remote: Remote,
}

impl HttpClient {
    pub fn new(remote: &Remote) -> Result<Self> {
        let normalized = normalize_server_url(&remote.server_url)?;
        // A trailing slash makes `Url::join` keep a path prefix.
        let base = Url::parse(&format!("{normalized}/"))
            .map_err(|e| Error::InvalidConfig(e.to_string()))?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("zunder-app/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Self {
            http,
            base,
            remote: remote.clone(),
        })
    }

    /// The server URL and token this client was built for.
    pub fn remote(&self) -> &Remote {
        &self.remote
    }

    fn url(&self, path: &str) -> Result<Url> {
        self.base
            .join(path)
            .map_err(|e| Error::InvalidConfig(e.to_string()))
    }

    async fn send<T: DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> Result<T> {
        let response = request
            .bearer_auth(&self.remote.api_token)
            .send()
            .await
            .map_err(network)?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(Error::Unauthorized);
        }
        let bytes = response.bytes().await.map_err(network)?;
        if !status.is_success() {
            return Err(match serde_json::from_slice::<ErrorBody>(&bytes) {
                Ok(body) => Error::Server {
                    status: status.as_u16(),
                    code: body.error.code,
                    message: body.error.message,
                },
                Err(_) => Error::Server {
                    status: status.as_u16(),
                    code: "unknown".into(),
                    message: String::from_utf8_lossy(&bytes).chars().take(200).collect(),
                },
            });
        }
        serde_json::from_slice(&bytes).map_err(|e| Error::Protocol(e.to_string()))
    }
}

fn network(e: reqwest::Error) -> Error {
    Error::Network(e.to_string())
}

impl SyncApi for HttpClient {
    async fn snapshot(&self) -> Result<ServerSnapshot> {
        self.send(self.http.get(self.url("api/snapshot")?)).await
    }

    async fn push(&self, client_id: &str, ops: &[Op]) -> Result<PushResponse> {
        let body = PushRequest { client_id, ops };
        self.send(self.http.post(self.url("api/sync/push")?).json(&body))
            .await
    }

    async fn pull(&self, since: i64, limit: u32) -> Result<PullPage> {
        let mut url = self.url("api/sync/pull")?;
        url.query_pairs_mut()
            .append_pair("since", &since.to_string())
            .append_pair("limit", &limit.to_string());
        self.send(self.http.get(url)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_keep_a_path_prefix() {
        let client = HttpClient::new(&Remote {
            server_url: "https://example.org/zunder/".into(),
            api_token: "zt_x".into(),
        })
        .unwrap();
        assert_eq!(
            client.url("api/sync/push").unwrap().as_str(),
            "https://example.org/zunder/api/sync/push"
        );
        let root = HttpClient::new(&Remote {
            server_url: "http://localhost:8080".into(),
            api_token: "zt_x".into(),
        })
        .unwrap();
        assert_eq!(
            root.url("api/snapshot").unwrap().as_str(),
            "http://localhost:8080/api/snapshot"
        );
    }
}
