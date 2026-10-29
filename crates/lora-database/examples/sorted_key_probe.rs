//! Latency of the reads and writes a range-backed key constraint serves,
//! for A/B runs of the sorted index (G-64): Festimap-shaped keys
//! (`source:kind:<uuid>`), written in random order under the default
//! uniqueness constraint.
//!
//! ```text
//! cargo run --release -p lora-database --example sorted_key_probe -- [nodes]
//! ```
//!
//! Prints, per operation, the median and 99th percentile of 200 runs
//! after 20 warm-ups; `create` is the whole load, in rows per second.

use std::collections::BTreeMap;
use std::time::Instant;

use lora_database::{Database, ExecuteOptions, LoraValue, ResultFormat};

const SPACES: [&str; 4] = [
    "musicbrainz:release-group:",
    "musicbrainz:artist:",
    "openstreetmap:venue:node/",
    "wikidata:item:Q",
];

/// A deterministic, uuid-shaped key for `i`, in a scrambled order.
fn key(i: u64) -> String {
    let h = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (i >> 7);
    let g = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    format!(
        "{}{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        SPACES[(i % 4) as usize],
        h >> 32,
        (h >> 16) & 0xffff,
        h & 0xffff,
        g >> 48,
        g & 0xffff_ffff_ffff
    )
}

fn main() {
    let n: u64 = std::env::args()
        .nth(1)
        .map_or(200_000, |a| a.parse().expect("node count"));
    let rows = Some(ExecuteOptions {
        format: ResultFormat::Rows,
    });
    let db = Database::in_memory();
    db.execute(
        "CREATE CONSTRAINT rk FOR (r:Record) REQUIRE r.key IS UNIQUE",
        rows,
    )
    .unwrap();

    let started = Instant::now();
    for b in (0..n).step_by(5_000) {
        let keys: Vec<LoraValue> = (b..(b + 5_000).min(n))
            .map(|i| LoraValue::String(key(i)))
            .collect();
        let params = BTreeMap::from([("keys".to_string(), LoraValue::List(keys))]);
        db.execute_with_params(
            "UNWIND $keys AS k CREATE (:Record {key: k, hash: '0123456789abcdef'})",
            rows,
            params,
        )
        .unwrap();
    }
    let load = started.elapsed().as_secs_f64();
    println!("create      {:>9.0} rows/s", n as f64 / load);

    let mid = key(n / 2);
    let prefix = &mid[..mid.len() - 30];
    let time = |name: &str, query: &str, params: BTreeMap<String, LoraValue>| {
        for _ in 0..20 {
            db.execute_with_params(query, rows, params.clone()).unwrap();
        }
        let mut samples: Vec<f64> = (0..200)
            .map(|_| {
                let t = Instant::now();
                db.execute_with_params(query, rows, params.clone()).unwrap();
                t.elapsed().as_secs_f64() * 1e6
            })
            .collect();
        samples.sort_by(f64::total_cmp);
        println!(
            "{name:<11} p50 {:>8.1} us  p99 {:>8.1} us",
            samples[100], samples[198]
        );
    };
    let string = |s: &str| LoraValue::String(s.to_string());
    time(
        "keyset",
        "MATCH (r:Record) WHERE r.key > $after RETURN r.key AS k ORDER BY r.key LIMIT 100",
        BTreeMap::from([("after".to_string(), string(&mid))]),
    );
    time(
        "prefix",
        "MATCH (r:Record) WHERE r.key STARTS WITH $p AND r.key > $after \
         RETURN r.key AS k ORDER BY r.key LIMIT 100",
        BTreeMap::from([
            ("p".to_string(), string(prefix)),
            ("after".to_string(), string(prefix)),
        ]),
    );
    time(
        "lookup",
        "MATCH (r:Record {key: $k}) RETURN r.hash AS h",
        BTreeMap::from([("k".to_string(), string(&mid))]),
    );
    // A key moved and moved back: a remove and an insert in the index each.
    time(
        "rekey",
        "MATCH (r:Record {key: $k}) SET r.key = $k + 'x' WITH r SET r.key = $k",
        BTreeMap::from([("k".to_string(), string(&mid))]),
    );
}
