//! WAL-aware microbenchmarks.
//!
//! These exercise the four durability profiles end-to-end through
//! `Database::execute_with_params`:
//!
//! - **`no_wal`**         — `Database::in_memory()` (the existing fast
//!                          path; serves as the baseline the others are
//!                          compared against).
//! - **`per_commit`**     — `WalConfig::Enabled` with `SyncMode::PerCommit`
//!                          (fsync before every commit returns).
//! - **`group`**          — `WalConfig::Enabled` with `SyncMode::Group`
//!                          (write-only on commit, bg flusher fsyncs).
//! - **`none`**           — `WalConfig::Enabled` with `SyncMode::None`
//!                          (no fsync at all, OS-buffered).
//!
//! The shape that matters is *commit latency* — every iteration runs a
//! single tiny `CREATE` statement so the engine work is negligible and
//! the WAL path dominates. On NVMe the gap between `no_wal` and
//! `per_commit` is roughly the cost of one `fsync` (50–200 µs); the
//! gap between `per_commit` and `group` / `none` measures how much
//! you save by deferring durability.
//!
//! There is also a small **`recovery`** bench that times opening a WAL
//! with N committed transactions and re-applying them. This is the
//! number that bounds startup time on a fresh process.
//!
//! Run with:
//!   `cargo bench -p lora-database --bench wal_benchmarks`

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use lora_database::{Database, ExecuteOptions, ResultFormat, SyncMode, WalConfig};
use std::hint::black_box;

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

/// Per-iteration scratch directory. Criterion's `iter_batched_setup`
/// runs setup once per batch, so this is called once per Criterion
/// "sample" — fine even at sample_size=20.
struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lora-wal-bench-{}-{}-{}",
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

fn enabled(dir: &std::path::Path, sync_mode: SyncMode) -> WalConfig {
    WalConfig::Enabled {
        dir: dir.to_path_buf(),
        sync_mode,
        segment_target_bytes: 8 * 1024 * 1024,
    }
}

fn smoke_config() -> Criterion {
    // Per-bench budget of ~3 s. fsync cost dominates per_commit so we
    // cannot run this as cheaply as the in-memory smoke suite, but we
    // also don't want a 30-second bench on every CI run.
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_millis(2_500))
        .sample_size(20)
        .noise_threshold(0.10)
}

fn bench_commit_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("wal/commit_latency");

    // ---- baseline: no WAL ---------------------------------------------------
    {
        group.bench_function("no_wal", |b| {
            b.iter_batched(
                Database::in_memory,
                |db| {
                    black_box(db.execute("CREATE (:N {v: 1})", opts()).unwrap());
                },
                BatchSize::SmallInput,
            );
        });
    }

    // ---- PerCommit (fsync per commit) --------------------------------------
    {
        group.bench_function("per_commit", |b| {
            b.iter_batched(
                || {
                    let dir = ScratchDir::new("per-commit");
                    let db =
                        Database::open_with_wal(enabled(&dir.path, SyncMode::PerCommit)).unwrap();
                    (dir, db)
                },
                |(_dir, db)| {
                    black_box(db.execute("CREATE (:N {v: 1})", opts()).unwrap());
                },
                BatchSize::SmallInput,
            );
        });
    }

    // ---- Group (write-only on commit, bg flusher fsyncs) -------------------
    {
        group.bench_function("group", |b| {
            b.iter_batched(
                || {
                    let dir = ScratchDir::new("group");
                    let db = Database::open_with_wal(enabled(
                        &dir.path,
                        SyncMode::Group { interval_ms: 50 },
                    ))
                    .unwrap();
                    (dir, db)
                },
                |(_dir, db)| {
                    black_box(db.execute("CREATE (:N {v: 1})", opts()).unwrap());
                },
                BatchSize::SmallInput,
            );
        });
    }

    // ---- None (no fsync at all) --------------------------------------------
    {
        group.bench_function("none", |b| {
            b.iter_batched(
                || {
                    let dir = ScratchDir::new("none");
                    let db = Database::open_with_wal(enabled(&dir.path, SyncMode::None)).unwrap();
                    (dir, db)
                },
                |(_dir, db)| {
                    black_box(db.execute("CREATE (:N {v: 1})", opts()).unwrap());
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn bench_recovery(c: &mut Criterion) {
    let mut group = c.benchmark_group("wal/recovery");

    // Time how long Database::open_with_wal takes on a directory that
    // has N committed transactions waiting to be replayed. This bounds
    // startup time on a crash-recovery boot.
    for n in [100usize, 1_000].iter().copied() {
        // Build the WAL once outside the iter loop.
        let dir = ScratchDir::new(&format!("recovery-{}", n));
        {
            let db = Database::open_with_wal(enabled(&dir.path, SyncMode::PerCommit)).unwrap();
            for _ in 0..n {
                db.execute("CREATE (:N {v: 1})", opts()).unwrap();
            }
            // Drop to release file handles; bg flusher (none here) joins.
            drop(db);
        }

        group.bench_function(format!("replay_{}", n), |b| {
            b.iter(|| {
                let db = Database::open_with_wal(enabled(&dir.path, SyncMode::None)).unwrap();
                black_box(db.node_count());
            });
        });

        // Keep `dir` alive across the iter so the WAL files persist
        // for every iteration. ScratchDir's Drop will clean up at
        // bench end.
        std::mem::forget(dir);
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = smoke_config();
    targets = bench_commit_latency, bench_recovery,
}
criterion_main!(benches);
