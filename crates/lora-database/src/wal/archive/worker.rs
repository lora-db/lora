use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::format::write_archive_atomic;
use super::ArchiveState;

const ARCHIVE_FLUSH_DEBOUNCE: Duration = Duration::from_secs(1);

pub(super) fn spawn_archive_worker(
    state: Arc<(Mutex<ArchiveState>, Condvar)>,
    write_lock: Arc<Mutex<()>>,
    work_dir: PathBuf,
    archive_path: PathBuf,
    max_archive_bytes: u64,
) -> JoinHandle<()> {
    thread::spawn(move || loop {
        let should_flush = {
            let (lock, cv) = &*state;
            let mut guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            while !guard.dirty && !guard.shutdown {
                guard = cv.wait(guard).unwrap_or_else(PoisonError::into_inner);
            }
            if guard.shutdown && !guard.dirty {
                return;
            }
            if !guard.force && !guard.shutdown {
                let (next_guard, _) = cv
                    .wait_timeout(guard, ARCHIVE_FLUSH_DEBOUNCE)
                    .unwrap_or_else(PoisonError::into_inner);
                guard = next_guard;
            }
            let should_flush = guard.dirty;
            guard.dirty = false;
            guard.force = false;
            should_flush
        };

        if should_flush {
            let _write_guard = write_lock.lock().unwrap_or_else(PoisonError::into_inner);
            let snapshot = {
                let (lock, _) = &*state;
                lock.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .snapshot
                    .clone()
            };
            if let Err(err) = write_archive_atomic(
                &work_dir,
                &archive_path,
                max_archive_bytes,
                snapshot.as_ref(),
            ) {
                let (lock, _) = &*state;
                let mut guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                guard.failure = Some(err.to_string());
            }
        }
    })
}
