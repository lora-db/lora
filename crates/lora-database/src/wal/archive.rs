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

use self::format::{write_archive_atomic, ContainerSnapshot};
use self::lock::ArchiveLock;
use self::platform::sync_dir;
use self::worker::spawn_archive_worker;
use self::workspace::{cleanup_stale_temp_paths, make_work_dir, prepare_work_dir};

/// Container-backed `.loradb` database file.
///
/// Every persist rewrites a complete Lora container from the current WAL work
/// directory to a temp file, fsyncs it, and atomically renames it over the
/// `.loradb` target. The container uses Lora-owned framing and keeps
/// codec/encryption choices under our control.
pub(crate) struct WalArchive {
    archive_path: PathBuf,
    work_dir: PathBuf,
    max_archive_bytes: u64,
    state: Arc<(Mutex<ArchiveState>, Condvar)>,
    write_lock: Arc<Mutex<()>>,
    worker: Option<JoinHandle<()>>,
    _archive_lock: ArchiveLock,
}

#[derive(Debug, Default)]
pub(super) struct ArchiveState {
    dirty: bool,
    force: bool,
    shutdown: bool,
    failure: Option<String>,
    snapshot: Option<ContainerSnapshot>,
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
        let snapshot = prepare_work_dir(&archive_path, &work_dir, max_archive_bytes)?;

        let state = Arc::new((Mutex::new(ArchiveState::default()), Condvar::new()));
        {
            lock_archive_state(&state)?.snapshot = snapshot;
        }
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
            worker,
            _archive_lock: archive_lock,
        })
    }

    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    pub fn snapshot_bytes(&self) -> Result<Option<Vec<u8>>, WalError> {
        Ok(lock_archive_state(&self.state)?
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.bytes.clone()))
    }

    pub fn persist_snapshot_bytes(&self, bytes: Vec<u8>) -> Result<(), WalError> {
        {
            let mut state = lock_archive_state(&self.state)?;
            if let Some(failure) = &state.failure {
                return Err(archive_writer_failure(failure));
            }
            state.snapshot = Some(ContainerSnapshot { bytes });
            state.dirty = true;
            state.force = true;
        }
        self.persist_force(&self.work_dir)
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
        {
            let state = lock_archive_state(&self.state)?;
            if let Some(failure) = &state.failure {
                return Err(archive_writer_failure(failure));
            }
        }

        let _write_guard = self.write_lock.lock().map_err(|_| WalError::Poisoned)?;
        {
            let state = lock_archive_state(&self.state)?;
            if let Some(failure) = &state.failure {
                return Err(archive_writer_failure(failure));
            }
        }
        let snapshot = lock_archive_state(&self.state)?.snapshot.clone();
        let result = write_archive_atomic(
            &self.work_dir,
            &self.archive_path,
            self.max_archive_bytes,
            snapshot.as_ref(),
        );
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
