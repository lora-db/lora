//! In-memory reference backend for the storage traits.
//!
//! Layout:
//! - `graph` — the [`InMemoryGraph`] struct, its `Debug`/`Clone` impls,
//!   and the inherent helpers (slab access, adjacency, label/type
//!   indexes, replay hooks).
//! - `impls` — `GraphStorage` / `BorrowedGraphStorage` / `GraphStorageMut`
//!   impls that delegate into the inherent helpers above.
//! - `property_index` — hash-bucket property indexes used by the
//!   `find_*_by_property` lookups.
//! - `snapshot` — bridge between [`InMemoryGraph`] and the portable
//!   [`crate::SnapshotPayload`] vocabulary.
//! - `tests` — unit tests covering the in-memory backend.

mod graph;
mod impls;
mod property_index;
mod snapshot;

#[cfg(test)]
mod tests;

pub use graph::InMemoryGraph;
