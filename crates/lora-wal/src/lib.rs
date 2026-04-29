//! Write-ahead log for LoraDB.
//!
//! `lora-wal` is the additive durability layer that sits between
//! [`lora_store::MutationEvent`] producers (the in-memory engine) and a
//! durable byte stream on disk. It is deliberately separate from
//! `lora-store`:
//!
//! - `lora-store` stays storage-only — backends do not learn about logs,
//!   segments, or fsync. The mutation surface they already expose
//!   (`MutationEvent` + `MutationRecorder`) is the only seam.
//! - `lora-wal` owns segment files, framing, replay, and truncation.
//!   Consumers depending on the no-WAL story (wasm, embedded read-only)
//!   can avoid this crate entirely.
//! - `lora-database` glues the two together: it installs a [`WalRecorder`]
//!   onto the store and brackets each query with `arm` / `commit` /
//!   `abort` markers so replay sees query-atomic units even though the
//!   recorder fires per primitive mutation.
//!
//! See `docs/decisions/0004-wal.md` for the full design and
//! `docs/operations/wal.md` for operator-facing semantics.

mod codec;
mod config;
mod dir;
mod error;
mod lock;
mod lsn;
mod record;
mod recorder_adapter;
mod replay;
mod segment;
#[cfg(test)]
mod testing;
mod wal;

// ---- Operator-facing surface ----------------------------------------------
//
// Everything below is what callers (`lora-database`, the HTTP server,
// admin paths, integration tests) need. Internal types — segment
// framing constants, the segment reader/writer, record codec — stay
// `pub(crate)`. Bumping that boundary later is opt-in and easy; the
// reverse (un-publishing a previously-public type) breaks downstream
// builds.

pub use config::{SyncMode, WalConfig};
pub use dir::SegmentId;
pub use error::WalError;
pub use lsn::Lsn;
pub use recorder_adapter::{WalMirror, WalRecorder, WroteCommit};
pub use replay::{replay_dir, ReplayOutcome, TornTailInfo};
pub use wal::Wal;
