//! HTTP client for the server's sync API (reqwest with rustls, so the same
//! code runs on desktop, iOS and Android without a system TLS library).

use std::future::Future;
use std::time::Duration;

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use url::Url;
use uuid::Uuid;
use zunder_core::Op;

use crate::error::{Error, Result};
use crate::store::{normalize_server_url, Remote};
use crate::wire::{ErrorBody, HistoryEntry, PullPage, PushRequest, PushResponse, ServerSnapshot};

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

/// Start or stop a task's flame (`POST /api/tasks/{id}/flame/start|stop`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlameAction {
    Start,
    Stop,
}

impl FlameAction {
    fn as_str(self) -> &'static str {
        match self {
            FlameAction::Start => "start",
            FlameAction::Stop => "stop",
        }
    }
}

/// Server calls besides sync that the web UI needs inside the app. The
/// webview's CSP only allows IPC (and the server allows no CORS), so the app
/// makes them on the UI's behalf with the stored API token.
impl HttpClient {
    /// `GET /api/tasks/{id}/history`: all operations of the task in `seq`
    /// order. The server only knows operations it has received.
    pub async fn task_history(&self, task_id: Uuid) -> Result<Vec<HistoryEntry>> {
        let path = format!("api/tasks/{task_id}/history");
        self.send(self.http.get(self.url(&path)?)).await
    }

    /// `POST /api/tasks/{id}/flame/start|stop`. Returns the server's outcome
    /// unchanged (`{task, running, stopped_task_ids, unchanged, timing,
    /// warnings}`); the flame fields themselves arrive with the next pull.
    /// Stopping without a running flame is `Error::Server` with status 409.
    pub async fn flame(&self, task_id: Uuid, action: FlameAction) -> Result<serde_json::Value> {
        let path = format!("api/tasks/{task_id}/flame/{}", action.as_str());
        self.send(self.http.post(self.url(&path)?)).await
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
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    use super::*;

    /// A request as the canned server saw it.
    #[derive(Debug)]
    struct Seen {
        request_line: String,
        authorization: Option<String>,
    }

    /// Answers the next `responses.len()` requests with the given status and
    /// JSON body, in order, and reports each request.
    fn canned(responses: Vec<(u16, &'static str)>) -> (String, mpsc::Receiver<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/zunder", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for (status, body) in responses {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut authorization = None;
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    let (name, value) = line.split_once(':').unwrap();
                    match name.to_ascii_lowercase().as_str() {
                        "authorization" => authorization = Some(value.trim().to_string()),
                        "content-length" => length = value.trim().parse().unwrap(),
                        _ => {}
                    }
                }
                let mut body_in = vec![0; length];
                reader.read_exact(&mut body_in).unwrap();
                tx.send(Seen {
                    request_line: request_line.trim_end().to_string(),
                    authorization,
                })
                .unwrap();
                let mut stream = reader.into_inner();
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (url, rx)
    }

    fn client(url: &str) -> HttpClient {
        HttpClient::new(&Remote {
            server_url: url.into(),
            api_token: "zt_secret".into(),
        })
        .unwrap()
    }

    const TASK: &str = "0192f0a0-0000-7000-8000-000000000001";

    #[tokio::test]
    async fn task_history_sends_the_token_and_parses_entries() {
        let body = r#"[{"seq":7,"op":{"op_id":"0192f0a0-0000-7000-8000-0000000000aa","entity":"task","entity_id":"0192f0a0-0000-7000-8000-000000000001","field":"title","value":"Zahnarzt","hlc":"1760000000000:00000:web-abc"},"source":"api:app","received_at":"2026-10-11T08:00:00Z"}]"#;
        let (url, seen) = canned(vec![(200, body)]);
        let entries = client(&url)
            .task_history(TASK.parse().unwrap())
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seq, 7);
        assert_eq!(entries[0].source, "api:app");
        assert_eq!(entries[0].op.field, "title");
        let request = seen.recv().unwrap();
        assert_eq!(
            request.request_line,
            format!("GET /zunder/api/tasks/{TASK}/history HTTP/1.1")
        );
        assert_eq!(request.authorization.as_deref(), Some("Bearer zt_secret"));
    }

    #[tokio::test]
    async fn flame_posts_and_maps_errors() {
        let (url, seen) = canned(vec![
            (200, r#"{"running":true,"unchanged":false}"#),
            (
                409,
                r#"{"error":{"code":"conflict","message":"Keine laufende Flamme"}}"#,
            ),
            (401, r#"{"error":{"code":"unauthorized","message":"nope"}}"#),
        ]);
        let client = client(&url);
        let id: Uuid = TASK.parse().unwrap();

        let outcome = client.flame(id, FlameAction::Start).await.unwrap();
        assert_eq!(outcome["running"], true);
        assert_eq!(
            seen.recv().unwrap().request_line,
            format!("POST /zunder/api/tasks/{TASK}/flame/start HTTP/1.1")
        );

        let err = client.flame(id, FlameAction::Stop).await.unwrap_err();
        assert!(
            matches!(&err, Error::Server { status: 409, code, .. } if code == "conflict"),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Server-Fehler (409): Keine laufende Flamme"
        );
        assert_eq!(
            seen.recv().unwrap().request_line,
            format!("POST /zunder/api/tasks/{TASK}/flame/stop HTTP/1.1")
        );

        let err = client.flame(id, FlameAction::Stop).await.unwrap_err();
        assert_eq!(err.code(), "unauthorized");
    }

    #[tokio::test]
    async fn unreachable_server_is_a_network_error() {
        // Bind and drop: nothing listens on this port any more.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let err = client(&format!("http://127.0.0.1:{port}"))
            .task_history(TASK.parse().unwrap())
            .await
            .unwrap_err();
        assert!(err.is_network(), "{err:?}");
    }

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
