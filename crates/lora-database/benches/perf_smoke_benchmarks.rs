//! Performance-regression smoke benchmarks.
//!
//! This is a deliberately tiny Criterion suite used as a CI "canary": it is
//! meant to detect obvious, large performance regressions (≥3× slower) in
//! core engine paths. It is **not** a source of truth for performance
//! numbers — see `engine_benchmarks`, `scale_benchmarks`,
//! `advanced_benchmarks`, and `temporal_spatial_benchmarks` for that.
//!
//! Run locally with:
//!   `cargo bench -p lora-database --bench perf_smoke_benchmarks`
//!
//! Regression-check with:
//!   `cargo bench -p lora-database --bench perf_smoke_benchmarks \
//!        -- --output-format bencher \
//!    | node scripts/check-perf-smoke.mjs`
//!
//! Baseline lives at
//! `crates/lora-database/benches/perf_smoke_baseline.json`. See
//! `docs/performance/perf-smoke.md` for the refresh flow.

mod fixtures;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use fixtures::*;
use lora_database::{ExecuteOptions, ResultFormat};
use std::hint::black_box;
use std::time::Duration;

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

/// CI-friendly Criterion config: short warmup + short measurement + modest
/// sample count. Total measurement budget per bench ≈ 1.8 s.
fn smoke_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_millis(1_500))
        .sample_size(30)
        // Don't fail the bench binary on small noise-driven regressions that
        // criterion flags; regression detection is handled by the external
        // check script against a checked-in baseline.
        .noise_threshold(0.10)
}

fn bench_perf_smoke(c: &mut Criterion) {
    let mut group = c.benchmark_group("perf_smoke");

    // --- 1. simple scan: MATCH + RETURN on 1 000 nodes ---------------------
    {
        let db = build_node_graph(Scale::SMALL);
        group.bench_function("scan_1k", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute("MATCH (n:Node) RETURN n.id", opts())
                        .unwrap(),
                );
            });
        });
    }

    // --- 2. filtered query: predicate evaluation on 1 000 nodes ------------
    {
        let db = build_node_graph(Scale::SMALL);
        group.bench_function("filter_1k", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute("MATCH (n:Node) WHERE n.value > 50 RETURN n.id", opts())
                        .unwrap(),
                );
            });
        });
    }

    // --- 3. single-hop traversal on a 500-node chain -----------------------
    {
        let db = build_chain(500);
        group.bench_function("traversal_chain_500", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute(
                            "MATCH (a:Chain)-[:NEXT]->(b:Chain) RETURN a.idx, b.idx",
                            opts(),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // --- 4. batched write: UNWIND + CREATE, fresh DB per iteration ---------
    group.bench_function("write_batch_100", |b| {
        b.iter_batched(
            BenchDb::new,
            |db| {
                black_box(
                    db.service
                        .execute(
                            "UNWIND range(1, 100) AS i CREATE (:B {id: i, val: i * 2})",
                            opts(),
                        )
                        .unwrap(),
                );
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = smoke_config();
    targets = bench_perf_smoke,
}
criterion_main!(benches);
