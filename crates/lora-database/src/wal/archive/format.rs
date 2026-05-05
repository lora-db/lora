use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;

use lora_wal::WalError;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::platform::{replace_file_atomic, sync_dir};
use super::workspace::{make_archive_tmp_path, sorted_wal_files};

const MANIFEST_NAME: &str = "manifest.json";
const MANIFEST_JSON: &str = r#"{"format":"lora.archive","version":1}"#;
const WAL_PREFIX: &str = "wal/";

pub(super) fn write_archive_atomic(
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
            .open(tmp_path)?;
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

pub(super) fn extract_archive(archive_path: &Path, work_dir: &Path) -> Result<(), WalError> {
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
