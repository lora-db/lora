//! On-disk format constants for the columnar snapshot.

use crate::errors::{Result, SnapshotCodecError};

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
/// Deepest nesting of lists and maps a property value may have, on encode
/// and decode, so a crafted or runaway value cannot overflow the stack of
/// the recursive codec. The same limit as the store and WAL codecs.
pub(crate) const MAX_VALUE_DEPTH: usize = lora_store::codec::MAX_VALUE_NESTING;

/// Depth of the values inside a list or map at `depth`.
pub(crate) fn nested_encode_depth(depth: usize) -> Result<usize> {
    if depth >= MAX_VALUE_DEPTH {
        return Err(SnapshotCodecError::Encode(format!(
            "property value nests lists and maps deeper than {MAX_VALUE_DEPTH} levels"
        )));
    }
    Ok(depth + 1)
}

/// [`nested_encode_depth`] for decoding.
pub(crate) fn nested_decode_depth(depth: usize) -> Result<usize> {
    if depth >= MAX_VALUE_DEPTH {
        return Err(SnapshotCodecError::Decode(format!(
            "property value nests lists and maps deeper than {MAX_VALUE_DEPTH} levels"
        )));
    }
    Ok(depth + 1)
}
