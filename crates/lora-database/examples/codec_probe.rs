//! Compare snapshot codecs and frame sizes on a real database.
//!
//! Run with:
//!
//! ```text
//! cargo run --release -p lora-database --example codec_probe -- \
//!     <file.loradb> <work dir> [<runs>] [<codecs>] [<frame ids>]
//! ```
//!
//! Copies `<file.loradb>` into `<work dir>` (the original is never
//! opened), opens the copy, and writes one framed snapshot per codec and
//! frame size: `<codecs>` is a comma list of `none`, `gzip1`, `gzip6`,
//! `zstd1`, `zstd3`, `zstd6`, `zstd9` (default: all), `<frame ids>`
//! a comma list of ids per frame (default `4096,16384,65536`). Then it
//! drops the database and loads every snapshot `<runs>` times (default
//! 3), so only one graph is in memory at a time. Prints each snapshot's
//! size, write time and median load time (a full restore: records,
//! indexes and constraints).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lora_database::{Compression, Database, DatabaseOpenOptions, SnapshotOptions, WalConfig};

fn codec(name: &str) -> Compression {
    match name {
        "none" => Compression::None,
        _ if name.starts_with("gzip") => Compression::Gzip {
            level: name[4..].parse().expect("gzip level"),
        },
        _ if name.starts_with("zstd") => Compression::Zstd {
            level: name[4..].parse().expect("zstd level"),
        },
        other => panic!("unknown codec {other}"),
    }
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let source = PathBuf::from(&args[1]);
    let work = PathBuf::from(&args[2]);
    let runs: usize = args.get(3).map_or(3, |n| n.parse().expect("runs"));
    let codecs: Vec<String> = args
        .get(4)
        .map_or("none,gzip1,gzip6,zstd1,zstd3,zstd6,zstd9", String::as_str)
        .split(',')
        .map(str::to_owned)
        .collect();
    let frames: Vec<u64> = args
        .get(5)
        .map_or("4096,16384,65536", String::as_str)
        .split(',')
        .map(|n| n.parse().expect("frame ids"))
        .collect();

    std::fs::create_dir_all(&work).unwrap();
    let copy = work.join("probe.loradb");
    std::fs::copy(&source, &copy).unwrap();
    println!(
        "container {:.1} MiB ({})",
        mib(std::fs::metadata(&copy).unwrap().len()),
        source.display()
    );

    let started = Instant::now();
    let db = Database::open_named(
        "probe",
        DatabaseOpenOptions::default().with_database_dir(&work),
    )
    .unwrap();
    println!(
        "opened the copy in {:.2} s: {} nodes, {} relationships\n",
        started.elapsed().as_secs_f64(),
        db.node_count(),
        db.relationship_count()
    );

    let mut written: Vec<(String, u64, PathBuf, Duration)> = Vec::new();
    for &frame in &frames {
        lora_snapshot::set_ids_per_frame(frame);
        for name in &codecs {
            let path = work.join(format!("{name}-{frame}.lsnap"));
            let options = SnapshotOptions {
                compression: codec(name),
                encryption: None,
            };
            let started = Instant::now();
            db.save_snapshot_to_with_options(&path, &options).unwrap();
            written.push((name.clone(), frame, path, started.elapsed()));
        }
    }
    drop(db);

    println!(
        "{:<7} {:>7} {:>10} {:>9} {:>9}",
        "codec", "frame", "size MiB", "write s", "load s"
    );
    for (name, frame, path, write) in &written {
        let size = std::fs::metadata(path).unwrap().len();
        let loads = (0..runs)
            .map(|_| {
                let started = Instant::now();
                let db = Database::recover(path, WalConfig::Disabled).unwrap();
                let elapsed = started.elapsed();
                drop(db);
                elapsed
            })
            .collect();
        println!(
            "{name:<7} {frame:>7} {:>10.1} {:>9.2} {:>9.2}",
            mib(size),
            write.as_secs_f64(),
            median(loads).as_secs_f64()
        );
        std::fs::remove_file(path).unwrap();
    }
    remove_named(&work);
}

/// Remove the copy and its work directory.
fn remove_named(work: &Path) {
    let _ = std::fs::remove_file(work.join("probe.loradb"));
    let _ = std::fs::remove_file(work.join("probe.loradb.lock"));
    let _ = std::fs::remove_dir_all(work.join("probe.loradb.wal"));
}
