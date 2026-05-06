//! Internal test helpers shared across the crate's `#[cfg(test)]`
//! modules. Three sibling modules used to roll their own near-identical
//! `TmpDir`; this is the single source of truth.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-local counter so parallel test threads can't collide on the
/// same scratch path. The wall-clock nanos we used previously are not
/// monotonic across threads — two tests entering `TmpDir::new` in the
/// same nanosecond produced identical paths and clobbered each other.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Per-test scratch directory under `std::env::temp_dir()`.
/// `tag` shows up in the directory name to make debugging stuck tests
/// easier when a test panics with the dir still on disk.
pub(crate) struct TmpDir {
    pub path: PathBuf,
}

impl TmpDir {
    pub fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lora-wal-test-{}-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
