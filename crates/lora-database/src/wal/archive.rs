mod format;
mod lock;
mod platform;
mod worker;
mod workspace;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use lora_wal::{WalError, WalMirror};

use self::format::{write_archive_atomic, SNAPSHOT_FILE_NAME, STAGED_SNAPSHOT_FILE_NAME};
use self::lock::ArchiveLock;
use self::platform::replace_file_atomic;
use self::platform::sync_dir;
use self::worker::spawn_archive_worker;
use self::workspace::{
    cleanup_stale_temp_paths, make_work_dir, prepare_work_dir, sorted_wal_files,
};

/// Container-backed `.loradb` database file.
///
/// Every persist rewrites a complete Lora container from the current WAL work
/// directory to a temp file, fsyncs it, and atomically renames it over the
/// `.loradb` target. The container uses Lora-owned framing and keeps
/// codec/encryption choices under our control.
///
/// The base snapshot lives beside the segments as `snapshot.lsnap` in the
/// work directory, so it is on disk, not in memory, between persists; each
/// persist streams it into the container.
pub(crate) struct WalArchive {
    archive_path: PathBuf,
    work_dir: PathBuf,
    max_archive_bytes: u64,
    state: Arc<(Mutex<ArchiveState>, Condvar)>,
    write_lock: Arc<Mutex<()>>,
    /// Serializes checkpoints, so a slower one cannot install an older
    /// snapshot over a newer one.
    checkpoint_lock: Mutex<()>,
    worker: Option<JoinHandle<()>>,
    _archive_lock: ArchiveLock,
}

#[derive(Debug, Default)]
pub(super) struct ArchiveState {
    dirty: bool,
    force: bool,
    shutdown: bool,
    failure: Option<String>,
}

fn lock_archive_state(
    state: &Arc<(Mutex<ArchiveState>, Condvar)>,
) -> Result<MutexGuard<'_, ArchiveState>, WalError> {
    let (lock, _) = &**state;
    lock.lock().map_err(|_| WalError::Poisoned)
}

fn archive_writer_failure(failure: &str) -> WalError {
    WalError::Malformed(format!("database container writer failed: {failure}"))
}

impl WalArchive {
    pub fn open(archive_path: PathBuf, max_archive_bytes: u64) -> Result<Self, WalError> {
        if archive_path.is_dir() {
            return Err(WalError::Malformed(format!(
                "database container path is a directory: {}",
                archive_path.display()
            )));
        }
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let archive_lock = ArchiveLock::acquire(&archive_path)?;
        cleanup_stale_temp_paths(&archive_path)?;
        let work_dir = make_work_dir(&archive_path);
        prepare_work_dir(&archive_path, &work_dir, max_archive_bytes)?;

        let state = Arc::new((Mutex::new(ArchiveState::default()), Condvar::new()));
        let write_lock = Arc::new(Mutex::new(()));
        let worker = Some(spawn_archive_worker(
            state.clone(),
            write_lock.clone(),
            work_dir.clone(),
            archive_path.clone(),
            max_archive_bytes,
        ));

        Ok(Self {
            archive_path,
            work_dir,
            max_archive_bytes,
            state,
            write_lock,
            checkpoint_lock: Mutex::new(()),
            worker,
            _archive_lock: archive_lock,
        })
    }

    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// The base snapshot's file, when the database has one.
    pub fn snapshot_path(&self) -> Option<PathBuf> {
        let path = self.work_dir.join(SNAPSHOT_FILE_NAME);
        path.is_file().then_some(path)
    }

    /// Size of the base snapshot's file, 0 without one.
    pub fn snapshot_bytes(&self) -> u64 {
        fs::metadata(self.work_dir.join(SNAPSHOT_FILE_NAME)).map_or(0, |m| m.len())
    }

    /// Total size of the WAL segments in the work directory.
    pub fn wal_bytes(&self) -> Result<u64, WalError> {
        let mut total = 0;
        for path in sorted_wal_files(&self.work_dir)? {
            total += fs::metadata(path)?.len();
        }
        Ok(total)
    }

    /// Where a checkpoint writes its snapshot before
    /// [`Self::install_snapshot`] puts it in place.
    pub fn staged_snapshot_path(&self) -> PathBuf {
        self.work_dir.join(STAGED_SNAPSHOT_FILE_NAME)
    }

    /// Held for the whole of a checkpoint.
    pub fn checkpoint_lock(&self) -> Result<MutexGuard<'_, ()>, WalError> {
        self.checkpoint_lock.lock().map_err(|_| WalError::Poisoned)
    }

    /// Make the durable file at [`Self::staged_snapshot_path`] the base
    /// snapshot, run `drop_covered_wal` to remove the segments it covers,
    /// and rewrite the container with it.
    ///
    /// Crash safety rests on the order: the work directory holds the new
    /// snapshot before any segment it covers goes, so the work directory
    /// recovers by itself at every step, and the old container, replaced
    /// atomically last, stays a whole database until then.
    pub fn install_snapshot(
        &self,
        drop_covered_wal: impl FnOnce() -> Result<(), WalError>,
    ) -> Result<(), WalError> {
        self.check_failure()?;
        let _write_guard = self.write_lock.lock().map_err(|_| WalError::Poisoned)?;
        self.check_failure()?;
        replace_file_atomic(
            &self.staged_snapshot_path(),
            &self.work_dir.join(SNAPSHOT_FILE_NAME),
        )?;
        sync_dir(&self.work_dir)?;
        drop_covered_wal()?;
        self.write_locked()
    }

    fn check_failure(&self) -> Result<(), WalError> {
        match &lock_archive_state(&self.state)?.failure {
            Some(failure) => Err(archive_writer_failure(failure)),
            None => Ok(()),
        }
    }

    /// Rewrite the container. Caller holds `write_lock`.
    fn write_locked(&self) -> Result<(), WalError> {
        let result =
            write_archive_atomic(&self.work_dir, &self.archive_path, self.max_archive_bytes);
        let mut state = lock_archive_state(&self.state)?;
        match result {
            Ok(()) => {
                state.dirty = false;
                state.force = false;
                Ok(())
            }
            Err(err) => {
                state.failure = Some(err.to_string());
                Err(err)
            }
        }
    }
}

impl WalMirror for WalArchive {
    fn persist(&self, wal_dir: &Path) -> Result<(), WalError> {
        if wal_dir != self.work_dir {
            return Err(WalError::Malformed(format!(
                "container mirror received unexpected WAL dir: {}",
                wal_dir.display()
            )));
        }
        let (_, cv) = &*self.state;
        let mut state = lock_archive_state(&self.state)?;
        if let Some(failure) = &state.failure {
            return Err(archive_writer_failure(failure));
        }
        state.dirty = true;
        cv.notify_one();
        Ok(())
    }

    fn persist_force(&self, wal_dir: &Path) -> Result<(), WalError> {
        if wal_dir != self.work_dir {
            return Err(WalError::Malformed(format!(
                "container mirror received unexpected WAL dir: {}",
                wal_dir.display()
            )));
        }
        self.check_failure()?;
        let _write_guard = self.write_lock.lock().map_err(|_| WalError::Poisoned)?;
        self.check_failure()?;
        self.write_locked()
    }
}

impl Drop for WalArchive {
    fn drop(&mut self) {
        {
            let (lock, cv) = &*self.state;
            match lock.lock() {
                Ok(mut state) => {
                    // The async archive worker may not have observed the latest dirty
                    // flag yet. Drop runs after the WAL handle is dropped, so always
                    // take one final archive snapshot from the fully flushed work
                    // directory.
                    state.dirty = true;
                    state.shutdown = true;
                    state.force = true;
                    cv.notify_one();
                }
                Err(mut poisoned) => {
                    let state = poisoned.get_mut();
                    state.failure.get_or_insert_with(|| {
                        "database container state lock was poisoned during shutdown".into()
                    });
                    state.shutdown = true;
                    cv.notify_one();
                }
            }
        }
        let mut shutdown_cleanly = true;
        if let Some(worker) = self.worker.take() {
            shutdown_cleanly = worker.join().is_ok();
        }
        {
            let (lock, _) = &*self.state;
            match lock.lock() {
                Ok(state) => shutdown_cleanly &= state.failure.is_none(),
                Err(_) => shutdown_cleanly = false,
            }
        }
        if shutdown_cleanly {
            let _ = fs::remove_dir_all(&self.work_dir);
            if let Some(parent) = self.work_dir.parent() {
                let _ = sync_dir(parent);
            }
        }
    }
}
