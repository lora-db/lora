//! Memory and disk bounds of opening a named database.
//!
//! A counting allocator measures the heap an open asks for, so the bounds
//! hold whatever malloc then keeps resident. The tests share that counter,
//! so they take [`MEASURE`] to run one at a time.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use lora_database::{Database, DatabaseOpenOptions, ExecuteOptions, ResultFormat};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);
static MEASURE: Mutex<()> = Mutex::new(());

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

struct TmpDir {
    path: PathBuf,
}

impl TmpDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lora-db-open-memory-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

const NODES: usize = 5_000;
const ROUNDS: usize = 12;
const SEGMENT_BYTES: u64 = 256 * 1024;
/// A checkpoint keeps the active segment and the one before it, and a
/// segment rotates at a transaction boundary, so each may run one
/// transaction past its target.
const TAIL_WAL_BYTES: u64 = 3 * SEGMENT_BYTES;

fn rows() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn options(dir: &Path, compact_on_open_wal_bytes: u64) -> DatabaseOpenOptions {
    DatabaseOpenOptions {
        segment_target_bytes: SEGMENT_BYTES,
        compact_on_open_wal_bytes,
        ..DatabaseOpenOptions::default().with_database_dir(dir)
    }
}

/// `NODES` nodes, each rewritten `ROUNDS` times: a WAL many times the
/// graph, as a long-running ingest leaves it.
fn write_history(db: &Database<lora_database::InMemoryGraph>) {
    let batch = 500;
    for round in 0..=ROUNDS {
        for start in (0..NODES).step_by(batch) {
            let end = (start + batch).min(NODES) - 1;
            let query = if round == 0 {
                format!(
                    "UNWIND list.range({start}, {end}) AS i \
                     CREATE (:Record {{key: i, v: 0, title: 'record ' + toString(i)}})"
                )
            } else {
                format!(
                    "MATCH (n:Record) WHERE n.key >= {start} AND n.key <= {end} \
                     SET n.v = {round}, n.title = 'record ' + toString(n.key) + ' r{round}'"
                )
            };
            db.execute(&query, rows()).unwrap();
        }
    }
}

fn wal_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir.join("app.loradb.wal"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("wal"))
        .map(|e| e.metadata().unwrap().len())
        .sum()
}

fn container(dir: &Path) -> Vec<u8> {
    std::fs::read(dir.join("app.loradb")).unwrap()
}

/// Whether the container's first frame is a base snapshot. A container
/// header is 32 bytes; a frame header starts with its magic and kind.
fn starts_with_snapshot(container: &[u8]) -> bool {
    &container[32..40] == b"LORAFRM\0" && container[40] == 2
}

struct Opened {
    db: Database<lora_database::InMemoryGraph>,
    /// Heap the open's result holds.
    retained: isize,
    /// Highest heap during the open, over what was live before it.
    peak: isize,
}

fn measured_open(dir: &Path, compact_on_open_wal_bytes: u64) -> Opened {
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let db = Database::open_named("app", options(dir, compact_on_open_wal_bytes)).unwrap();
    Opened {
        db,
        retained: LIVE.load(Ordering::Relaxed) - before,
        peak: PEAK.load(Ordering::Relaxed) - before,
    }
}

/// Every node is there with the last round's write.
fn assert_graph(db: &Database<lora_database::InMemoryGraph>) {
    assert_eq!(db.node_count(), NODES);
    let result = db
        .execute(
            &format!("MATCH (n:Record) WHERE n.v <> {ROUNDS} RETURN count(n) AS stale"),
            rows(),
        )
        .unwrap();
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["rows"], serde_json::json!([{"stale": 0}]));
}

#[test]
fn opening_a_long_wal_history_replays_it_in_bounded_memory_and_compacts() {
    let _guard = MEASURE.lock().unwrap_or_else(|p| p.into_inner());
    let dir = TmpDir::new("history");

    {
        let db = Database::open_named("app", options(&dir.path, u64::MAX)).unwrap();
        write_history(&db);
    }
    let history = container(&dir.path);
    assert!(
        !starts_with_snapshot(&history),
        "the history container holds only WAL"
    );

    let first = measured_open(&dir.path, 1024 * 1024);
    assert_graph(&first.db);
    let graph = first.retained;
    // Collecting the replayed log before applying it peaked at about 50x
    // the graph. Applying each transaction as it is read leaves the graph
    // plus one transaction and the decode buffers: about 0.7 MB here,
    // twice this small graph (5,000 nodes without relationships, 331 KB).
    assert!(
        first.peak < 4 * graph,
        "open peaked at {} bytes over a {graph}-byte graph",
        first.peak
    );

    // The open checkpointed: the container is the snapshot and the WAL's
    // tail, and the work directory keeps at most two segments.
    let compacted = container(&dir.path);
    assert!(starts_with_snapshot(&compacted));
    assert!(
        compacted.len() * 4 < history.len(),
        "container {} bytes after compaction, {} before",
        compacted.len(),
        history.len()
    );
    assert!(wal_bytes(&dir.path) <= TAIL_WAL_BYTES);
    drop(first);

    let second = measured_open(&dir.path, 1024 * 1024);
    assert_graph(&second.db);
    assert!(second.peak < 2 * graph);
}

#[test]
fn opening_a_checkpoint_writes_no_wal_for_the_graph() {
    let _guard = MEASURE.lock().unwrap_or_else(|p| p.into_inner());
    let dir = TmpDir::new("checkpoint");

    {
        let db = Database::open_named("app", options(&dir.path, u64::MAX)).unwrap();
        write_history(&db);
        db.sync().unwrap();
    }
    // Leave only the container, as a backup restore does.
    let _ = std::fs::remove_dir_all(dir.path.join("app.loradb.wal"));
    let checkpoint = container(&dir.path);
    assert!(starts_with_snapshot(&checkpoint));

    let opened = measured_open(&dir.path, 1024 * 1024);
    assert_graph(&opened.db);
    assert!(
        opened.peak < 2 * opened.retained,
        "open peaked at {} bytes over a {}-byte graph",
        opened.peak,
        opened.retained
    );
    // The snapshot is the base: the open extracts the tail sync() kept and
    // writes nothing for the graph itself.
    assert!(
        wal_bytes(&dir.path) <= TAIL_WAL_BYTES,
        "open left {} bytes of WAL",
        wal_bytes(&dir.path)
    );
}

/// Nodes the large-transaction test writes in one statement: their WAL
/// records come to over three times what replay buffers of one
/// transaction (`lora_wal::PENDING_BUDGET_BYTES`).
const LARGE_TX_NODES: usize = 400_000;

/// Heap replaying a large transaction may take over the graph: one WAL
/// record (8 MB at most) and its events decoded, about 35 MB.
const REPLAY_SLACK: isize = 48 * 1024 * 1024;

/// Heap a checkpoint may take over the graph: copy buffers and the
/// compressors, about 1 MB, never a segment or the snapshot.
const CHECKPOINT_SLACK: isize = 8 * 1024 * 1024;

#[test]
fn a_transaction_larger_than_the_replay_buffer_opens_and_syncs_in_bounded_memory() {
    let _guard = MEASURE.lock().unwrap_or_else(|p| p.into_inner());
    let dir = TmpDir::new("large-tx");

    {
        let db = Database::open_named("app", options(&dir.path, u64::MAX)).unwrap();
        // One transaction never spans segments: this leaves one segment
        // of about 50 MB, whatever the 256 KB target.
        db.execute(
            &format!(
                "UNWIND list.range(0, {}) AS i \
                 CREATE (:Record {{key: 'src:item:' + toString(i), hash: '0123456789abcdef', v: i}})",
                LARGE_TX_NODES - 1
            ),
            rows(),
        )
        .unwrap();
    }
    let wal = wal_bytes_in_container(&dir.path);
    assert!(
        wal > 3 * lora_wal::PENDING_BUDGET_BYTES as u64,
        "the transaction's {wal} bytes of WAL fit the replay buffer"
    );

    // Without a snapshot the open replays the transaction. Buffering it
    // whole held its decoded events beside the graph it builds: 216 MB
    // at the peak for a 20 MB graph. Applied in parts it holds the graph
    // and one 8 MB WAL record decoded: 55 MB.
    let opened = measured_open(&dir.path, u64::MAX);
    assert_eq!(opened.db.node_count(), LARGE_TX_NODES);
    let graph = opened.retained;
    assert!(
        opened.peak < graph + REPLAY_SLACK,
        "open peaked at {} bytes over a {graph}-byte graph",
        opened.peak
    );

    // A checkpoint streams the snapshot and the WAL segments into the
    // container: under 1 MB over the graph. It used to read each segment,
    // this one included, into memory and compress it there: 56 MB.
    for _ in 0..2 {
        let before = LIVE.load(Ordering::Relaxed);
        PEAK.store(before, Ordering::Relaxed);
        opened.db.sync().unwrap();
        let peak = PEAK.load(Ordering::Relaxed) - before;
        assert!(
            peak < CHECKPOINT_SLACK,
            "sync() peaked at {peak} bytes over the graph"
        );
    }
    drop(opened);

    let reopened = measured_open(&dir.path, u64::MAX);
    assert_eq!(reopened.db.node_count(), LARGE_TX_NODES);
    let result = reopened
        .db
        .execute(
            "MATCH (n:Record {key: 'src:item:123'}) RETURN n.v AS v",
            rows(),
        )
        .unwrap();
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["rows"], serde_json::json!([{"v": 123}]));
}

/// Raw bytes of the WAL segments the container holds: the frames'
/// decoded lengths, from their headers.
fn wal_bytes_in_container(dir: &Path) -> u64 {
    let bytes = container(dir);
    let mut at = 32;
    let mut total = 0;
    while &bytes[at..at + 8] == b"LORAFRM\0" {
        let kind = bytes[at + 8];
        let name_len = u16::from_le_bytes([bytes[at + 12], bytes[at + 13]]) as usize;
        let body_len = u64::from_le_bytes(bytes[at + 16..at + 24].try_into().unwrap()) as usize;
        let raw_len = u64::from_le_bytes(bytes[at + 24..at + 32].try_into().unwrap());
        if kind == 1 {
            total += raw_len;
        }
        at += 48 + name_len + body_len;
    }
    total
}
