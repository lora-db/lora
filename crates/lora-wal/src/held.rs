//! Directory locks this process holds.
//!
//! The OS lock answers "is the directory taken?" but not "by whom". A
//! handle that is being closed in this process (its last reference was
//! dropped on another thread, and its WAL is still flushing) holds the
//! lock for a moment after nothing can reach it any more, and a re-open
//! in that moment fails as if another process owned the directory. Each
//! lock records its path here while held, and takes it out only after
//! the OS lock is released, so an opener that knows the previous handle
//! is gone can wait for [`wait_until_released`] instead of failing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

struct Held {
    paths: Mutex<BTreeMap<PathBuf, usize>>,
    released: Condvar,
}

fn held() -> &'static Held {
    static HELD: OnceLock<Held> = OnceLock::new();
    HELD.get_or_init(|| Held {
        paths: Mutex::new(BTreeMap::new()),
        released: Condvar::new(),
    })
}

fn paths() -> MutexGuard<'static, BTreeMap<PathBuf, usize>> {
    held().paths.lock().unwrap_or_else(|p| p.into_inner())
}

/// Records `path` as held by this process until dropped. Declare it after
/// the field holding the OS lock, so the lock is released first.
pub struct HeldLock {
    path: PathBuf,
}

impl HeldLock {
    pub fn new(path: &Path) -> Self {
        *paths().entry(path.to_path_buf()).or_insert(0) += 1;
        Self {
            path: path.to_path_buf(),
        }
    }
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        let mut paths = paths();
        if let Some(count) = paths.get_mut(&self.path) {
            *count -= 1;
            if *count == 0 {
                paths.remove(&self.path);
            }
        }
        drop(paths);
        held().released.notify_all();
    }
}

/// Whether this process holds the lock file at `path`.
pub fn is_held(path: &Path) -> bool {
    paths().contains_key(path)
}

/// Wait until this process holds none of the lock files in `paths`, for at
/// most `timeout`. Returns whether they were all released in time; a lock
/// another process holds is not seen here and does not delay the return.
pub fn wait_until_released(lock_paths: &[PathBuf], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut paths = paths();
    loop {
        if !lock_paths.iter().any(|p| paths.contains_key(p)) {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        paths = held()
            .released
            .wait_timeout(paths, deadline - now)
            .unwrap_or_else(|p| p.into_inner())
            .0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_a_lock_released_on_another_thread() {
        let path = PathBuf::from("/held-test/a.lock");
        let lock = HeldLock::new(&path);
        assert!(is_held(&path));
        assert!(!wait_until_released(
            std::slice::from_ref(&path),
            Duration::from_millis(5)
        ));
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            drop(lock);
        });
        assert!(wait_until_released(
            std::slice::from_ref(&path),
            Duration::from_secs(5)
        ));
        assert!(!is_held(&path));
        releaser.join().unwrap();
    }

    #[test]
    fn a_path_stays_held_until_every_holder_drops() {
        let path = PathBuf::from("/held-test/b.lock");
        let first = HeldLock::new(&path);
        let second = HeldLock::new(&path);
        drop(first);
        assert!(is_held(&path));
        drop(second);
        assert!(!is_held(&path));
    }
}
