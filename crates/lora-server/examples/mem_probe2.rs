use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat};

fn rss_kb() -> u64 {
    let pid = std::process::id();
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

/// Print one labelled snapshot: process RSS plus the
/// `MemoryReport::summary()` line attributing retained bytes to graph
/// components. Together they triangulate whether a leak is in the
/// graph proper, the query pipeline, or the allocator's freelist.
fn snapshot(label: &str, svc: &Database<InMemoryGraph>) {
    let summary = svc.with_store(|g| g.memory_estimate().summary());
    println!("[{label}] rss_kb={} {summary}", rss_kb());
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    println!("[before anything] rss_kb={}", rss_kb());
    let svc = Database::in_memory();
    snapshot("after service creation", &svc);

    svc.execute(
        &format!("UNWIND range(0, {}) AS i CREATE (:Chain {{idx: i}})", n - 1),
        opts(),
    )
    .unwrap();
    snapshot("after node creation", &svc);

    svc.execute(&format!("UNWIND range(0, {}) AS i MATCH (a:Chain {{idx: i}}), (b:Chain {{idx: i+1}}) CREATE (a)-[:NEXT]->(b)", n - 2), opts()).unwrap();
    snapshot("after edge creation", &svc);

    for _ in 0..3 {
        let _ = svc
            .execute("MATCH (n:Chain) RETURN count(n) AS c", opts())
            .unwrap();
    }
    snapshot("after warmup", &svc);

    // Now do 100 runs of a shortestPath query
    for _ in 0..100 {
        let _ = svc.execute("MATCH p = shortestPath((a:Chain {idx:0})-[:NEXT*]->(b:Chain {idx:10})) RETURN length(p) AS len", opts()).unwrap();
    }
    snapshot("after 100 shortestPath", &svc);

    for _ in 0..1000 {
        let _ = svc.execute("MATCH p = shortestPath((a:Chain {idx:0})-[:NEXT*]->(b:Chain {idx:10})) RETURN length(p) AS len", opts()).unwrap();
    }
    snapshot("after 1100 shortestPath", &svc);
}
