//! Opening a database whose directory cannot be written (taste N-09): the
//! error names the cause instead of "storage is temporarily unavailable".

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use lora_database::{Database, DatabaseOpenOptions, LoraErrorCode};

struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lora-ro-{tag}-{}-{}",
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

impl Drop for Dir {
    fn drop(&mut self) {
        set_mode(&self.0, 0o755);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `mode` on `path` and everything under it.
fn set_mode(path: &Path, mode: u32) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
            set_mode(&entry.path(), mode);
        }
    }
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

/// Root ignores permissions: the test then has nothing to show.
fn permissions_apply(dir: &Path) -> bool {
    std::fs::write(dir.join("probe"), b"").is_err()
}

fn options(dir: &Path) -> DatabaseOpenOptions {
    DatabaseOpenOptions::default().with_database_dir(dir)
}

#[test]
fn an_unwritable_directory_is_named_as_the_cause() {
    let dir = Dir::new("existing");
    {
        let db = Database::open_named("app", options(&dir.0)).unwrap();
        db.execute("CREATE (:N {k: 1})", None).unwrap();
    }
    set_mode(&dir.0, 0o555);
    if !permissions_apply(&dir.0) {
        return;
    }
    let Err(err) = Database::open_named("app", options(&dir.0)) else {
        panic!("opened an unwritable directory");
    };
    assert_eq!(
        err.code(),
        LoraErrorCode::StoragePermission,
        "{}",
        err.debug_context()
    );
    let public = err.public_message();
    assert!(
        public.contains("permission") && public.contains("writable"),
        "{public}"
    );
}

#[test]
fn a_new_database_under_an_unwritable_directory_is_named_as_the_cause() {
    let dir = Dir::new("fresh");
    set_mode(&dir.0, 0o555);
    if !permissions_apply(&dir.0) {
        return;
    }
    let Err(err) = Database::open_named("app", options(&dir.0)) else {
        panic!("opened an unwritable directory");
    };
    assert_eq!(
        err.code(),
        LoraErrorCode::StoragePermission,
        "{}",
        err.debug_context()
    );
}
