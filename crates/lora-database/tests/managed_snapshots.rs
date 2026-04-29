//! Managed snapshot tests: snapshots live beside the WAL and carry an LSN
//! fence so recovery can load the snapshot first, then replay only newer WAL
//! records.

use std::path::{Path, PathBuf};

use lora_database::{Database, ExecuteOptions, QueryResult, ResultFormat, SnapshotConfig};
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
        sync_mode: SyncMode::PerCommit,
        segment_target_bytes: 512,
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
