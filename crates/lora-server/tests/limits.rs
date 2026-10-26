//! Query limits: a slow query neither stalls the server nor runs forever.
//!
//! Engine calls run on the blocking pool under a cancellable deadline, so
//! `/health` stays responsive while a query runs, a query past
//! `QueryLimits::timeout` fails with `LORA_TIMEOUT`, and a query whose
//! client hangs up is cancelled and gives its slot back.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use lora_database::Database;
use lora_server::{serve_with, QueryLimits};

/// A read that takes minutes in a dev build: a three-way cartesian
/// product over `n` nodes, streamed into a count.
const SLOW_QUERY: &str = r#"{"query":"MATCH (a:N), (b:N), (c:N) RETURN count(*) AS c"}"#;

fn seeded_db() -> Arc<Database<lora_database::InMemoryGraph>> {
    let db = Arc::new(Database::in_memory());
    db.execute("UNWIND range(1, 2000) AS i CREATE (:N {i: i})", None)
        .unwrap();
    db
}

fn start_server(
    db: Arc<Database<lora_database::InMemoryGraph>>,
    limits: QueryLimits,
) -> SocketAddr {
    let (addr_tx, addr_rx) = mpsc::channel();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            addr_tx.send(listener.local_addr().unwrap()).unwrap();
            serve_with(listener, db, limits, None).await.expect("serve");
        });
    });
    addr_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("server did not start")
}

fn send(addr: SocketAddr, method: &str, path: &str, body: &str) -> TcpStream {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    stream
}

fn read_response(mut stream: TcpStream) -> (String, String) {
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    let status = response.lines().next().unwrap_or_default().to_string();
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

fn request(addr: SocketAddr, method: &str, path: &str, body: &str) -> (String, String) {
    read_response(send(addr, method, path, body))
}

#[test]
fn slow_query_times_out_while_health_stays_responsive() {
    let addr = start_server(
        seeded_db(),
        QueryLimits {
            timeout: Some(Duration::from_millis(1500)),
            ..QueryLimits::default()
        },
    );

    let started = Instant::now();
    let slow = send(addr, "POST", "/query", SLOW_QUERY);

    // The slow query runs on the blocking pool; health is answered at once.
    thread::sleep(Duration::from_millis(200));
    let health_started = Instant::now();
    let (status, _) = request(addr, "GET", "/health", "");
    assert!(status.contains(" 200 "), "{status}");
    assert!(
        health_started.elapsed() < Duration::from_millis(1000),
        "health waited {:?} behind a slow query",
        health_started.elapsed()
    );

    let (status, body) = read_response(slow);
    assert!(status.contains(" 408 "), "{status}: {body}");
    assert!(body.contains("LORA_TIMEOUT"), "{body}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "timeout took {:?}",
        started.elapsed()
    );

    // /profile is bounded by the same budget.
    let (status, body) = request(addr, "POST", "/profile", SLOW_QUERY);
    assert!(status.contains(" 408 "), "{status}: {body}");
}

#[test]
fn client_disconnect_cancels_query_and_frees_its_slot() {
    // One slot and no timeout: the only way the second request gets to run
    // is the first being cancelled when its client hangs up.
    let addr = start_server(
        seeded_db(),
        QueryLimits {
            timeout: None,
            max_concurrent: Some(1),
            ..QueryLimits::default()
        },
    );

    let slow = send(addr, "POST", "/query", SLOW_QUERY);
    thread::sleep(Duration::from_millis(300));
    slow.shutdown(std::net::Shutdown::Both).unwrap();
    drop(slow);

    let started = Instant::now();
    let (status, body) = request(
        addr,
        "POST",
        "/query",
        r#"{"query":"MATCH (n:N) RETURN count(n) AS c","format":"rows"}"#,
    );
    assert!(status.contains(" 200 "), "{status}: {body}");
    assert!(body.contains(r#""c":2000"#), "{body}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "second query waited {:?}: the disconnected one kept running",
        started.elapsed()
    );
}

#[test]
fn oversized_body_is_rejected() {
    let addr = start_server(
        Arc::new(Database::in_memory()),
        QueryLimits {
            max_body_bytes: 1024,
            ..QueryLimits::default()
        },
    );
    let padding = "x".repeat(4096);
    let body = format!(r#"{{"query":"RETURN 1 AS one","params":{{"pad":"{padding}"}}}}"#);
    let (status, _) = request(addr, "POST", "/query", &body);
    assert!(status.contains(" 413 "), "{status}");

    let (status, _) = request(addr, "POST", "/query", r#"{"query":"RETURN 1 AS one"}"#);
    assert!(status.contains(" 200 "), "{status}");
}
