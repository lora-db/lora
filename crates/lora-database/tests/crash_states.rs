//! What a killed process can leave of a named database on disk, state by
//! state, and what the next open makes of each: the whole graph, or an
//! error. Never a graph that is short of a write the disk had.
//!
//! A checkpoint writes the snapshot to `snapshot.lsnap.tmp`, fsyncs it,
//! renames it over `snapshot.lsnap` in the work directory, drops the WAL
//! segments it covers, and last rewrites the container through a
//! temporary file and a rename. The states here are the ones a kill
//! between or inside those steps leaves (`examples/crash_probe.rs` kills a
//! writing process at random and checks the same from the outside).

use std::path::{Path, PathBuf};

use lora_database::{Database, DatabaseOpenOptions, ExecuteOptions, InMemoryGraph, ResultFormat};

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lora-crash-states-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn options(dir: &Path) -> DatabaseOpenOptions {
    DatabaseOpenOptions {
        // Several segments, so a checkpoint has some to drop.
        segment_target_bytes: 64 * 1024,
        ..DatabaseOpenOptions::default().with_database_dir(dir)
    }
}

fn open(dir: &Path) -> Database<InMemoryGraph> {
    Database::open_named("app", options(dir)).unwrap()
}

fn run(db: &Database<InMemoryGraph>, query: &str) -> Vec<serde_json::Value> {
    let result = db
        .execute(
            query,
            Some(ExecuteOptions {
                format: ResultFormat::Rows,
            }),
        )
        .unwrap();
    serde_json::to_value(&result).unwrap()["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Documents `from..to`, a hundred to a transaction, so the WAL runs over
/// several segments.
fn create(db: &Database<InMemoryGraph>, from: u64, to: u64) {
    let mut start = from;
    while start < to {
        let end = (start + 100).min(to);
        run(
            db,
            &format!(
                "UNWIND range({start}, {}) AS i CREATE (:Doc {{id: i, title: 'doc' + toString(i) + ' text'}})",
                end - 1
            ),
        );
        start = end;
    }
}

/// The graph holds exactly the documents `0..docs`, read by a scan,
/// through the uniqueness constraint's index and through the fulltext
/// index the open rebuilt.
fn assert_docs(db: &Database<InMemoryGraph>, docs: u64) {
    let counts = &run(
        db,
        "MATCH (n:Doc) RETURN count(n) AS n, count(DISTINCT n.id) AS d, max(n.id) AS m",
    )[0];
    assert_eq!(counts["n"].as_u64().unwrap(), docs, "documents");
    assert_eq!(counts["d"].as_u64().unwrap(), docs, "distinct ids");
    assert_eq!(counts["m"].as_u64().unwrap(), docs - 1, "highest id");
    for id in [0, docs / 2, docs - 1] {
        let seek = run(
            db,
            &format!("MATCH (n:Doc {{id: {id}}}) RETURN count(n) AS n"),
        );
        assert_eq!(seek[0]["n"].as_u64().unwrap(), 1, "seek of {id}");
    }
    let text = run(
        db,
        "CALL db.index.fulltext.queryNodes('doc_text', 'text') YIELD node RETURN count(node) AS n",
    );
    assert_eq!(text[0]["n"].as_u64().unwrap(), docs, "fulltext index");
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

const CHECKPOINTED: u64 = 3_000;
const WRITTEN: u64 = 4_000;

/// A database with a schema, `CHECKPOINTED` documents under a checkpoint
/// and the rest up to `WRITTEN` in the WAL only, copied to `<dir>/killed`
/// as a kill at that moment leaves it: the work directory beside a
/// container that lags it. Returns the copy's directory. The source is
/// then closed, so `<dir>/live` is a cleanly closed database of the same
/// content: a container alone.
fn killed_and_closed(dir: &TmpDir) -> (PathBuf, PathBuf) {
    let live = dir.join("live");
    let killed = dir.join("killed");
    let db = open(&live);
    run(
        &db,
        "CREATE CONSTRAINT doc_id FOR (n:Doc) REQUIRE n.id IS UNIQUE",
    );
    run(
        &db,
        "CREATE FULLTEXT INDEX doc_text FOR (n:Doc) ON EACH [n.title]",
    );
    create(&db, 0, CHECKPOINTED);
    db.sync().unwrap();
    create(&db, CHECKPOINTED, WRITTEN);
    db.flush().unwrap();
    copy_dir(&live, &killed);
    drop(db);
    assert!(
        killed
            .join("app.loradb.wal")
            .join("snapshot.lsnap")
            .is_file(),
        "the killed copy has its work directory"
    );
    assert!(
        !live.join("app.loradb.wal").exists(),
        "a clean close leaves the container alone"
    );
    (killed, live)
}

#[test]
fn a_kill_after_a_checkpoint_opens_with_every_flushed_write() {
    let dir = TmpDir::new("flushed");
    let (killed, closed) = killed_and_closed(&dir);
    let db = open(&killed);
    assert_docs(&db, WRITTEN);
    // And it is a database like any other afterwards: written, closed
    // and opened again.
    create(&db, WRITTEN, WRITTEN + 10);
    drop(db);
    assert_docs(&open(&killed), WRITTEN + 10);
    assert_docs(&open(&closed), WRITTEN);
}

/// A kill inside a checkpoint, a container rewrite, the unpacking of a
/// container on open or the removal of the work directory on close leaves
/// a half-written file or directory under a temporary name. None of them
/// is read, and the next open removes them.
#[test]
fn the_leftovers_of_a_killed_checkpoint_or_close_are_ignored_and_removed() {
    let dir = TmpDir::new("leftovers");
    let (killed, closed) = killed_and_closed(&dir);
    for state in [&killed, &closed] {
        let leftovers = [
            state.join("app.loradb.4242.1700000000000000000.7.tmp"),
            state.join("app.loradb.wal.extract.4242.1700000000000000000.8"),
            state.join("app.loradb.wal.trash.4242.1700000000000000000.9"),
        ];
        std::fs::write(&leftovers[0], b"half a container").unwrap();
        for half in &leftovers[1..] {
            std::fs::create_dir_all(half).unwrap();
            std::fs::write(half.join("snapshot.lsnap"), b"half a snapshot").unwrap();
            std::fs::write(half.join("0000000001.wal"), b"half a segment").unwrap();
        }
        let staged = state.join("app.loradb.wal").join("snapshot.lsnap.tmp");
        if state == &killed {
            // A checkpoint killed while it wrote its snapshot.
            std::fs::write(&staged, b"half a snapshot").unwrap();
        }
        let db = open(state);
        assert_docs(&db, WRITTEN);
        for leftover in &leftovers {
            assert!(!leftover.exists(), "{} was removed", leftover.display());
        }
        assert!(!staged.exists(), "the half-written snapshot was removed");
    }
}

/// While the work directory is there it is the database: the container
/// beside it lags by design, and a kill inside its rewrite cannot reach it
/// (it is replaced by a rename). Even a damaged one does not matter.
#[test]
fn a_damaged_container_beside_a_whole_work_directory_does_not_matter() {
    let dir = TmpDir::new("lagging");
    let (killed, _) = killed_and_closed(&dir);
    let container = killed.join("app.loradb");
    let bytes = std::fs::read(&container).unwrap();
    std::fs::write(&container, &bytes[..bytes.len() / 2]).unwrap();
    assert_docs(&open(&killed), WRITTEN);
    // The clean close rewrote it whole.
    assert_docs(&open(&killed), WRITTEN);
}

/// A container cut short (a copy that did not finish, a disk that lost
/// its end) is refused at every length. It never opens as the part of the
/// graph that happened to fit, and the refusal leaves it as it was.
#[test]
fn a_cut_container_is_refused_at_every_length_never_opened_short() {
    let dir = TmpDir::new("cut");
    let (_, closed) = killed_and_closed(&dir);
    let container = closed.join("app.loradb");
    let whole = std::fs::read(&container).unwrap();
    let cuts =
        (1..16)
            .map(|part| whole.len() * part / 16)
            .chain([whole.len() - 1, whole.len() - 20, 1]);
    for cut in cuts {
        std::fs::write(&container, &whole[..cut]).unwrap();
        let opened = Database::open_named("app", options(&closed));
        assert!(
            opened.is_err(),
            "a container cut at {cut} of {} opened",
            whole.len()
        );
        drop(opened);
        assert_eq!(
            std::fs::read(&container).unwrap().len(),
            cut,
            "left as it was"
        );
        assert!(
            !closed.join("app.loradb.wal").exists(),
            "nothing half unpacked is left to be trusted by the next open"
        );
    }
    std::fs::write(&container, &whole).unwrap();
    assert_docs(&open(&closed), WRITTEN);
}

/// The snapshot's checksum covers all of it: one flipped byte or a
/// missing end is an error at open, not a smaller graph, and the files
/// stay for a repair.
#[test]
fn a_damaged_snapshot_is_refused_never_opened_short() {
    let dir = TmpDir::new("snapshot");
    let (killed, _) = killed_and_closed(&dir);
    let snapshot = killed.join("app.loradb.wal").join("snapshot.lsnap");
    let whole = std::fs::read(&snapshot).unwrap();

    let damaged = [
        {
            let mut flipped = whole.clone();
            let at = flipped.len() / 2;
            flipped[at] ^= 0x40;
            flipped
        },
        whole[..whole.len() / 2].to_vec(),
        whole[..whole.len() - 1].to_vec(),
        Vec::new(),
    ];
    for bytes in damaged {
        std::fs::write(&snapshot, &bytes).unwrap();
        let opened = Database::open_named("app", options(&killed));
        assert!(
            opened.is_err(),
            "a damaged snapshot of {} bytes opened",
            bytes.len()
        );
        drop(opened);
        assert_eq!(std::fs::read(&snapshot).unwrap(), bytes, "left as it was");
    }
    // Without the snapshot the WAL starts after what it covered: an
    // error too, not the documents written since the checkpoint alone.
    std::fs::remove_file(&snapshot).unwrap();
    let container = killed.join("app.loradb");
    let kept = std::fs::read(&container).unwrap();
    std::fs::remove_file(&container).unwrap();
    let segments: Vec<String> = std::fs::read_dir(killed.join("app.loradb.wal"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".wal"))
        .collect();
    assert!(
        !segments.contains(&"0000000001.wal".to_string()),
        "the checkpoint dropped the segments it covers: {segments:?}"
    );
    assert!(Database::open_named("app", options(&killed)).is_err());

    std::fs::write(&container, kept).unwrap();
    std::fs::write(&snapshot, &whole).unwrap();
    assert_docs(&open(&killed), WRITTEN);
}
