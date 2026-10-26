//! Heap probe for opening a named database whose container holds only WAL:
//! a long write history over a small graph, never `sync()`ed, the shape
//! Festimap's extraction graph reached (G-71).
//!
//! Run with:
//!
//! ```text
//! cargo run --release -p lora-database --example replay_memory_probe -- build <dir> <nodes> <rounds>
//! cargo run --release -p lora-database --example replay_memory_probe -- measure <dir>
//! ```
//!
//! `build` creates `<nodes>` nodes, then rewrites every node's properties
//! `<rounds>` times, one transaction per 1,000 nodes, so the WAL is about
//! `<rounds>` times the graph. `measure` opens the database twice, reporting
//! the live heap after each open, the peak during it and the bytes the
//! work directory held after it. Live and peak come from a counting
//! allocator, so they are what the process asked for, whatever malloc then
//! keeps resident.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Instant;

use lora_database::{Database, DatabaseOpenOptions, ExecuteOptions, ResultFormat};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

fn grew(by: isize) {
    let live = LIVE.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            grew(layout.size() as isize);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = System.realloc(ptr, layout, new_size);
        if !p.is_null() {
            grew(new_size as isize - layout.size() as isize);
        }
        p
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn mib(bytes: isize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

fn options(dir: &Path) -> DatabaseOpenOptions {
    DatabaseOpenOptions::default().with_database_dir(dir)
}

fn rows() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("build") => {
            let dir = PathBuf::from(&args[2]);
            let nodes: usize = args[3].parse().expect("node count");
            let rounds: usize = args[4].parse().expect("round count");
            let db = Database::open_named("probe", options(&dir)).unwrap();
            db.execute(
                "CREATE CONSTRAINT record_key FOR (n:Record) REQUIRE n.key IS UNIQUE",
                rows(),
            )
            .unwrap();
            let batch = 1_000;
            for round in 0..=rounds {
                let mut start = 0;
                while start < nodes {
                    let end = (start + batch).min(nodes) - 1;
                    let query = if round == 0 {
                        format!(
                            "UNWIND list.range({start}, {end}) AS i \
                             CREATE (:Record {{key: i, v: 0, title: 'record ' + toString(i)}})"
                        )
                    } else {
                        format!(
                            "UNWIND list.range({start}, {end}) AS i MATCH (n:Record {{key: i}}) \
                             SET n.v = {round}, n.title = 'record ' + toString(n.key) + ' r{round}'"
                        )
                    };
                    db.execute(&query, rows()).unwrap();
                    start = end + 1;
                }
            }
            println!(
                "built {} nodes over {rounds} rewrite rounds",
                db.node_count()
            );
            drop(db);
            let archive = dir.join("probe.loradb");
            println!(
                "container {:.1} MiB",
                mib(std::fs::metadata(&archive).unwrap().len() as isize)
            );
        }
        Some("measure") => {
            let dir = PathBuf::from(&args[2]);
            for pass in 1..=2 {
                PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
                let before = LIVE.load(Ordering::Relaxed);
                let started = Instant::now();
                let db = Database::open_named("probe", options(&dir)).unwrap();
                let elapsed = started.elapsed();
                println!(
                    "open {pass}  live {:>8.1} MiB   peak {:>8.1} MiB (+{:>7.1})   work dir {:>8.1} MiB   {:>6.2} s   {} nodes",
                    mib(LIVE.load(Ordering::Relaxed)),
                    mib(PEAK.load(Ordering::Relaxed)),
                    mib(PEAK.load(Ordering::Relaxed) - before),
                    mib(dir_bytes(&dir.join("probe.loradb.wal")) as isize),
                    elapsed.as_secs_f64(),
                    db.node_count(),
                );
                drop(db);
            }
        }
        _ => eprintln!("usage: replay_memory_probe build <dir> <nodes> <rounds> | measure <dir>"),
    }
}
