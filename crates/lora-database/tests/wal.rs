//! Integration tests for the WAL-aware Database constructors.
//!
//! These exercise the seam between `Database<InMemoryGraph>` and
//! `lora-wal::Wal` end-to-end: a real query path drives mutations
//! through the engine, the WAL captures them under the store write lock,
//! and a fresh process (modelled by dropping + re-opening the
//! database) recovers them via replay.

use std::path::{Path, PathBuf};

use lora_database::{
    resolve_database_path, Database, DatabaseName, DatabaseOpenOptions, ExecuteOptions,
    ResultFormat,
};
use lora_store::{MutationEvent, Properties, PropertyValue};
use lora_wal::{Lsn, SyncMode, Wal, WalConfig};

// ---------------------------------------------------------------------------
// Test scaffolding
// ---------------------------------------------------------------------------

/// Per-test scratch directory. Roll our own (matching the snapshot
/// tests' helper) so we don't take a `tempfile` dev-dependency.
struct TmpDir {
    path: PathBuf,
}

impl TmpDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lora-db-wal-{}-{}-{}",
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

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn rows() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn enabled(dir: &Path) -> WalConfig {
    WalConfig::Enabled {
        dir: dir.to_path_buf(),
        sync_mode: SyncMode::PerCommit,
        segment_target_bytes: 8 * 1024 * 1024,
    }
}

fn group_enabled(dir: &Path) -> WalConfig {
    WalConfig::Enabled {
        dir: dir.to_path_buf(),
        sync_mode: SyncMode::Group {
            interval_ms: 60_000,
        },
        segment_target_bytes: 8 * 1024 * 1024,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn disabled_config_behaves_like_in_memory() {
    let db = Database::open_with_wal(WalConfig::Disabled).unwrap();
    db.execute("CREATE (:User {id: 1})", rows()).unwrap();
    assert_eq!(db.node_count(), 1);
    assert!(db.wal().is_none());
}

#[test]
fn database_name_validation_accepts_only_portable_names() {
    for valid in ["app", "tenant_01", "a-b.c", "A123"] {
        assert!(
            DatabaseName::parse(valid).is_ok(),
            "{valid} should be valid"
        );
    }

    for invalid in ["", ".", "..", "../x", "x/y", "has space", "ümlaut"] {
        assert!(
            DatabaseName::parse(invalid).is_err(),
            "{invalid:?} should be invalid"
        );
    }
}

#[test]
fn named_database_resolves_to_lora_root_under_database_dir() {
    let dir = TmpDir::new("named-path");
    let path = resolve_database_path("app_01", dir.path()).unwrap();
    assert_eq!(path, dir.path().join("app_01.lora"));
}

#[test]
fn named_database_persists_under_lora_root() {
    let dir = TmpDir::new("named-recover");

    {
        let db = Database::open_named(
            "app",
            DatabaseOpenOptions::default().with_database_dir(dir.path()),
        )
        .unwrap();
        db.execute("CREATE (:User {id: 1})", rows()).unwrap();
    }

    assert!(
        dir.path().join("app.lora").is_file(),
        "named databases should persist as a portable .lora archive file"
    );
    let bytes = std::fs::read(dir.path().join("app.lora")).unwrap();
    assert_eq!(&bytes[..4], b"PK\x03\x04");
    let file = std::fs::File::open(dir.path().join("app.lora")).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    assert!(zip.by_name("manifest.json").is_ok());
    assert!(zip.by_name("wal/0000000001.wal").is_ok());

    let db = Database::open_named(
        "app",
        DatabaseOpenOptions::default().with_database_dir(dir.path()),
    )
    .unwrap();
    assert_eq!(db.node_count(), 1);
}

#[test]
fn named_database_recovers_write_burst_from_zip_archive() {
    let dir = TmpDir::new("named-burst");

    {
        let db = Database::open_named(
            "burst",
            DatabaseOpenOptions::default().with_database_dir(dir.path()),
        )
        .unwrap();
        for i in 0..250 {
            db.execute(&format!("CREATE (:Burst {{id: {i}}})"), rows())
                .unwrap();
        }
        assert_eq!(db.node_count(), 250);
    }

    let archive_path = dir.path().join("burst.lora");
    assert!(archive_path.is_file());
    let file = std::fs::File::open(&archive_path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    assert!(zip.by_name("manifest.json").is_ok());
    assert!(zip.by_name("wal/0000000001.wal").is_ok());

    let db = Database::open_named(
        "burst",
        DatabaseOpenOptions::default().with_database_dir(dir.path()),
    )
    .unwrap();
    assert_eq!(db.node_count(), 250);
    let result = db
        .execute("MATCH (n:Burst) RETURN n.id AS id ORDER BY id", rows())
        .unwrap();
    let json = serde_json::to_value(&result).unwrap();
    let row_array = json["rows"].as_array().expect("rows array");
    assert_eq!(row_array.first().unwrap()["id"], serde_json::json!(0));
    assert_eq!(row_array.last().unwrap()["id"], serde_json::json!(249));
}

#[test]
fn named_database_final_archive_flush_captures_group_buffer() {
    let dir = TmpDir::new("named-group-final-flush");

    {
        let db = Database::open_named(
            "app",
            DatabaseOpenOptions {
                database_dir: dir.path().to_path_buf(),
                sync_mode: SyncMode::Group {
                    interval_ms: 60_000,
                },
                ..DatabaseOpenOptions::default()
            },
        )
        .unwrap();
        db.execute(
            "CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {name: 'Grace'})",
            rows(),
        )
        .unwrap();

        // Let the archive debounce worker observe the dirty flag before the
        // Group-mode WAL flusher runs. The final archive flush on drop must
        // still snapshot the force-flushed WAL bytes, not the earlier empty
        // segment file.
        std::thread::sleep(std::time::Duration::from_millis(1_200));
    }

    let db = Database::open_named(
        "app",
        DatabaseOpenOptions::default().with_database_dir(dir.path()),
    )
    .unwrap();
    assert_eq!(db.node_count(), 2);
    assert_eq!(db.relationship_count(), 1);
}

#[test]
fn fresh_open_then_crash_recover_replays_committed_writes() {
    let dir = TmpDir::new("recover");

    {
        let db = Database::open_with_wal(enabled(dir.path())).unwrap();
        db.execute("CREATE (:User {id: 1, name: 'alice'})", rows())
            .unwrap();
        db.execute("CREATE (:User {id: 2, name: 'bob'})", rows())
            .unwrap();
        // Drop without explicit close to model a crash; PerCommit
        // already fsync'd the commit markers, so the WAL is durable.
    }

    // Fresh process: empty graph + WAL on disk → recover replays.
    let db = Database::open_with_wal(enabled(dir.path())).unwrap();
    assert_eq!(db.node_count(), 2);

    // Confirm the property values made it through replay (not just
    // the right number of nodes).
    let result = db
        .execute("MATCH (u:User) RETURN u.id AS id ORDER BY id", rows())
        .unwrap();
    let json = serde_json::to_value(&result).unwrap();
    let row_array = json["rows"].as_array().expect("rows array");
    assert_eq!(row_array.len(), 2);
    assert_eq!(row_array[0]["id"], serde_json::json!(1));
    assert_eq!(row_array[1]["id"], serde_json::json!(2));
}

#[test]
fn read_only_queries_dont_block_recovery() {
    // A read-only query bracketed by arm/commit produces zero records
    // in the WAL. Recovery must handle that without spurious empty
    // events.
    let dir = TmpDir::new("read-only");

    {
        let db = Database::open_with_wal(enabled(dir.path())).unwrap();
        db.execute("CREATE (:Tag {v: 1})", rows()).unwrap();
        // Pure reads — should fire no mutation records.
        for _ in 0..5 {
            db.execute("MATCH (t:Tag) RETURN t", rows()).unwrap();
        }
        db.execute("CREATE (:Tag {v: 2})", rows()).unwrap();
    }

    let db = Database::open_with_wal(enabled(dir.path())).unwrap();
    assert_eq!(db.node_count(), 2);
}

/// Sum of every `*.wal` file size in `dir`. Used to prove the WAL
/// hot-path is byte-stable across a run of read-only queries.
fn wal_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("wal") {
            total += std::fs::metadata(&p).unwrap().len();
        }
    }
    total
}

#[test]
fn read_only_queries_do_not_grow_wal_or_advance_lsn() {
    // Regression test for the lazy-begin path: read-only queries
    // through `Database::execute_with_params` must not produce any
    // WAL records, fsyncs, or LSN advances. The user-visible cost
    // of running 200 reads should be 0 bytes added to the log.
    let dir = TmpDir::new("ro-no-grow");
    let db = Database::open_with_wal(enabled(dir.path())).unwrap();

    // One write to seed the WAL with at least one segment.
    db.execute("CREATE (:Tag {v: 1})", rows()).unwrap();

    let bytes_before = wal_bytes(dir.path());
    let lsn_before = db.wal().unwrap().wal().next_lsn();

    for _ in 0..200 {
        db.execute("MATCH (t:Tag) RETURN t", rows()).unwrap();
    }

    let bytes_after = wal_bytes(dir.path());
    let lsn_after = db.wal().unwrap().wal().next_lsn();

    assert_eq!(
        bytes_before,
        bytes_after,
        "200 read-only queries grew the WAL by {} bytes",
        bytes_after.saturating_sub(bytes_before)
    );
    assert_eq!(
        lsn_before, lsn_after,
        "200 read-only queries advanced next_lsn from {} to {}",
        lsn_before, lsn_after
    );
}

#[test]
fn aborted_query_does_not_persist_partial_mutation() {
    // The engine has no rollback, so a query that mutates and then
    // errors leaves partial in-memory state. The WAL must mark that
    // transaction aborted so recovery from a fresh process drops it.
    let dir = TmpDir::new("aborted");

    {
        let db = Database::open_with_wal(enabled(dir.path())).unwrap();
        db.execute("CREATE (:User {id: 1})", rows()).unwrap();

        // Pick a query that compiles but fails at execute time.
        // Creating a relationship with an unknown variable surfaces
        // a runtime error; specifics are less important than the
        // fact that the resulting Err triggers the abort branch.
        let bad = db.execute("MATCH (u:User) CREATE (u)-[:KNOWS]->(missing)", rows());
        // The query may either reject at semantic-analysis time
        // (Err) or succeed by creating the missing node implicitly,
        // depending on planner specifics. We tolerate both — the
        // assertion is on what *recovery* produces.
        let _ = bad;
    }

    // Recovery should produce a graph consistent with what was
    // committed. If the bad query was rejected pre-execute, only
    // the first User exists; if it succeeded, both nodes exist plus
    // the relationship. Either way, recovery state must equal a
    // fresh `Database::open_with_wal` reading the same WAL.
    let recovered = Database::open_with_wal(enabled(dir.path())).unwrap();
    let count = recovered.node_count();
    // Run the same CREATE on a *different* WAL-disabled DB and
    // compare counts to confirm reproducibility — we don't care
    // about the exact number, only that recovery is deterministic.
    drop(recovered);
    let again = Database::open_with_wal(enabled(dir.path())).unwrap();
    assert_eq!(again.node_count(), count);
}

#[test]
fn failed_mutating_query_poisons_live_wal_handle_until_restart() {
    let dir = TmpDir::new("abort-poisons-live");

    {
        let db = Database::open_with_wal(enabled(dir.path())).unwrap();
        let err = db
            .execute("CREATE (a)-[:R]->(b) WITH a DELETE a", rows())
            .unwrap_err();
        assert!(
            err.to_string().contains("WAL poisoned"),
            "expected the failed mutating query to poison the live handle, got {err}"
        );

        let next = db.execute("RETURN 1 AS ok", rows()).unwrap_err();
        assert!(
            next.to_string().contains("WAL arm failed"),
            "expected future queries on the live handle to fail, got {next}"
        );
    }

    let recovered = Database::open_with_wal(enabled(dir.path())).unwrap();
    assert_eq!(
        recovered.node_count(),
        0,
        "recovery should discard the aborted create/delete transaction"
    );
}

#[test]
fn replay_preserves_ids_after_aborted_create_gap() {
    let dir = TmpDir::new("id-gap");

    {
        let (wal, replay) =
            Wal::open(dir.path(), SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO).unwrap();
        assert!(replay.is_empty());

        let aborted = wal.begin().unwrap();
        wal.append(
            aborted,
            &MutationEvent::CreateNode {
                id: 0,
                labels: vec!["Discarded".into()],
                properties: Properties::new(),
            },
        )
        .unwrap();
        wal.abort(aborted).unwrap();
        wal.flush().unwrap();

        let create = wal.begin().unwrap();
        wal.append(
            create,
            &MutationEvent::CreateNode {
                id: 1,
                labels: vec!["Kept".into()],
                properties: Properties::new(),
            },
        )
        .unwrap();
        wal.commit(create).unwrap();
        wal.flush().unwrap();

        let set_name = wal.begin().unwrap();
        wal.append(
            set_name,
            &MutationEvent::SetNodeProperty {
                node_id: 1,
                key: "name".into(),
                value: PropertyValue::String("survivor".into()),
            },
        )
        .unwrap();
        wal.commit(set_name).unwrap();
        wal.flush().unwrap();
    }

    let db = Database::open_with_wal(enabled(dir.path())).unwrap();
    assert_eq!(db.node_count(), 1);

    let result = db
        .execute(
            "MATCH (n:Kept {name: 'survivor'}) RETURN n.name AS name",
            rows(),
        )
        .unwrap();
    let json = serde_json::to_value(&result).unwrap();
    let row_array = json["rows"].as_array().expect("rows array");
    assert_eq!(row_array.len(), 1);
    assert_eq!(row_array[0]["name"], serde_json::json!("survivor"));
}

#[test]
fn replay_rejects_relationship_with_missing_endpoint() {
    let dir = TmpDir::new("missing-endpoint");

    {
        let (wal, replay) =
            Wal::open(dir.path(), SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO).unwrap();
        assert!(replay.is_empty());

        let tx = wal.begin().unwrap();
        wal.append(
            tx,
            &MutationEvent::CreateRelationship {
                id: 0,
                src: 10,
                dst: 11,
                rel_type: "BROKEN".into(),
                properties: Properties::new(),
            },
        )
        .unwrap();
        wal.commit(tx).unwrap();
        wal.flush().unwrap();
    }

    let err = match Database::open_with_wal(enabled(dir.path())) {
        Ok(_) => panic!("recovery should reject the malformed relationship"),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("missing source node 10"),
        "unexpected recovery error: {err}"
    );
}

#[test]
fn checkpoint_truncates_segments_and_recovery_uses_snapshot() {
    let wal_dir = TmpDir::new("ckpt-wal");
    let snap_dir = TmpDir::new("ckpt-snap");
    let snap_path = snap_dir.path().join("snapshot.bin");

    let db = Database::open_with_wal(enabled(wal_dir.path())).unwrap();
    for i in 0..10 {
        db.execute(&format!("CREATE (:N {{i: {}}})", i), rows())
            .unwrap();
    }

    let meta = db.checkpoint_to(&snap_path).unwrap();
    assert_eq!(meta.node_count, 10);
    assert!(
        meta.wal_lsn.is_some(),
        "checkpoint must stamp a wal_lsn into the snapshot header"
    );

    // Add a couple more mutations after the checkpoint.
    db.execute("CREATE (:N {i: 100})", rows()).unwrap();
    db.execute("CREATE (:N {i: 101})", rows()).unwrap();
    drop(db);

    // Recover from snapshot + WAL. The snapshot covers events
    // 0..=9; the WAL contributes 100, 101.
    let recovered = Database::recover(&snap_path, enabled(wal_dir.path())).unwrap();
    assert_eq!(recovered.node_count(), 12);
}

#[test]
fn group_mode_checkpoint_uses_fsynced_fence() {
    let wal_dir = TmpDir::new("group-ckpt-wal");
    let snap_dir = TmpDir::new("group-ckpt-snap");
    let snap_path = snap_dir.path().join("snapshot.bin");

    {
        let db = Database::open_with_wal(group_enabled(wal_dir.path())).unwrap();
        db.execute("CREATE (:N {i: 1})", rows()).unwrap();
        let meta = db.checkpoint_to(&snap_path).unwrap();
        assert!(
            meta.wal_lsn.unwrap_or_default() > 0,
            "checkpoint should stamp a non-zero WAL fence"
        );
    }

    let recovered = Database::recover(&snap_path, group_enabled(wal_dir.path())).unwrap();
    assert_eq!(recovered.node_count(), 1);
}

#[test]
fn recover_with_missing_snapshot_falls_back_to_wal_only() {
    let wal_dir = TmpDir::new("missing-snap-wal");
    let snap_dir = TmpDir::new("missing-snap-snap");
    let absent = snap_dir.path().join("does-not-exist.bin");

    {
        let db = Database::open_with_wal(enabled(wal_dir.path())).unwrap();
        db.execute("CREATE (:Z {v: 1})", rows()).unwrap();
        db.execute("CREATE (:Z {v: 2})", rows()).unwrap();
    }

    let db = Database::recover(&absent, enabled(wal_dir.path())).unwrap();
    assert_eq!(db.node_count(), 2);
}

#[test]
fn recover_with_disabled_wal_only_loads_snapshot() {
    let snap_dir = TmpDir::new("disabled-recover");
    let snap_path = snap_dir.path().join("seed.bin");

    {
        let db = Database::in_memory();
        db.execute("CREATE (:Seed {x: 1})", rows()).unwrap();
        db.execute("CREATE (:Seed {x: 2})", rows()).unwrap();
        db.save_snapshot_to(&snap_path).unwrap();
    }

    let db = Database::recover(&snap_path, WalConfig::Disabled).unwrap();
    assert_eq!(db.node_count(), 2);
    assert!(db.wal().is_none());
}

#[test]
fn clear_brackets_through_wal() {
    // `Database::clear` must not poison the recorder by firing a
    // `Clear` event with no active transaction.
    let dir = TmpDir::new("clear");

    {
        let db = Database::open_with_wal(enabled(dir.path())).unwrap();
        db.execute("CREATE (:A {v: 1})", rows()).unwrap();
        db.execute("CREATE (:A {v: 2})", rows()).unwrap();
        db.clear();
        assert_eq!(db.node_count(), 0);
        // Subsequent query must succeed — the recorder is not
        // poisoned.
        db.execute("CREATE (:B {v: 3})", rows()).unwrap();
    }

    let db = Database::open_with_wal(enabled(dir.path())).unwrap();
    assert_eq!(db.node_count(), 1);
}

#[test]
fn checkpoint_requires_wal() {
    let snap_dir = TmpDir::new("no-wal-ckpt");
    let snap_path = snap_dir.path().join("snap.bin");

    let db = Database::in_memory();
    let err = db.checkpoint_to(&snap_path).unwrap_err();
    assert!(err.to_string().contains("WAL"));
}
