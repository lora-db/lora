//! Phase 16 probe: condition pushdown, `IN` seeks and label counts.
//!
//! Run with `cargo run --release -p lora-database --example pushdown_probe`.
//! Builds a festival graph (users follow festivals), then times the query
//! shapes the lora-graphql package emits.
use lora_database::{Database, ExecuteOptions, InMemoryGraph, LoraValue, ResultFormat};
use std::collections::BTreeMap;
use std::time::Instant;

const FESTIVALS: usize = 20_000;
const USERS: usize = 20_000;
const FOLLOWS_PER_USER: usize = 10;

fn p50_ms(db: &Database<InMemoryGraph>, q: &str, params: &BTreeMap<String, LoraValue>) -> f64 {
    let o = Some(ExecuteOptions {
        format: ResultFormat::Rows,
    });
    for _ in 0..3 {
        db.execute_with_params(q, o, params.clone()).unwrap();
    }
    let mut samples: Vec<f64> = (0..15)
        .map(|_| {
            let t = Instant::now();
            db.execute_with_params(q, o, params.clone()).unwrap();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    samples[samples.len() / 2]
}

fn main() {
    let db = Database::in_memory();
    for q in [
        "CREATE CONSTRAINT fk IF NOT EXISTS FOR (n:Festival) REQUIRE n.key IS UNIQUE",
        "CREATE CONSTRAINT uk IF NOT EXISTS FOR (n:User) REQUIRE n.key IS UNIQUE",
    ] {
        db.execute(q, None).unwrap();
    }
    db.execute(
        &format!(
            "UNWIND range(0, {}) AS i CREATE (:Festival {{key: 'f' + toString(i)}})",
            FESTIVALS - 1
        ),
        None,
    )
    .unwrap();
    db.execute(
        &format!(
            "UNWIND range(0, {}) AS i CREATE (:User {{key: 'u' + toString(i)}})",
            USERS - 1
        ),
        None,
    )
    .unwrap();
    db.execute(
        &format!(
            "UNWIND range(0, {}) AS i \
             MATCH (u:User {{key: 'u' + toString(i)}}) \
             UNWIND range(1, {FOLLOWS_PER_USER}) AS j \
             MATCH (f:Festival {{key: 'f' + toString((i * 7 + j * 131) % {FESTIVALS})}}) \
             CREATE (u)-[:FOLLOWS]->(f)",
            USERS - 1
        ),
        None,
    )
    .unwrap();

    let key_params = BTreeMap::from([
        ("x".to_string(), LoraValue::String("f138".to_string())),
        ("y".to_string(), LoraValue::String("u1".to_string())),
    ]);
    let rows = LoraValue::List(vec![LoraValue::Map(BTreeMap::from([
        ("from".to_string(), LoraValue::String("f138".to_string())),
        ("to".to_string(), LoraValue::String("u1".to_string())),
    ]))]);
    let rows_params = BTreeMap::from([("rows".to_string(), rows)]);
    let keys = LoraValue::List(
        (0..20)
            .map(|i| LoraValue::String(format!("f{}", i * 97)))
            .collect(),
    );
    let in_params = BTreeMap::from([("keys".to_string(), keys)]);
    let none = BTreeMap::new();

    let cases: [(&str, &str, &BTreeMap<String, LoraValue>); 6] = [
        (
            "pushdown_where_both_ends",
            "MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) WHERE a.key = $x AND b.key = $y RETURN count(r) AS c",
            &key_params,
        ),
        (
            "pushdown_unwind_rows",
            "UNWIND $rows AS row MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) WHERE a.key = row.from AND b.key = row.to RETURN count(r) AS c",
            &rows_params,
        ),
        (
            "start_from_bound_tail",
            "MATCH (b:User {key: $y}) MATCH (a:Festival)<-[r:FOLLOWS]-(b) RETURN count(r) AS c",
            &key_params,
        ),
        (
            "in_list_seek",
            "MATCH (f:Festival) WHERE f.key IN $keys RETURN count(f) AS c",
            &in_params,
        ),
        ("count_label", "MATCH (f:Festival) RETURN count(f) AS c", &none),
        ("count_star_label", "MATCH (f:Festival) RETURN count(*) AS c", &none),
    ];
    for (name, q, params) in cases {
        let result = db
            .execute_with_params(
                q,
                Some(ExecuteOptions {
                    format: ResultFormat::Rows,
                }),
                params.clone(),
            )
            .unwrap();
        println!(
            "{name} p50_ms={:.4} result={}",
            p50_ms(&db, q, params),
            serde_json::to_string(&result).unwrap()
        );
    }
}
