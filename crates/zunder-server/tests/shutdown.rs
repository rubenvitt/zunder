//! Graceful shutdown ends after the drain deadline even when clients keep
//! requests open.

mod common;

use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use common::{test_config, BOOTSTRAP};

#[tokio::test]
async fn shutdown_does_not_wait_forever_for_open_requests() {
    let dir = tempfile::tempdir().unwrap();
    let (app, router) = zunder_server::build(test_config(&dir)).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let drain = Duration::from_millis(300);
    let server = tokio::spawn(zunder_server::serve(
        listener,
        app,
        router,
        async move {
            let _ = stopped.await;
        },
        drain,
    ));

    // An unfinished request head and a push whose body never completes.
    let mut head = TcpStream::connect(addr).await.unwrap();
    head.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n")
        .await
        .unwrap();
    let mut push = TcpStream::connect(addr).await.unwrap();
    push.write_all(
        format!(
            "POST /api/sync/push HTTP/1.1\r\nHost: localhost\r\n\
             Authorization: Bearer {BOOTSTRAP}\r\nContent-Type: application/json\r\n\
             Content-Length: 1000\r\n\r\n{{"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let started = Instant::now();
    stop.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("serve must return after the drain deadline")
        .unwrap();
    result.unwrap();
    assert!(started.elapsed() >= drain, "waited for the requests first");
    drop((head, push));
}
