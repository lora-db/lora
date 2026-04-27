//! Segment-directory helpers.
//!
//! `Wal::open`, `replay_dir`, and `Wal::truncate_up_to` all need to:
//!
//! - turn a [`SegmentId`] into the canonical `<NNNNNNNNNN>.wal` path,
//! - parse a path back into a [`SegmentId`],
//! - list every well-formed segment file in a directory in ascending
//!   id order,
//! - read just the `base_lsn` of a segment without paying for a full
//!   record walk.
//!
//! The same operations were inlined in two places before the refactor.
//! Pulling them behind a single `SegmentDir` and a `SegmentId` newtype
//! removes the duplication and makes the magic number "10 zero-padded
//! digits" live in exactly one location.
//!
//! `SegmentDir` does not hold an open `DirHandle`. Every call hits the
//! filesystem fresh — segment listings happen at open time and at
//! truncate time, neither of which is in any hot path, so caching is
//! not worth the invalidation work.

use std::fmt;
use std::fs;
#[cfg(unix)]
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::error::WalError;
use crate::lsn::Lsn;
use crate::segment::SegmentReader;

/// Width of the zero-padded segment id in file names. 10 digits is
/// enough for ~10 billion segments, which at the default 8 MiB target
/// is ~80 EiB of log. Plenty.
const SEGMENT_ID_WIDTH: usize = 10;

/// Monotonic identifier for a WAL segment file.
///
/// Allocation policy: ids start at 1 (`SegmentId(0)` is reserved as a
/// "no segment" sentinel that callers should never encounter for a
/// live WAL), and rotation simply does `id + 1`. Ids are stable: a
/// truncated segment retains its id even after every preceding segment
/// has been deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SegmentId(u64);

impl SegmentId {
    pub const FIRST: SegmentId = SegmentId(1);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// Predecessor id, saturating at zero. Used for the "active and
    /// the segment immediately preceding it" tombstone-retention rule
    /// in [`crate::wal::Wal::truncate_up_to`].
    pub fn saturating_prev(self) -> Self {
        Self(self.0.saturating_sub(1))
    }
}

impl fmt::Display for SegmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A `(SegmentId, PathBuf)` pair. Returned by [`SegmentDir::list`] so
/// callers can iterate once and not have to parse the id back out of
/// the path themselves.
#[derive(Debug, Clone)]
pub struct SegmentEntry {
    pub id: SegmentId,
    pub path: PathBuf,
}

/// Owns the canonical naming scheme for a WAL directory and the
/// operations that depend on it. Cheap to construct (`Clone` is a
/// `PathBuf` clone) — the type is just a typed wrapper over a
/// directory path.
#[derive(Debug, Clone)]
pub struct SegmentDir {
    root: PathBuf,
}

impl SegmentDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Best-effort portability boundary for directory-entry durability.
    /// Unix targets can fsync directories directly; other targets keep
    /// the existing file-level guarantees until a platform-specific
    /// directory sync implementation is added.
    #[cfg(unix)]
    pub fn sync_dir(&self) -> Result<(), WalError> {
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn sync_dir(&self) -> Result<(), WalError> {
        Ok(())
    }

    /// Canonical path for the segment with id `id`.
    pub fn path_for(&self, id: SegmentId) -> PathBuf {
        self.root
            .join(format!("{:0width$}.wal", id.0, width = SEGMENT_ID_WIDTH))
    }

    /// Parse a `<NNNNNNNNNN>.wal` path back into a [`SegmentId`].
    /// Returns `None` if the file name does not match the canonical
    /// pattern (e.g. a leftover `.tmp` or a non-numeric stem).
    pub fn id_of(path: &Path) -> Option<SegmentId> {
        path.extension()
            .and_then(|s| s.to_str())
            .filter(|ext| *ext == "wal")?;
        path.file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u64>().ok())
            .map(SegmentId)
    }

    /// List every `*.wal` file in the directory in ascending id order.
    /// Files whose names do not match the canonical pattern are ignored
    /// so a stray `.tmp` does not block boot. Directory entry I/O errors
    /// still abort the listing rather than risking an incomplete replay.
    pub fn list(&self) -> Result<Vec<SegmentEntry>, WalError> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            if let Some(id) = Self::id_of(&path) {
                out.push(SegmentEntry { id, path });
            }
        }
        out.sort_by_key(|e| e.id);
        Ok(out)
    }

    /// `base_lsn` recorded in `segment`'s header. Used by
    /// `truncate_up_to` to compute the LSN range each sealed segment
    /// covers without re-walking its records.
    pub fn base_lsn(segment: &Path) -> Result<Lsn, WalError> {
        // `SegmentReader::open` already validates the magic, format,
        // and header CRC — no point re-implementing the layout here
        // just to skip a few bytes.
        let reader = SegmentReader::open(segment)?;
        Ok(reader.header().base_lsn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_id_path_round_trip() {
        let dir = SegmentDir::new("/tmp");
        let id = SegmentId::new(42);
        let path = dir.path_for(id);
        assert_eq!(path.to_str().unwrap(), "/tmp/0000000042.wal");
        assert_eq!(SegmentDir::id_of(&path), Some(id));
    }

    #[test]
    fn id_of_rejects_non_wal_files() {
        assert_eq!(SegmentDir::id_of(Path::new("/tmp/0000000001.txt")), None);
        assert_eq!(SegmentDir::id_of(Path::new("/tmp/notanumber.wal")), None);
        assert_eq!(SegmentDir::id_of(Path::new("/tmp/CURRENT")), None);
    }

    #[test]
    fn saturating_prev_does_not_underflow() {
        assert_eq!(SegmentId::new(0).saturating_prev(), SegmentId::new(0));
        assert_eq!(SegmentId::new(1).saturating_prev(), SegmentId::new(0));
        assert_eq!(SegmentId::new(7).saturating_prev(), SegmentId::new(6));
    }
}
