//! Per-node memory probe for opening a named database (G-64): Festimap's
//! sync-tracker `Record` nodes under a uniqueness constraint on `key`.
//! `phase3` holds a 58-byte key, a 16-hex hash and two small integers;
//! `phase1` ten properties (seven short strings) and two relationships to
//! shared nodes; `four_props` and `five_props` differ by one integer.
//!
//! Run with:
//!
//! ```text
//! cargo run --release -p lora-database --example node_memory_probe -- <dir> <nodes>
//! ```
//!
//! For each layout it writes `<nodes>` records to a fresh named database
//! (`PROBE_ONLY=<layout>` picks one), closes it, and opens it again in a
//! process of its own, so pages the writes freed do not hide what the
//! open costs. It reports per node, over what the process held before the
//! open:
//!
//! - `rss`: resident set size, what Festimap's probe measures;
//! - `footprint`: macOS's physical footprint (dirty and compressed pages,
//!   no clean code pages), steadier than RSS on a loaded machine;
//! - `live` and `peak`: heap the process asked for, from a counting
//!   allocator, after the open and at its highest during it;
//! - the graph's `MemoryReport` estimate, by component.
//!
//! The gap between `rss` and `live` is what malloc keeps (free space in
//! partly used pages, freed large blocks it caches) plus the code pages
//! the open touched. By default the open replays the WAL, as Festimap's
//! probe does; `PROBE_SYNC=1` checkpoints after the writes so it loads a
//! snapshot instead. `PROBE_TRACE=1` prints what the open freed, by
//! size: the blocks malloc may keep resident after it (G-64). Buffers in
//! `lora_store::scratch` are mapped pages on macOS, which the counting
//! allocator does not see, so `peak` leaves them out.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};

use lora_database::{
    Database, DatabaseOpenOptions, ExecuteOptions, InMemoryGraph, LoraValue, ResultFormat,
};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);
static TRACE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicIsize = AtomicIsize::new(0);
static FREED: [AtomicIsize; 40] = [ZERO; 40];
static FREED_N: [AtomicIsize; 40] = [ZERO; 40];
fn freed(size: usize) {
    if TRACE.load(Ordering::Relaxed) {
        let b = (usize::BITS - size.leading_zeros()) as usize;
        FREED[b].fetch_add(size as isize, Ordering::Relaxed);
        FREED_N[b].fetch_add(1, Ordering::Relaxed);
    }
}

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
        freed(layout.size());
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = System.realloc(ptr, layout, new_size);
        if !p.is_null() {
            freed(layout.size());
            grew(new_size as isize - layout.size() as isize);
        }
        p
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const LAYOUTS: [(&str, &str); 6] = [
    ("phase3", "CREATE (:Record {key: 'musicbrainz:release-group:' + toString(i) + '-0000-0000-0000-000000000000', hash: '0123456789abcdef', seenIn: 7, changedIn: 5})"),
    ("phase1", "MATCH (s:Source {name: 'musicbrainz'}), (k:Kind {name: 'release-group'}) CREATE (r:Record {key: 'musicbrainz:release-group:' + toString(i) + '-0000-0000-0000-000000000000', source: 'musicbrainz', kind: 'release-group', externalId: toString(i), url: 'https://musicbrainz.org/release-group/' + toString(i), hash: '0123456789abcdef', resolvedHash: 'fedcba9876543210', retrievedAt: '2026-10-22T10:00:00Z', seenIn: '20261022', changedIn: '20261015'}), (r)-[:FROM]->(s), (r)-[:OF]->(k)"),
    ("four_props", "CREATE (:Record {key: 'k' + toString(i), a: 1, b: 2, c: 3})"),
    ("five_props", "CREATE (:Record {key: 'k' + toString(i), a: 1, b: 2, c: 3, d: 4})"),
    // The same records under `OPTIONS { indexProvider: 'hash' }` (a layout
    // name ending in `_hash`): no sorted copy of the key.
    ("phase3_hash", "CREATE (:Record {key: 'musicbrainz:release-group:' + toString(i) + '-0000-0000-0000-000000000000', hash: '0123456789abcdef', seenIn: 7, changedIn: 5})"),
    ("phase1_hash", "MATCH (s:Source {name: 'musicbrainz'}), (k:Kind {name: 'release-group'}) CREATE (r:Record {key: 'musicbrainz:release-group:' + toString(i) + '-0000-0000-0000-000000000000', source: 'musicbrainz', kind: 'release-group', externalId: toString(i), url: 'https://musicbrainz.org/release-group/' + toString(i), hash: '0123456789abcdef', resolvedHash: 'fedcba9876543210', retrievedAt: '2026-10-22T10:00:00Z', seenIn: '20261022', changedIn: '20261015'}), (r)-[:FROM]->(s), (r)-[:OF]->(k)"),
];

fn options(dir: &Path) -> DatabaseOpenOptions {
    let mut options = DatabaseOpenOptions::default().with_database_dir(dir);
    if std::env::var_os("PROBE_NO_COMPACT").is_some() {
        // Replay without the open's checkpoint of a long WAL.
        options.compact_on_open_wal_bytes = u64::MAX;
    }
    options
}

fn rows() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn build(dir: &Path, n: usize, name: &str, create: &str) {
    let _ = std::fs::remove_dir_all(dir);
    let db = Database::open_named("probe", options(dir)).unwrap();
    db.execute(
        "CREATE (:Source {name: 'musicbrainz'}), (:Kind {name: 'release-group'})",
        rows(),
    )
    .unwrap();
    let backing = if name.ends_with("_hash") {
        " OPTIONS { indexProvider: 'hash' }"
    } else {
        ""
    };
    db.execute(
        &format!(
            "CREATE CONSTRAINT rk IF NOT EXISTS FOR (r:Record) REQUIRE r.key IS UNIQUE{backing}"
        ),
        rows(),
    )
    .unwrap();
    for b in (0..n).step_by(5_000) {
        let params = BTreeMap::from([
            ("b".to_string(), LoraValue::Int(b as i64)),
            (
                "e".to_string(),
                LoraValue::Int((b + 4_999).min(n - 1) as i64),
            ),
        ]);
        db.execute_with_params(
            &format!("UNWIND range($b, $e) AS i {create}"),
            rows(),
            params,
        )
        .unwrap();
    }
    if std::env::var_os("PROBE_SYNC").is_some() {
        db.sync().unwrap();
    }
}

/// Resident set size and physical footprint of this process, in bytes.
#[cfg(target_os = "macos")]
fn resident() -> (f64, f64) {
    extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut u64) -> i32;
    }
    // `rusage_info_v2`: a 16-byte UUID, then 64-bit counters, of which
    // `ri_resident_size` is the seventh and `ri_phys_footprint` the eighth.
    const RUSAGE_INFO_V2: i32 = 2;
    let mut info = [0u64; 32];
    let ok =
        unsafe { proc_pid_rusage(std::process::id() as i32, RUSAGE_INFO_V2, info.as_mut_ptr()) };
    assert_eq!(ok, 0, "proc_pid_rusage failed");
    (info[8] as f64, info[9] as f64)
}

/// Resident set size of this process, in bytes (no footprint off macOS).
#[cfg(not(target_os = "macos"))]
fn resident() -> (f64, f64) {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: f64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(0.0);
    (pages * 4096.0, f64::NAN)
}

fn measure(name: &str, dir: &Path, n: usize) {
    let (rss_before, footprint_before) = resident();
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    let before = LIVE.load(Ordering::Relaxed);
    TRACE.store(std::env::var_os("PROBE_TRACE").is_some(), Ordering::Relaxed);
    let db = Database::open_named("probe", options(dir)).unwrap();
    TRACE.store(false, Ordering::Relaxed);
    for b in 0..40 {
        let n = FREED_N[b].load(Ordering::Relaxed);
        if n > 0 {
            eprintln!(
                "freed <{:>9}: {:>8} blocks {:>8.1} MB",
                1usize << b,
                n,
                FREED[b].load(Ordering::Relaxed) as f64 / 1e6
            );
        }
    }
    db.execute("MATCH (r:Record) RETURN count(r)", rows())
        .unwrap();
    let live = LIVE.load(Ordering::Relaxed) - before;
    let peak = PEAK.load(Ordering::Relaxed) - before;
    let (rss, footprint) = resident();
    let report = db.with_store(|g: &InMemoryGraph| g.memory_estimate());
    if let Ok(secs) = std::env::var("PROBE_PAUSE_SECS") {
        // Time to inspect the open process (`vmmap`, `heap`).
        eprintln!("pid {}", std::process::id());
        std::thread::sleep(std::time::Duration::from_secs(secs.parse().unwrap()));
    }
    let per = |b: f64| b / n as f64;
    println!(
        "{name:<10} n={n} rss={:.0} footprint={:.0} live={:.0} peak={:.0} | estimate: total={:.0} \
         nodes={:.0} rels={:.0} out={:.0} in={:.0} labels={:.0} hash_idx={:.0} sorted_idx={:.0}",
        per(rss - rss_before),
        per(footprint - footprint_before),
        per(live as f64),
        per(peak as f64),
        per(report.total_bytes() as f64),
        per(report.nodes_bytes as f64),
        per(report.relationships_bytes as f64),
        per(report.outgoing_bytes as f64),
        per(report.incoming_bytes as f64),
        per(report.label_index_bytes as f64),
        per(report.property_index_bytes as f64),
        per(report.sorted_index_bytes as f64),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).expect("usage: node_memory_probe <dir> <nodes>"));
    let n: usize = args
        .get(2)
        .map_or(50_000, |a| a.parse().expect("node count"));
    // `<dir> <nodes> <layout>`: the child that opens a built layout.
    if let Some(layout) = args.get(3) {
        measure(layout, &dir, n);
        return;
    }
    let only = std::env::var("PROBE_ONLY").ok();
    for (name, create) in LAYOUTS {
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        build(&dir, n, name, create);
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([dir.to_str().unwrap(), &n.to_string(), name])
            .output()
            .expect("the probe runs");
        print!("{}", String::from_utf8_lossy(&out.stdout));
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
    }
}
