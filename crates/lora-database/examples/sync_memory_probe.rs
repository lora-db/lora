//! Heap probe for a named database's checkpoint (`sync()`) and open.
//!
//! Run with:
//!
//! ```text
//! cargo run --release -p lora-database --example sync_memory_probe -- build <dir> <nodes>
//! cargo run --release -p lora-database --example sync_memory_probe -- measure <dir>
//! ```
//!
//! `build` writes a `.loradb` with `<nodes>` nodes (a unique key, a short
//! hash, two integers, a float and a list each) and as many relationships.
//! `writes <dir> <count>` times single-statement creates against it.
//! `measure` opens it, calls `sync()` three times and drops it, reporting
//! for each step the live heap after it and the peak during it. The
//! allocator is wrapped with a live-byte counter, so the figures are the
//! bytes the process asked for, whatever malloc then keeps resident; the
//! resident set after each step is printed beside them.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
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

/// Resident set size from `ps`, in bytes: what malloc kept, beside the
/// counter's view of what the process asked for.
fn rss() -> isize {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|out| out.trim().parse::<isize>().ok())
        .map_or(0, |kib| kib * 1024)
}

fn mib(bytes: isize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Run `step`, printing the live heap after it and the peak during it.
fn measure<T>(name: &str, step: impl FnOnce() -> T) -> T {
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    let before = LIVE.load(Ordering::Relaxed);
    let started = Instant::now();
    let out = step();
    let elapsed = started.elapsed();
    let after = LIVE.load(Ordering::Relaxed);
    let peak = PEAK.load(Ordering::Relaxed);
    println!(
        "{name:<10} live {:>8.1} MiB   peak {:>8.1} MiB   (+{:>7.1} over before)   rss {:>8.1} MiB   {:>6.2} s",
        mib(after),
        mib(peak),
        mib(peak - before),
        mib(rss()),
        elapsed.as_secs_f64()
    );
    out
}

fn options(dir: &PathBuf) -> DatabaseOpenOptions {
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
            let db = Database::open_named("probe", options(&dir)).unwrap();
            db.execute(
                "CREATE CONSTRAINT probe_key FOR (n:Place) REQUIRE n.key IS UNIQUE",
                rows(),
            )
            .unwrap();
            let batch = 50_000;
            let mut start = 0;
            while start < nodes {
                let end = (start + batch).min(nodes) - 1;
                db.execute(
                    &format!(
                        "UNWIND list.range({start}, {end}) AS i \
                         CREATE (:Place {{key: i, hash: 'h' + toString(i * 7919), \
                         a: i % 1000, b: i * 3, lat: i / 1000.0, tags: ['x', i % 7]}})"
                    ),
                    rows(),
                )
                .unwrap();
                start = end + 1;
            }
            let mut start = 1;
            while start < nodes {
                let end = (start + batch).min(nodes) - 1;
                db.execute(
                    &format!(
                        "UNWIND list.range({start}, {end}) AS i \
                         MATCH (a:Place {{key: i - 1}}), (b:Place {{key: i}}) \
                         CREATE (a)-[:NEXT {{d: i % 13}}]->(b)"
                    ),
                    rows(),
                )
                .unwrap();
                start = end + 1;
            }
            db.sync().unwrap();
            println!(
                "built {} nodes, {} relationships",
                db.node_count(),
                db.relationship_count()
            );
        }
        Some("measure") => {
            let dir = PathBuf::from(&args[2]);
            let db = measure("open", || {
                Database::open_named("probe", options(&dir)).unwrap()
            });
            for i in 1..=3 {
                measure(&format!("sync {i}"), || db.sync().unwrap());
            }
            measure("dispose", || drop(db));
        }
        Some("writes") => {
            // Single-statement creates against the built database: commit
            // throughput while the archive worker rewrites the container
            // behind them.
            let dir = PathBuf::from(&args[2]);
            let count: usize = args[3].parse().expect("write count");
            let db = Database::open_named("probe", options(&dir)).unwrap();
            let started = Instant::now();
            for i in 0..count {
                db.execute(&format!("CREATE (:Extra {{i: {i}}})"), rows())
                    .unwrap();
            }
            let writes = started.elapsed();
            let started = Instant::now();
            drop(db);
            println!(
                "{count} writes in {:.2} s ({:.0}/s), dispose {:.2} s",
                writes.as_secs_f64(),
                count as f64 / writes.as_secs_f64(),
                started.elapsed().as_secs_f64()
            );
        }
        _ => eprintln!(
            "usage: sync_memory_probe build <dir> <nodes> | measure <dir> | writes <dir> <count>"
        ),
    }
}
