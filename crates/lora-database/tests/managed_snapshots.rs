//! Managed snapshot tests: snapshots live beside the WAL and carry an LSN
//! fence so recovery can load the snapshot first, then replay only newer WAL
//! records.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lora_database::{
    Database, ExecuteOptions, LoraValue, QueryResult, ResultFormat, SnapshotConfig, TransactionMode,
};
use lora_snapshot::{
    Compression, EncryptionKey, PasswordKdfParams, SnapshotOptions, SnapshotPassword,
};
use lora_wal::{SyncMode, WalConfig};

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

fn wal_enabled(dir: &Path) -> WalConfig {
    WalConfig::Enabled {
        dir: dir.to_path_buf(),
        sync_mode: SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        segment_target_bytes: 512,
        truncate_corrupt_tail: false,
    }
}

#[test]
fn managed_snapshot_recovers_snapshot_then_newer_wal() {
    let dir = TmpDir::new("recover_snapshot_then_newer_wal");
    let wal_dir = dir.path().join("wal");
    let snapshot_dir = dir.path().join("snapshots");

    {
        let db = Database::open_with_wal_snapshots(
            wal_enabled(&wal_dir),
            SnapshotConfig::enabled(&snapshot_dir),
        )
        .unwrap();
        db.execute("CREATE (:User {name: 'alice'})", opts())
            .unwrap();
        db.execute("CREATE (:User {name: 'bob'})", opts()).unwrap();
        db.checkpoint_managed().unwrap();
        db.execute("CREATE (:User {name: 'carol'})", opts())
            .unwrap();
    }

    assert!(snapshot_dir.join("CURRENT").exists());

    let reopened = Database::open_with_wal_snapshots(
        wal_enabled(&wal_dir),
        SnapshotConfig::enabled(&snapshot_dir),
    )
    .unwrap();
    let rows = row_count(reopened.execute("MATCH (u:User) RETURN u", opts()).unwrap());
    assert_eq!(rows, 3);
}

#[test]
fn managed_snapshot_can_checkpoint_after_commit_count() {
    let dir = TmpDir::new("checkpoint_after_commit_count");
    let wal_dir = dir.path().join("wal");
    let snapshot_dir = dir.path().join("snapshots");

    {
        let db = Database::open_with_wal_snapshots(
            wal_enabled(&wal_dir),
            SnapshotConfig::enabled(&snapshot_dir).every_commits(2),
        )
        .unwrap();
        db.execute("CREATE (:User {name: 'alice'})", opts())
            .unwrap();
        assert!(!snapshot_dir.join("CURRENT").exists());
        db.execute("CREATE (:User {name: 'bob'})", opts()).unwrap();
        assert!(snapshot_dir.join("CURRENT").exists());
    }

    let reopened = Database::open_with_wal_snapshots(
        wal_enabled(&wal_dir),
        SnapshotConfig::enabled(&snapshot_dir).every_commits(2),
    )
    .unwrap();
    let rows = row_count(reopened.execute("MATCH (u:User) RETURN u", opts()).unwrap());
    assert_eq!(rows, 2);
}

#[test]
fn direct_graph_mutation_helpers_trigger_managed_checkpoint() {
    let dir = TmpDir::new("direct_graph_helpers_checkpoint");
    let wal_dir = dir.path().join("wal");
    let snapshot_dir = dir.path().join("snapshots");

    {
        let db = Database::open_with_wal_snapshots(
            wal_enabled(&wal_dir),
            SnapshotConfig::enabled(&snapshot_dir).every_commits(2),
        )
        .unwrap();

        let node = db
            .graph_create_node(vec!["User".to_string()], BTreeMap::new())
            .unwrap();
        assert!(!snapshot_dir.join("CURRENT").exists());

        db.graph_set_node_property(
            node.id,
            "name".to_string(),
            LoraValue::String("alice".to_string()),
        )
        .unwrap();
        assert!(snapshot_dir.join("CURRENT").exists());
    }

    let reopened = Database::open_with_wal_snapshots(
        wal_enabled(&wal_dir),
        SnapshotConfig::enabled(&snapshot_dir).every_commits(2),
    )
    .unwrap();
    let rows = row_count(
        reopened
            .execute("MATCH (u:User) WHERE u.name = 'alice' RETURN u", opts())
            .unwrap(),
    );
    assert_eq!(rows, 1);
}

#[test]
fn readwrite_transaction_with_only_reads_does_not_trigger_managed_checkpoint() {
    let dir = TmpDir::new("readwrite_read_only_tx_no_checkpoint");
    let wal_dir = dir.path().join("wal");
    let snapshot_dir = dir.path().join("snapshots");

    {
        let db = Database::open_with_wal_snapshots(
            wal_enabled(&wal_dir),
            SnapshotConfig::enabled(&snapshot_dir).every_commits(2),
        )
        .unwrap();
        db.execute("CREATE (:User {name: 'alice'})", opts())
            .unwrap();
        assert!(!snapshot_dir.join("CURRENT").exists());

        let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
        let rows = row_count(tx.execute("MATCH (u:User) RETURN u", opts()).unwrap());
        assert_eq!(rows, 1);
        tx.commit().unwrap();
        assert!(
            !snapshot_dir.join("CURRENT").exists(),
            "read-only-in-effect ReadWrite tx must not count as a durable WAL commit"
        );

        db.execute("CREATE (:User {name: 'bob'})", opts()).unwrap();
        assert!(snapshot_dir.join("CURRENT").exists());
    }

    let reopened = Database::open_with_wal_snapshots(
        wal_enabled(&wal_dir),
        SnapshotConfig::enabled(&snapshot_dir).every_commits(2),
    )
    .unwrap();
    let rows = row_count(reopened.execute("MATCH (u:User) RETURN u", opts()).unwrap());
    assert_eq!(rows, 2);
}

#[test]
fn managed_snapshot_can_use_encryption_and_compression() {
    let dir = TmpDir::new("encryption_and_compression");
    let wal_dir = dir.path().join("wal");
    let snapshot_dir = dir.path().join("snapshots");
    let codec = SnapshotOptions {
        compression: Compression::Gzip { level: 5 },
        encryption: Some(EncryptionKey::new("local-test", [7; 32]).into()),
    };

    {
        let db = Database::open_with_wal_snapshots(
            wal_enabled(&wal_dir),
            SnapshotConfig::enabled(&snapshot_dir).codec(codec.clone()),
        )
        .unwrap();
        db.execute("CREATE (:User {name: 'alice'})", opts())
            .unwrap();
        db.checkpoint_managed().unwrap();
    }

    let reopened = Database::open_with_wal_snapshots(
        wal_enabled(&wal_dir),
        SnapshotConfig::enabled(&snapshot_dir).codec(codec),
    )
    .unwrap();
    let rows = row_count(reopened.execute("MATCH (u:User) RETURN u", opts()).unwrap());
    assert_eq!(rows, 1);
}

#[test]
fn managed_snapshot_can_use_password_encryption() {
    let dir = TmpDir::new("password_encryption");
    let wal_dir = dir.path().join("wal");
    let snapshot_dir = dir.path().join("snapshots");
    let codec = SnapshotOptions {
        compression: Compression::Gzip { level: 6 },
        encryption: Some(
            SnapshotPassword::with_params(
                "operator-password",
                "super-secret",
                PasswordKdfParams {
                    memory_cost_kib: 512,
                    time_cost: 1,
                    parallelism: 1,
                },
            )
            .into(),
        ),
    };

    {
        let db = Database::open_with_wal_snapshots(
            wal_enabled(&wal_dir),
            SnapshotConfig::enabled(&snapshot_dir).codec(codec.clone()),
        )
        .unwrap();
        db.execute("CREATE (:User {name: 'alice'})", opts())
            .unwrap();
        db.checkpoint_managed().unwrap();
    }

    let reopened = Database::open_with_wal_snapshots(
        wal_enabled(&wal_dir),
        SnapshotConfig::enabled(&snapshot_dir).codec(codec),
    )
    .unwrap();
    let rows = row_count(reopened.execute("MATCH (u:User) RETURN u", opts()).unwrap());
    assert_eq!(rows, 1);
}

/// A failing automatic checkpoint must not fail the write it follows: the
/// commit is already in the WAL, so the write has to be published (and the
/// next write must not reuse its ids) or recovery diverges from what callers
/// saw.
#[cfg(unix)]
#[test]
fn failing_automatic_checkpoint_does_not_fail_or_lose_writes() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TmpDir::new("failing_auto_checkpoint");
    let wal_dir = dir.path().join("wal");
    let snapshot_dir = dir.path().join("snapshots");
    let config = || SnapshotConfig::enabled(&snapshot_dir).every_commits(1);

    {
        let db = Database::open_with_wal_snapshots(wal_enabled(&wal_dir), config()).unwrap();
        std::fs::set_permissions(&snapshot_dir, std::fs::Permissions::from_mode(0o500)).unwrap();

        // Auto-commit, explicit transaction and graph API write paths.
        db.execute("CREATE (:User {name: 'alice'})", opts())
            .unwrap();
        db.execute("CREATE (:User {name: 'bob'})", opts()).unwrap();
        db.execute(
            "MATCH (u:User {name: 'alice'}) SET u.age = 30 WITH u MATCH (b:User {name: 'bob'}) CREATE (u)-[:KNOWS]->(b)",
            opts(),
        )
        .unwrap();
        let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
        tx.execute("CREATE (:User {name: 'carol'})", opts())
            .unwrap();
        tx.commit().unwrap();
        db.graph_create_node(vec!["User".to_string()], BTreeMap::new())
            .unwrap();
        assert_eq!(
            row_count(db.execute("MATCH (u:User) RETURN u", opts()).unwrap()),
            4
        );
        assert!(!snapshot_dir.join("CURRENT").exists());

        std::fs::set_permissions(&snapshot_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    let reopened = Database::open_with_wal_snapshots(wal_enabled(&wal_dir), config()).unwrap();
    assert_eq!(
        row_count(reopened.execute("MATCH (u:User) RETURN u", opts()).unwrap()),
        4
    );
    assert_eq!(
        row_count(
            reopened
                .execute("MATCH (:User {age: 30})-[:KNOWS]->(:User) RETURN 1", opts())
                .unwrap()
        ),
        1
    );
    // With the directory writable again the next commit checkpoints.
    reopened
        .execute("CREATE (:User {name: 'dave'})", opts())
        .unwrap();
    assert!(snapshot_dir.join("CURRENT").exists());
}

fn wal_segments(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().extension() == Some("wal".as_ref()))
        .count()
}

fn write_users(db: &Database<lora_database::InMemoryGraph>, ids: std::ops::Range<u32>) {
    for i in ids {
        db.execute(
            &format!("CREATE (:User {{id: {i}, pad: 'xxxxxxxxxxxxxxxxxxxxxxxx'}})"),
            opts(),
        )
        .unwrap();
    }
}

/// Wait for the background checkpoint to leave a snapshot.
fn wait_for(mut done: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !done() {
        if std::time::Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    true
}

/// A process that writes for days must not keep every WAL segment: managed
/// snapshots checkpoint once the WAL has grown by `checkpoint_wal_bytes`,
/// without a commit count or an explicit call.
#[test]
fn managed_snapshots_checkpoint_by_wal_size() {
    let dir = TmpDir::new("checkpoint_by_wal_size");
    let wal_dir = dir.path().join("wal");
    let snapshot_dir = dir.path().join("snapshots");
    let config = || SnapshotConfig::enabled(&snapshot_dir).checkpoint_wal_bytes(16 * 1024);

    {
        let db = Database::open_with_wal_snapshots(wal_enabled(&wal_dir), config()).unwrap();
        write_users(&db, 0..300);
        assert!(wait_for(|| snapshot_dir.join("CURRENT").exists()));
        // Writes keep going while it runs and after it.
        write_users(&db, 300..600);
        // 600 commits fill about 200 segments; 16 KiB is about 32.
        assert!(wait_for(|| wal_segments(&wal_dir) < 60));
    }

    let reopened = Database::open_with_wal_snapshots(wal_enabled(&wal_dir), config()).unwrap();
    assert_eq!(
        row_count(reopened.execute("MATCH (u:User) RETURN u", opts()).unwrap()),
        600
    );
}

#[test]
fn managed_snapshots_wal_size_checkpoint_can_be_turned_off() {
    let dir = TmpDir::new("checkpoint_by_wal_size_off");
    let wal_dir = dir.path().join("wal");
    let snapshot_dir = dir.path().join("snapshots");
    let db = Database::open_with_wal_snapshots(
        wal_enabled(&wal_dir),
        SnapshotConfig::enabled(&snapshot_dir).checkpoint_wal_bytes(0),
    )
    .unwrap();
    write_users(&db, 0..600);
    drop(db);
    assert!(!snapshot_dir.join("CURRENT").exists());
    // About three commits per 512-byte segment.
    assert!(wal_segments(&wal_dir) > 150, "{}", wal_segments(&wal_dir));
}

/// The checkpoint by WAL size runs after the commit, on its own thread: a
/// failure is logged, never returned, and loses nothing.
#[cfg(unix)]
#[test]
fn failing_wal_size_checkpoint_does_not_fail_or_lose_writes() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TmpDir::new("failing_wal_size_checkpoint");
    let wal_dir = dir.path().join("wal");
    let snapshot_dir = dir.path().join("snapshots");
    let config = || SnapshotConfig::enabled(&snapshot_dir).checkpoint_wal_bytes(16 * 1024);

    {
        let db = Database::open_with_wal_snapshots(wal_enabled(&wal_dir), config()).unwrap();
        std::fs::set_permissions(&snapshot_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        write_users(&db, 0..300);
        let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
        tx.execute("CREATE (:User {id: 300})", opts()).unwrap();
        tx.commit().unwrap();
        // Dropping waits for a running checkpoint.
        drop(db);
        assert!(!snapshot_dir.join("CURRENT").exists());
        std::fs::set_permissions(&snapshot_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    let reopened = Database::open_with_wal_snapshots(wal_enabled(&wal_dir), config()).unwrap();
    assert_eq!(
        row_count(reopened.execute("MATCH (u:User) RETURN u", opts()).unwrap()),
        301
    );
    // With the directory writable again a later checkpoint succeeds.
    write_users(&reopened, 400..700);
    assert!(wait_for(|| snapshot_dir.join("CURRENT").exists()));
}

struct TmpDir {
    path: PathBuf,
}

impl TmpDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lora-managed-snapshot-test-{}-{}-{}",
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
