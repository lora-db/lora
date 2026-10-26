//! Commit throughput of a WAL-backed database, with and without a change
//! feed draining every batch.
//!
//! ```text
//! cargo run --release -p lora-database --example wal_write_probe -- <dir> <mode> <writes>
//! ```
//!
//! `mode` is `plain` (WAL only), `feed` (WAL plus a consumer thread) or
//! `managed` (WAL plus managed snapshots with their default automatic
//! checkpoints). `<dir>` is wiped first.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use lora_database::{ChangeFeedOptions, ChangePoll, Database, LoraValue, SnapshotConfig};
use lora_database::{SyncMode, WalConfig};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(&args[1]);
    let mode = args[2].as_str();
    let writes: usize = args[3].parse().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let wal = WalConfig::Enabled {
        dir: dir.join("wal"),
        sync_mode: SyncMode::GroupSync { interval_ms: 50 },
        segment_target_bytes: 8 * 1024 * 1024,
        truncate_corrupt_tail: false,
    };
    let db = match mode {
        "managed" => {
            Database::open_with_wal_snapshots(wal, SnapshotConfig::enabled(dir.join("snap")))
                .unwrap()
        }
        _ => Database::open_with_wal(wal).unwrap(),
    };
    let consumer = (mode == "feed").then(|| {
        let mut feed = db
            .changes(ChangeFeedOptions {
                buffer_size: 1 << 20,
                ..Default::default()
            })
            .unwrap();
        std::thread::spawn(move || {
            let mut seen = 0usize;
            while seen < writes {
                match feed.next_timeout(Duration::from_secs(10)).unwrap() {
                    ChangePoll::Batch(_) => seen += 1,
                    ChangePoll::Pending => panic!("feed stalled at {seen}"),
                    ChangePoll::Closed => break,
                }
            }
            Instant::now()
        })
    });
    let pad = "x".repeat(64);
    let start = Instant::now();
    for i in 0..writes {
        let mut params = BTreeMap::new();
        params.insert("i".to_string(), LoraValue::Int(i as i64));
        params.insert("s".to_string(), LoraValue::String(pad.clone()));
        db.execute_with_params("CREATE (:N {i: $i, s: $s})", None, params)
            .unwrap();
    }
    let elapsed = start.elapsed();
    let drained = consumer.map(|c| c.join().unwrap().duration_since(start));
    println!(
        "mode={mode} writes={writes} elapsed={:.3}s rate={:.0}/s drained={:?}",
        elapsed.as_secs_f64(),
        writes as f64 / elapsed.as_secs_f64(),
        drained.map(|d| d.as_secs_f64())
    );
}
