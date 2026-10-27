//! An open of a named database that this process is still closing waits
//! for it instead of failing with `LORA_LOCKED` (Festimap G-71, the
//! `close()` and same-process reopen of the Node binding build on this).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lora_database::{
    wait_for_release, Database, DatabaseName, DatabaseOpenOptions, ExecuteOptions, LoraErrorCode,
    ResultFormat,
};

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lora-named-release-{tag}-{}-{}",
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

fn options(dir: &TmpDir) -> DatabaseOpenOptions {
    DatabaseOpenOptions {
        database_dir: dir.0.clone(),
        ..Default::default()
    }
}

#[test]
fn wait_for_release_follows_the_last_reference() {
    let dir = TmpDir::new("follows");
    let options = options(&dir);
    let path = options.database_path_for(&DatabaseName::parse("app").unwrap());
    // Nothing held yet.
    assert!(wait_for_release(&path, Duration::ZERO));

    let db = Arc::new(Database::open_named("app", options.clone()).unwrap());
    db.execute("UNWIND range(1, 1000) AS i CREATE (:N {i: i})", None)
        .unwrap();
    assert!(!wait_for_release(&path, Duration::from_millis(10)));
    // A second open while it is live is refused, as before.
    let err = Database::open_named("app", options.clone()).err().unwrap();
    assert_eq!(err.code(), LoraErrorCode::Locked);

    // The last reference goes on another thread, after a moment.
    let last = db.clone();
    drop(db);
    let closer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        drop(last);
    });
    assert!(wait_for_release(&path, Duration::from_secs(30)));
    let reopened = Database::open_named("app", options).unwrap();
    let result = reopened
        .execute(
            "MATCH (n:N) RETURN count(n) AS c",
            Some(ExecuteOptions {
                format: ResultFormat::Rows,
            }),
        )
        .unwrap();
    assert_eq!(serde_json::to_value(&result).unwrap()["rows"][0]["c"], 1000);
    closer.join().unwrap();
}
