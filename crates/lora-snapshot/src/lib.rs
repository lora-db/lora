//! Efficient snapshots for LoraDB graph state.
//!
//! This crate is intentionally separate from `lora-store` and `lora-wal`:
//! the store owns the canonical in-memory records, the WAL owns ordered
//! mutation recovery, and this crate owns compact point-in-time state images.
//!
//! The current format is column-oriented rather than bincode-over-struct:
//! nodes, labels, relationships, relationship types, and properties are stored
//! in separate columns. That keeps the format friendly to future Arrow /
//! Parquet backends while avoiding those heavy dependencies in the first
//! implementation. Compression and authenticated encryption are applied to the
//! encoded column body.
//!
//! Layout:
//! - `format` — magic + format/version constants.
//! - `codec` — top-level `encode_snapshot` / `decode_snapshot` /
//!   `write_snapshot` / `read_snapshot` and `SnapshotInfo`.
//! - `envelope`, `body`, `columnar`, `transform`, `view` — the layered
//!   on-disk format implementation.
//! - `errors`, `options` — public vocabulary.

mod body;
mod codec;
mod columnar;
mod envelope;
mod errors;
mod format;
mod options;
mod transform;
mod view;

#[cfg(test)]
mod tests;

pub use codec::{
    decode_snapshot, encode_snapshot, encode_snapshot_with_options, open_snapshot_view,
    read_snapshot, snapshot_info, write_snapshot, SnapshotInfo,
};
pub use errors::{Result, SnapshotCodecError};
pub use format::DATABASE_SNAPSHOT_MAGIC;
pub use options::{
    Compression, EncryptionKey, PasswordKdfParams, SnapshotCredentials, SnapshotEncryption,
    SnapshotOptions, SnapshotPassword,
};
pub use view::{SnapshotView, StringTableView, U32ColumnView, U64ColumnView};
