//! Festimap E-4 repro: a value the row carries (a collected list) should not
//! multiply the cost of an operator that makes a row per candidate.
//! `cargo run --release -p lora-database --example carried_probe [-- '<body>']`
//! times `MATCH (c:C {key: 'c'}) WITH c, <big> AS big <body>` with `[]` and
//! with a 10k-element list; the ratio should stay under 2x.
use std::time::Instant;

use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat};

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn time(db: &Database<InMemoryGraph>, q: &str) -> f64 {
    for _ in 0..3 {
        db.execute(q, opts()).unwrap();
    }
    let n = 50;
    let t = Instant::now();
    for _ in 0..n {
        db.execute(q, opts()).unwrap();
    }
    t.elapsed().as_secs_f64() * 1000.0 / n as f64
}

fn main() {
    let db = Database::in_memory();
    db.execute(
        "CREATE (c:C {key: 'c'}) WITH c UNWIND range(1, 200) AS i CREATE (:M {i: i})-[:IN]->(c)",
        opts(),
    )
    .unwrap();
    let bodies: Vec<String> = match std::env::args().nth(1) {
        Some(body) => vec![body],
        None => vec![
            "MATCH (c)<-[:IN]-(m:M) RETURN count(m) AS n, size(big) AS b".into(),
            "OPTIONAL MATCH (c)<-[:IN]-(m:M) RETURN count(m) AS n, size(big) AS b".into(),
        ],
    };
    for body in bodies {
        let q = |big: &str| format!("MATCH (c:C {{key: 'c'}}) WITH c, {big} AS big {body}");
        let small = time(&db, &q("[]"));
        let large = time(&db, &q("range(1, 10000)"));
        println!(
            "{body}\n  []: {small:.3} ms  10k: {large:.3} ms  ratio {:.1}x",
            large / small
        );
    }
}
