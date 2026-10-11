//! The real binary: startup, `healthcheck` subcommand and graceful shutdown
//! on SIGTERM with an open SSE connection.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_zunder-server");
const TOKEN: &str = "zt_process-test-bootstrap-token-0123456789";

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn healthcheck(port: u16) -> bool {
    Command::new(BIN)
        .arg("healthcheck")
        .env("ZUNDER_BIND", format!("0.0.0.0:{port}"))
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

#[test]
fn healthcheck_and_graceful_shutdown() {
    let port = free_port();
    assert!(!healthcheck(port), "nothing listens yet");

    let dir = tempfile::tempdir().unwrap();
    let mut server = Server(
        Command::new(BIN)
            .env("ZUNDER_BIND", format!("127.0.0.1:{port}"))
            .env("ZUNDER_DATA_DIR", dir.path().join("data"))
            .env("ZUNDER_WEB_DIR", dir.path().join("web"))
            .env("ZUNDER_BOOTSTRAP_TOKEN", TOKEN)
            .env("RUST_LOG", "warn")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    while !healthcheck(port) {
        assert!(Instant::now() < deadline, "server did not become healthy");
        std::thread::sleep(Duration::from_millis(100));
    }

    // Keep an SSE stream open; shutdown must still complete.
    let mut sse = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        sse,
        "GET /api/sync/events HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
    )
    .unwrap();
    sse.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut reader = BufReader::new(sse.try_clone().unwrap());
    let mut status_line = String::new();
    reader.read_line(&mut status_line).unwrap();
    assert!(status_line.starts_with("HTTP/1.1 200"), "{status_line}");

    let pid = server.0.id().to_string();
    assert!(Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap()
        .success());
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "no graceful shutdown");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "exit status {status}");
    assert!(dir.path().join("data/zunder.db").is_file());
}

#[test]
fn invalid_configuration_fails_fast() {
    let output = Command::new(BIN)
        .env("ZUNDER_BACKUP_HOUR", "25")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ZUNDER_BACKUP_HOUR"));

    let output = Command::new(BIN).arg("unsinn").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}
