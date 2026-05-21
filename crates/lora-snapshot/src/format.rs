//! On-disk format constants for the columnar snapshot.

pub(crate) const MAGIC: &[u8; 8] = b"LORACOL1";
pub const DATABASE_SNAPSHOT_MAGIC: &[u8; 8] = MAGIC;
pub(crate) const FORMAT_VERSION: u32 = 2;
pub(crate) const HEADER_LEN: usize = 8 + 4 + 4 + 8 + 32;
/// `2` was the last release before the catalog trailer; readable on this branch
/// for forward compatibility with older snapshots that lack indexes.
pub(crate) const BODY_FORMAT_VERSION_V2: u32 = 2;
/// `3` introduced the index-catalog trailer.
pub(crate) const BODY_FORMAT_VERSION_V3: u32 = 3;
/// `4` added the constraint-catalog trailer (after indexes).
pub(crate) const BODY_FORMAT_VERSION_V4: u32 = 4;
/// `5` added the vector-index backend trailer (HNSW graph state)
/// after constraints. JSON-encoded payload for now — a follow-up
/// can swap to a hand-rolled binary format without a version bump
/// if the JSON-byte framing is preserved.
pub(crate) const BODY_FORMAT_VERSION: u32 = 5;
