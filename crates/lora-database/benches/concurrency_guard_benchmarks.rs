//! Tight local performance guard for concurrency work.
//!
//! `perf_smoke_benchmarks` is the broad CI canary and intentionally allows
//! large runner noise. This suite is narrower and meant for phase-by-phase
//! local checks while changing the concurrency/write/WAL plumbing:
//!
//! ```text
//! cargo bench -p lora-database --bench concurrency_guard_benchmarks \
//!     -- --output-format bencher > /tmp/lora-before.bencher
//! # make one implementation step
//! cargo bench -p lora-database --bench concurrency_guard_benchmarks \
//!     -- --output-format bencher > /tmp/lora-after.bencher
//! node scripts/check-bench-delta.mjs \
//!     --baseline /tmp/lora-before.bencher \
//!     --current /tmp/lora-after.bencher \
//!     --threshold 1.15
//! ```
//!
//! The cases here deliberately cover the hot surfaces most likely to regress
//! while introducing concurrent writes: snapshot reads, live streams,
//! auto-commit writes, explicit transactions, mixed read/write threads, and
//! WAL-backed write paths.

mod fixtures;

use std::collections::BTreeMap;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use fixtures::{build_node_graph, BenchDb, Scale};
use lora_database::{
    Database, ExecuteOptions, InMemoryGraph, LoraValue, ResultFormat, SyncMode, TransactionMode,
    WalConfig,
};

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn guard_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(400))
        .measurement_time(Duration::from_millis(1_800))
        .sample_size(30)
        .noise_threshold(0.08)
}

struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lora-concurrency-guard-{}-{}-{}",
            tag,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn wal_config(dir: &std::path::Path, sync_mode: SyncMode) -> WalConfig {
    WalConfig::Enabled {
        dir: dir.to_path_buf(),
        sync_mode,
        segment_target_bytes: 8 * 1024 * 1024,
    }
}

fn int_param(name: &str, value: i64) -> BTreeMap<String, LoraValue> {
    let mut params = BTreeMap::new();
    params.insert(name.to_string(), LoraValue::Int(value));
    params
}

fn two_int_params(a: (&str, i64), b: (&str, i64)) -> BTreeMap<String, LoraValue> {
    let mut params = BTreeMap::new();
    params.insert(a.0.to_string(), LoraValue::Int(a.1));
    params.insert(b.0.to_string(), LoraValue::Int(b.1));
    params
}

fn bench_concurrency_guard(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrency_guard");

    // Snapshot read path: catches regressions in `LiveStore::load_full`,
    // plan execution, and read-only WAL prechecks.
    {
        let db = build_node_graph(Scale::SMALL);
        group.bench_function("read_scan_1k", |b| {
            b.iter(|| {
                black_box(
                    db.service
                        .execute("MATCH (n:Node) RETURN n.id", opts())
                        .unwrap(),
                );
            });
        });
    }

    // Live read stream: pins an Arc snapshot and drops after one row.
    {
        let db = build_node_graph(Scale::SMALL);
        group.bench_function("stream_pull_one_1k", |b| {
            b.iter(|| {
                let mut stream = db.service.stream("MATCH (n:Node) RETURN n.id").unwrap();
                black_box(stream.next_row().unwrap());
            });
        });
    }

    // Auto-commit create on one long-lived DB. This keeps the benchmark on the
    // write hot path instead of measuring fixture setup.
    {
        let db = BenchDb::new();
        let mut next = 0i64;
        group.bench_function("write_create_one_steady", |b| {
            b.iter(|| {
                next += 1;
                black_box(
                    db.service
                        .execute_with_params(
                            "CREATE (:Guard {id: $id})",
                            opts(),
                            int_param("id", next),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // Existing-record write on a 1k graph. Future per-record commit
    // validation should keep this close to the current single-writer cost.
    {
        let db = build_node_graph(Scale::SMALL);
        let mut next = 0i64;
        group.bench_function("write_set_existing_1k", |b| {
            b.iter(|| {
                next += 1;
                black_box(
                    db.service
                        .execute_with_params(
                            "MATCH (n:Node {id: $id}) SET n.value = $value",
                            opts(),
                            two_int_params(("id", 500), ("value", next)),
                        )
                        .unwrap(),
                );
            });
        });
    }

    // Fixed explicit transaction cost. This should stay tiny while
    // introducing concurrent write bookkeeping.
    {
        let db = build_node_graph(Scale::SMALL);
        group.bench_function("tx_roundtrip_empty", |b| {
            b.iter(|| {
                let tx = db
                    .service
                    .begin_transaction(TransactionMode::ReadWrite)
                    .unwrap();
                tx.commit().unwrap();
            });
        });
    }

    // Explicit transaction write path, including buffering recorder and
    // publish. Fresh DB per iteration so the committed write cannot
    // accumulate into the next sample.
    group.bench_function("tx_write_create_one", |b| {
        b.iter_batched(
            BenchDb::new,
            |db| {
                let mut tx = db
                    .service
                    .begin_transaction(TransactionMode::ReadWrite)
                    .unwrap();
                black_box(tx.execute("CREATE (:TxGuard {id: 1})", opts()).unwrap());
                tx.commit().unwrap();
            },
            BatchSize::SmallInput,
        );
    });

    // Mixed reader/writer pressure. Thread spawn overhead is intentional here:
    // this is a coarse local guard for accidental global locks on read paths.
    {
        let db = Arc::new(build_node_graph(Scale::SMALL).service);
        let mut next = 0i64;
        group.bench_function("mixed_4_readers_1_writer", |b| {
            b.iter(|| {
                next += 1;
                let barrier = Arc::new(Barrier::new(5));
                let handles: Vec<_> = (0..4)
                    .map(|_| {
                        let db = db.clone();
                        let barrier = barrier.clone();
                        std::thread::spawn(move || {
                            barrier.wait();
                            black_box(db.execute("MATCH (n:Node) RETURN n.id", opts()).unwrap());
                        })
                    })
                    .collect();

                let writer = {
                    let db = db.clone();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        black_box(
                            db.execute_with_params(
                                "CREATE (:MixedGuard {id: $id})",
                                opts(),
                                int_param("id", next),
                            )
                            .unwrap(),
                        );
                    })
                };

                for handle in handles {
                    handle.join().unwrap();
                }
                writer.join().unwrap();
            });
        });
    }

    // WAL write paths without the per-iteration directory setup measured.
    // `None` isolates WAL encoding/flush-buffer overhead; cooperative Group
    // isolates the path that future concurrent fsync coordination will touch.
    {
        let dir = ScratchDir::new("wal-none");
        let db = Database::<InMemoryGraph>::open_with_wal(wal_config(&dir.path, SyncMode::None))
            .unwrap();
        let mut next = 0i64;
        group.bench_function("wal_none_create_delete_one", |b| {
            b.iter(|| {
                next += 1;
                black_box(
                    db.execute_with_params(
                        "CREATE (n:WalGuard {id: $id}) DELETE n",
                        opts(),
                        int_param("id", next),
                    )
                    .unwrap(),
                );
            });
        });
    }

    {
        let dir = ScratchDir::new("wal-group");
        let db = Database::<InMemoryGraph>::open_with_wal(wal_config(
            &dir.path,
            SyncMode::Group { interval_ms: 50 },
        ))
        .unwrap();
        let mut next = 0i64;
        group.bench_function("wal_group_create_delete_one", |b| {
            b.iter(|| {
                next += 1;
                black_box(
                    db.execute_with_params(
                        "CREATE (n:WalGuard {id: $id}) DELETE n",
                        opts(),
                        int_param("id", next),
                    )
                    .unwrap(),
                );
            });
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = guard_config();
    targets = bench_concurrency_guard,
}
criterion_main!(benches);
