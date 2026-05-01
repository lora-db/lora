//! Multi-threaded throughput benchmarks.
//!
//! These exist to validate the concurrent reads + writes story (Phase 1
//! lock-free reads via `ArcSwap`, Phase 2 cheap snapshot clones via
//! per-record Arcs, Phase 3 optimistic auto-commit writes with CAS
//! publish). The single-threaded smoke benches can't show whether
//! readers actually run in parallel or whether writers contend at the
//! commit point — that's what these measure.
//!
//! Each group sweeps a thread-count axis (1, 2, 4, 8) and reports
//! throughput. Linear scaling means "concurrency works for this
//! workload"; flat or worse-than-linear scaling means we're hitting a
//! contention point.

mod fixtures;

use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use fixtures::*;
use lora_database::{Database, ExecuteOptions, LoraValue, ResultFormat};

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

const THREAD_COUNTS: &[usize] = &[1, 2, 4, 8];

/// Pure read concurrency. The fixture is shared across threads; each
/// thread runs the same query repeatedly. Phase 1 (`ArcSwap` snapshot
/// reads) should let this scale ~linearly with thread count up to
/// physical core count. Anything sub-linear means readers are hitting
/// shared mutable state on the read path.
fn bench_concurrent_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_reads");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(15);

    let db = Arc::new(build_node_graph(Scale::SMALL).service);
    let queries_per_thread = 50usize;

    for &threads in THREAD_COUNTS {
        let total_ops = threads * queries_per_thread;
        group.throughput(Throughput::Elements(total_ops as u64));
        group.bench_with_input(
            BenchmarkId::new("threads", threads),
            &threads,
            |b, &threads| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let barrier = Arc::new(Barrier::new(threads));
                        let start = Instant::now();
                        let handles: Vec<_> = (0..threads)
                            .map(|_| {
                                let db = db.clone();
                                let barrier = barrier.clone();
                                std::thread::spawn(move || {
                                    barrier.wait();
                                    for _ in 0..queries_per_thread {
                                        let result = db
                                            .execute("MATCH (n:Node) RETURN n.id", opts())
                                            .unwrap();
                                        black_box(result);
                                    }
                                })
                            })
                            .collect();
                        for h in handles {
                            h.join().unwrap();
                        }
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }
    group.finish();
}

/// Concurrent CREATE workload — each thread creates nodes with
/// thread-disjoint labels/values. Phase 3's CAS-based OCC means
/// concurrent writers retry on every conflict, so even though the
/// data is logically disjoint we expect throughput to *not* scale
/// (and may even regress at high thread counts due to retry overhead).
/// This bench is the "honest motivation" for going to per-record
/// locks: at high thread counts the retry budget can be exhausted and
/// commits start failing outright. We cap the bench at 4 threads to
/// keep the data clean — that's already enough to show the trend.
fn bench_concurrent_creates(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_creates");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(15);

    let writes_per_thread = 50usize;
    // Stop at 4 threads. Past that, the optimistic CAS retry budget
    // gets exhausted under sustained contention and the bench would
    // panic — which is itself useful data, but not what we're trying
    // to measure here.
    let create_thread_counts: &[usize] = &[1, 2, 4];

    for &threads in create_thread_counts {
        let total_ops = threads * writes_per_thread;
        group.throughput(Throughput::Elements(total_ops as u64));
        group.bench_with_input(
            BenchmarkId::new("threads", threads),
            &threads,
            |b, &threads| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        // Fresh DB per iteration so timings reflect
                        // steady-state CREATE cost rather than
                        // accumulating-state cost.
                        let db = Arc::new(Database::in_memory());
                        let barrier = Arc::new(Barrier::new(threads));
                        let start = Instant::now();
                        let handles: Vec<_> = (0..threads)
                            .map(|tid| {
                                let db = db.clone();
                                let barrier = barrier.clone();
                                std::thread::spawn(move || {
                                    barrier.wait();
                                    for i in 0..writes_per_thread {
                                        let mut params = BTreeMap::new();
                                        params
                                            .insert("tid".to_string(), LoraValue::Int(tid as i64));
                                        params.insert("idx".to_string(), LoraValue::Int(i as i64));
                                        db.execute_with_params(
                                            "CREATE (:N {tid: $tid, idx: $idx})",
                                            opts(),
                                            params,
                                        )
                                        .unwrap();
                                    }
                                })
                            })
                            .collect();
                        for h in handles {
                            h.join().unwrap();
                        }
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }
    group.finish();
}

/// Mixed read + write concurrency. Holds the writer count fixed at 1
/// and scales the reader count. Phase 1 says writers don't block
/// readers and vice versa, so adding readers shouldn't regress the
/// single writer's throughput. If they do, the read path is
/// contending with the writer somewhere we missed.
fn bench_mixed_read_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_mixed");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(15);

    let ops_per_thread = 50usize;

    for &readers in &[0usize, 1, 4, 8] {
        let total_threads = readers + 1; // 1 writer
        let total_ops = total_threads * ops_per_thread;
        group.throughput(Throughput::Elements(total_ops as u64));
        group.bench_with_input(
            BenchmarkId::new("readers", readers),
            &readers,
            |b, &readers| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        // Fresh DB pre-seeded with 100 nodes so readers
                        // have something to scan from the very first
                        // iteration.
                        let db = Arc::new(BenchDb::new().service);
                        for i in 0..100 {
                            let mut params = BTreeMap::new();
                            params.insert("idx".to_string(), LoraValue::Int(i as i64));
                            db.execute_with_params("CREATE (:N {idx: $idx})", opts(), params)
                                .unwrap();
                        }

                        let total_threads = readers + 1;
                        let barrier = Arc::new(Barrier::new(total_threads));
                        let start = Instant::now();
                        let mut handles = Vec::with_capacity(total_threads);

                        // Reader threads.
                        for _ in 0..readers {
                            let db = db.clone();
                            let barrier = barrier.clone();
                            handles.push(std::thread::spawn(move || {
                                barrier.wait();
                                for _ in 0..ops_per_thread {
                                    let result =
                                        db.execute("MATCH (n:N) RETURN n.idx", opts()).unwrap();
                                    black_box(result);
                                }
                            }));
                        }

                        // Single writer thread.
                        {
                            let db = db.clone();
                            let barrier = barrier.clone();
                            handles.push(std::thread::spawn(move || {
                                barrier.wait();
                                for i in 0..ops_per_thread {
                                    let mut params = BTreeMap::new();
                                    params.insert(
                                        "idx".to_string(),
                                        LoraValue::Int((i + 1000) as i64),
                                    );
                                    db.execute_with_params(
                                        "CREATE (:N {idx: $idx})",
                                        opts(),
                                        params,
                                    )
                                    .unwrap();
                                }
                            }));
                        }

                        for h in handles {
                            h.join().unwrap();
                        }
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }
    group.finish();
}

criterion_group! {
    name = concurrent_benchmarks;
    config = Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2));
    targets =
        bench_concurrent_reads,
        bench_concurrent_creates,
        bench_mixed_read_write,
}
criterion_main!(concurrent_benchmarks);
