//! Committed-change feed (`Database::changes`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lora_database::{
    Change, ChangeBatch, ChangeFeed, ChangeFeedOptions, ChangePoll, Database, LoraErrorCode,
    LoraValue, SnapshotConfig, TransactionMode,
};
use lora_store::PropertyValue;
use lora_wal::{SyncMode, WalConfig};

struct TmpDir {
    path: PathBuf,
}

impl TmpDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lora-db-changes-{}-{}-{}",
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

fn wal_config(dir: &Path, segment_target_bytes: u64) -> WalConfig {
    WalConfig::Enabled {
        dir: dir.to_path_buf(),
        sync_mode: SyncMode::GroupSync { interval_ms: 1000 },
        segment_target_bytes,
    }
}

fn open(db: &Database<lora_database::InMemoryGraph>) -> ChangeFeed {
    db.changes(ChangeFeedOptions::default()).unwrap()
}

fn open_from(db: &Database<lora_database::InMemoryGraph>, from: u64) -> ChangeFeed {
    db.changes(ChangeFeedOptions {
        from_lsn: Some(from),
        ..ChangeFeedOptions::default()
    })
    .unwrap()
}

fn next(feed: &mut ChangeFeed) -> Arc<ChangeBatch> {
    match feed.next_timeout(Duration::from_secs(5)).unwrap() {
        ChangePoll::Batch(batch) => batch,
        other => panic!("expected a batch, got {other:?}"),
    }
}

fn assert_empty(feed: &mut ChangeFeed) {
    match feed.poll().unwrap() {
        ChangePoll::Pending => {}
        other => panic!("expected no batch, got {other:?}"),
    }
}

fn exec(db: &Database<lora_database::InMemoryGraph>, query: &str) {
    db.execute(query, None).unwrap();
}

fn prop(value: &str) -> PropertyValue {
    PropertyValue::String(value.into())
}

#[test]
fn reports_node_and_relationship_lifecycle() {
    let db = Database::in_memory();
    let mut feed = open(&db);

    exec(&db, "CREATE (:User {name: 'ada', age: 36})");
    let b1 = next(&mut feed);
    assert_eq!(b1.changes.len(), 1);
    let Change::NodeCreated {
        id: ada,
        labels,
        properties,
    } = &b1.changes[0]
    else {
        panic!("{:?}", b1.changes);
    };
    assert_eq!(labels, &vec!["User".to_string()]);
    assert_eq!(properties.get("name"), Some(&prop("ada")));
    let ada = *ada;

    exec(
        &db,
        "MATCH (u:User {name: 'ada'}) SET u.age = 37, u:Admin REMOVE u.name",
    );
    let b2 = next(&mut feed);
    assert!(b2.lsn > b1.lsn);
    match &b2.changes[..] {
        [Change::NodeUpdated {
            id,
            labels,
            properties,
            set_keys,
            removed_keys,
            added_labels,
            removed_labels,
        }] => {
            assert_eq!(*id, ada);
            assert!(labels.contains(&"Admin".to_string()));
            assert_eq!(properties.get("age"), Some(&PropertyValue::Int(37)));
            assert!(properties.get("name").is_none());
            assert_eq!(set_keys, &vec!["age".to_string()]);
            assert_eq!(removed_keys, &vec!["name".to_string()]);
            assert_eq!(added_labels, &vec!["Admin".to_string()]);
            assert!(removed_labels.is_empty());
        }
        other => panic!("{other:?}"),
    }

    exec(
        &db,
        "MATCH (u:User) CREATE (u)-[:WROTE {at: 1}]->(:Post {title: 'hi'})",
    );
    let b3 = next(&mut feed);
    let rel = b3
        .changes
        .iter()
        .find_map(|c| match c {
            Change::RelationshipCreated {
                id,
                rel_type,
                start,
                end,
                properties,
            } => {
                assert_eq!(rel_type, "WROTE");
                assert_eq!(*start, ada);
                assert_ne!(*end, ada);
                assert_eq!(properties.get("at"), Some(&PropertyValue::Int(1)));
                Some(*id)
            }
            _ => None,
        })
        .expect("relationship created");
    assert!(b3.changes.iter().any(
        |c| matches!(c, Change::NodeCreated { labels, .. } if labels == &vec!["Post".to_string()])
    ));

    exec(&db, "MATCH ()-[r:WROTE]->() SET r.at = 2");
    let b4 = next(&mut feed);
    match &b4.changes[..] {
        [Change::RelationshipUpdated {
            id,
            set_keys,
            properties,
            ..
        }] => {
            assert_eq!(*id, rel);
            assert_eq!(set_keys, &vec!["at".to_string()]);
            assert_eq!(properties.get("at"), Some(&PropertyValue::Int(2)));
        }
        other => panic!("{other:?}"),
    }

    exec(&db, "MATCH (u:User) DETACH DELETE u");
    let b5 = next(&mut feed);
    assert!(b5.changes.iter().any(|c| matches!(
        c,
        Change::RelationshipDeleted { id, rel_type, properties, .. }
            if *id == rel && rel_type == "WROTE" && properties.get("at") == Some(&PropertyValue::Int(2))
    )));
    assert!(b5.changes.iter().any(|c| matches!(
        c,
        Change::NodeDeleted { id, labels, properties }
            if *id == ada
                && labels.contains(&"User".to_string())
                && properties.get("age") == Some(&PropertyValue::Int(37))
    )));

    let lsns = [b1.lsn, b2.lsn, b3.lsn, b4.lsn, b5.lsn];
    assert!(lsns.windows(2).all(|w| w[0] < w[1]), "{lsns:?}");
    assert_empty(&mut feed);
}

#[test]
fn plain_delete_carries_last_known_properties() {
    // `MATCH ... DELETE` without DETACH takes the in-place fast path, which
    // collects deleted records as the store drops them.
    let dir = TmpDir::new("plain-delete");
    let dbs = [
        Database::in_memory(),
        Database::open_with_wal(wal_config(dir.path(), 8 * 1024 * 1024)).unwrap(),
    ];
    for db in &dbs {
        exec(db, "CREATE (:Tag {slug: 'rust'})");
        let mut feed = open(db);
        exec(db, "MATCH (t:Tag) DELETE t");
        let batch = next(&mut feed);
        match &batch.changes[..] {
            [Change::NodeDeleted {
                labels, properties, ..
            }] => {
                assert_eq!(labels, &vec!["Tag".to_string()]);
                assert_eq!(properties.get("slug"), Some(&prop("rust")));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(db.node_count(), 0);
    }
}

#[test]
fn transactions_commit_one_batch_and_rollbacks_produce_nothing() {
    let db = Database::in_memory();
    let mut feed = open(&db);

    let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
    tx.execute("CREATE (:A {k: 1})", None).unwrap();
    tx.execute("CREATE (:B {k: 2})", None).unwrap();
    tx.execute("CREATE (t:Temp) DELETE t", None).unwrap();
    tx.commit().unwrap();
    let batch = next(&mut feed);
    assert_eq!(batch.changes.len(), 2, "{:?}", batch.changes);

    let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
    tx.execute("CREATE (:C)", None).unwrap();
    tx.rollback().unwrap();

    {
        let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
        tx.execute("CREATE (:D)", None).unwrap();
        // dropped without commit
    }

    // A query that fails midway is rolled back too.
    assert!(db
        .execute("CREATE (a:E)-[:R]->(:E) WITH a DELETE a", None)
        .is_err());

    exec(&db, "CREATE (:F)");
    let batch = next(&mut feed);
    assert!(matches!(
        &batch.changes[..],
        [Change::NodeCreated { labels, .. }] if labels == &vec!["F".to_string()]
    ));
    assert_empty(&mut feed);
}

#[test]
fn every_write_path_is_captured() {
    let db = Database::in_memory();
    let mut feed = open(&db);

    // Parameterised execute (imports use this path).
    let mut params = BTreeMap::new();
    params.insert(
        "rows".to_string(),
        LoraValue::List(vec![LoraValue::Int(1), LoraValue::Int(2)]),
    );
    db.execute_with_params("UNWIND $rows AS r CREATE (:Row {r: r})", None, params)
        .unwrap();
    assert_eq!(next(&mut feed).changes.len(), 2);

    // Streamed write.
    let mut stream = db.stream("CREATE (n:Streamed) RETURN n").unwrap();
    while stream.next_row().unwrap().is_some() {}
    drop(stream);
    assert!(matches!(
        &next(&mut feed).changes[..],
        [Change::NodeCreated { labels, .. }] if labels == &vec!["Streamed".to_string()]
    ));

    // MERGE (staged path) and a timeout-bounded write.
    exec(&db, "MERGE (:Merged {k: 1})");
    assert_eq!(next(&mut feed).changes.len(), 1);
    db.execute_with_timeout("CREATE (:Bounded)", None, Duration::from_secs(5))
        .unwrap();
    assert_eq!(next(&mut feed).changes.len(), 1);

    // Schema commands change no data and produce no batch.
    exec(&db, "CREATE INDEX row_r FOR (n:Row) ON (n.r)");
    assert_empty(&mut feed);

    // clear() and snapshot restore reset the graph.
    let snapshot = db.save_snapshot_to_bytes().unwrap();
    db.clear();
    assert_eq!(next(&mut feed).changes, vec![Change::Reset]);
    db.load_snapshot_from_bytes(&snapshot).unwrap();
    assert_eq!(next(&mut feed).changes, vec![Change::Reset]);
}

#[test]
fn resumes_inside_the_in_memory_window_and_rejects_older_lsns() {
    let db = Database::in_memory();
    db.set_change_retention(3);
    let mut feed = open(&db);
    let mut lsns = Vec::new();
    for i in 0..5 {
        exec(&db, &format!("CREATE (:N {{i: {i}}})"));
        lsns.push(next(&mut feed).lsn);
    }

    let mut resumed = open_from(&db, lsns[2]);
    assert_eq!(next(&mut resumed).lsn, lsns[3]);
    assert_eq!(next(&mut resumed).lsn, lsns[4]);
    exec(&db, "CREATE (:N {i: 5})");
    assert!(next(&mut resumed).lsn > lsns[4]);

    let err = db
        .changes(ChangeFeedOptions {
            from_lsn: Some(lsns[0]),
            ..ChangeFeedOptions::default()
        })
        .err()
        .unwrap();
    assert_eq!(err.code(), LoraErrorCode::ChangesTruncated);

    let err = db
        .changes(ChangeFeedOptions {
            from_lsn: Some(10_000),
            ..ChangeFeedOptions::default()
        })
        .err()
        .unwrap();
    assert_eq!(err.code(), LoraErrorCode::ChangesTruncated);
}

#[test]
fn slow_consumer_lags_without_blocking_writers() {
    let db = Database::in_memory();
    let mut feed = db
        .changes(ChangeFeedOptions {
            from_lsn: None,
            buffer_size: 2,
        })
        .unwrap();
    for i in 0..10 {
        exec(&db, &format!("CREATE (:N {{i: {i}}})"));
    }
    let first = next(&mut feed);
    let second = next(&mut feed);
    let err = feed.poll().unwrap_err();
    assert_eq!(err.code(), LoraErrorCode::ChangesLagged);
    assert!(matches!(feed.poll().unwrap(), ChangePoll::Closed));
    assert!(second.lsn > first.lsn);

    // Resume from the last processed LSN.
    let mut resumed = open_from(&db, second.lsn);
    let mut count = 0;
    while let ChangePoll::Batch(_) = resumed.poll().unwrap() {
        count += 1;
    }
    assert_eq!(count, 8);
}

#[test]
fn closing_and_dropping_the_database_end_feeds() {
    let db = Database::in_memory();
    let mut a = open(&db);
    let b = open(&db);
    b.closer().close();
    exec(&db, "CREATE (:N)");
    drop(b);
    next(&mut a);
    drop(db);
    assert!(matches!(
        a.next_timeout(Duration::from_secs(1)).unwrap(),
        ChangePoll::Closed
    ));
}

#[test]
fn wal_lsns_resume_across_restart() {
    let dir = TmpDir::new("restart");
    let (before_restart, kept_id) = {
        let db = Database::open_with_wal(wal_config(dir.path(), 8 * 1024 * 1024)).unwrap();
        let mut feed = open(&db);
        exec(&db, "CREATE (:Doc {slug: 'a'}), (:Doc {slug: 'b'})");
        let first = next(&mut feed);
        exec(&db, "MATCH (d:Doc {slug: 'a'}) SET d.title = 'A'");
        next(&mut feed);
        let kept = match &first.changes[1] {
            Change::NodeCreated { id, .. } => *id,
            other => panic!("{other:?}"),
        };
        (first.lsn, kept)
    };

    let db = Database::open_with_wal(wal_config(dir.path(), 8 * 1024 * 1024)).unwrap();
    // A write after restart, before the feed opens.
    exec(&db, "MATCH (d:Doc {slug: 'b'}) DETACH DELETE d");

    let mut feed = open_from(&db, before_restart);
    let update = next(&mut feed);
    assert!(update.lsn > before_restart);
    assert!(matches!(
        &update.changes[..],
        [Change::NodeUpdated { labels, properties, set_keys, .. }]
            if labels == &vec!["Doc".to_string()]
                && properties.get("title") == Some(&prop("A"))
                && set_keys == &vec!["title".to_string()]
    ));
    let delete = next(&mut feed);
    assert!(delete.lsn > update.lsn);
    assert!(matches!(
        &delete.changes[..],
        [Change::NodeDeleted { id, properties, .. }]
            if *id == kept_id && properties.get("slug") == Some(&prop("b"))
    ));
    assert_empty(&mut feed);

    // Live batches follow the history seamlessly.
    exec(&db, "CREATE (:Doc {slug: 'c'})");
    assert!(next(&mut feed).lsn > delete.lsn);

    // Resuming from 0 replays everything the WAL still holds.
    let mut all = open_from(&db, 0);
    let mut count = 0;
    while let ChangePoll::Batch(_) = all.poll().unwrap() {
        count += 1;
    }
    assert_eq!(count, 4);
}

#[test]
fn wal_history_uses_managed_snapshots_and_reports_truncation() {
    let wal = TmpDir::new("trunc-wal");
    let snaps = TmpDir::new("trunc-snap");
    let open_db = || {
        Database::open_with_wal_snapshots(
            wal_config(wal.path(), 256),
            SnapshotConfig::enabled(snaps.path()),
        )
        .unwrap()
    };

    let first_lsn = {
        let db = open_db();
        let mut feed = open(&db);
        exec(&db, "CREATE (:Early {n: 0})");
        let first = next(&mut feed).lsn;
        for i in 1..20 {
            exec(&db, &format!("CREATE (:Early {{n: {i}}})"));
        }
        db.checkpoint_managed().unwrap();
        first
    };

    let db = open_db();
    // Keep nothing in memory so every resume goes through the WAL.
    db.set_change_retention(0);
    let head_before = {
        // Open a feed to learn the head, then write past the checkpoint.
        let feed = open(&db);
        drop(feed);
        db.changes_head().unwrap()
    };
    exec(&db, "MATCH (e:Early {n: 3}) SET e.late = true");

    // The checkpoint truncated the early WAL segments.
    let err = db
        .changes(ChangeFeedOptions {
            from_lsn: Some(first_lsn),
            ..ChangeFeedOptions::default()
        })
        .err()
        .unwrap();
    assert_eq!(err.code(), LoraErrorCode::ChangesTruncated);

    // Resuming after the checkpoint rebuilds from the snapshot.
    let mut feed = open_from(&db, head_before);
    let batch = next(&mut feed);
    assert!(matches!(
        &batch.changes[..],
        [Change::NodeUpdated { labels, properties, .. }]
            if labels == &vec!["Early".to_string()] && properties.get("n") == Some(&PropertyValue::Int(3))
    ));
}

#[test]
fn concurrent_writers_arrive_in_commit_order_without_gaps() {
    let db = Arc::new(Database::in_memory());
    let mut feed = db
        .changes(ChangeFeedOptions {
            from_lsn: None,
            buffer_size: 10_000,
        })
        .unwrap();
    let writers: Vec<_> = (0..4)
        .map(|w| {
            let db = db.clone();
            std::thread::spawn(move || {
                for i in 0..50 {
                    db.execute(&format!("CREATE (:W {{w: {w}, i: {i}}})"), None)
                        .unwrap();
                }
            })
        })
        .collect();
    for handle in writers {
        handle.join().unwrap();
    }
    let mut lsns = Vec::new();
    while let ChangePoll::Batch(batch) = feed.poll().unwrap() {
        lsns.push(batch.lsn);
    }
    assert_eq!(lsns.len(), 200);
    assert!(
        lsns.windows(2).all(|w| w[1] == w[0] + 1),
        "in-memory LSNs are dense"
    );
}
