//! Wall-clock probe for opening and checkpointing a named database.
//!
//! Run with:
//!
//! ```text
//! cargo run --release -p lora-database --example open_probe -- <dir> [<runs>]
//! ```
//!
//! Opens `<dir>/probe.loradb` (as `sync_memory_probe build` writes it)
//! `<runs>` times (default 3), timing each open and one `sync()`. With
//! `wal <MB>` instead of `<runs>` it first writes updates until the WAL
//! holds about that many megabytes, then times opens that replay it
//! (without the `sync()`, which would checkpoint it away). Unlike
//! `sync_memory_probe` it keeps the system allocator: that probe's
//! counting allocator updates two shared counters on every allocation,
//! which serializes the threads a snapshot is decoded on.

use std::path::PathBuf;
use std::time::Instant;

use lora_database::{Database, DatabaseOpenOptions};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&args[1]);
    if args.get(2).map(String::as_str) == Some("wal") {
        let mb: u64 = args[3].parse().expect("WAL megabytes");
        return replay(&dir, mb);
    }
    let runs: usize = args.get(2).map_or(3, |n| n.parse().expect("run count"));
    for run in 1..=runs {
        let started = Instant::now();
        let db = Database::open_named(
            "probe",
            DatabaseOpenOptions::default().with_database_dir(&dir),
        )
        .unwrap();
        let opened = started.elapsed();
        let started = Instant::now();
        db.sync().unwrap();
        let synced = started.elapsed();
        println!(
            "run {run}: open {:>6.2} s   sync {:>6.2} s   ({} nodes, {} relationships)",
            opened.as_secs_f64(),
            synced.as_secs_f64(),
            db.node_count(),
            db.relationship_count()
        );
    }
}

/// Bytes of WAL segments beside the container.
fn wal_bytes(dir: &std::path::Path) -> u64 {
    std::fs::read_dir(dir.join("probe.loradb.wal"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "wal"))
                .map(|e| e.metadata().map_or(0, |m| m.len()))
                .sum()
        })
        .unwrap_or(0)
}

fn replay(dir: &std::path::Path, mb: u64) {
    let options = || {
        let mut options = DatabaseOpenOptions::default().with_database_dir(dir);
        // Keep the log: no checkpoint while writing or on open.
        options.checkpoint_wal_bytes = 0;
        options.compact_on_open_wal_bytes = u64::MAX;
        options
    };
    {
        let db = Database::open_named("probe", options()).unwrap();
        let mut round = 0;
        while wal_bytes(dir) < mb * 1024 * 1024 {
            let start = (round * 50_000) % 3_000_000;
            db.execute(
                &format!(
                    "MATCH (p:Place) WHERE p.key >= {start} AND p.key < {} SET p.a = p.a + 1, p.lat = p.lat + 0.5",
                    start + 50_000
                ),
                None,
            )
            .unwrap();
            round += 1;
        }
        println!(
            "wrote {:.1} MB of WAL in {round} transactions",
            wal_bytes(dir) as f64 / 1048576.0
        );
    }
    for run in 1..=2 {
        let started = Instant::now();
        let db = Database::open_named("probe", options()).unwrap();
        println!(
            "replaying open {run}: {:>6.2} s ({} nodes)",
            started.elapsed().as_secs_f64(),
            db.node_count()
        );
    }
}
