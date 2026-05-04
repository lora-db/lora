//! Regression baseline for `SnapshotCodecError` `Display` output.
//!
//! Pinning each variant ensures wording drift gets flagged in CI
//! before it changes user-visible exception messages.

use lora_snapshot::SnapshotCodecError;

#[test]
fn io() {
    let inner = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
    let err = SnapshotCodecError::Io(inner);
    assert_eq!(err.to_string(), "snapshot I/O error: missing");
}

#[test]
fn encode() {
    let err = SnapshotCodecError::Encode("buffer overflow".into());
    assert_eq!(err.to_string(), "snapshot encode error: buffer overflow");
}

#[test]
fn decode() {
    let err = SnapshotCodecError::Decode("trailing bytes".into());
    assert_eq!(err.to_string(), "snapshot decode error: trailing bytes");
}

#[test]
fn bad_magic() {
    assert_eq!(
        SnapshotCodecError::BadMagic.to_string(),
        "snapshot has bad magic"
    );
}

#[test]
fn unsupported_version() {
    let err = SnapshotCodecError::UnsupportedVersion(99);
    assert_eq!(err.to_string(), "unsupported snapshot format version: 99");
}

#[test]
fn unsupported_compression() {
    let err = SnapshotCodecError::UnsupportedCompression("brotli");
    assert_eq!(
        err.to_string(),
        "snapshot compression `brotli` is not supported by this build"
    );
}

#[test]
fn checksum_mismatch() {
    assert_eq!(
        SnapshotCodecError::ChecksumMismatch.to_string(),
        "snapshot checksum mismatch"
    );
}

#[test]
fn missing_encryption_key() {
    let err = SnapshotCodecError::MissingEncryptionKey("primary".into());
    assert_eq!(
        err.to_string(),
        "snapshot is encrypted with key id `primary`, but no matching key was supplied"
    );
}

#[test]
fn missing_password() {
    let err = SnapshotCodecError::MissingPassword("primary".into());
    assert_eq!(
        err.to_string(),
        "snapshot is password-encrypted with key id `primary`, but no matching password was supplied"
    );
}

#[test]
fn password_kdf() {
    let err = SnapshotCodecError::PasswordKdf("argon2 failed".into());
    assert_eq!(
        err.to_string(),
        "snapshot password key derivation failed: argon2 failed"
    );
}

#[test]
fn encrypt() {
    assert_eq!(
        SnapshotCodecError::Encrypt.to_string(),
        "snapshot encryption failed"
    );
}

#[test]
fn decrypt() {
    assert_eq!(
        SnapshotCodecError::Decrypt.to_string(),
        "snapshot decryption failed"
    );
}
