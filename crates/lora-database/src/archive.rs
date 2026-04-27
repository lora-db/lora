use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
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

        let work_dir = make_work_dir(&archive_path);
        fs::create_dir_all(&work_dir)?;
        if archive_path.exists() {
            let existing_len = fs::metadata(&archive_path)?.len();
            if existing_len > max_archive_bytes {
                return Err(WalError::Malformed(format!(
                    "database archive {} is {} bytes, above configured limit {}",
                    archive_path.display(),
                    existing_len,
                    max_archive_bytes
                )));
            }
            extract_archive(&archive_path, &work_dir)?;
        }

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
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = fs::remove_dir_all(&self.work_dir);
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
    let mut dir = std::env::temp_dir();
    let stem = archive_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("database");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!(
        "lora-archive-{}-{}-{nanos}",
        std::process::id(),
        sanitize_for_temp(stem)
    ));
    dir
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
    let tmp_path = archive_path.with_extension("loradb.tmp");
    {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
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
    fs::rename(&tmp_path, archive_path)?;
    if let Some(parent) = archive_path.parent() {
        sync_dir(parent)?;
    }
    Ok(())
}

fn sorted_wal_files(wal_dir: &Path) -> Result<Vec<PathBuf>, WalError> {
    let mut entries: Vec<_> = fs::read_dir(wal_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("wal"))
        .collect();
    entries.sort();
    Ok(entries)
}

fn extract_archive(archive_path: &Path, work_dir: &Path) -> Result<(), WalError> {
    let file = File::open(archive_path)?;
    let mut zip = ZipArchive::new(file).map_err(zip_error)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(zip_error)?;
        let name = entry.name().to_string();
        if name == MANIFEST_NAME || name.ends_with('/') {
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
        let mut out = File::create(path)?;
        io::copy(&mut entry, &mut out)?;
        out.sync_all()?;
    }
    Ok(())
}

fn is_safe_wal_file_name(name: &str) -> bool {
    name.ends_with(".wal")
        && !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b'w' || b == b'a' || b == b'l')
}

fn zip_error(err: zip::result::ZipError) -> WalError {
    match err {
        zip::result::ZipError::Io(e) => WalError::Io(e),
        other => WalError::Malformed(format!("database archive ZIP error: {other}")),
    }
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}
