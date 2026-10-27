//! Online snapshots of a named database (`Database::snapshot_to_dir`) and
//! the explicit `Database::checkpoint`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lora_database::{
    Change, ChangeFeedOptions, ChangePoll, Database, DatabaseOpenOptions, ExecuteOptions,
    InMemoryGraph, LoraErrorCode, ResultFormat,
};

struct TmpDir {
    path: PathBuf,
}

impl TmpDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lora-db-dir-snapshot-{}-{}-{}",
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

fn rows() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn options(dir: &Path) -> DatabaseOpenOptions {
    DatabaseOpenOptions {
        segment_target_bytes: 256 * 1024,
        ..DatabaseOpenOptions::default().with_database_dir(dir)
    }
}

fn open(dir: &Path) -> Database<InMemoryGraph> {
    Database::open_named("app", options(dir)).unwrap()
}

fn json_rows(db: &Database<InMemoryGraph>, query: &str) -> Vec<serde_json::Value> {
    let result = db.execute(query, rows()).unwrap();
    serde_json::to_value(&result).unwrap()["rows"]
        .as_array()
        .cloned()
        .unwrap()
}

/// Nodes each write transaction of the concurrent tests creates: a copy
/// taken between transactions holds a multiple of it.
const PER_TX: usize = 7;

fn write_tx(db: &Database<InMemoryGraph>, tx: usize) {
    db.execute(
        &format!("UNWIND list.range(1, {PER_TX}) AS i CREATE (:W {{tx: {tx}, i: i}})"),
        rows(),
    )
    .unwrap();
}

#[test]
fn a_snapshot_opens_as_the_graph_it_was_taken_from() {
    let dir = TmpDir::new("basic");
    let live = dir.path.join("live");
    let target = dir.path.join("copy");
    let db = open(&live);
    db.execute(
        "CREATE CONSTRAINT person_name FOR (p:Person) REQUIRE p.name IS UNIQUE",
        rows(),
    )
    .unwrap();
    db.execute(
        "UNWIND list.range(1, 500) AS i CREATE (:Person {name: 'p' + toString(i)})-[:KNOWS]->(:Pet {i: i})",
        rows(),
    )
    .unwrap();

    let snapshot = db.snapshot_to_dir(&target).unwrap();
    assert_eq!(snapshot.path, target.join("app.loradb"));
    assert_eq!(snapshot.node_count, 1000);
    assert_eq!(snapshot.relationship_count, 500);
    assert_eq!(
        snapshot.bytes,
        std::fs::metadata(&snapshot.path).unwrap().len()
    );
    assert_eq!(
        Some(snapshot.lsn),
        db.wal().map(|w| w.wal().durable_lsn().raw())
    );
    // Only the container: no work directory, lock or scratch file.
    let entries: Vec<_> = std::fs::read_dir(&target)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(entries, vec!["app.loradb".to_string()]);

    // The live database keeps its writes to itself.
    db.execute("CREATE (:Person {name: 'later'})", rows())
        .unwrap();

    let copy = open(&target);
    assert_eq!(copy.node_count(), 1000);
    assert_eq!(copy.relationship_count(), 500);
    assert_eq!(
        json_rows(
            &copy,
            "MATCH (p:Person {name: 'p42'})-[:KNOWS]->(x) RETURN x.i AS i"
        ),
        vec![serde_json::json!({"i": 42})]
    );
    // The uniqueness constraint came along.
    let err = copy
        .execute("CREATE (:Person {name: 'p1'})", rows())
        .unwrap_err();
    assert_eq!(err.code(), LoraErrorCode::UniqueConstraint);
    // New writes in the copy continue after the snapshot's fence.
    copy.execute("CREATE (:Person {name: 'copy'})", rows())
        .unwrap();
    copy.sync().unwrap();
    assert!(copy.wal().unwrap().wal().durable_lsn().raw() > snapshot.lsn);
    drop(copy);
    let reopened = open(&target);
    assert_eq!(reopened.node_count(), 1001);
}

#[test]
fn a_snapshot_keeps_the_database_name_path() {
    let dir = TmpDir::new("nested");
    let live = dir.path.join("live");
    let db = Database::open_named("tenant/app", options(&live)).unwrap();
    db.execute("CREATE (:X)", rows()).unwrap();
    let snapshot = db.snapshot_to_dir(dir.path.join("copy")).unwrap();
    assert_eq!(snapshot.path, dir.path.join("copy/tenant/app.loradb"));
    let copy = Database::open_named("tenant/app", options(&dir.path.join("copy"))).unwrap();
    assert_eq!(copy.node_count(), 1);
}

#[test]
fn a_snapshot_refuses_a_target_that_is_not_empty() {
    let dir = TmpDir::new("not-empty");
    let live = dir.path.join("live");
    let db = open(&live);
    db.execute("CREATE (:X)", rows()).unwrap();

    // Its own directory, which holds the live container.
    let err = db.snapshot_to_dir(&live).unwrap_err();
    assert_eq!(err.code(), LoraErrorCode::Validation);
    assert!(err.to_string().contains("is not empty"), "{err}");

    let target = dir.path.join("copy");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("other"), b"x").unwrap();
    let err = db.snapshot_to_dir(&target).unwrap_err();
    assert_eq!(err.code(), LoraErrorCode::Validation);

    // An empty directory is fine.
    let empty = dir.path.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    db.snapshot_to_dir(&empty).unwrap();
}

#[test]
fn only_a_named_database_takes_a_directory_snapshot() {
    let dir = TmpDir::new("in-memory");
    let db = Database::in_memory();
    let err = db.snapshot_to_dir(dir.path.join("copy")).unwrap_err();
    assert_eq!(err.code(), LoraErrorCode::Config);
    let err = db.checkpoint().unwrap_err();
    assert_eq!(err.code(), LoraErrorCode::Config);
}

#[test]
fn checkpoint_on_a_named_database_is_a_sync_that_reports_it() {
    let dir = TmpDir::new("checkpoint");
    let db = open(&dir.path);
    db.execute("UNWIND list.range(1, 10) AS i CREATE (:X {i: i})", rows())
        .unwrap();
    let meta = db.checkpoint().unwrap();
    assert_eq!(meta.node_count, 10);
    assert_eq!(
        meta.wal_lsn,
        Some(db.wal().unwrap().wal().durable_lsn().raw())
    );
    // The container now starts with a base snapshot frame.
    let container = std::fs::read(dir.path.join("app.loradb")).unwrap();
    assert_eq!(&container[32..40], b"LORAFRM\0");
    assert_eq!(container[40], 2);
}

/// Taken while another thread commits transactions of `PER_TX` nodes as
/// fast as it can, a snapshot holds whole transactions, and exactly those
/// with an LSN up to the one it reports.
#[test]
fn a_snapshot_taken_during_writes_holds_the_transactions_up_to_its_lsn() {
    let dir = TmpDir::new("concurrent");
    let live = dir.path.join("live");
    let db = Arc::new(open(&live));
    let mut feed = db
        .changes(ChangeFeedOptions {
            from_lsn: None,
            buffer_size: 1_000_000,
        })
        .unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (db, stop) = (db.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut tx = 0;
            while !stop.load(Ordering::Relaxed) {
                write_tx(&db, tx);
                tx += 1;
            }
            tx
        })
    };
    let mut snapshots = Vec::new();
    for round in 0..3 {
        std::thread::sleep(Duration::from_millis(50));
        snapshots.push(
            db.snapshot_to_dir(dir.path.join(format!("copy-{round}")))
                .unwrap(),
        );
    }
    stop.store(true, Ordering::Relaxed);
    let written = writer.join().unwrap();
    assert!(written > 0);

    // Nodes created by the batches at or below each LSN.
    let mut created: Vec<(u64, u64)> = Vec::new();
    while let ChangePoll::Batch(batch) = feed.poll().unwrap() {
        let nodes = batch
            .changes
            .iter()
            .filter(|c| matches!(c, Change::NodeCreated { .. }))
            .count() as u64;
        created.push((batch.lsn, nodes));
    }
    assert_eq!(created.len(), written);

    for (round, snapshot) in snapshots.iter().enumerate() {
        let copy = open(&dir.path.join(format!("copy-{round}")));
        let count = copy.node_count() as u64;
        assert_eq!(count, snapshot.node_count);
        assert_eq!(count % PER_TX as u64, 0, "a transaction was cut");
        let committed: u64 = created
            .iter()
            .filter(|(lsn, _)| *lsn <= snapshot.lsn)
            .map(|(_, nodes)| nodes)
            .sum();
        assert_eq!(count, committed, "round {round} at LSN {}", snapshot.lsn);
    }
}

// ---------------------------------------------------------------------------
// kill -9 during a snapshot
// ---------------------------------------------------------------------------

const CHILD_DIR: &str = "LORA_DIR_SNAPSHOT_CHILD_DIR";
const PRELOAD: usize = 50_000;

/// The child side of [`killing_the_owner_mid_snapshot_leaves_both_directories_valid`]:
/// does nothing unless that test started this binary for it.
#[test]
fn snapshot_child_process() {
    let Ok(dir) = std::env::var(CHILD_DIR) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let db = Arc::new(open(&dir.join("live")));
    db.execute(
        &format!(
            "UNWIND list.range(1, {PRELOAD}) AS i CREATE (:Pre {{i: i, s: 'node ' + toString(i)}})"
        ),
        rows(),
    )
    .unwrap();
    {
        let db = db.clone();
        std::thread::spawn(move || {
            let mut tx = 0;
            loop {
                write_tx(&db, tx);
                tx += 1;
            }
        });
    }
    let mut round = 0;
    loop {
        println!("start {round}");
        let snapshot = db
            .snapshot_to_dir(dir.join(format!("copy-{round}")))
            .unwrap();
        println!("done {round} {} {}", snapshot.lsn, snapshot.node_count);
        round += 1;
    }
}

/// `kill -9` of the process that owns a database while it writes a
/// snapshot leaves the live directory and every snapshot that had already
/// returned valid, and the snapshot it was writing without a `.loradb`
/// (or, if it got that far, a complete one).
#[test]
fn killing_the_owner_mid_snapshot_leaves_both_directories_valid() {
    if std::env::var(CHILD_DIR).is_ok() {
        return;
    }
    let dir = TmpDir::new("kill");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "snapshot_child_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_DIR, &dir.path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut done: Vec<(usize, u64, u64)> = Vec::new();
    let mut killed_in = None;
    for line in lines {
        let line = line.unwrap();
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["done", round, lsn, count] => {
                done.push((
                    round.parse().unwrap(),
                    lsn.parse().unwrap(),
                    count.parse().unwrap(),
                ));
            }
            ["start", round] if !done.is_empty() => {
                // Into the next snapshot: a few milliseconds in, it is
                // writing the snapshot file or the container.
                std::thread::sleep(Duration::from_millis(5 + (done.len() as u64 * 7) % 20));
                child.kill().unwrap();
                killed_in = Some(round.parse::<usize>().unwrap());
                break;
            }
            _ => {}
        }
    }
    child.wait().unwrap();
    let killed_in = killed_in.expect("the child finished a snapshot and started another");

    let check = |db: &Database<InMemoryGraph>| {
        let count = db.node_count();
        assert!(count >= PRELOAD);
        assert_eq!((count - PRELOAD) % PER_TX, 0, "a transaction was cut");
        count
    };

    // The live directory opens (the lock went with the process) whole.
    let live = open(&dir.path.join("live"));
    let live_count = check(&live);
    drop(live);

    for (round, _lsn, count) in &done {
        let copy = open(&dir.path.join(format!("copy-{round}")));
        assert_eq!(check(&copy) as u64, *count);
        assert!(*count as usize <= live_count);
    }

    // The interrupted one has no container, only scratch an open of the
    // directory removes, or a complete one.
    let interrupted = dir.path.join(format!("copy-{killed_in}"));
    if interrupted.join("app.loradb").exists() {
        let copy = open(&interrupted);
        check(&copy);
    } else {
        for entry in std::fs::read_dir(&interrupted).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(
                name.starts_with("app.loradb.") && name.ends_with(".tmp"),
                "unexpected `{name}` in an interrupted snapshot"
            );
        }
    }
}
