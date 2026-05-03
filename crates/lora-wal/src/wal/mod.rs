//! `Wal` — the durable log handle.
//!
//! Layout:
//! - `wal` — the [`Wal`] struct, its open/append/flush/truncate methods,
//!   the inner state machine, and `Drop`.
//! - `group_flusher` — the OS thread that periodically `fsync`s under
//!   `SyncMode::Group`. Spawned by [`Wal::open`] when the configured
//!   sync mode requires it; `Drop` joins the thread.
//! - `tests` — directory-level WAL tests.

mod group_flusher;
#[allow(clippy::module_inception)]
mod wal;

#[cfg(test)]
mod tests;

pub use wal::Wal;
