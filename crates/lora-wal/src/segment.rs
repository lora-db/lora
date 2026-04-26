//! Segment files — the unit of WAL rotation, sealing, and truncation.
//!
//! A segment is a single file on disk holding a fixed header and a
//! sequence of [`WalRecord`]-encoded byte runs. Segments rotate at a
//! configured size threshold (`WalConfig::Enabled.segment_target_bytes`)
//! but only at a `TxBegin` boundary, so a transaction is always wholly
//! contained in one segment.
//!
//! ```text
//! [0..8)    magic         b"LORAWAL\0"
//! [8..12)   format        u32 LE — see SEGMENT_FORMAT_VERSION
//! [12..20)  base_lsn      u64 LE — first LSN allocated in this segment
//! [20..24)  flags         u32 LE — bit 0 = sealed
//! [24..28)  reserved      4 bytes — zeroed; future header fields land here
//! [28..32)  header_crc    u32 LE — IEEE CRC32 over [0..28)
//! [32..)    records       sequence of WalRecord-framed bytes
//! ```
//!
//! Why not put a length / record-count in the header? Because a sealed
//! segment is allowed to have its tail truncated by replay if the last
//! transaction was torn — keeping the length in the header would force
//! us to rewrite the header after every truncation, and the per-record
//! CRC already detects torn tails on its own. The header stores only
//! invariants that apply for the whole life of the segment.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::WalError;
use crate::lsn::Lsn;
use crate::record::WalRecord;

/// Magic bytes at the head of every segment.
pub(crate) const SEGMENT_MAGIC: &[u8; 8] = b"LORAWAL\0";

/// Current segment format version. Bump on any header-layout change.
pub(crate) const SEGMENT_FORMAT_VERSION: u32 = 1;

/// Oldest segment format version this build accepts.
pub(crate) const SEGMENT_MIN_SUPPORTED_FORMAT_VERSION: u32 = 1;

/// Total size of the segment header, including the trailing CRC.
pub(crate) const SEGMENT_HEADER_LEN: usize = 32;

/// Bit set in `flags` when the segment has been sealed (no more appends).
const FLAG_SEALED: u32 = 1 << 0;

/// Decoded segment header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SegmentHeader {
    pub format_version: u32,
    pub base_lsn: Lsn,
    pub sealed: bool,
}

impl SegmentHeader {
    pub fn new(base_lsn: Lsn) -> Self {
        Self {
            format_version: SEGMENT_FORMAT_VERSION,
            base_lsn,
            sealed: false,
        }
    }

    fn encode(&self) -> [u8; SEGMENT_HEADER_LEN] {
        let mut out = [0u8; SEGMENT_HEADER_LEN];
        out[0..8].copy_from_slice(SEGMENT_MAGIC);
        out[8..12].copy_from_slice(&self.format_version.to_le_bytes());
        out[12..20].copy_from_slice(&self.base_lsn.raw().to_le_bytes());
        let flags = if self.sealed { FLAG_SEALED } else { 0 };
        out[20..24].copy_from_slice(&flags.to_le_bytes());
        // [24..28) reserved, stays zero.
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&out[..28]);
        let crc = hasher.finalize();
        out[28..32].copy_from_slice(&crc.to_le_bytes());
        out
    }

    fn decode(bytes: &[u8]) -> Result<Self, WalError> {
        if bytes.len() < SEGMENT_HEADER_LEN {
            return Err(WalError::Truncated {
                expected: SEGMENT_HEADER_LEN,
                actual: bytes.len(),
            });
        }
        if &bytes[0..8] != SEGMENT_MAGIC {
            return Err(WalError::BadSegmentHeader("bad magic"));
        }
        let stored_crc = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&bytes[..28]);
        if hasher.finalize() != stored_crc {
            return Err(WalError::BadSegmentHeader("header crc mismatch"));
        }
        let format_version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if format_version < SEGMENT_MIN_SUPPORTED_FORMAT_VERSION
            || format_version > SEGMENT_FORMAT_VERSION
        {
            return Err(WalError::BadSegmentHeader("unsupported format version"));
        }
        let base_lsn = Lsn::new(u64::from_le_bytes(bytes[12..20].try_into().unwrap()));
        let flags = u32::from_le_bytes(bytes[20..24].try_into().unwrap());
        let sealed = flags & FLAG_SEALED != 0;
        Ok(Self {
            format_version,
            base_lsn,
            sealed,
        })
    }
}

/// Append-side handle to a segment.
///
/// The writer batches encoded record bytes in `pending` and flushes them
/// to the underlying file in a single `write_all` per [`flush`] call. The
/// caller drives the flush cadence:
///
/// - `SyncMode::PerCommit` calls `flush()` (write_all + fsync) at the
///   end of every committed transaction, while still holding the
///   store write lock.
/// - `SyncMode::Group` calls `flush_buffer()` per commit and `fsync()`
///   on a background timer.
/// - `SyncMode::None` calls only `flush_buffer()`; durability is
///   provided by whatever the OS decides to flush.
///
/// Sealing rewrites the header in place (`flags |= SEALED`, recomputed
/// CRC) and `fsync`s. Once sealed, [`append`] returns
/// [`WalError::Poisoned`] — a sealed segment is immutable.
pub(crate) struct SegmentWriter {
    file: File,
    header: SegmentHeader,
    bytes_written: u64,
    pending: Vec<u8>,
}

impl SegmentWriter {
    /// Create a new segment file at `path` with `base_lsn` as its first
    /// LSN. Fails if the file already exists — accidental clobber of an
    /// existing segment is one of the few things that can silently lose
    /// data, so the call is opt-in via the explicit caller-side delete.
    pub fn create(path: PathBuf, base_lsn: Lsn) -> Result<Self, WalError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let header = SegmentHeader::new(base_lsn);
        file.write_all(&header.encode())?;
        // Flush the header to disk immediately so a crash before any
        // record appears at least leaves a recoverable, sealed-empty
        // segment behind.
        file.sync_all()?;
        Ok(Self {
            file,
            header,
            bytes_written: SEGMENT_HEADER_LEN as u64,
            pending: Vec::with_capacity(64 * 1024),
        })
    }

    /// Re-open an existing segment for further appends. Walks the
    /// payload to compute `bytes_written`, returning the position past
    /// the last well-formed record. A torn tail is *not* truncated here
    /// — the caller does that explicitly via [`truncate_to`] once it
    /// has decided what to do.
    ///
    /// The walk uses an independent file descriptor (via
    /// [`SegmentReader::open`]) rather than `try_clone()` because POSIX
    /// `dup` shares the file offset, which would silently desynchronise
    /// the writer's cursor from the reader's.
    pub fn open_for_append(path: PathBuf) -> Result<(Self, Option<TornTail>), WalError> {
        let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut header_bytes = [0u8; SEGMENT_HEADER_LEN];
        file.read_exact(&mut header_bytes)?;
        let header = SegmentHeader::decode(&header_bytes)?;
        if header.sealed {
            return Err(WalError::BadSegmentHeader(
                "segment is sealed; cannot append",
            ));
        }

        // Walk records to find the last good byte offset using a
        // separate read-only handle.
        let mut cursor = SegmentReader::open(&path)?;
        let mut last_good = SEGMENT_HEADER_LEN as u64;
        let torn = loop {
            match cursor.read_record() {
                Ok(Some(_)) => last_good = cursor.position(),
                Ok(None) => break None,
                Err(err) => {
                    break Some(TornTail {
                        last_good_offset: last_good,
                        cause: err,
                    })
                }
            }
        };
        drop(cursor);

        file.seek(SeekFrom::Start(last_good))?;
        Ok((
            Self {
                file,
                header,
                bytes_written: last_good,
                pending: Vec::with_capacity(64 * 1024),
            },
            torn,
        ))
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written + self.pending.len() as u64
    }

    /// Append a single record to the in-memory buffer. The record is
    /// not on disk until [`flush_buffer`] is called.
    pub fn append(&mut self, record: &WalRecord) -> Result<(), WalError> {
        if self.header.sealed {
            return Err(WalError::Poisoned);
        }
        record.encode(&mut self.pending)?;
        Ok(())
    }

    /// Drain the in-memory buffer to the OS file (no `fsync`).
    pub fn flush_buffer(&mut self) -> Result<(), WalError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.file.write_all(&self.pending)?;
        self.bytes_written += self.pending.len() as u64;
        self.pending.clear();
        Ok(())
    }

    /// Drain the in-memory buffer *and* `fsync` the file. The default
    /// durability path under `SyncMode::PerCommit`.
    pub fn flush_and_sync(&mut self) -> Result<(), WalError> {
        self.flush_buffer()?;
        self.file.sync_all()?;
        Ok(())
    }

    /// Mark the segment as sealed (no more appends). Rewrites the
    /// header in place with `FLAG_SEALED` set and `fsync`s.
    pub fn seal(&mut self) -> Result<(), WalError> {
        if self.header.sealed {
            return Ok(());
        }
        self.flush_and_sync()?;
        self.header.sealed = true;
        let header_bytes = self.header.encode();
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header_bytes)?;
        // Restore the write cursor to end-of-segment so any stray
        // append errors loudly instead of corrupting the header region.
        self.file.seek(SeekFrom::Start(self.bytes_written))?;
        self.file.sync_all()?;
        Ok(())
    }

    /// Drop everything past `offset`. Used by recovery to chop a
    /// torn-tail transaction off the active segment.
    pub fn truncate_to(&mut self, offset: u64) -> Result<(), WalError> {
        if offset > self.bytes_written {
            return Err(WalError::BadSegmentHeader(
                "truncation point past end of segment",
            ));
        }
        self.flush_buffer()?;
        self.file.set_len(offset)?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.bytes_written = offset;
        self.file.sync_all()?;
        Ok(())
    }
}

/// Description of where the last well-formed record ended in a segment
/// being reopened, plus the error encountered immediately past that
/// point.
///
/// Today the production open path uses [`crate::replay::ReplayOutcome::torn_tail`]
/// for this — the replay walk already decodes every record on boot
/// and reports the first failure it sees. `SegmentWriter::open_for_append`
/// returns its own copy for the segment-level test that exercises the
/// writer in isolation; consolidating to one source of truth (and
/// dropping the duplicate active-segment walk on every boot) is
/// tracked as a follow-up. See `docs/decisions/0004-wal.md`.
#[derive(Debug)]
#[allow(dead_code)] // see doc comment above
pub(crate) struct TornTail {
    pub last_good_offset: u64,
    pub cause: WalError,
}

/// Read-side handle to a segment.
///
/// Iteration is via repeated [`read_record`] calls — `Ok(None)` on
/// clean EOF, an error on a torn tail or corruption.
pub(crate) struct SegmentReader {
    file: File,
    header: SegmentHeader,
    position: u64,
}

impl SegmentReader {
    pub fn open(path: &Path) -> Result<Self, WalError> {
        let mut file = OpenOptions::new().read(true).open(path)?;
        file.seek(SeekFrom::Start(0))?;
        let mut header_bytes = [0u8; SEGMENT_HEADER_LEN];
        file.read_exact(&mut header_bytes)?;
        let header = SegmentHeader::decode(&header_bytes)?;
        Ok(Self {
            file,
            header,
            position: SEGMENT_HEADER_LEN as u64,
        })
    }

    pub fn header(&self) -> &SegmentHeader {
        &self.header
    }

    /// Byte offset into the file of the next record to read. After a
    /// successful `read_record`, this is the offset at which the
    /// *next* record begins; on a torn tail the caller can truncate
    /// to this offset to chop the bad bytes.
    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn read_record(&mut self) -> Result<Option<WalRecord>, WalError> {
        // We need to know how many bytes were consumed so we can
        // advance `position`. Wrap the file in a counting reader for
        // the duration of this single decode.
        let start = self.position;
        let mut counting = CountingRead {
            inner: &mut self.file,
            consumed: 0,
        };
        match WalRecord::decode(&mut counting) {
            Ok(Some(record)) => {
                self.position = start + counting.consumed;
                Ok(Some(record))
            }
            Ok(None) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

struct CountingRead<'a> {
    inner: &'a mut File,
    consumed: u64,
}

impl<'a> Read for CountingRead<'a> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.consumed += n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lora_store::{MutationEvent, Properties, PropertyValue};

    use crate::testing::TmpDir;

    fn tmpdir(tag: &str) -> TmpDir {
        TmpDir::new(tag)
    }

    fn sample_event(id: u64) -> MutationEvent {
        let mut props = Properties::new();
        props.insert("k".into(), PropertyValue::Int(id as i64));
        MutationEvent::CreateNode {
            id,
            labels: vec!["N".into()],
            properties: props,
        }
    }

    fn mutation(lsn: u64, tx: u64) -> WalRecord {
        WalRecord::Mutation {
            lsn: Lsn::new(lsn),
            tx_begin_lsn: Lsn::new(tx),
            event: sample_event(lsn),
        }
    }

    #[test]
    fn header_round_trip() {
        let h = SegmentHeader::new(Lsn::new(123));
        let bytes = h.encode();
        let decoded = SegmentHeader::decode(&bytes).unwrap();
        assert_eq!(decoded, h);
        assert!(!decoded.sealed);
    }

    #[test]
    fn header_crc_catches_corruption() {
        let h = SegmentHeader::new(Lsn::new(1));
        let mut bytes = h.encode();
        bytes[12] ^= 0xff;
        let err = SegmentHeader::decode(&bytes).unwrap_err();
        assert!(matches!(err, WalError::BadSegmentHeader(_)));
    }

    #[test]
    fn bad_magic_rejected() {
        let h = SegmentHeader::new(Lsn::new(1));
        let mut bytes = h.encode();
        bytes[0] = b'X';
        let err = SegmentHeader::decode(&bytes).unwrap_err();
        assert!(matches!(err, WalError::BadSegmentHeader("bad magic")));
    }

    #[test]
    fn create_append_flush_read_back() {
        let dir = tmpdir("seg");
        let path = dir.path().join("000000000001.wal");
        let mut writer = SegmentWriter::create(path.clone(), Lsn::new(1)).unwrap();

        writer
            .append(&WalRecord::TxBegin { lsn: Lsn::new(1) })
            .unwrap();
        writer.append(&mutation(2, 1)).unwrap();
        writer
            .append(&WalRecord::TxCommit {
                lsn: Lsn::new(3),
                tx_begin_lsn: Lsn::new(1),
            })
            .unwrap();
        writer.flush_and_sync().unwrap();

        let mut reader = SegmentReader::open(&path).unwrap();
        assert_eq!(reader.header().base_lsn, Lsn::new(1));
        let r1 = reader.read_record().unwrap().unwrap();
        assert!(matches!(r1, WalRecord::TxBegin { .. }));
        let r2 = reader.read_record().unwrap().unwrap();
        assert!(matches!(r2, WalRecord::Mutation { .. }));
        let r3 = reader.read_record().unwrap().unwrap();
        assert!(matches!(r3, WalRecord::TxCommit { .. }));
        assert!(reader.read_record().unwrap().is_none());
    }

    #[test]
    fn seal_round_trips_in_header() {
        let dir = tmpdir("seg");
        let path = dir.path().join("000000000001.wal");
        let mut writer = SegmentWriter::create(path.clone(), Lsn::new(1)).unwrap();
        writer
            .append(&WalRecord::TxBegin { lsn: Lsn::new(1) })
            .unwrap();
        writer.flush_and_sync().unwrap();
        writer.seal().unwrap();
        drop(writer);

        let reader = SegmentReader::open(&path).unwrap();
        assert!(reader.header().sealed);
    }

    #[test]
    fn append_after_seal_is_rejected() {
        let dir = tmpdir("seg");
        let path = dir.path().join("000000000001.wal");
        let mut writer = SegmentWriter::create(path, Lsn::new(1)).unwrap();
        writer.seal().unwrap();
        let err = writer
            .append(&WalRecord::TxBegin { lsn: Lsn::new(1) })
            .unwrap_err();
        assert!(matches!(err, WalError::Poisoned));
    }

    #[test]
    fn reopen_unsealed_segment_finds_torn_tail() {
        let dir = tmpdir("seg");
        let path = dir.path().join("000000000001.wal");
        let mut writer = SegmentWriter::create(path.clone(), Lsn::new(1)).unwrap();
        writer
            .append(&WalRecord::TxBegin { lsn: Lsn::new(1) })
            .unwrap();
        writer.append(&mutation(2, 1)).unwrap();
        writer
            .append(&WalRecord::TxCommit {
                lsn: Lsn::new(3),
                tx_begin_lsn: Lsn::new(1),
            })
            .unwrap();
        writer.flush_and_sync().unwrap();
        let good_size = writer.bytes_written();
        drop(writer);

        // Simulate a torn tail: append some garbage bytes by hand.
        {
            use std::io::Write as _;
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&[0xde, 0xad, 0xbe, 0xef, 0x00, 0x00]).unwrap();
            f.sync_all().unwrap();
        }

        let (mut writer, torn) = SegmentWriter::open_for_append(path.clone()).unwrap();
        let torn = torn.expect("torn tail should be reported");
        assert_eq!(torn.last_good_offset, good_size);
        // After truncation, subsequent appends pick up at the right
        // boundary and the file reads back cleanly.
        writer.truncate_to(torn.last_good_offset).unwrap();
        writer
            .append(&WalRecord::TxAbort {
                lsn: Lsn::new(4),
                tx_begin_lsn: Lsn::new(1),
            })
            .unwrap();
        writer.flush_and_sync().unwrap();
        drop(writer);

        let mut reader = SegmentReader::open(&path).unwrap();
        let mut count = 0;
        while reader.read_record().unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 4); // 3 originals + 1 abort
    }

    #[test]
    fn create_refuses_to_clobber_existing_file() {
        let dir = tmpdir("seg");
        let path = dir.path().join("000000000001.wal");
        let _ = SegmentWriter::create(path.clone(), Lsn::new(1)).unwrap();
        match SegmentWriter::create(path, Lsn::new(1)) {
            Err(WalError::Io(_)) => {}
            Ok(_) => panic!("second create unexpectedly succeeded"),
            Err(other) => panic!("expected Io error from second create, got {other:?}"),
        }
    }
}
