//! Throughput of property writes by key (G-76): Festimap's
//! `probe-write-throughput.ts` in Rust. An in-memory graph of `nodes`
//! entities under a uniqueness constraint on `key` and an index on the
//! sparse `dirty` marker, then each statement over every node in
//! `UNWIND` batches, as rows per second.
//!
//! ```text
//! cargo run --release -p lora-database --example keyed_set_probe -- [nodes] [batch] [only]
//! ```
//!
//! Statements run with a query timeout, as the Node binding's
//! `queryTimeoutMs` gives them. `only` names one statement to run (for a
//! profiler), several times over.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use lora_database::{Database, ExecuteOptions, LoraValue, ResultFormat};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: usize = args.get(1).map_or(200_000, |a| a.parse().unwrap());
    let batch: usize = args.get(2).map_or(5_000, |a| a.parse().unwrap());
    let only = args.get(3).cloned();
    let rows = Some(ExecuteOptions {
        format: ResultFormat::Rows,
    });
    let timeout = Duration::from_secs(600);
    let db = Database::in_memory();
    db.execute(
        "CREATE CONSTRAINT entity_key FOR (n:Entity) REQUIRE n.key IS UNIQUE",
        rows,
    )
    .unwrap();
    db.execute(
        "CREATE INDEX entity_dirty FOR (n:Entity) ON (n.dirty)",
        rows,
    )
    .unwrap();
    let keys: Vec<String> = (0..n).map(|i| format!("artist:a{i:08}")).collect();
    let string = |s: String| LoraValue::String(s);
    for chunk in keys.chunks(batch) {
        let list = chunk.iter().cloned().map(string).collect();
        db.execute_with_params_timeout(
            "UNWIND $keys AS k CREATE (:Entity {key: k, kind: 'artist', dirty: k, \
             state: 'ACTIVE', name: k, inScope: true, seen: 'ACTIVE:0'})",
            rows,
            BTreeMap::from([("keys".to_string(), LoraValue::List(list))]),
            timeout,
        )
        .unwrap();
    }

    type Row = fn(&str, usize) -> Vec<(&'static str, LoraValue)>;
    let map = |pairs: Vec<(&str, LoraValue)>| {
        LoraValue::Map(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    };
    let statements: [(&str, &str, Row); 6] = [
        (
            "null",
            "UNWIND $rows AS row MATCH (e:Entity {key: row.key}) SET e.dirty = null",
            |_, _| vec![],
        ),
        (
            "indexed",
            "UNWIND $rows AS row MATCH (e:Entity {key: row.key}) SET e.dirty = e.key",
            |_, _| vec![],
        ),
        (
            "four",
            "UNWIND $rows AS row MATCH (e:Entity {key: row.key}) \
             SET e.inScope = row.inScope, e.seen = row.seen, e.name = row.name, \
             e.promote = row.promote",
            |key, i| {
                vec![
                    ("inScope", LoraValue::Bool(i % 2 == 0)),
                    ("seen", LoraValue::String(format!("ACTIVE:{i}"))),
                    ("name", LoraValue::String(format!("{key}!"))),
                    ("promote", LoraValue::String(format!("artist:{i}|{key}"))),
                ]
            },
        ),
        (
            "map",
            "UNWIND $rows AS row MATCH (e:Entity {key: row.key}) SET e += row.set",
            |key, i| {
                vec![(
                    "set",
                    LoraValue::Map(BTreeMap::from([
                        ("inScope".to_string(), LoraValue::Bool(i % 2 == 1)),
                        (
                            "seen".to_string(),
                            LoraValue::String(format!("ACTIVE:{}", i + 1)),
                        ),
                        ("name".to_string(), LoraValue::String(format!("{key}?"))),
                        ("dirty".to_string(), LoraValue::Null),
                    ])),
                )]
            },
        ),
        (
            "case",
            "UNWIND $rows AS row MATCH (e:Entity {key: row.key}) \
             SET e.seen = CASE WHEN row.mark THEN row.seen ELSE e.seen END, \
             e.promote = CASE WHEN e.promote IS NULL THEN row.promote ELSE e.promote END",
            |_, i| {
                vec![
                    ("mark", LoraValue::Bool(i % 2 == 0)),
                    ("seen", LoraValue::String(format!("x{i}"))),
                    ("promote", LoraValue::String(format!("p{i}"))),
                ]
            },
        ),
        (
            "read",
            "UNWIND $rows AS row MATCH (e:Entity {key: row.key}) \
             RETURN e.key AS key, properties(e) AS props",
            |_, _| vec![],
        ),
    ];
    for (name, query, row) in statements {
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        let batches: Vec<LoraValue> = keys
            .chunks(batch)
            .enumerate()
            .map(|(b, chunk)| {
                LoraValue::List(
                    chunk
                        .iter()
                        .enumerate()
                        .map(|(j, key)| {
                            let mut pairs = row(key, b * batch + j);
                            pairs.push(("key", LoraValue::String(key.clone())));
                            map(pairs)
                        })
                        .collect(),
                )
            })
            .collect();
        let rounds = if only.is_some() { 10 } else { 1 };
        let started = Instant::now();
        for _ in 0..rounds {
            for rows_param in &batches {
                db.execute_with_params_timeout(
                    query,
                    rows,
                    BTreeMap::from([("rows".to_string(), rows_param.clone())]),
                    timeout,
                )
                .unwrap();
            }
        }
        let secs = started.elapsed().as_secs_f64();
        println!("{name:<8} {:>9.0} rows/s", (n * rounds) as f64 / secs);
    }
}
