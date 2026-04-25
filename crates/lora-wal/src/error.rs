use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::lsn::Lsn;

/// Failure modes for WAL operations.
///
/// Errors from the append path are surfaced through [`WalSink::append`] /
/// [`WalSink::commit`] / [`WalSink::flush`] (defined in `wal.rs`, not yet
/// implemented). Errors from replay are surfaced through `WalReplay`. The
/// distinction matters because a recorder error needs to *poison* the
/// engine's mutex critical section, whereas a replay error happens at
/// boot, before queries are accepted.
#[derive(Debug, Error)]
pub enum WalError {
    #[error("WAL I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("WAL record could not be encoded: {0}")]
    Encode(String),

    #[error("WAL record could not be decoded: {0}")]
    Decode(String),

    #[error("WAL record CRC mismatch at lsn {lsn}: expected 0x{expected:08x}, got 0x{actual:08x}")]
    CrcMismatch {
        lsn: Lsn,
        expected: u32,
        actual: u32,
    },

    #[error("WAL record truncated: expected {expected} bytes, got {actual}")]
    Truncated { expected: usize, actual: usize },

    #[error("unknown WAL record kind: {0}")]
    UnknownKind(u8),

    #[error("WAL segment header is malformed: {0}")]
    BadSegmentHeader(&'static str),

    #[error("WAL structure is malformed: {0}")]
    Malformed(String),

    #[error("WAL directory is already open by another live handle: {dir}")]
    AlreadyOpen { dir: PathBuf },

    #[error("WAL is poisoned: a previous append failed and the log is no longer durable")]
    Poisoned,
}
