use lora_database::{Database, ExecuteOptions, ResultFormat};

fn rss_kb() -> u64 {
    let pid = std::process::id();
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
}

fn opts() -> Option<ExecuteOptions> { Some(ExecuteOptions { format: ResultFormat::Rows }) }

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(500);
    println!("before anything: {}", rss_kb());
    let svc = Database::in_memory();
    println!("after service creation: {}", rss_kb());

    svc.execute(&format!("UNWIND range(0, {}) AS i CREATE (:Chain {{idx: i}})", n - 1), opts()).unwrap();
    println!("after node creation: {}", rss_kb());

    svc.execute(&format!("UNWIND range(0, {}) AS i MATCH (a:Chain {{idx: i}}), (b:Chain {{idx: i+1}}) CREATE (a)-[:NEXT]->(b)", n - 2), opts()).unwrap();
    println!("after edge creation: {}", rss_kb());

    for _ in 0..3 { let _ = svc.execute("MATCH (n:Chain) RETURN count(n) AS c", opts()).unwrap(); }
    println!("after warmup: {}", rss_kb());

    // Now do 100 runs of a shortestPath query
    for _ in 0..100 {
        let _ = svc.execute("MATCH p = shortestPath((a:Chain {idx:0})-[:NEXT*]->(b:Chain {idx:10})) RETURN length(p) AS len", opts()).unwrap();
    }
    println!("after 100 shortestPath: {}", rss_kb());

    for _ in 0..1000 {
        let _ = svc.execute("MATCH p = shortestPath((a:Chain {idx:0})-[:NEXT*]->(b:Chain {idx:10})) RETURN length(p) AS len", opts()).unwrap();
    }
    println!("after 1100 shortestPath: {}", rss_kb());
}
