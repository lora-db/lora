//! Heap a small node costs, its index entry included (G-64).
//!
//! Festimap's sync tracker keeps one `Record` node per source item, keyed
//! by a uniqueness constraint. A counting allocator measures the heap a
//! named database holds after opening such a graph, per node, so the
//! bounds hold whatever malloc then keeps resident. The tests share that
//! counter, so they take [`MEASURE`] to run one at a time.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use lora_database::{Database, DatabaseOpenOptions, ExecuteOptions, LoraValue, ResultFormat};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static MEASURE: Mutex<()> = Mutex::new(());

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
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
            LIVE.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
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
            "lora-db-node-memory-{}-{}-{}",
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

const NODES: usize = 20_000;

/// A key of 58 bytes, a 16-hex hash and two small integers.
const PHASE3: &str = "CREATE (:Record {key: 'musicbrainz:release-group:' + toString(i) + '-0000-0000-0000-000000000000', hash: '0123456789abcdef', seenIn: 7, changedIn: 5})";
/// Ten properties, seven of them short strings, and two relationships to
/// shared nodes.
const PHASE1: &str = "MATCH (s:Source {name: 'musicbrainz'}), (k:Kind {name: 'release-group'}) CREATE (r:Record {key: 'musicbrainz:release-group:' + toString(i) + '-0000-0000-0000-000000000000', source: 'musicbrainz', kind: 'release-group', externalId: toString(i), url: 'https://musicbrainz.org/release-group/' + toString(i), hash: '0123456789abcdef', resolvedHash: 'fedcba9876543210', retrievedAt: '2026-10-22T10:00:00Z', seenIn: '20261022', changedIn: '20261015'}), (r)-[:FROM]->(s), (r)-[:OF]->(k)";
const FOUR: &str = "CREATE (:Record {key: 'k' + toString(i), a: 1, b: 2, c: 3})";
const FIVE: &str = "CREATE (:Record {key: 'k' + toString(i), a: 1, b: 2, c: 3, d: 4})";

fn rows() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn options(dir: &Path) -> DatabaseOpenOptions {
    DatabaseOpenOptions::default().with_database_dir(dir)
}

/// Write `NODES` records with `create` under a uniqueness constraint on
/// `key`, close the database, and return the heap its reopening holds per
/// record (the WAL is replayed, as Festimap's probe opens it).
fn bytes_per_record(tag: &str, create: &str) -> f64 {
    let _guard = MEASURE.lock().unwrap_or_else(|p| p.into_inner());
    let dir = TmpDir::new(tag);
    {
        let db = Database::open_named("app", options(&dir.path)).unwrap();
        db.execute(
            "CREATE (:Source {name: 'musicbrainz'}), (:Kind {name: 'release-group'})",
            rows(),
        )
        .unwrap();
        db.execute(
            "CREATE CONSTRAINT rk FOR (r:Record) REQUIRE r.key IS UNIQUE",
            rows(),
        )
        .unwrap();
        for start in (0..NODES).step_by(2_500) {
            let params = BTreeMap::from([
                ("b".to_string(), LoraValue::Int(start as i64)),
                ("e".to_string(), LoraValue::Int((start + 2_499) as i64)),
            ]);
            db.execute_with_params(
                &format!("UNWIND range($b, $e) AS i {create}"),
                rows(),
                params,
            )
            .unwrap();
        }
    }
    let before = LIVE.load(Ordering::Relaxed);
    let db = Database::open_named("app", options(&dir.path)).unwrap();
    let held = LIVE.load(Ordering::Relaxed) - before;
    assert_eq!(db.node_count(), NODES + 2);
    held as f64 / NODES as f64
}

/// The phase-3 record: a 58-byte key, a hash and two integers. The key's
/// hash index entry is a digest and an id, not a second copy of the key;
/// the record and the key's sorted (RANGE) entry are the rest. It held
/// 341 bytes in 0.30.0, when the hash index kept its own copy of every
/// key (104 bytes) in a 32-byte entry.
#[test]
fn a_keyed_record_costs_its_payload_and_one_copy_of_its_key() {
    let per = bytes_per_record("phase3", PHASE3);
    assert!(per < 260.0, "a phase-3 record holds {per:.0} bytes");
}

/// The phase-1 record: ten properties and two relationships. 568 bytes in
/// 0.30.0.
#[test]
fn a_wide_record_with_two_relationships_stays_under_500_bytes() {
    let per = bytes_per_record("phase1", PHASE1);
    assert!(per < 500.0, "a phase-1 record holds {per:.0} bytes");
}

/// A fifth small property costs a few bytes in the record, not a bigger
/// property map (0.29.0 doubled the map past four).
#[test]
fn a_fifth_property_costs_a_few_bytes() {
    let four = bytes_per_record("four", FOUR);
    let five = bytes_per_record("five", FIVE);
    assert!(
        five - four < 20.0,
        "four properties hold {four:.0} bytes a record, five {five:.0}"
    );
}
