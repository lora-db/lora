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

/// Print one labelled snapshot — RSS plus the `MemoryReport` summary
/// — so the bulk-create benchmark surfaces both process-wide growth
/// and the per-component attribution from the engine.
fn snapshot(label: &str, svc: &Database<InMemoryGraph>) {
    let summary = svc.with_store(|g| g.memory_estimate().summary());
    println!("[{label}] rss_kb={} {summary}", rss_kb());
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let pattern: String = std::env::args().nth(2).unwrap_or_else(|| "original".into());
    println!("n={} pattern={}", n, pattern);

    let svc = Database::in_memory();
    snapshot("start", &svc);

    svc.execute(
        &format!("UNWIND range(0, {}) AS i CREATE (:Chain {{idx: i}})", n - 1),
        opts(),
    )
    .unwrap();
    snapshot("after nodes", &svc);

    let q: String = match pattern.as_str() {
        "original" => format!("UNWIND range(0, {}) AS i MATCH (a:Chain {{idx: i}}), (b:Chain {{idx: i+1}}) CREATE (a)-[:NEXT]->(b)", n-2),
        "collect" => "MATCH (a:Chain) WITH a ORDER BY a.idx WITH collect(a) AS nodes UNWIND range(0, size(nodes)-2) AS i WITH nodes[i] AS a, nodes[i+1] AS b CREATE (a)-[:NEXT]->(b)".into(),
        "small_batch" => {
            // Same as original but smaller batch would need to be run in a loop
            // Simulate by batching manually here
            let batch = 100;
            let mut i = 0;
            while i < n - 1 {
                let end = (i + batch).min(n - 1);
                svc.execute(&format!("UNWIND range({i}, {}) AS i MATCH (a:Chain {{idx: i}}), (b:Chain {{idx: i+1}}) CREATE (a)-[:NEXT]->(b)", end-1), opts()).unwrap();
                i = end;
            }
            snapshot("after edges (small_batch=100)", &svc);
            return;
        }
        _ => panic!()
    };
    svc.execute(&q, opts()).unwrap();
    snapshot(&format!("after edges ({pattern})"), &svc);

    // Now verify edge count
    let r = svc
        .execute("MATCH ()-[r:NEXT]->() RETURN count(r) AS c", opts())
        .unwrap();
    println!("edges: {:?}", r);
}
