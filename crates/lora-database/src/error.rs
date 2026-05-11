//! Top-level error type and stable error-code catalog for LoraDB.
//!
//! Internal `lora-database` code still produces `anyhow::Result` because
//! `?`-chaining over many lower-layer error types is convenient. The
//! public boundary, however, surfaces a typed [`LoraError`] so transports
//! and bindings can route on the stable [`LoraErrorCode`] wire string
//! without parsing message text.
//!
//! # Stable contract
//!
//! - [`LoraErrorCode::as_str`] returns the wire string. These strings are
//!   part of the public API and never change between releases.
//! - [`LoraError::message`] returns a user-friendly clause. Wording **may**
//!   change between minor versions to improve clarity — bindings and
//!   integration tests must not match on it.
//! - [`LoraError::category`] returns whether the failure was the caller's
//!   fault (`Client`) or the engine's (`Server`). The HTTP layer uses this
//!   to pick the response status; bindings can use it to tag exceptions.
//!
//! See `docs/design/error-style.md` for the message style guide.

use std::error::Error;
use std::fmt;

use lora_analyzer::SemanticError;
use lora_executor::ExecutorError;
use lora_parser::ParseError;
use lora_snapshot::SnapshotCodecError;
use lora_store::SnapshotError;
use lora_wal::{WalBufferedCommitError, WalCommitError, WalError};

use crate::transaction::TransactionError;
use crate::DatabaseNameError;

/// Stable error-code catalog. The wire string returned by [`Self::as_str`]
/// is part of LoraDB's public API. Consumers — bindings, the HTTP layer,
/// integration tests — should match on this rather than on the message
/// text, which is allowed to change between releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoraErrorCode {
    // -------- Client errors --------
    /// Cypher syntax could not be parsed.
    Parse,
    /// Cypher analysis (unknown variable, label, function, type mismatch, …).
    Semantic,
    /// A parameter value passed by the caller could not be coerced.
    InvalidParams,
    /// A mutating statement was issued in a read-only context.
    ReadOnlyViolation,
    /// A named entity (database, label, key) does not exist.
    NotFound,
    /// A precondition (e.g. delete-with-relationships) is not satisfied.
    ConstraintViolation,
    /// A vector value failed dimension / coordinate-type validation.
    InvalidVector,
    /// A query exceeded its cooperative deadline.
    Timeout,
    /// A logical database name violates the portable-path rules.
    DatabaseName,
    /// Required parameters are missing or malformed (CLI / config flags).
    Config,

    // -------- Server errors --------
    /// I/O failure outside the WAL / snapshot boundaries.
    Io,
    /// WAL record was truncated, mis-CRC'd, or otherwise unreadable.
    WalCorruption,
    /// The WAL is poisoned and no longer accepts durable writes.
    WalPoisoned,
    /// Snapshot codec failure (bad magic, version, checksum, …).
    SnapshotCodec,
    /// Snapshot encryption / decryption / KDF failure.
    SnapshotCrypto,
    /// Last-resort fallback when the engine cannot classify the failure.
    Internal,
}

/// Whether a [`LoraErrorCode`] represents a caller-visible mistake or an
/// engine-side failure. Used by the HTTP transport to choose between
/// 4xx and 5xx status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoraErrorCategory {
    Client,
    Server,
}

impl LoraErrorCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
        }
    }
}

impl LoraErrorCode {
    /// Stable wire string. Part of the public API — never changes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parse => "LORA_PARSE",
            Self::Semantic => "LORA_SEMANTIC",
            Self::InvalidParams => "LORA_INVALID_PARAMS",
            Self::ReadOnlyViolation => "LORA_READ_ONLY",
            Self::NotFound => "LORA_NOT_FOUND",
            Self::ConstraintViolation => "LORA_CONSTRAINT",
            Self::InvalidVector => "LORA_INVALID_VECTOR",
            Self::Timeout => "LORA_TIMEOUT",
            Self::DatabaseName => "LORA_DATABASE_NAME",
            Self::Config => "LORA_CONFIG",
            Self::Io => "LORA_IO",
            Self::WalCorruption => "LORA_WAL_CORRUPTION",
            Self::WalPoisoned => "LORA_WAL_POISONED",
            Self::SnapshotCodec => "LORA_SNAPSHOT_CODEC",
            Self::SnapshotCrypto => "LORA_SNAPSHOT_CRYPTO",
            Self::Internal => "LORA_INTERNAL",
        }
    }

    /// Whether this code is the caller's fault or the engine's.
    pub fn category(self) -> LoraErrorCategory {
        match self {
            Self::Parse
            | Self::Semantic
            | Self::InvalidParams
            | Self::ReadOnlyViolation
            | Self::NotFound
            | Self::ConstraintViolation
            | Self::InvalidVector
            | Self::Timeout
            | Self::DatabaseName
            | Self::Config => LoraErrorCategory::Client,
            Self::Io
            | Self::WalCorruption
            | Self::WalPoisoned
            | Self::SnapshotCodec
            | Self::SnapshotCrypto
            | Self::Internal => LoraErrorCategory::Server,
        }
    }
}

impl fmt::Display for LoraErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Public error type at the `lora-database` boundary.
///
/// Construct via [`Self::from_anyhow`] (the typical path — the engine
/// uses `anyhow::Error` internally) or via the `From` impls for any
/// known concrete error type.
pub struct LoraError {
    code: LoraErrorCode,
    message: String,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl LoraError {
    pub fn new(code: LoraErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(
        code: LoraErrorCode,
        message: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }

    pub fn code(&self) -> LoraErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn category(&self) -> LoraErrorCategory {
        self.code.category()
    }

    /// Convert an `anyhow::Error` from the engine's internal `?`-chains
    /// into a typed `LoraError`. Best-effort: downcasts the chain to any
    /// known concrete error type and picks the matching code; falls back
    /// to [`LoraErrorCode::Internal`] with the original message preserved.
    pub fn from_anyhow(err: anyhow::Error) -> Self {
        Self::from_anyhow_ref(&err)
    }

    /// Borrowed version of [`Self::from_anyhow`]. Useful in binding
    /// layers that hold `&anyhow::Error` from a `Result::Err` capture
    /// and don't want to move the error.
    pub fn from_anyhow_ref(err: &anyhow::Error) -> Self {
        // If the chain already carries a typed `LoraError` (because some
        // intermediate layer wrapped one with `?` or `.into()`), preserve
        // its code rather than re-classifying as `Internal`.
        if let Some(e) = err.downcast_ref::<LoraError>() {
            return Self::new(e.code, e.message.clone());
        }
        if let Some(e) = err.downcast_ref::<ParseError>() {
            return Self::new(LoraErrorCode::Parse, e.to_string());
        }
        if let Some(e) = err.downcast_ref::<SemanticError>() {
            return Self::new(LoraErrorCode::Semantic, e.to_string());
        }
        if let Some(e) = err.downcast_ref::<ExecutorError>() {
            return Self::new(executor_code(e), e.to_string());
        }
        if let Some(e) = err.downcast_ref::<WalError>() {
            return Self::new(wal_code(e), e.to_string());
        }
        if let Some(e) = err.downcast_ref::<WalCommitError>() {
            return Self::new(wal_commit_code(e), e.to_string());
        }
        if let Some(e) = err.downcast_ref::<WalBufferedCommitError>() {
            return Self::new(wal_buffered_commit_code(e), e.to_string());
        }
        if let Some(e) = err.downcast_ref::<SnapshotCodecError>() {
            return Self::new(snapshot_codec_code(e), e.to_string());
        }
        if let Some(e) = err.downcast_ref::<SnapshotError>() {
            return Self::new(snapshot_store_code(e), e.to_string());
        }
        if let Some(e) = err.downcast_ref::<DatabaseNameError>() {
            return Self::new(LoraErrorCode::DatabaseName, e.to_string());
        }
        if let Some(e) = err.downcast_ref::<TransactionError>() {
            return Self::new(transaction_code(e), e.to_string());
        }
        if let Some(e) = err.downcast_ref::<std::io::Error>() {
            return Self::new(LoraErrorCode::Io, e.to_string());
        }
        // Fallback: an external `anyhow::Error` we don't recognise. Internal
        // sites all surface typed errors that the downcasts above route
        // precisely, so anything that lands here is from a third-party crate
        // or a legacy `anyhow!("...")` we have not yet converted.
        Self::new(LoraErrorCode::Internal, format!("{err:#}"))
    }
}

fn executor_code(err: &ExecutorError) -> LoraErrorCode {
    match err {
        ExecutorError::ReadOnlyCreate { .. }
        | ExecutorError::ReadOnlyMerge { .. }
        | ExecutorError::ReadOnlyDelete { .. }
        | ExecutorError::ReadOnlySet { .. }
        | ExecutorError::ReadOnlyRemove { .. } => LoraErrorCode::ReadOnlyViolation,
        ExecutorError::QueryTimeout => LoraErrorCode::Timeout,
        ExecutorError::DeleteNodeWithRelationships { .. } => LoraErrorCode::ConstraintViolation,
        _ => LoraErrorCode::Internal,
    }
}

fn wal_code(err: &WalError) -> LoraErrorCode {
    match err {
        WalError::Io(_) | WalError::AlreadyOpen { .. } => LoraErrorCode::Io,
        WalError::CrcMismatch { .. }
        | WalError::Truncated { .. }
        | WalError::UnknownKind(_)
        | WalError::BadSegmentHeader(_)
        | WalError::Malformed(_)
        | WalError::Encode(_)
        | WalError::Decode(_) => LoraErrorCode::WalCorruption,
        WalError::Poisoned => LoraErrorCode::WalPoisoned,
    }
}

fn wal_commit_code(err: &WalCommitError) -> LoraErrorCode {
    match err {
        WalCommitError::Commit(inner) | WalCommitError::Flush(inner) => wal_code(inner),
    }
}

fn wal_buffered_commit_code(err: &WalBufferedCommitError) -> LoraErrorCode {
    match err {
        WalBufferedCommitError::Arm(inner) => wal_code(inner),
        WalBufferedCommitError::Poisoned(_) | WalBufferedCommitError::ReplayPoisoned(_) => {
            LoraErrorCode::WalPoisoned
        }
        WalBufferedCommitError::Commit(inner) => wal_commit_code(inner),
    }
}

fn snapshot_codec_code(err: &SnapshotCodecError) -> LoraErrorCode {
    match err {
        SnapshotCodecError::Io(_) => LoraErrorCode::Io,
        SnapshotCodecError::MissingEncryptionKey(_)
        | SnapshotCodecError::MissingPassword(_)
        | SnapshotCodecError::PasswordKdf(_)
        | SnapshotCodecError::Encrypt
        | SnapshotCodecError::Decrypt => LoraErrorCode::SnapshotCrypto,
        SnapshotCodecError::BadMagic
        | SnapshotCodecError::UnsupportedVersion(_)
        | SnapshotCodecError::UnsupportedCompression(_)
        | SnapshotCodecError::ChecksumMismatch
        | SnapshotCodecError::Encode(_)
        | SnapshotCodecError::Decode(_) => LoraErrorCode::SnapshotCodec,
    }
}

fn snapshot_store_code(err: &SnapshotError) -> LoraErrorCode {
    match err {
        SnapshotError::Io(_) => LoraErrorCode::Io,
        SnapshotError::Decode(_) | SnapshotError::Encode(_) => LoraErrorCode::SnapshotCodec,
    }
}

fn transaction_code(err: &TransactionError) -> LoraErrorCode {
    match err {
        TransactionError::ReadOnlyMutation
        | TransactionError::ReadOnlyCommit
        | TransactionError::StreamingRequiresReadWrite => LoraErrorCode::ReadOnlyViolation,
        TransactionError::AlreadyClosed
        | TransactionError::NoGraphGuard
        | TransactionError::NoStagedGraph
        | TransactionError::CursorActiveCommit
        | TransactionError::CursorActiveStatement
        | TransactionError::Poisoned => LoraErrorCode::Internal,
    }
}

impl fmt::Debug for LoraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoraError")
            .field("code", &self.code)
            .field("message", &self.message)
            .finish()
    }
}

impl fmt::Display for LoraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for LoraError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|s| s as &(dyn Error + 'static))
    }
}

// -------- From impls for direct construction --------

impl From<ParseError> for LoraError {
    fn from(e: ParseError) -> Self {
        let msg = e.to_string();
        Self::with_source(LoraErrorCode::Parse, msg, e)
    }
}

impl From<SemanticError> for LoraError {
    fn from(e: SemanticError) -> Self {
        let msg = e.to_string();
        Self::with_source(LoraErrorCode::Semantic, msg, e)
    }
}

impl From<ExecutorError> for LoraError {
    fn from(e: ExecutorError) -> Self {
        let code = executor_code(&e);
        let msg = e.to_string();
        Self::with_source(code, msg, e)
    }
}

impl From<WalError> for LoraError {
    fn from(e: WalError) -> Self {
        let code = wal_code(&e);
        let msg = e.to_string();
        Self::with_source(code, msg, e)
    }
}

impl From<SnapshotCodecError> for LoraError {
    fn from(e: SnapshotCodecError) -> Self {
        let code = snapshot_codec_code(&e);
        let msg = e.to_string();
        Self::with_source(code, msg, e)
    }
}

impl From<SnapshotError> for LoraError {
    fn from(e: SnapshotError) -> Self {
        let code = snapshot_store_code(&e);
        let msg = e.to_string();
        Self::with_source(code, msg, e)
    }
}

impl From<DatabaseNameError> for LoraError {
    fn from(e: DatabaseNameError) -> Self {
        let msg = e.to_string();
        Self::with_source(LoraErrorCode::DatabaseName, msg, e)
    }
}

impl From<TransactionError> for LoraError {
    fn from(e: TransactionError) -> Self {
        let code = transaction_code(&e);
        let msg = e.to_string();
        Self::with_source(code, msg, e)
    }
}

impl From<std::io::Error> for LoraError {
    fn from(e: std::io::Error) -> Self {
        let msg = e.to_string();
        Self::with_source(LoraErrorCode::Io, msg, e)
    }
}

impl From<anyhow::Error> for LoraError {
    fn from(e: anyhow::Error) -> Self {
        Self::from_anyhow(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_is_client_parse() {
        let e = ParseError::new("expected `MATCH`", 0, 5);
        let mapped: LoraError = anyhow::Error::from(e).into();
        assert_eq!(mapped.code(), LoraErrorCode::Parse);
        assert_eq!(mapped.category(), LoraErrorCategory::Client);
        assert!(mapped.message().contains("parse error"));
    }

    #[test]
    fn semantic_error_is_client_semantic() {
        let e = SemanticError::UnknownVariable("n".into());
        let mapped = LoraError::from(e);
        assert_eq!(mapped.code(), LoraErrorCode::Semantic);
        assert_eq!(mapped.message(), "unknown variable `n`");
    }

    #[test]
    fn executor_timeout_is_client_timeout() {
        let mapped = LoraError::from(ExecutorError::QueryTimeout);
        assert_eq!(mapped.code(), LoraErrorCode::Timeout);
    }

    #[test]
    fn wal_io_is_server_io() {
        let inner = std::io::Error::other("disk full");
        let mapped = LoraError::from(WalError::Io(inner));
        assert_eq!(mapped.code(), LoraErrorCode::Io);
        assert_eq!(mapped.category(), LoraErrorCategory::Server);
    }

    #[test]
    fn unknown_anyhow_falls_back_to_internal() {
        let e = anyhow::anyhow!("something else entirely");
        let mapped = LoraError::from_anyhow(e);
        assert_eq!(mapped.code(), LoraErrorCode::Internal);
    }

    #[test]
    fn typed_transaction_error_routes_readonly() {
        let mapped = LoraError::from(TransactionError::ReadOnlyMutation);
        assert_eq!(mapped.code(), LoraErrorCode::ReadOnlyViolation);
        assert_eq!(
            mapped.message(),
            "cannot execute mutating query in read-only transaction"
        );
    }

    #[test]
    fn typed_transaction_error_round_trips_through_anyhow() {
        let any: anyhow::Error = TransactionError::AlreadyClosed.into();
        let mapped = LoraError::from_anyhow(any);
        assert_eq!(mapped.code(), LoraErrorCode::Internal);
        assert_eq!(mapped.message(), "transaction is already closed");
    }

    #[test]
    fn code_wire_strings_are_stable() {
        // Sanity check: these strings are part of the public API and
        // must not change between releases.
        assert_eq!(LoraErrorCode::Parse.as_str(), "LORA_PARSE");
        assert_eq!(LoraErrorCode::Timeout.as_str(), "LORA_TIMEOUT");
        assert_eq!(LoraErrorCode::WalPoisoned.as_str(), "LORA_WAL_POISONED");
        assert_eq!(LoraErrorCode::Internal.as_str(), "LORA_INTERNAL");
    }
}
