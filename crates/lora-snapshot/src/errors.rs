use thiserror::Error;

#[derive(Debug, Error)]
pub enum SnapshotCodecError {
    #[error("snapshot I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("snapshot encode error: {0}")]
    Encode(String),
    #[error("snapshot decode error: {0}")]
    Decode(String),
    #[error("snapshot has bad magic")]
    BadMagic,
    #[error("unsupported snapshot format version: {0}")]
    UnsupportedVersion(u32),
    #[error("snapshot compression `{0}` is not supported by this build")]
    UnsupportedCompression(&'static str),
    #[error("snapshot checksum mismatch")]
    ChecksumMismatch,
    #[error("snapshot is encrypted with key id `{0}`, but no matching key was supplied")]
    MissingEncryptionKey(String),
    #[error(
        "snapshot is password encrypted with key id `{0}`, but no matching password was supplied"
    )]
    MissingPassword(String),
    #[error("snapshot password key derivation failed: {0}")]
    PasswordKdf(String),
    #[error("snapshot encryption failed")]
    Encrypt,
    #[error("snapshot decryption failed")]
    Decrypt,
}

pub type Result<T> = std::result::Result<T, SnapshotCodecError>;
