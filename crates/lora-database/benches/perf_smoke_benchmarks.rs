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
use lora_database::{ExecuteOptions, ResultFormat, TransactionMode};
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

    // --- 5. streaming read: full drain via Database::stream ----------------
    //
    // Exercises the live pull cursor (`StreamInner::Live`). Same workload
    // as `scan_1k` but goes through the streaming pipeline instead of
    // `execute_compiled_rows`. A regression here means the per-operator
    // pull pipeline got slower (or the LiveCursor mutex/guard wrapper did).
    {
        let db = build_node_graph(Scale::SMALL);
        group.bench_function("stream_scan_1k", |b| {
            b.iter(|| {
                let stream = db.service.stream("MATCH (n:Node) RETURN n.id").unwrap();
                let mut count = 0usize;
                for row in stream {
                    black_box(row);
                    count += 1;
                }
                black_box(count);
            });
        });
    }

    // --- 6. streaming read: pull-and-drop --------------------------------
    //
    // Pulls a single row then drops the stream. Should be O(1)-ish
    // regardless of underlying graph size — the test guards that the
    // pull pipeline stays lazy and the LiveCursor's Drop releases
    // promptly.
    {
        let db = build_node_graph(Scale::SMALL);
        group.bench_function("stream_pull_one", |b| {
            b.iter(|| {
                let mut stream = db.service.stream("MATCH (n:Node) RETURN n.id").unwrap();
                black_box(stream.next_row().unwrap());
                drop(stream);
            });
        });
    }

    // --- 7. streaming write: auto-commit drain ---------------------------
    //
    // Auto-commit write stream — exercises classify-stream → hidden tx
    // → mutable executor → AutoCommitGuard (delegating to
    // Transaction::commit). Fresh DB per iteration since the writes
    // commit on exhaustion.
    group.bench_function("stream_write_100", |b| {
        b.iter_batched(
            BenchDb::new,
            |db| {
                let stream = db
                    .service
                    .stream("UNWIND range(1, 100) AS i CREATE (:B {id: i}) RETURN i")
                    .unwrap();
                let mut count = 0usize;
                for row in stream {
                    black_box(row);
                    count += 1;
                }
                black_box(count);
            },
            BatchSize::SmallInput,
        );
    });

    // --- 8. transaction round-trip: empty begin → commit -----------------
    //
    // Measures the fixed cost of opening + committing an explicit
    // read-write transaction with zero statements. Staging is lazy, so this
    // should not clone the graph.
    group.bench_function("tx_roundtrip_empty", |b| {
        let db = build_node_graph(Scale::SMALL);
        b.iter(|| {
            let tx = db
                .service
                .begin_transaction(TransactionMode::ReadWrite)
                .unwrap();
            tx.commit().unwrap();
        });
    });

    // --- 9. transaction with one read statement --------------------------
    //
    // Single-statement read-only transaction: no staging clone, just the
    // transaction wrapper plus executor + commit.
    group.bench_function("tx_read_1k", |b| {
        let db = build_node_graph(Scale::SMALL);
        b.iter(|| {
            let mut tx = db
                .service
                .begin_transaction(TransactionMode::ReadOnly)
                .unwrap();
            black_box(tx.execute("MATCH (n:Node) RETURN n.id", opts()).unwrap());
            tx.commit().unwrap();
        });
    });

    // --- 10. transaction with one write statement ------------------------
    //
    // Single-statement read-write transaction. Fresh DB per
    // iteration because the writes get published on commit.
    group.bench_function("tx_write_100", |b| {
        b.iter_batched(
            BenchDb::new,
            |db| {
                let mut tx = db
                    .service
                    .begin_transaction(TransactionMode::ReadWrite)
                    .unwrap();
                black_box(
                    tx.execute("UNWIND range(1, 100) AS i CREATE (:B {id: i})", opts())
                        .unwrap(),
                );
                tx.commit().unwrap();
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
