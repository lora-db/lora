//! Concurrent writes never stall the server.
//!
//! The binary serves every request on one current-thread tokio runtime
//! (`main.rs`) and each handler runs its query synchronously, with no
//! `.await` while the engine holds the writer lock. So a write never waits
//! for a lock held by another request: requests run one after another, and
//! health checks and reads are answered between them. This guard starts the
//! server on the same runtime shape, sends many concurrent writes plus reads
//! and health checks from client threads, and fails (instead of hanging)
//! if they do not all finish in time.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use lora_database::Database;
use lora_server::serve;

/// Start `serve` on its own thread with a current-thread runtime, as
/// `main.rs` does, and return the bound address.
fn start_server(db: Arc<Database<lora_database::InMemoryGraph>>) -> SocketAddr {
    let (addr_tx, addr_rx) = mpsc::channel();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            addr_tx.send(listener.local_addr().unwrap()).unwrap();
            serve(listener, db).await.expect("serve");
        });
    });
    addr_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("server did not start")
}

/// One HTTP/1.1 request on a fresh connection; returns the status line
/// and body.
fn request(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> (String, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let body = body.unwrap_or("");
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    let status = response.lines().next().unwrap_or_default().to_string();
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

#[test]
fn concurrent_writes_reads_and_health_all_finish() {
    const WRITERS: usize = 64;
    const READERS: usize = 16;

    let db = Arc::new(Database::in_memory());
    let addr = start_server(Arc::clone(&db));

    let (done_tx, done_rx) = mpsc::channel::<(&'static str, String)>();
    for i in 0..WRITERS {
        let done = done_tx.clone();
        thread::spawn(move || {
            // Each write does enough work to overlap with the others.
            let body = format!(
                r#"{{"query":"UNWIND range(1, 200) AS j CREATE (:W {{w: $w, j: j}})","params":{{"w":{i}}}}}"#
            );
            let (status, _) = request(addr, "POST", "/query", Some(&body));
            let _ = done.send(("write", status));
        });
    }
    for _ in 0..READERS {
        let done = done_tx.clone();
        thread::spawn(move || {
            let (status, _) = request(
                addr,
                "POST",
                "/query",
                Some(r#"{"query":"MATCH (n:W) RETURN count(n) AS c"}"#),
            );
            let _ = done.send(("read", status));
            let (status, _) = request(addr, "GET", "/health", None);
            let _ = done.send(("health", status));
        });
    }
    drop(done_tx);

    let expected = WRITERS + 2 * READERS;
    for n in 0..expected {
        let (kind, status) = done_rx
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| panic!("server stalled: {n} of {expected} requests answered"));
        assert!(status.contains(" 200 "), "{kind} failed: {status}");
    }

    let (status, body) = request(
        addr,
        "POST",
        "/query",
        Some(r#"{"query":"MATCH (n:W) RETURN count(n) AS c","format":"rows"}"#),
    );
    assert!(status.contains(" 200 "), "{status}");
    assert!(
        body.contains(&format!(r#""c":{}"#, WRITERS * 200)),
        "every write committed: {body}"
    );
}
