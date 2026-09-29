//! Festimap P1-1 repro: OPTIONAL MATCH vs pattern comprehension.
use std::collections::BTreeMap;
use std::time::Instant;

use lora_database::{Database, ExecuteOptions, InMemoryGraph, LoraValue, ResultFormat};

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn build(festivals: usize, per: usize) -> Database<InMemoryGraph> {
    let db = Database::in_memory();
    db.execute(
        &format!(
            "UNWIND range(0, {}) AS i CREATE (f:Festival {{key: toString(i)}}) \
             WITH f UNWIND range(0, {}) AS j \
             CREATE (:User {{key: f.key + '-' + toString(j)}})-[:FOLLOWS]->(f)",
            festivals - 1,
            per - 1
        ),
        opts(),
    )
    .unwrap();
    db
}

fn time(db: &Database<InMemoryGraph>, q: &str, params: &BTreeMap<String, LoraValue>) -> f64 {
    for _ in 0..3 {
        db.execute_with_params(q, opts(), params.clone()).unwrap();
    }
    let n = 10;
    let t = Instant::now();
    for _ in 0..n {
        db.execute_with_params(q, opts(), params.clone()).unwrap();
    }
    t.elapsed().as_secs_f64() * 1000.0 / n as f64
}

fn main() {
    let optional = "UNWIND $ks AS k MATCH (f:Festival {key: k}) OPTIONAL MATCH (f)<-[r:FOLLOWS]-() RETURN k, count(r) AS n";
    let comprehension = "UNWIND $ks AS k MATCH (f:Festival {key: k}) RETURN k, size([(f)<-[:FOLLOWS]-(u) | u]) AS n";
    if std::env::args().any(|a| a == "--loop") {
        let db = build(200, 30);
        let ks: Vec<LoraValue> = (0..100).map(|i| LoraValue::String(i.to_string())).collect();
        let mut params = BTreeMap::new();
        params.insert("ks".to_string(), LoraValue::List(ks));
        let t = Instant::now();
        while t.elapsed().as_secs() < 6 {
            db.execute_with_params(optional, opts(), params.clone())
                .unwrap();
        }
        return;
    }
    for (festivals, per) in [(200, 30), (20_000, 5)] {
        let db = build(festivals, per);
        let ks: Vec<LoraValue> = (0..100).map(|i| LoraValue::String(i.to_string())).collect();
        let mut params = BTreeMap::new();
        params.insert("ks".to_string(), LoraValue::List(ks));
        let a = time(&db, optional, &params);
        let b = time(&db, comprehension, &params);
        println!(
            "optional_probe festivals={festivals} follows={} optional_ms={a:.2} comprehension_ms={b:.2} ratio={:.1}",
            festivals * per,
            a / b
        );
    }
    if std::env::args().any(|a| a == "--explain") {
        let db = build(10, 2);
        let plan = db.explain(optional, None).unwrap();
        println!("{plan:#?}");
    }
}
