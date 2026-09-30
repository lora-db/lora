//! A write to a node whose indexed value is shared by many nodes must not
//! cost O(nodes with that value).
//!
//! Low-cardinality indexed properties (enums, booleans, defaults) put most
//! nodes in one posting list. Index maps are copy-on-write, so a write that
//! touches a shared posting list copies it; the posting list itself is
//! chunked so that copy is one chunk plus a pointer table, not every id.

mod test_helpers;
use std::time::Instant;

use test_helpers::TestDb;

/// `n` festivals, all with `capacity: 100`, under a RANGE index (hash plus
/// sorted buckets) and a lookup index on `key`.
fn festivals(n: usize) -> TestDb {
    let db = TestDb::new();
    db.run("CREATE INDEX cap FOR (f:Festival) ON (f.capacity)");
    db.run("CREATE CONSTRAINT fk FOR (f:Festival) REQUIRE f.key IS UNIQUE");
    db.run(&format!(
        "UNWIND range(0, {}) AS i CREATE (:Festival {{key: 'f' + toString(i), capacity: 100}})",
        n - 1
    ));
    // Warm the hash index for the shared value.
    db.run("MATCH (f:Festival {capacity: 100}) RETURN count(f) AS c");
    db
}

const WRITES: usize = 200;

/// Seconds per write for `query`, which must create or move one node into
/// the shared bucket. With `hold_reader`, a reader snapshot is open across
/// every write, so even an in-place write must copy what it touches.
fn per_write(db: &TestDb, query: &str, hold_reader: bool) -> f64 {
    let write = || {
        let reader = hold_reader.then(|| db.service.snapshot());
        db.run(query);
        drop(reader);
    };
    for _ in 0..20 {
        write();
    }
    let start = Instant::now();
    for _ in 0..WRITES {
        write();
    }
    start.elapsed().as_secs_f64() / WRITES as f64
}

const AUTO_COMMIT: &str = "CREATE (:Festival {key: randomUUID(), capacity: 100})";
const STAGED: &str = "MERGE (f:Festival {key: randomUUID()}) SET f.capacity = 100";
const UPDATE: &str = "MATCH (f:Festival {key: 'f7'}) \
     SET f.capacity = CASE f.capacity WHEN 100 THEN 101 ELSE 100 END";

const CASES: [&str; 4] = [
    "auto-commit",
    "auto-commit with a reader",
    "staged",
    "update",
];

fn measure(n: usize) -> [f64; 4] {
    let db = festivals(n);
    [
        per_write(&db, AUTO_COMMIT, false),
        per_write(&db, AUTO_COMMIT, true),
        per_write(&db, STAGED, false),
        per_write(&db, UPDATE, false),
    ]
}

#[test]
fn write_cost_is_independent_of_shared_bucket_size() {
    let small = measure(2_000);
    let large = measure(60_000);
    eprintln!("small {small:?}\nlarge {large:?}");
    for (name, (s, l)) in CASES.iter().zip(small.iter().zip(large.iter())) {
        // A whole-bucket copy makes the large case ~10x slower or worse.
        assert!(
            *l < s * 3.0 + 0.0005,
            "{name}: small {s:.6}s, large {l:.6}s"
        );
    }
}

#[test]
fn shared_bucket_stays_exact_across_writes() {
    let db = festivals(3_000);
    let before = db.service.snapshot();
    db.run("MATCH (f:Festival) WHERE f.key IN ['f1', 'f2', 'f3'] SET f.capacity = 50");
    db.run("MATCH (f:Festival {key: 'f4'}) DETACH DELETE f");
    db.run("CREATE (:Festival {key: 'new', capacity: 100})");
    assert_eq!(
        db.scalar("MATCH (f:Festival {capacity: 100}) RETURN count(f) AS c"),
        2_997
    );
    assert_eq!(
        db.scalar("MATCH (f:Festival) WHERE f.capacity = 50 RETURN count(f) AS c"),
        3
    );
    assert_eq!(
        db.scalar("MATCH (f:Festival) WHERE f.capacity > 60 RETURN count(f) AS c"),
        2_997
    );
    // Ascending id order within a bucket.
    let keys = db.run("MATCH (f:Festival {capacity: 100}) RETURN f.key AS k LIMIT 3");
    let keys: Vec<_> = keys
        .iter()
        .map(|r| r["k"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(keys, ["f0", "f5", "f6"]);
    // The snapshot taken before the writes still sees the old bucket.
    assert_eq!(lora_store::GraphStorage::node_count(&*before), 3_000);
}
