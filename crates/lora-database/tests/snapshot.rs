//! End-to-end snapshot tests driven through `Database`.

use lora_database::{Database, ExecuteOptions, QueryResult, ResultFormat};

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::RowArrays,
    })
}

fn row_count(result: QueryResult) -> usize {
    match result {
        QueryResult::RowArrays(r) => r.rows.len(),
        other => panic!("expected RowArrays, got {:?}", other),
    }
}

#[test]
fn save_and_load_roundtrip_through_filesystem() {
    let dir = tempdir_like("save_and_load_roundtrip");
    let path = dir.join("snap.bin");

    // Build a graph, snapshot, drop, reload into a fresh db.
    {
        let db = Database::in_memory();
        db.execute("CREATE (:Person {name: 'Alice'})", opts())
            .unwrap();
        db.execute("CREATE (:Person {name: 'Bob'})", opts())
            .unwrap();
        db.execute(
            "MATCH (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'}) \
             CREATE (a)-[:KNOWS]->(b)",
            opts(),
        )
        .unwrap();

        let meta = db.save_snapshot_to(&path).unwrap();
        assert_eq!(meta.node_count, 2);
        assert_eq!(meta.relationship_count, 1);
        assert_eq!(meta.wal_lsn, None);
    }

    let db = Database::in_memory_from_snapshot(&path).unwrap();
    assert_eq!(db.node_count(), 2);
    assert_eq!(db.relationship_count(), 1);

    let rows = row_count(
        db.execute(
            "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name",
            opts(),
        )
        .unwrap(),
    );
    assert_eq!(rows, 1);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_replaces_existing_state() {
    let dir = tempdir_like("load_replaces_existing");
    let path = dir.join("snap.bin");

    let donor = Database::in_memory();
    donor.execute("CREATE (:A {n: 1})", opts()).unwrap();
    donor.save_snapshot_to(&path).unwrap();

    let target = Database::in_memory();
    target.execute("CREATE (:B {n: 2})", opts()).unwrap();
    target.execute("CREATE (:B {n: 3})", opts()).unwrap();
    assert_eq!(target.node_count(), 2);

    target.load_snapshot_from(&path).unwrap();
    // The pre-existing :B nodes must be gone — node_count is 1, and the :B
    // label is no longer in the catalog (analyzer rejects a query against
    // an unknown label, which itself proves the restore erased :B).
    assert_eq!(target.node_count(), 1);
    let rows = row_count(target.execute("MATCH (x:A) RETURN x", opts()).unwrap());
    assert_eq!(rows, 1);
    assert!(target.execute("MATCH (x:B) RETURN x", opts()).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn atomic_write_leaves_no_tmp_file() {
    let dir = tempdir_like("atomic_write");
    let path = dir.join("snap.bin");

    let db = Database::in_memory();
    db.execute("CREATE (:N)", opts()).unwrap();
    db.save_snapshot_to(&path).unwrap();

    assert!(path.exists());
    // The .tmp file must not be lying around after a successful save.
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    assert!(!tmp.exists(), "stale .tmp file found at {}", tmp.display());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn snapshot_survives_new_writes_after_restore() {
    let dir = tempdir_like("post_restore_writes");
    let path = dir.join("snap.bin");

    let donor = Database::in_memory();
    donor.execute("CREATE (:Counter {n: 0})", opts()).unwrap();
    donor.save_snapshot_to(&path).unwrap();

    let db = Database::in_memory_from_snapshot(&path).unwrap();
    // New writes must keep using fresh IDs — no collision with the restored
    // state's IDs.
    db.execute("CREATE (:Counter {n: 1})", opts()).unwrap();
    db.execute("CREATE (:Counter {n: 2})", opts()).unwrap();
    assert_eq!(db.node_count(), 3);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_save_cleans_up_tmp_file() {
    let dir = tempdir_like("failed_save");
    // Make the target itself an existing directory so that the final
    // `rename(tmp, target)` step fails with EISDIR — everything before the
    // rename (open, serialize, fsync) succeeds, so this exercises the
    // cleanup guard.
    let target = dir.join("snap.bin");
    std::fs::create_dir_all(&target).unwrap();

    let db = Database::in_memory();
    db.execute("CREATE (:N)", opts()).unwrap();

    let err = db.save_snapshot_to(&target);
    assert!(
        err.is_err(),
        "expected save to fail because target is a directory"
    );

    // The `.tmp` scratch file must not be left behind on a failed save.
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    assert!(
        !tmp.exists(),
        "stale .tmp file at {} after failed save",
        tmp.display()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_load_preserves_existing_state() {
    let dir = tempdir_like("failed_load");
    let path = dir.join("bad.bin");
    // Write something that will fail the magic check. The load must error
    // out without touching the in-memory graph.
    std::fs::write(&path, b"not a snapshot at all").unwrap();

    let db = Database::in_memory();
    db.execute("CREATE (:Keep {n: 1})", opts()).unwrap();
    db.execute("CREATE (:Keep {n: 2})", opts()).unwrap();
    assert_eq!(db.node_count(), 2);

    let err = db.load_snapshot_from(&path);
    assert!(err.is_err(), "expected load to fail on garbage file");

    // Original state must survive a failed restore byte-for-byte.
    assert_eq!(db.node_count(), 2);
    let rows = row_count(db.execute("MATCH (x:Keep) RETURN x", opts()).unwrap());
    assert_eq!(rows, 2);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Minimal temp-dir helper. The test suite does not depend on `tempfile`,
/// so we roll our own: `std::env::temp_dir()` + a per-test suffix, cleaned
/// up at the end of each test.
fn tempdir_like(tag: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "lora-snap-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
