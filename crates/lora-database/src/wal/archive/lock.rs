use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
#[cfg(not(any(unix, windows)))]
use std::path::PathBuf;

use lora_wal::WalError;

pub(super) struct ArchiveLock {
    _file: File,
    #[cfg(not(any(unix, windows)))]
    path: PathBuf,
}

impl ArchiveLock {
    pub(super) fn acquire(archive_path: &Path) -> Result<Self, WalError> {
        let lock_path = archive_path.with_extension("loradb.lock");

        #[cfg(unix)]
        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)?;
            lock_exclusive_nonblocking(&file).map_err(|err| {
                if err.kind() == io::ErrorKind::WouldBlock {
                    WalError::AlreadyOpen {
                        dir: archive_path.to_path_buf(),
                    }
                } else {
                    WalError::Io(err)
                }
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
            lock_exclusive_nonblocking(&file).map_err(|err| {
                if is_windows_lock_conflict(&err) {
                    WalError::AlreadyOpen {
                        dir: archive_path.to_path_buf(),
                    }
                } else {
                    WalError::Io(err)
                }
            })?;
            Ok(Self { _file: file })
        }

        #[cfg(not(any(unix, windows)))]
        {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(file) => Ok(Self {
                    _file: file,
                    path: lock_path,
                }),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    Err(WalError::AlreadyOpen {
                        dir: archive_path.to_path_buf(),
                    })
                }
                Err(err) => Err(WalError::Io(err)),
            }
        }
    }
}

#[cfg(windows)]
impl Drop for ArchiveLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self._file);
    }
}

#[cfg(not(any(unix, windows)))]
impl Drop for ArchiveLock {
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
