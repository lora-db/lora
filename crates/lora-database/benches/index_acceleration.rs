//! Index-acceleration benchmarks.
//!
//! Pairs every read query against two seeded copies of the same graph —
//! one with a property/text/point index, one without — so the runtime
//! delta directly attributes to the cost-based rewrite picking up the
//! index. Covers the four operators added in v0.8 plus their rel-side
//! mirrors:
//!
//! * `NodeByPropertyRangeScan` ← `WHERE n.prop > X`
//! * `NodeByTextScan`          ← `WHERE n.prop STARTS WITH …`
//! * `RelByPropertyRangeScan`  ← `MATCH ()-[r:T]->() WHERE r.prop > X`
//! * `RelByTextScan`           ← `MATCH ()-[r:T]->() WHERE r.prop STARTS WITH …`
//!
//! Run with:
//!   `cargo bench -p lora-database --bench index_acceleration`
//!
//! Each scenario seeds once and is reused across iterations; only the
//! query is measured. Set `LORA_BENCH_NODES` / `LORA_BENCH_RELS` in the
//! environment to override the defaults (10k / 50k) for a quick local
//! sweep, e.g. `LORA_BENCH_NODES=2000 LORA_BENCH_RELS=8000 cargo bench …`.

mod fixtures;

use std::env;
use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use fixtures::BenchDb;
use lora_database::{ExecuteOptions, ResultFormat};

const DEFAULT_NODES: usize = 10_000;
const DEFAULT_RELS: usize = 50_000;
const SEED_BATCH: usize = 2_000;

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn env_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn bench_config() -> Criterion {
    // Rel-side scenarios at default scale need ~3 ms per iter; 4 s of
    // measurement keeps the 30-sample target reachable without a
    // warning. Override with `--measurement-time` for shorter runs.
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_millis(4_000))
        .sample_size(30)
}

/// Seed `n` `:Person` nodes and `m` `:KNOWS` relationships connecting
/// random pairs. Properties are chosen so:
///
/// * `n.age` spans 0..100 → range queries hit ~half the corpus when
///   unindexed, a bounded slice when indexed.
/// * `n.name` is `'p_<i>'` → STARTS WITH 'p_5' matches ~1/10 of the
///   corpus, exercising the trigram path under load.
/// * `r.since` spans 1990..2030 → range queries split the rel set.
/// * `r.note` is `'note_<i>'` → STARTS WITH 'note_5' matches ~1/10.
///
/// The src/dst pairings are deterministic (seeded by index), so two
/// builds of the same `(nodes, rels)` produce identical edges — the
/// indexed and non-indexed databases see exactly the same data.
fn seed_graph(db: &BenchDb, nodes: usize, rels: usize) {
    let mut i = 0;
    while i < nodes {
        let end = (i + SEED_BATCH).min(nodes);
        db.run(&format!(
            "UNWIND range({i}, {}) AS i \
             CREATE (:Person {{id: i, age: i % 100, name: 'p_' + toString(i)}})",
            end - 1
        ));
        i = end;
    }

    let mut j = 0;
    while j < rels {
        let end = (j + SEED_BATCH).min(rels);
        db.run(&format!(
            "UNWIND range({j}, {}) AS i \
             MATCH (a:Person {{id: i % {nodes}}}), (b:Person {{id: (i * 7 + 3) % {nodes}}}) \
             CREATE (a)-[:KNOWS {{since: 1990 + i % 41, note: 'note_' + toString(i % 100), idx: i}}]->(b)",
            end - 1
        ));
        j = end;
    }
}

/// Build two databases with identical data but different index
/// catalogs: `(without_index, with_index)`. Index DDL runs *after*
/// the seed so the index is built once over the existing corpus
/// rather than incrementally per CREATE.
fn build_pair<F: Fn(&BenchDb)>(nodes: usize, rels: usize, install_index: F) -> (BenchDb, BenchDb) {
    let plain = BenchDb::with_capacity_hint(nodes, rels);
    seed_graph(&plain, nodes, rels);

    let indexed = BenchDb::with_capacity_hint(nodes, rels);
    seed_graph(&indexed, nodes, rels);
    install_index(&indexed);

    (plain, indexed)
}

fn run(db: &BenchDb, query: &str) {
    black_box(db.service.execute(query, opts()).unwrap());
}

fn bench_node_range(c: &mut Criterion) {
    let nodes = env_usize("LORA_BENCH_NODES", DEFAULT_NODES);
    let rels = env_usize("LORA_BENCH_RELS", DEFAULT_RELS);

    let (plain, indexed) = build_pair(nodes, rels, |db| {
        db.run("CREATE INDEX person_age FOR (n:Person) ON (n.age)");
    });

    let query = "MATCH (n:Person) WHERE n.age > 95 RETURN n.id";

    let mut group = c.benchmark_group("index_acceleration/node_range");
    group.bench_function("without_index", |b| b.iter(|| run(&plain, query)));
    group.bench_function("with_index", |b| b.iter(|| run(&indexed, query)));
    group.finish();
}

fn bench_node_text(c: &mut Criterion) {
    let nodes = env_usize("LORA_BENCH_NODES", DEFAULT_NODES);
    let rels = env_usize("LORA_BENCH_RELS", DEFAULT_RELS);

    let (plain, indexed) = build_pair(nodes, rels, |db| {
        db.run("CREATE TEXT INDEX person_name FOR (n:Person) ON (n.name)");
    });

    let query = "MATCH (n:Person) WHERE n.name STARTS WITH 'p_99' RETURN n.id";

    let mut group = c.benchmark_group("index_acceleration/node_text");
    group.bench_function("without_index", |b| b.iter(|| run(&plain, query)));
    group.bench_function("with_index", |b| b.iter(|| run(&indexed, query)));
    group.finish();
}

fn bench_rel_range(c: &mut Criterion) {
    let nodes = env_usize("LORA_BENCH_NODES", DEFAULT_NODES);
    let rels = env_usize("LORA_BENCH_RELS", DEFAULT_RELS);

    let (plain, indexed) = build_pair(nodes, rels, |db| {
        db.run("CREATE INDEX knows_since FOR ()-[r:KNOWS]-() ON (r.since)");
    });

    let query = "MATCH ()-[r:KNOWS]->() WHERE r.since > 2025 RETURN r.idx";

    let mut group = c.benchmark_group("index_acceleration/rel_range");
    group.bench_function("without_index", |b| b.iter(|| run(&plain, query)));
    group.bench_function("with_index", |b| b.iter(|| run(&indexed, query)));
    group.finish();
}

fn bench_rel_text(c: &mut Criterion) {
    let nodes = env_usize("LORA_BENCH_NODES", DEFAULT_NODES);
    let rels = env_usize("LORA_BENCH_RELS", DEFAULT_RELS);

    let (plain, indexed) = build_pair(nodes, rels, |db| {
        db.run("CREATE TEXT INDEX knows_note FOR ()-[r:KNOWS]-() ON (r.note)");
    });

    let query = "MATCH ()-[r:KNOWS]->() WHERE r.note STARTS WITH 'note_9' RETURN r.idx";

    let mut group = c.benchmark_group("index_acceleration/rel_text");
    group.bench_function("without_index", |b| b.iter(|| run(&plain, query)));
    group.bench_function("with_index", |b| b.iter(|| run(&indexed, query)));
    group.finish();
}

criterion_group! {
    name = index_acceleration;
    config = bench_config();
    targets =
        bench_node_range,
        bench_node_text,
        bench_rel_range,
        bench_rel_text,
}
criterion_main!(index_acceleration);
