//! Regression baseline for `WalError`, `WalCommitError`, and
//! `WalBufferedCommitError` `Display` output.
//!
//! These messages flow into `LoraError::message()`. Pinning each
//! variant catches wording drift in CI before it reaches a binding.
//! `WalPoisonError` has a private constructor and is exercised
//! indirectly through `WalBufferedCommitError::Poisoned`, whose
//! wording mirrors it.

use std::path::PathBuf;

use lora_wal::{Lsn, WalBufferedCommitError, WalCommitError, WalError};

#[test]
fn io() {
    let inner = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    let err = WalError::Io(inner);
    assert_eq!(err.to_string(), "WAL I/O error: denied");
}

#[test]
fn encode() {
    let err = WalError::Encode("buffer too small".into());
    assert_eq!(
        err.to_string(),
        "WAL record could not be encoded: buffer too small"
    );
}

#[test]
fn decode() {
    let err = WalError::Decode("trailing bytes".into());
    assert_eq!(
        err.to_string(),
        "WAL record could not be decoded: trailing bytes"
    );
}

#[test]
fn crc_mismatch() {
    let err = WalError::CrcMismatch {
        lsn: Lsn::new(42),
        expected: 0xdead_beef,
        actual: 0xfeed_face,
    };
    assert_eq!(
        err.to_string(),
        "WAL record CRC mismatch at lsn 42: expected 0xdeadbeef, got 0xfeedface"
    );
}

#[test]
fn truncated() {
    let err = WalError::Truncated {
        expected: 16,
        actual: 8,
    };
    assert_eq!(
        err.to_string(),
        "WAL record truncated: expected 16 bytes, got 8"
    );
}

#[test]
fn unknown_kind() {
    let err = WalError::UnknownKind(7);
    assert_eq!(err.to_string(), "unknown WAL record kind: 7");
}

#[test]
fn bad_segment_header() {
    let err = WalError::BadSegmentHeader("magic mismatch");
    assert_eq!(
        err.to_string(),
        "WAL segment header is malformed: magic mismatch"
    );
}

#[test]
fn malformed() {
    let err = WalError::Malformed("dangling commit".into());
    assert_eq!(
        err.to_string(),
        "WAL structure is malformed: dangling commit"
    );
}

#[test]
fn already_open() {
    let err = WalError::AlreadyOpen {
        dir: PathBuf::from("/tmp/wal"),
    };
    assert_eq!(
        err.to_string(),
        "WAL directory is already open by another live handle: /tmp/wal"
    );
}

#[test]
fn poisoned() {
    assert_eq!(
        WalError::Poisoned.to_string(),
        "WAL is poisoned: a previous append failed and the log is no longer durable"
    );
}

#[test]
fn commit_error_commit() {
    let inner = WalError::Io(std::io::Error::new(std::io::ErrorKind::Other, "disk full"));
    let err = WalCommitError::Commit(inner);
    assert_eq!(
        err.to_string(),
        "WAL commit failed: WAL I/O error: disk full"
    );
}

#[test]
fn commit_error_flush() {
    let inner = WalError::Poisoned;
    let err = WalCommitError::Flush(inner);
    assert_eq!(
        err.to_string(),
        "WAL flush failed: WAL is poisoned: a previous append failed and the log is no longer durable"
    );
}

#[test]
fn buffered_commit_arm() {
    let inner = WalError::Io(std::io::Error::new(std::io::ErrorKind::Other, "io"));
    let err = WalBufferedCommitError::Arm(inner);
    assert_eq!(err.to_string(), "WAL arm failed: WAL I/O error: io");
}

#[test]
fn buffered_commit_poisoned() {
    let err = WalBufferedCommitError::Poisoned("prior commit failed".into());
    assert_eq!(err.to_string(), "WAL poisoned: prior commit failed");
}

#[test]
fn buffered_commit_replay_poisoned() {
    let err = WalBufferedCommitError::ReplayPoisoned("torn tail".into());
    assert_eq!(
        err.to_string(),
        "WAL poisoned during commit replay: torn tail"
    );
}

#[test]
fn buffered_commit_commit() {
    let inner = WalCommitError::Commit(WalError::Io(std::io::Error::new(
        std::io::ErrorKind::Other,
        "boom",
    )));
    let err = WalBufferedCommitError::Commit(inner);
    // `#[error(transparent)]` defers to the inner Display.
    assert_eq!(err.to_string(), "WAL commit failed: WAL I/O error: boom");
}
