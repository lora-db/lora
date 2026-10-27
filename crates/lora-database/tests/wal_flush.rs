//! `Database::flush` (G-74): a durability point for the commits before it,
//! at the cost of one WAL `fsync`, with no checkpoint.

use std::path::PathBuf;

use lora_database::{Database, DatabaseOpenOptions, ExecuteOptions, ResultFormat, SyncMode};

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lora-wal-flush-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rows() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

#[test]
fn flush_makes_each_commit_durable_with_one_fsync_and_no_snapshot() {
    let dir = TmpDir::new("fence");
    let mut options = DatabaseOpenOptions::default().with_database_dir(&dir.0);
    // A group sync far away: only flush() makes anything durable here.
    options.sync_mode = SyncMode::GroupSync {
        interval_ms: 3_600_000,
    };
    let db = Database::open_named("app", options.clone()).unwrap();
    db.execute("UNWIND range(1, 20000) AS i CREATE (:N {i: i})", rows())
        .unwrap();
    db.flush().unwrap();

    let wal = db.wal().unwrap().wal().clone();
    let fsyncs = wal.fsync_count();
    const CALLS: u64 = 100;
    let mut last = 0;
    for i in 0..CALLS {
        db.execute(&format!("CREATE (:Batch {{id: {i}}})"), rows())
            .unwrap();
        let before = wal.durable_lsn().raw();
        let lsn = db.flush().unwrap().expect("a WAL-backed database");
        assert!(lsn > before, "flush moved the durable LSN past the commit");
        assert!(lsn > last);
        last = lsn;
    }
    // One fsync per call, nothing more.
    assert_eq!(wal.fsync_count() - fsyncs, CALLS);
    // A flush with nothing new does not fsync.
    db.flush().unwrap();
    assert_eq!(wal.fsync_count() - fsyncs, CALLS);
    // No snapshot was written.
    assert!(!dir.0.join("app.loradb.wal").join("snapshot.lsnap").exists());

    drop(db);
    let reopened = Database::open_named("app", options).unwrap();
    assert_eq!(reopened.node_count(), 20_000 + CALLS as usize);
}

#[test]
fn flush_on_an_in_memory_database_has_no_lsn() {
    assert_eq!(Database::in_memory().flush().unwrap(), None);
}
