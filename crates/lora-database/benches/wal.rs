//! WAL-aware microbenchmarks.
//!
//! These exercise the active durability profiles end-to-end through
//! `Database::execute_with_params`:
//!
//! - **`no_wal`** — `Database::in_memory()` (the existing fast path;
//!   serves as the baseline the others are compared against).
//! - **`group_sync`** — `WalConfig::Enabled` with `SyncMode::GroupSync`
//!   (write on commit, background flusher fsyncs).
//!
//! The shape that matters is *commit latency* — every iteration runs a
//! single tiny `CREATE` statement so the engine work is negligible and
//! the WAL path dominates. The gap between `no_wal` and `group_sync`
//! measures WAL encoding, buffering, and file-write overhead without an
//! inline fsync in the commit path.
//!
//! There is also a small **`recovery`** bench that times opening a WAL
//! with N committed transactions and re-applying them. This is the
//! number that bounds startup time on a fresh process.
//!
//! Run with:
//!   `cargo bench -p lora-database --bench wal`

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use lora_database::{
    Database, DatabaseOpenOptions, ExecuteOptions, InMemoryGraph, ResultFormat, SyncMode,
    TransactionMode, WalConfig,
};
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
    // Per-bench budget of ~3 s. WAL setup still touches the filesystem, but
    // GroupSync keeps commit latency bounded enough for a compact smoke run.
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_millis(2_500))
        .sample_size(20)
        .noise_threshold(0.10)
}

#[derive(Clone, Copy)]
enum SmokeProfile {
    MemoryOnly,
    WalDisabled,
    WalGroupSync,
}

impl SmokeProfile {
    const ALL: [Self; 3] = [Self::MemoryOnly, Self::WalDisabled, Self::WalGroupSync];

    fn label(self) -> &'static str {
        match self {
            Self::MemoryOnly => "memory_only",
            Self::WalDisabled => "wal_disabled",
            Self::WalGroupSync => "wal_group_sync",
        }
    }
}

/// Comparison axis for the named_archive groups: each variant is either
/// purely in-memory or backed by a persistent storage engine. Benches use
/// this to iterate over the same workload across engines without
/// duplicating setup.
#[derive(Clone, Copy)]
enum EngineProfile {
    /// `Database::in_memory()` — no durability, baseline.
    InMemory,
    /// `Database::open_named()` — persistent `.loradb` archive
    /// (snapshot-on-shutdown).
    LoraArchive,
    /// `Database::open_with_wal(Enabled)` — persistent WAL with the
    /// default GroupSync flusher.
    WalPersistent,
}

impl EngineProfile {
    const ALL: [Self; 3] = [Self::InMemory, Self::LoraArchive, Self::WalPersistent];

    fn label(self) -> &'static str {
        match self {
            Self::InMemory => "memory_only",
            Self::LoraArchive => "lora_archive",
            Self::WalPersistent => "wal_persistent",
        }
    }

    fn is_persistent(self) -> bool {
        !matches!(self, Self::InMemory)
    }

    /// Open a fresh database for this profile. Returns the `ScratchDir`
    /// alongside so persistent variants keep their files alive for the
    /// lifetime of the bench iteration.
    fn open(self, tag: &str) -> (Option<ScratchDir>, Database<InMemoryGraph>) {
        match self {
            Self::InMemory => (None, Database::in_memory()),
            Self::LoraArchive => {
                let dir = ScratchDir::new(&format!("{tag}-lora-archive"));
                let db = Database::open_named(
                    "bench",
                    DatabaseOpenOptions::default().with_database_dir(&dir.path),
                )
                .unwrap();
                (Some(dir), db)
            }
            Self::WalPersistent => {
                let dir = ScratchDir::new(&format!("{tag}-wal-persistent"));
                let db = Database::open_with_wal(enabled(
                    &dir.path,
                    SyncMode::GroupSync { interval_ms: 50 },
                ))
                .unwrap();
                (Some(dir), db)
            }
        }
    }
}

fn open_smoke_db(profile: SmokeProfile) -> (Option<ScratchDir>, Database<InMemoryGraph>) {
    match profile {
        SmokeProfile::MemoryOnly => (None, Database::in_memory()),
        SmokeProfile::WalDisabled => (None, Database::open_with_wal(WalConfig::Disabled).unwrap()),
        SmokeProfile::WalGroupSync => {
            let dir = ScratchDir::new("perf-smoke-wal-group-sync");
            let db = Database::open_with_wal(enabled(
                &dir.path,
                SyncMode::GroupSync { interval_ms: 50 },
            ))
            .unwrap();
            (Some(dir), db)
        }
    }
}

fn seed_smoke_nodes(db: &Database<InMemoryGraph>) {
    db.execute(
        "UNWIND list.range(1, 1000) AS i CREATE (:Node {id: i, value: i % 100})",
        opts(),
    )
    .unwrap();
}

fn bench_perf_smoke_profiles(c: &mut Criterion) {
    let mut group = c.benchmark_group("wal/perf_smoke_profiles");

    for profile in SmokeProfile::ALL {
        let (_dir, db) = open_smoke_db(profile);
        seed_smoke_nodes(&db);
        group.bench_function(format!("scan_1k/{}", profile.label()), |b| {
            b.iter(|| {
                black_box(db.execute("MATCH (n:Node) RETURN n.id", opts()).unwrap());
            });
        });
    }

    for profile in SmokeProfile::ALL {
        let (_dir, db) = open_smoke_db(profile);
        group.bench_function(format!("write_steady_100/{}", profile.label()), |b| {
            b.iter(|| {
                black_box(
                    db.execute(
                        "UNWIND list.range(1, 100) AS i CREATE (n:B {id: i, val: i * 2}) DELETE n",
                        opts(),
                    )
                    .unwrap(),
                );
                black_box(db.node_count());
            });
        });
    }

    for profile in SmokeProfile::ALL {
        let (_dir, db) = open_smoke_db(profile);
        group.bench_function(format!("tx_write_steady_100/{}", profile.label()), |b| {
            b.iter(|| {
                let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
                black_box(
                    tx.execute(
                        "UNWIND list.range(1, 100) AS i CREATE (n:B {id: i}) DELETE n",
                        opts(),
                    )
                    .unwrap(),
                );
                tx.commit().unwrap();
                black_box(db.node_count());
            });
        });
    }

    group.finish();
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

    // ---- GroupSync (write on commit, bg flusher fsyncs) --------------------
    {
        group.bench_function("group_sync", |b| {
            b.iter_batched(
                || {
                    let dir = ScratchDir::new("group-sync");
                    let db = Database::open_with_wal(enabled(
                        &dir.path,
                        SyncMode::GroupSync { interval_ms: 50 },
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
            let db = Database::open_with_wal(enabled(
                &dir.path,
                SyncMode::GroupSync { interval_ms: 50 },
            ))
            .unwrap();
            for _ in 0..n {
                db.execute("CREATE (:N {v: 1})", opts()).unwrap();
            }
            // Drop to release file handles; the GroupSync flusher joins.
            drop(db);
        }

        group.bench_function(format!("replay_{}", n), |b| {
            b.iter(|| {
                let db = Database::open_with_wal(enabled(
                    &dir.path,
                    SyncMode::GroupSync { interval_ms: 50 },
                ))
                .unwrap();
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

fn bench_named_archive_write_heavy(c: &mut Criterion) {
    let mut group = c.benchmark_group("named_archive/write_heavy");

    // One timed iteration performs a realistic write burst. For persistent
    // variants, dropping the DB at the end joins any background writer and
    // includes the final flush, so the result measures more than just
    // "enqueue dirty flag".
    const WRITES: usize = 1_000;

    for profile in EngineProfile::ALL {
        group.bench_function(format!("{}_1000_creates", profile.label()), |b| {
            b.iter_batched(
                || profile.open("write-heavy"),
                |(_dir, db)| {
                    for i in 0..WRITES {
                        black_box(
                            db.execute(&format!("CREATE (:N {{v: {i}}})"), opts())
                                .unwrap(),
                        );
                    }
                    black_box(db.node_count());
                    if profile.is_persistent() {
                        drop(db);
                    }
                },
                BatchSize::SmallInput,
            );
        });
    }

    for profile in EngineProfile::ALL {
        group.bench_function(format!("{}_batch_1000", profile.label()), |b| {
            b.iter_batched(
                || profile.open("write-heavy-batch"),
                |(_dir, db)| {
                    black_box(
                        db.execute("UNWIND list.range(1, 1000) AS i CREATE (:N {v: i})", opts())
                            .unwrap(),
                    );
                    black_box(db.node_count());
                    if profile.is_persistent() {
                        drop(db);
                    }
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn bench_named_archive_steady_state(c: &mut Criterion) {
    let mut group = c.benchmark_group("named_archive/steady_state");

    for profile in EngineProfile::ALL {
        let (_dir, db) = profile.open("steady");
        group.bench_function(format!("{}_create_delete", profile.label()), |b| {
            b.iter(|| {
                black_box(
                    db.execute("CREATE (n:Tmp {v: 1}) DELETE n", opts())
                        .unwrap(),
                );
                black_box(db.node_count());
            });
        });
    }

    for profile in EngineProfile::ALL {
        let (_dir, db) = profile.open("steady-batch");
        group.bench_function(
            format!("{}_batch_create_delete_1000", profile.label()),
            |b| {
                b.iter(|| {
                    black_box(
                        db.execute(
                            "UNWIND list.range(1, 1000) AS i CREATE (n:Tmp {v: i}) DELETE n",
                            opts(),
                        )
                        .unwrap(),
                    );
                    black_box(db.node_count());
                });
            },
        );
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = smoke_config();
    targets = bench_perf_smoke_profiles, bench_commit_latency, bench_recovery, bench_named_archive_write_heavy, bench_named_archive_steady_state,
}
criterion_main!(benches);
