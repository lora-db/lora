//! Memory-footprint benchmark for `InMemoryGraph`.
//!
//! Run with: `cargo bench -p lora-database --bench memory`
//!
//! Unlike the other benches in this directory, this one is *not* a
//! latency benchmark. It builds representative graph shapes, walks
//! the new [`MemoryReport`] estimator over each, and prints a
//! `grep`-friendly summary so a future regression gate can diff two
//! runs without having to re-derive the methodology.
//!
//! Output format (one line per scenario):
//!
//! ```text
//! memreport scenario=chain_100k nodes=100000 rels=99999 \
//!   total=NNNN graph=NNNN nodes_bytes=NNNN ... bytes_per_node=NNN.N
//! ```
//!
//! The Criterion harness is used only to wire `cargo bench --bench
//! memory` into the existing workflow; per-scenario timing is *not*
//! the headline metric (we ignore the iteration count and
//! re-construct the dataset for every iter to surface allocator
//! noise, not to micro-measure construction speed).

mod fixtures;

use criterion::{criterion_group, criterion_main, Criterion};
use fixtures::{
    build_chain, build_node_graph, build_social_graph, build_star, build_vector_graph, BenchDb,
};
use lora_database::{InMemoryGraph, MemoryReport};
use std::collections::BTreeMap;
use std::time::Duration;

fn bench_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(50))
        .measurement_time(Duration::from_millis(200))
        .sample_size(10)
}

/// Render one scenario's report as a `memreport` line. Keep the key
/// order stable so future tooling can `awk` over it.
fn print_report(scenario: &str, db: &BenchDb) {
    let report: MemoryReport = db
        .service
        .with_store(|s: &InMemoryGraph| s.memory_estimate());

    // Capture the breakdown via the public field set so any future
    // additions to `MemoryReport` show up in the output the next time
    // the bench is regenerated.
    let mut kv: BTreeMap<&str, u128> = BTreeMap::new();
    kv.insert("total", report.total_bytes() as u128);
    kv.insert("graph", report.graph_core_bytes() as u128);
    kv.insert("indexes", report.secondary_index_bytes() as u128);
    kv.insert("catalogs", report.catalog_bytes() as u128);
    kv.insert("nodes_bytes", report.nodes_bytes as u128);
    kv.insert("rels_bytes", report.relationships_bytes as u128);
    kv.insert("outgoing_bytes", report.outgoing_bytes as u128);
    kv.insert("incoming_bytes", report.incoming_bytes as u128);
    kv.insert("label_index_bytes", report.label_index_bytes as u128);
    kv.insert("type_index_bytes", report.type_index_bytes as u128);
    kv.insert("property_index_bytes", report.property_index_bytes as u128);
    kv.insert("sorted_index_bytes", report.sorted_index_bytes as u128);
    kv.insert("text_index_bytes", report.text_index_bytes as u128);
    kv.insert("point_index_bytes", report.point_index_bytes as u128);
    kv.insert("fulltext_index_bytes", report.fulltext_index_bytes as u128);
    kv.insert("vector_index_bytes", report.vector_index_bytes as u128);

    let mut line = format!(
        "memreport scenario={} nodes={} rels={} tomb_nodes={} tomb_rels={}",
        scenario,
        report.live_node_count,
        report.live_relationship_count,
        report.node_tombstone_count,
        report.relationship_tombstone_count,
    );
    for (k, v) in &kv {
        line.push_str(&format!(" {k}={v}"));
    }
    line.push_str(&format!(
        " bytes_per_node={:.1} bytes_per_rel={:.1}",
        report.bytes_per_live_node(),
        report.bytes_per_live_relationship(),
    ));
    println!("{line}");
}

/// Build a graph and run the documented workloads, capturing both
/// the post-build report and a post-query report — peak-bytes
/// regression candidates live in the difference, not the absolute
/// value.
fn print_with_query(scenario: &str, db: &BenchDb, query: &str) {
    print_report(&format!("{scenario}.build"), db);
    let _ = db.service.execute(query, None).unwrap();
    print_report(&format!("{scenario}.after_query"), db);
}

fn bench_memory(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_estimate");

    // ---- isolated nodes ----
    {
        let db = build_node_graph(10_000);
        print_report("node_only_10k", &db);
        group.bench_function("node_only_10k_estimate", |b| {
            b.iter(|| {
                let _ = db
                    .service
                    .with_store(|s: &InMemoryGraph| s.memory_estimate());
            });
        });
    }

    // ---- chains (1 edge / node) ----
    for &n in &[1_000usize, 10_000, 100_000] {
        let db = build_chain(n);
        print_with_query(
            &format!("chain_{n}"),
            &db,
            "MATCH (n:Chain) RETURN count(n) AS c",
        );
    }

    // ---- hub-and-spoke (star) ----
    for &spokes in &[1_000usize, 10_000, 100_000] {
        let db = build_star(spokes);
        print_with_query(
            &format!("star_{spokes}"),
            &db,
            "MATCH (h:Hub)-[:ARM]->(s) RETURN count(s) AS c",
        );
    }

    // ---- social graph (mixed fanout + properties) ----
    {
        let db = build_social_graph(20_000, 8);
        print_with_query(
            "social_20k_x8",
            &db,
            "MATCH (a:Person)-[:KNOWS]->(b) RETURN count(b) AS c",
        );
    }

    // ---- vector workloads ----
    {
        let db = build_vector_graph(5_000, 128, "cosine", "flat");
        print_report("vector_flat_5k_d128", &db);
    }
    {
        let db = build_vector_graph(5_000, 128, "cosine", "hnsw");
        print_report("vector_hnsw_5k_d128", &db);
    }

    group.finish();
}

criterion_group!(
    name = benches;
    config = bench_config();
    targets = bench_memory
);
criterion_main!(benches);
