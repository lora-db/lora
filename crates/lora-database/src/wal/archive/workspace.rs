use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use lora_wal::WalError;

use super::format::extract_archive;
use super::platform::sync_dir;

static ARCHIVE_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn make_work_dir(archive_path: &Path) -> PathBuf {
    archive_path.with_extension("loradb.wal")
}

pub(super) fn prepare_work_dir(
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

pub(super) fn make_archive_tmp_path(archive_path: &Path) -> PathBuf {
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

pub(super) fn cleanup_stale_temp_paths(archive_path: &Path) -> Result<(), WalError> {
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
        let is_archive_tmp = is_generated_archive_tmp_name(file_name, &archive_tmp_prefix);
        let is_extract_tmp = is_generated_extract_tmp_name(file_name, &extract_tmp_prefix);
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

fn is_generated_archive_tmp_name(file_name: &str, prefix: &str) -> bool {
    let Some(rest) = file_name.strip_prefix(prefix) else {
        return false;
    };
    let Some(rest) = rest.strip_suffix(".tmp") else {
        return false;
    };
    let mut parts = rest.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(pid), Some(nanos), Some(sequence), None)
            if is_ascii_digits(pid) && is_ascii_digits(nanos) && is_ascii_digits(sequence)
    )
}

fn is_generated_extract_tmp_name(file_name: &str, prefix: &str) -> bool {
    let Some(rest) = file_name.strip_prefix(prefix) else {
        return false;
    };
    let mut parts = rest.split('.');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(pid), Some(nanos), Some(sequence), None)
            if is_ascii_digits(pid) && is_ascii_digits(nanos) && is_ascii_digits(sequence)
    )
}

fn is_ascii_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

pub(super) fn sorted_wal_files(wal_dir: &Path) -> Result<Vec<PathBuf>, WalError> {
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
