//! Query-latency probe used while profiling.
//!
//! `cargo run --release -p lora-database --example perf_probe` prints
//! mean latency per scenario. `... -- <scenario> <seconds>` loops one
//! scenario for the given time so an external sampler can attach.

use std::hint::black_box;
use std::time::{Duration, Instant};

use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat};

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn run(db: &Database<InMemoryGraph>, q: &str) {
    db.execute(q, opts()).unwrap_or_else(|e| panic!("{q}: {e}"));
}

fn node_graph(n: usize) -> Database<InMemoryGraph> {
    let db = Database::in_memory();
    run(
        &db,
        &format!(
            "UNWIND list.range(0, {}) AS i CREATE (:Node {{id: i, name: 'node_' + type.cast(i, STRING), value: i % 100}})",
            n - 1
        ),
    );
    db
}

fn social_graph(n: usize) -> Database<InMemoryGraph> {
    let db = Database::in_memory();
    run(
        &db,
        &format!(
            "UNWIND list.range(0, {}) AS i CREATE (:Person {{id: i, age: 20 + (i % 41)}})",
            n - 1
        ),
    );
    run(&db, "CREATE INDEX person_id FOR (p:Person) ON (p.id)");
    run(
        &db,
        &format!(
            "UNWIND list.range(0, {}) AS i MATCH (a:Person {{id: i}}), (b:Person {{id: (i * 7 + 1) % {n}}}) CREATE (a)-[:KNOWS]->(b)",
            n - 1
        ),
    );
    run(
        &db,
        &format!(
            "UNWIND list.range(0, {}) AS i MATCH (a:Person {{id: i}}), (b:Person {{id: (i * 13 + 5) % {n}}}) CREATE (a)-[:KNOWS]->(b)",
            n - 1
        ),
    );
    db
}

/// 50k festivals with a uniqueness constraint, a full-text and a point
/// index: writes pay for copying every index their staged graph holds.
fn indexed_festivals() -> Database<InMemoryGraph> {
    let db = Database::in_memory();
    run(
        &db,
        "CREATE CONSTRAINT fk FOR (f:Festival) REQUIRE f.key IS UNIQUE",
    );
    run(
        &db,
        "CREATE FULLTEXT INDEX ft FOR (f:Festival) ON EACH [f.name]",
    );
    run(
        &db,
        "CREATE POINT INDEX loc FOR (f:Festival) ON (f.location)",
    );
    run(&db, "CREATE TEXT INDEX tx FOR (f:Festival) ON (f.name)");
    run(
        &db,
        "UNWIND range(0, 49999) AS i CREATE (:Festival {key: 'f' + toString(i), \
         name: 'Festival number ' + toString(i), \
         location: point({latitude: 50.0 + (i % 100) / 100.0, longitude: 4.0 + (i % 50) / 50.0})})",
    );
    db
}

struct Scenario {
    name: &'static str,
    db: fn() -> Database<InMemoryGraph>,
    query: &'static str,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "scan_1k",
        db: || node_graph(1_000),
        query: "MATCH (n:Node) RETURN n.id",
    },
    Scenario {
        name: "filter_1k",
        db: || node_graph(1_000),
        query: "MATCH (n:Node) WHERE n.value > 50 RETURN n.id",
    },
    Scenario {
        name: "return_node_1k",
        db: || node_graph(1_000),
        query: "MATCH (n:Node) RETURN n",
    },
    Scenario {
        name: "count_10k",
        db: || node_graph(10_000),
        query: "MATCH (n:Node) RETURN count(n)",
    },
    Scenario {
        name: "agg_10k",
        db: || node_graph(10_000),
        query: "MATCH (n:Node) RETURN n.value, count(*) AS c",
    },
    Scenario {
        name: "order_limit_10k",
        db: || node_graph(10_000),
        query: "MATCH (n:Node) RETURN n.name ORDER BY n.name DESC LIMIT 10",
    },
    Scenario {
        name: "expand_2hop_5k",
        db: || social_graph(5_000),
        query: "MATCH (a:Person)-[:KNOWS]->(b)-[:KNOWS]->(c) RETURN count(c)",
    },
    Scenario {
        name: "index_lookup",
        db: || social_graph(5_000),
        query: "MATCH (p:Person {id: 1234}) RETURN p.age",
    },
    Scenario {
        name: "write_one",
        db: || node_graph(10),
        query: "CREATE (:B {id: 1, val: 2})",
    },
    Scenario {
        name: "set_one_indexed",
        db: || social_graph(5_000),
        query: "MATCH (p:Person {id: 42}) SET p.age = p.age + 1",
    },
    Scenario {
        name: "create_rel_5k",
        db: || social_graph(5_000),
        query: "MATCH (a:Person {id: 1}), (b:Person {id: 2}) CREATE (a)-[:KNOWS]->(b)",
    },
    Scenario {
        name: "create_rel_50k",
        db: || social_graph(50_000),
        query: "MATCH (a:Person {id: 1}), (b:Person {id: 2}) CREATE (a)-[:KNOWS]->(b)",
    },
    Scenario {
        name: "set_indexed_50k",
        db: indexed_festivals,
        query: "MATCH (f:Festival {key: 'f42'}) SET f.visits = coalesce(f.visits, 0) + 1",
    },
    Scenario {
        name: "create_indexed_50k",
        db: indexed_festivals,
        query: "CREATE (:Festival {key: randomUUID(), name: 'New festival', \
                location: point({latitude: 51.0, longitude: 4.4})})",
    },
    Scenario {
        name: "merge_indexed_50k",
        db: indexed_festivals,
        query: "MERGE (f:Festival {key: 'f7'}) SET f.name = 'Renamed ' + randomUUID()",
    },
    Scenario {
        name: "fulltext_query_50k",
        db: indexed_festivals,
        query: "CALL db.index.fulltext.queryNodes('ft', 'festival 4242') YIELD node RETURN node.key AS k",
    },
    Scenario {
        name: "contains_query_50k",
        db: indexed_festivals,
        query: "MATCH (f:Festival) WHERE f.name CONTAINS 'number 4242' RETURN f.key AS k",
    },
    Scenario {
        name: "write_100",
        db: || node_graph(10),
        query: "UNWIND list.range(1, 100) AS i CREATE (:B {id: i, val: i * 2})",
    },
];

fn time(s: &Scenario, budget: Duration) -> (f64, u64) {
    let db = (s.db)();
    // Warm up: plan cache, allocator.
    for _ in 0..10 {
        black_box(db.execute(s.query, opts()).unwrap());
    }
    let start = Instant::now();
    let mut iters = 0u64;
    while start.elapsed() < budget {
        black_box(db.execute(s.query, opts()).unwrap());
        iters += 1;
    }
    (start.elapsed().as_nanos() as f64 / iters as f64, iters)
}

/// Same query through the pull pipeline (`Database::stream`), drained.
fn time_stream(s: &Scenario, budget: Duration) -> f64 {
    let db = (s.db)();
    let drain = |db: &Database<InMemoryGraph>| {
        let stream = db.stream(s.query).unwrap();
        let mut n = 0usize;
        for row in stream {
            black_box(row);
            n += 1;
        }
        black_box(n);
    };
    for _ in 0..10 {
        drain(&db);
    }
    let start = Instant::now();
    let mut iters = 0u64;
    while start.elapsed() < budget {
        drain(&db);
        iters += 1;
    }
    start.elapsed().as_nanos() as f64 / iters as f64
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, name, secs] = args.as_slice() {
        if flag == "--stream-one" {
            let s = SCENARIOS
                .iter()
                .find(|s| s.name == name)
                .expect("unknown scenario");
            let ns = time_stream(s, Duration::from_secs(secs.parse().unwrap()));
            println!("{} stream {:.0} ns/iter", s.name, ns);
            return;
        }
    }
    if args.get(1).map(String::as_str) == Some("--stream") {
        for s in SCENARIOS.iter().filter(|s| !s.name.starts_with("write")) {
            let eager = (0..3)
                .map(|_| time(s, Duration::from_millis(500)).0)
                .fold(f64::INFINITY, f64::min);
            let pull = (0..3)
                .map(|_| time_stream(s, Duration::from_millis(500)))
                .fold(f64::INFINITY, f64::min);
            println!(
                "cmp scenario={} eager_ns={eager:.0} pull_ns={pull:.0}",
                s.name
            );
        }
        return;
    }
    if let [_, name, secs] = args.as_slice() {
        let s = SCENARIOS
            .iter()
            .find(|s| s.name == name)
            .expect("unknown scenario");
        let (ns, iters) = time(s, Duration::from_secs(secs.parse().unwrap()));
        println!("{} {:.0} ns/iter ({iters} iters)", s.name, ns);
        return;
    }
    for s in SCENARIOS {
        // Best of three short runs damps scheduler noise.
        let best = (0..3)
            .map(|_| time(s, Duration::from_millis(700)).0)
            .fold(f64::INFINITY, f64::min);
        println!("perf scenario={} ns_per_iter={:.0}", s.name, best);
    }
}
