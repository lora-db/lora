//! WAL directory exclusion.
//!
//! The log format has a single active segment and no inter-process
//! coordination protocol. A best-effort advisory lock gives every caller
//! the same simple rule: one live `Wal::open` per directory.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::errors::WalError;

const LOCK_FILE_NAME: &str = ".lora-wal.lock";

/// Brief window for swallowing transient `EWOULDBLOCK` between a clean
/// drop and a subsequent re-open of the same directory. Under load on
/// Linux we have observed the close+flock-release of the previous
/// owner racing with a new `flock()` attempt; a few short retries
/// absorb that without weakening the invariant — a real concurrent
/// owner still surfaces as `AlreadyOpen` after the window elapses.
const ACQUIRE_RETRY_BUDGET: Duration = Duration::from_millis(100);
const ACQUIRE_RETRY_INITIAL_BACKOFF: Duration = Duration::from_micros(100);

/// RAII guard for a WAL directory lock.
pub(crate) struct DirLock {
    _file: File,
    #[cfg(not(any(unix, windows)))]
    path: std::path::PathBuf,
}

impl DirLock {
    pub(crate) fn acquire(dir: &Path) -> Result<Self, WalError> {
        let lock_path = dir.join(LOCK_FILE_NAME);

        #[cfg(unix)]
        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)?;
            retry_on_contention(dir, || {
                lock_exclusive_nonblocking(&file).map_err(classify_unix_lock_err)
            })?;
            Ok(Self { _file: file })
        }

        #[cfg(windows)]
        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)?;
            retry_on_contention(dir, || {
                lock_exclusive_nonblocking(&file).map_err(classify_windows_lock_err)
            })?;
            Ok(Self { _file: file })
        }

        #[cfg(not(any(unix, windows)))]
        {
            // Fallback for targets where this crate does not yet carry a
            // platform advisory-lock implementation. This still prevents two
            // LoraDB handles from opening the same WAL concurrently, although
            // a process crash can leave a stale lock file that operators must
            // remove manually.
            let file = retry_on_contention(dir, || {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&lock_path)
                    .map_err(|err| {
                        if err.kind() == io::ErrorKind::AlreadyExists {
                            LockAttemptError::Contended
                        } else {
                            LockAttemptError::Io(err)
                        }
                    })
            })?;
            Ok(Self {
                _file: file,
                path: lock_path,
            })
        }
    }
}

/// Result of a single platform-specific lock attempt, classified so the
/// retry helper can distinguish "another live owner" (try again, briefly)
/// from real I/O errors (fail immediately).
enum LockAttemptError {
    Contended,
    Io(io::Error),
}

#[cfg(unix)]
fn classify_unix_lock_err(err: io::Error) -> LockAttemptError {
    if err.kind() == io::ErrorKind::WouldBlock {
        LockAttemptError::Contended
    } else {
        LockAttemptError::Io(err)
    }
}

#[cfg(windows)]
fn classify_windows_lock_err(err: io::Error) -> LockAttemptError {
    if is_windows_lock_conflict(&err) {
        LockAttemptError::Contended
    } else {
        LockAttemptError::Io(err)
    }
}

/// Run `attempt` repeatedly until it succeeds, returns a non-contention
/// I/O error, or the retry budget is exhausted. Contention is the
/// expected signal "another live owner" — under load on Linux we have
/// observed the close+flock-release of a previous owner racing with a
/// new `flock()` attempt; a short retry window absorbs that without
/// weakening the invariant. A real concurrent owner persists past the
/// window and surfaces as `AlreadyOpen` to the caller.
fn retry_on_contention<T, F>(dir: &Path, mut attempt: F) -> Result<T, WalError>
where
    F: FnMut() -> Result<T, LockAttemptError>,
{
    let start = Instant::now();
    let mut backoff = ACQUIRE_RETRY_INITIAL_BACKOFF;
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(LockAttemptError::Io(err)) => return Err(WalError::Io(err)),
            Err(LockAttemptError::Contended) => {
                if start.elapsed() >= ACQUIRE_RETRY_BUDGET {
                    return Err(WalError::AlreadyOpen {
                        dir: dir.to_path_buf(),
                    });
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_millis(5));
            }
        }
    }
}

#[cfg(windows)]
impl Drop for DirLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self._file);
    }
}

#[cfg(not(any(unix, windows)))]
impl Drop for DirLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn lock_exclusive_nonblocking(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::raw::c_int;

    const LOCK_EX: c_int = 2;
    const LOCK_NB: c_int = 4;

    unsafe extern "C" {
        fn flock(fd: c_int, operation: c_int) -> c_int;
    }

    loop {
        let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
        if rc == 0 {
            return Ok(());
        }

        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

#[cfg(windows)]
fn lock_exclusive_nonblocking(file: &File) -> io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x1;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x2;

    unsafe extern "system" {
        fn LockFileEx(
            hFile: *mut c_void,
            dwFlags: u32,
            dwReserved: u32,
            nNumberOfBytesToLockLow: u32,
            nNumberOfBytesToLockHigh: u32,
            lpOverlapped: *mut WindowsOverlapped,
        ) -> i32;
    }

    let mut overlapped = WindowsOverlapped::zeroed();
    let rc = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if rc == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn unlock_file(file: &File) -> io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    unsafe extern "system" {
        fn UnlockFileEx(
            hFile: *mut c_void,
            dwReserved: u32,
            nNumberOfBytesToUnlockLow: u32,
            nNumberOfBytesToUnlockHigh: u32,
            lpOverlapped: *mut WindowsOverlapped,
        ) -> i32;
    }

    let mut overlapped = WindowsOverlapped::zeroed();
    let rc = unsafe { UnlockFileEx(file.as_raw_handle(), 0, u32::MAX, u32::MAX, &mut overlapped) };
    if rc == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn is_windows_lock_conflict(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock || matches!(err.raw_os_error(), Some(32 | 33))
}

#[cfg(windows)]
#[repr(C)]
struct WindowsOverlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    h_event: *mut std::ffi::c_void,
}

#[cfg(windows)]
impl WindowsOverlapped {
    fn zeroed() -> Self {
        Self {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            h_event: std::ptr::null_mut(),
        }
    }
}
