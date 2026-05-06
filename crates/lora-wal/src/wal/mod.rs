//! `Wal` — the durable log handle.
//!
//! Layout:
//! - `wal` — the [`Wal`] struct, its open/append/flush/truncate methods,
//!   the inner state machine, and `Drop`.
//! - `group_flusher` — background OS thread that periodically `fsync`s the
//!   WAL when `SyncMode::Group` is configured. Compiled out on `wasm32`,
//!   where threads and `fsync` are unavailable; Group mode falls back to
//!   the cooperative drop-time flush there.
//! - `tests` — directory-level WAL tests.

#[allow(clippy::module_inception)]
mod wal;

#[cfg(not(target_arch = "wasm32"))]
mod group_flusher;

#[cfg(test)]
mod tests;

pub use wal::Wal;
