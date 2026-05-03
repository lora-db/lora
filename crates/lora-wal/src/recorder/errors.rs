//! Error types and the [`WroteCommit`] flag returned from
//! [`super::WalRecorder::commit`].

use thiserror::Error;

use crate::errors::WalError;

/// Whether [`super::WalRecorder::commit`] actually wrote a `TxCommit` to the
/// log. Read-only queries — those that never trigger
/// `MutationRecorder::record` — return [`WroteCommit::No`] so the host
/// can skip the surrounding `flush()` and avoid a per-query `fsync`
/// just to record an empty transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WroteCommit {
    /// A `TxBegin` had been lazily allocated and was paired with a
    /// matching `TxCommit`. Caller should `flush()` (under PerCommit).
    Yes,
    /// No mutation events fired during the query, so neither `TxBegin`
    /// nor `TxCommit` was appended. Caller can skip `flush()` entirely.
    No,
}

impl WroteCommit {
    pub fn wrote(self) -> bool {
        matches!(self, Self::Yes)
    }
}

#[derive(Debug, Error)]
pub enum WalCommitError {
    #[error("WAL commit failed: {0}")]
    Commit(#[source] WalError),
    #[error("WAL flush failed: {0}")]
    Flush(#[source] WalError),
}

#[derive(Debug, Error)]
#[error("WAL poisoned: {reason}")]
pub struct WalPoisonError {
    pub(super) reason: String,
}

impl WalPoisonError {
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

#[derive(Debug, Error)]
pub enum WalBufferedCommitError {
    #[error("WAL arm failed: {0}")]
    Arm(#[source] WalError),
    #[error("WAL poisoned: {0}")]
    Poisoned(String),
    #[error("WAL poisoned during commit replay: {0}")]
    ReplayPoisoned(String),
    #[error(transparent)]
    Commit(#[from] WalCommitError),
}
