use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use lora_wal::{WalError, WalMirror};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const MANIFEST_NAME: &str = "manifest.json";
const MANIFEST_JSON: &str = r#"{"format":"lora.archive","version":1}"#;
const WAL_PREFIX: &str = "wal/";
const ARCHIVE_FLUSH_DEBOUNCE: Duration = Duration::from_secs(1);
static ARCHIVE_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// ZIP-backed `.loradb` database file.
///
/// Every persist rewrites a complete ZIP archive from the current WAL work
/// directory to a temp file, fsyncs it, and atomically renames it over the
/// `.loradb` target. Any ZIP-compatible tool (WinRAR, Explorer, unzip, 7-Zip)
/// can inspect the resulting database file.
pub(crate) struct WalArchive {
    work_dir: PathBuf,
    state: Arc<(Mutex<ArchiveState>, Condvar)>,
    worker: Option<JoinHandle<()>>,
    _archive_lock: ArchiveLock,
}

#[derive(Debug, Default)]
struct ArchiveState {
    dirty: bool,
    force: bool,
    shutdown: bool,
    failure: Option<String>,
}

impl WalArchive {
    pub fn open(archive_path: PathBuf, max_archive_bytes: u64) -> Result<Self, WalError> {
        if archive_path.is_dir() {
            return Err(WalError::Malformed(format!(
                "database archive path is a directory: {}",
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
        let worker = Some(spawn_archive_worker(
            state.clone(),
            work_dir.clone(),
            archive_path.clone(),
            max_archive_bytes,
        ));

        Ok(Self {
            work_dir,
            state,
            worker,
            _archive_lock: archive_lock,
        })
    }

    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }
}

impl WalMirror for WalArchive {
    fn persist(&self, wal_dir: &Path) -> Result<(), WalError> {
        if wal_dir != self.work_dir {
            return Err(WalError::Malformed(format!(
                "archive mirror received unexpected WAL dir: {}",
                wal_dir.display()
            )));
        }
        let (lock, cv) = &*self.state;
        let mut state = lock.lock().unwrap();
        if let Some(failure) = &state.failure {
            return Err(WalError::Malformed(format!(
                "database archive writer failed: {failure}"
            )));
        }
        state.dirty = true;
        cv.notify_one();
        Ok(())
    }
}

impl Drop for WalArchive {
    fn drop(&mut self) {
        {
            let (lock, cv) = &*self.state;
            let mut state = lock.lock().unwrap();
            // The async archive worker may have already consumed the dirty flag
            // before Group-mode WAL bytes were forced out of the in-memory
            // segment buffer. Drop runs after the WAL handle is dropped, so
            // always take one final archive snapshot from the now-flushed work
            // directory.
            state.dirty = true;
            state.shutdown = true;
            state.force = true;
            cv.notify_one();
        }
        let mut shutdown_cleanly = true;
        if let Some(worker) = self.worker.take() {
            shutdown_cleanly = worker.join().is_ok();
        }
        {
            let (lock, _) = &*self.state;
            let state = lock.lock().unwrap();
            shutdown_cleanly &= state.failure.is_none();
        }
        if shutdown_cleanly {
            let _ = fs::remove_dir_all(&self.work_dir);
            if let Some(parent) = self.work_dir.parent() {
                let _ = sync_dir(parent);
            }
        }
    }
}

fn spawn_archive_worker(
    state: Arc<(Mutex<ArchiveState>, Condvar)>,
    work_dir: PathBuf,
    archive_path: PathBuf,
    max_archive_bytes: u64,
) -> JoinHandle<()> {
    thread::spawn(move || loop {
        let should_flush = {
            let (lock, cv) = &*state;
            let mut guard = lock.lock().unwrap();
            while !guard.dirty && !guard.shutdown {
                guard = cv.wait(guard).unwrap();
            }
            if guard.shutdown && !guard.dirty {
                return;
            }
            if !guard.force && !guard.shutdown {
                let (next_guard, _) = cv.wait_timeout(guard, ARCHIVE_FLUSH_DEBOUNCE).unwrap();
                guard = next_guard;
            }
            let should_flush = guard.dirty;
            guard.dirty = false;
            guard.force = false;
            should_flush
        };

        if should_flush {
            if let Err(err) = write_archive_atomic(&work_dir, &archive_path, max_archive_bytes) {
                let (lock, _) = &*state;
                let mut guard = lock.lock().unwrap();
                guard.failure = Some(err.to_string());
            }
        }
    })
}

fn make_work_dir(archive_path: &Path) -> PathBuf {
    archive_path.with_extension("loradb.wal")
}

fn prepare_work_dir(
    archive_path: &Path,
    work_dir: &Path,
    max_archive_bytes: u64,
) -> Result<(), WalError> {
    if has_wal_files(work_dir)? {
        // A durable sidecar means the previous process stopped before the
        // final archive flush/cleanup completed. Trust it over the archive,
        // which may intentionally lag behind the live WAL for throughput.
        return Ok(());
    }

    if work_dir.exists() {
        fs::remove_dir_all(work_dir)?;
    }

    if archive_path.exists() {
        let existing_len = fs::metadata(archive_path)?.len();
        if existing_len > max_archive_bytes {
            return Err(WalError::Malformed(format!(
                "database archive {} is {} bytes, above configured limit {}",
                archive_path.display(),
                existing_len,
                max_archive_bytes
            )));
        }
        extract_archive_into_work_dir(archive_path, work_dir)?;
    } else {
        fs::create_dir_all(work_dir)?;
    }
    Ok(())
}

fn extract_archive_into_work_dir(archive_path: &Path, work_dir: &Path) -> Result<(), WalError> {
    let tmp_dir = make_extract_tmp_path(work_dir);
    let result = (|| {
        fs::create_dir_all(&tmp_dir)?;
        extract_archive(archive_path, &tmp_dir)?;
        sync_dir(&tmp_dir)?;
        fs::rename(&tmp_dir, work_dir)?;
        if let Some(parent) = work_dir.parent() {
            sync_dir(parent)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&tmp_dir);
        let _ = fs::remove_dir_all(work_dir);
    }
    result
}

fn sanitize_for_temp(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.') {
                b as char
            } else {
                '_'
            }
        })
        .collect()
}

fn write_archive_atomic(
    wal_dir: &Path,
    archive_path: &Path,
    max_archive_bytes: u64,
) -> Result<(), WalError> {
    let tmp_path = make_archive_tmp_path(archive_path);
    let result = write_archive_tmp(wal_dir, &tmp_path).and_then(|_| {
        let len = fs::metadata(&tmp_path)?.len();
        if len > max_archive_bytes {
            let _ = fs::remove_file(&tmp_path);
            return Err(WalError::Malformed(format!(
                "database archive {} would be {} bytes, above configured limit {}",
                archive_path.display(),
                len,
                max_archive_bytes
            )));
        }
        replace_file_atomic(&tmp_path, archive_path)?;
        if let Some(parent) = archive_path.parent() {
            sync_dir(parent)?;
        }
        Ok(())
    });
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn write_archive_tmp(wal_dir: &Path, tmp_path: &Path) -> Result<(), WalError> {
    {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        let writer = BufWriter::new(file);
        let mut zip = ZipWriter::new(writer);
        // Fast deflate keeps the ZIP broadly compatible (WinRAR, Explorer,
        // 7-Zip) while reducing the bytes we have to write and fsync on each
        // archive refresh. Level 1 is intentionally biased toward write-heavy
        // workloads rather than maximum compression ratio.
        let options = FileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(1))
            .unix_permissions(0o644);

        zip.start_file(MANIFEST_NAME, options).map_err(zip_error)?;
        zip.write_all(MANIFEST_JSON.as_bytes())?;

        for entry in sorted_wal_files(wal_dir)? {
            let name = entry
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| WalError::Malformed("WAL file name is not UTF-8".into()))?;
            if !is_safe_wal_file_name(name) {
                return Err(WalError::Malformed(format!(
                    "unsafe WAL archive entry name: {name}"
                )));
            }
            zip.start_file(format!("{WAL_PREFIX}{name}"), options)
                .map_err(zip_error)?;
            let mut file = File::open(&entry)?;
            io::copy(&mut file, &mut zip)?;
        }

        let writer = zip.finish().map_err(zip_error)?;
        let file = writer
            .into_inner()
            .map_err(|e| WalError::Io(e.into_error()))?;
        file.sync_all()?;
    }
    Ok(())
}

fn make_archive_tmp_path(archive_path: &Path) -> PathBuf {
    let archive_name = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("database.loradb");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = ARCHIVE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    archive_path.with_file_name(format!(
        "{}.{}.{}.{}.tmp",
        sanitize_for_temp(archive_name),
        std::process::id(),
        nanos,
        sequence
    ))
}

fn make_extract_tmp_path(work_dir: &Path) -> PathBuf {
    let dir_name = work_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("database.loradb.wal");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = ARCHIVE_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    work_dir.with_file_name(format!(
        "{}.extract.{}.{}.{}",
        sanitize_for_temp(dir_name),
        std::process::id(),
        nanos,
        sequence
    ))
}

fn cleanup_stale_temp_paths(archive_path: &Path) -> Result<(), WalError> {
    let parent = archive_path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        return Ok(());
    }
    let archive_name = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("database.loradb");
    let archive_tmp_prefix = format!("{}.", sanitize_for_temp(archive_name));
    let extract_tmp_prefix = format!(
        "{}.extract.",
        sanitize_for_temp(
            make_work_dir(archive_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("database.loradb.wal")
        )
    );

    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let is_archive_tmp =
            file_name.starts_with(&archive_tmp_prefix) && file_name.ends_with(".tmp");
        let is_extract_tmp = file_name.starts_with(&extract_tmp_prefix);
        if !is_archive_tmp && !is_extract_tmp {
            continue;
        }

        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn sorted_wal_files(wal_dir: &Path) -> Result<Vec<PathBuf>, WalError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(wal_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some("wal") {
            entries.push(path);
        }
    }
    entries.sort();
    Ok(entries)
}

fn has_wal_files(wal_dir: &Path) -> Result<bool, WalError> {
    if !wal_dir.exists() {
        return Ok(false);
    }
    Ok(sorted_wal_files(wal_dir)?.into_iter().next().is_some())
}

fn extract_archive(archive_path: &Path, work_dir: &Path) -> Result<(), WalError> {
    let file = File::open(archive_path)?;
    let mut zip = ZipArchive::new(file).map_err(zip_error)?;
    let mut manifest_seen = false;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(zip_error)?;
        let name = entry.name().to_string();
        if name == MANIFEST_NAME {
            if manifest_seen {
                return Err(WalError::Malformed(
                    "database archive has duplicate manifest".into(),
                ));
            }
            let mut manifest = String::new();
            entry.read_to_string(&mut manifest)?;
            if manifest != MANIFEST_JSON {
                return Err(WalError::Malformed(
                    "database archive manifest is not supported".into(),
                ));
            }
            manifest_seen = true;
            continue;
        }
        if name.ends_with('/') {
            continue;
        }
        let Some(wal_name) = name.strip_prefix(WAL_PREFIX) else {
            return Err(WalError::Malformed(format!(
                "unexpected archive entry: {name}"
            )));
        };
        if !is_safe_wal_file_name(wal_name) {
            return Err(WalError::Malformed(format!(
                "unsafe archive entry name: {name}"
            )));
        }
        let path = work_dir.join(wal_name);
        let mut out = OpenOptions::new().write(true).create_new(true).open(path)?;
        io::copy(&mut entry, &mut out)?;
        out.sync_all()?;
    }
    if !manifest_seen {
        return Err(WalError::Malformed(
            "database archive manifest is missing".into(),
        ));
    }
    Ok(())
}

fn is_safe_wal_file_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".wal") else {
        return false;
    };
    !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit())
}

fn zip_error(err: zip::result::ZipError) -> WalError {
    match err {
        zip::result::ZipError::Io(e) => WalError::Io(e),
        other => WalError::Malformed(format!("database archive ZIP error: {other}")),
    }
}

struct ArchiveLock {
    _file: File,
    #[cfg(not(any(unix, windows)))]
    path: PathBuf,
}

impl ArchiveLock {
    fn acquire(archive_path: &Path) -> Result<Self, WalError> {
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

#[cfg(windows)]
fn replace_file_atomic(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let src = wide(src);
    let dst = wide(dst);
    let rc = unsafe {
        MoveFileExW(
            src.as_ptr(),
            dst.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if rc == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file_atomic(src: &Path, dst: &Path) -> io::Result<()> {
    fs::rename(src, dst)
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}
