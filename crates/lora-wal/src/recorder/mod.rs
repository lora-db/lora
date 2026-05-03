//! WAL recorder: bridges the storage-side `MutationRecorder` observer
//! into the durable [`crate::Wal`].
//!
//! Layout:
//! - `recorder` — the [`WalRecorder`] adapter and `MutationRecorder` impl.
//! - `mirror` — the [`WalMirror`] trait for archive-backed sidecars.
//! - `errors` — recorder-specific error types and [`WroteCommit`].
//! - `tests` — recorder integration tests against a real `Wal` directory.

mod errors;
mod mirror;
#[allow(clippy::module_inception)]
mod recorder;

#[cfg(test)]
mod tests;

pub use errors::{WalBufferedCommitError, WalCommitError, WalPoisonError, WroteCommit};
pub use mirror::WalMirror;
pub use recorder::WalRecorder;
