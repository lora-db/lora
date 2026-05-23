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

/// Append one probe row in `iter,rss_kb,total,graph,indexes,catalogs`
/// format — one line per sample so callers can paste the output into
/// a spreadsheet. `total` is the `MemoryReport` total bytes; `graph`
/// is the core (slabs + adjacency + label/type) breakdown; the rest
/// fall into the indexes / catalogs columns.
fn probe(svc: &Database<InMemoryGraph>, iter: usize) {
    let r = svc.with_store(|g| g.memory_estimate());
    println!(
        "{iter},{},{},{},{},{}",
        rss_kb(),
        r.total_bytes(),
        r.graph_core_bytes(),
        r.secondary_index_bytes(),
        r.catalog_bytes(),
    );
}

fn build_chain(n: usize) -> Database<InMemoryGraph> {
    let db = Database::in_memory();
    let batch = 2_000;
    let mut i = 0;
    while i < n {
        let end = (i + batch).min(n);
        db.execute(
            &format!(
                "UNWIND range({i}, {}) AS i CREATE (:Chain {{idx: i}})",
                end - 1
            ),
            opts(),
        )
        .unwrap();
        i = end;
    }
    let mut i = 0;
    while i < n - 1 {
        let end = (i + batch).min(n - 1);
        db.execute(&format!(
            "UNWIND range({i}, {}) AS i MATCH (a:Chain {{idx: i}}), (b:Chain {{idx: i+1}}) CREATE (a)-[:NEXT]->(b)", end - 1), opts()).unwrap();
        i = end;
    }
    db
}

fn main() {
    let scenario = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "varlen".to_string());
    let iters: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let probe_every: usize = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);

    println!("scenario: {scenario}");
    println!("iter,rss_kb,total,graph,indexes,catalogs");

    match scenario.as_str() {
        "varlen" => {
            let svc = build_chain(100);
            probe(&svc, 0);
            for k in 1..=iters {
                let _ = std::hint::black_box(
                    svc.execute(
                        "MATCH (a:Chain {idx:0})-[:NEXT*1..5]->(b) RETURN b.idx",
                        opts(),
                    )
                    .unwrap(),
                );
                if k % probe_every == 0 {
                    probe(&svc, k);
                }
            }
        }
        "varlen_unbounded" => {
            let svc = build_chain(500);
            probe(&svc, 0);
            for k in 1..=iters {
                let _ = std::hint::black_box(
                    svc.execute(
                        "MATCH (a:Chain {idx:0})-[:NEXT*]->(b) RETURN count(b) AS cnt",
                        opts(),
                    )
                    .unwrap(),
                );
                if k % probe_every == 0 {
                    probe(&svc, k);
                }
            }
        }
        "shortest" => {
            let svc = build_chain(500);
            probe(&svc, 0);
            for k in 1..=iters {
                let _ = std::hint::black_box(svc.execute(
                    "MATCH p = shortestPath((a:Chain {idx:0})-[:NEXT*]->(b:Chain {idx:10})) RETURN length(p) AS len", opts()).unwrap());
                if k % probe_every == 0 {
                    probe(&svc, k);
                }
            }
        }
        "bulk_create" => {
            // Repeatedly CREATE then DELETE batches to see retention.
            let svc = Database::in_memory();
            probe(&svc, 0);
            for k in 1..=iters {
                svc.execute(
                    "UNWIND list.range(1, 100) AS i CREATE (:Tmp {id: i, name: 'x' + type.cast(i, STRING)})",
                    opts(),
                )
                .unwrap();
                svc.execute("MATCH (n:Tmp) DETACH DELETE n", opts())
                    .unwrap();
                if k % probe_every == 0 {
                    probe(&svc, k);
                }
            }
        }
        "fresh_db_each_iter" => {
            // RSS-only here — the per-iter graph is dropped, so a
            // per-graph MemoryReport would always be near zero.
            println!("0,{},,,,", rss_kb());
            for k in 1..=iters {
                let svc = Database::in_memory();
                svc.execute("UNWIND range(1, 100) AS i CREATE (:Tmp {id: i})", opts())
                    .unwrap();
                if k % probe_every == 0 {
                    println!("{k},{},,,,", rss_kb());
                }
                drop(svc);
            }
        }
        "optional_match" => {
            // Build a small social graph
            let svc = Database::in_memory();
            svc.execute(
                "UNWIND list.range(0, 49) AS i CREATE (:Person {id: i, name: 'p_' + type.cast(i, STRING)})",
                opts(),
            )
            .unwrap();
            for j in 1..=3 {
                svc.execute(&format!(
                    "UNWIND range(0, 49) AS i MATCH (a:Person {{id: i}}), (b:Person {{id: (i + {j}) % 50}}) CREATE (a)-[:KNOWS]->(b)"), opts()).unwrap();
            }
            probe(&svc, 0);
            for k in 1..=iters {
                let _ = std::hint::black_box(svc.execute(
                    "MATCH (p:Person) OPTIONAL MATCH (p)-[:KNOWS]->(f) RETURN p.id, count(f) AS friends", opts()).unwrap());
                if k % probe_every == 0 {
                    probe(&svc, k);
                }
            }
        }
        _ => panic!("unknown scenario {scenario}"),
    }
}
