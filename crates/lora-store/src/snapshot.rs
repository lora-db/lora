//! Durable snapshots of a `GraphStorage` implementation.
//!
//! The format is:
//!
//! ```text
//! [0..8)   magic         b"LORASNAP"
//! [8..12)  format        u32 little-endian — see SNAPSHOT_FORMAT_VERSION
//! [12..16) header_flags  u32 little-endian — bit 0 = has_wal_lsn, others reserved
//! [16..24) wal_lsn       u64 little-endian — 0 when `has_wal_lsn` is unset
//! [24..40) reserved      16 bytes — zeroed; future header fields land here
//! [40..)   payload       bincode-serialized payload, layout keyed by `format`
//! last 4B  crc32         IEEE CRC over header + payload
//! ```
//!
//! The reserved-but-declared `wal_lsn` / `has_wal_lsn` fields are the seam
//! that makes a future WAL/checkpoint hybrid work without bumping the format
//! version: a checkpoint is a snapshot with `has_wal_lsn = 1` and `wal_lsn`
//! set to the log position at which the snapshot was taken.
//!
//! # Forward compatibility with legacy snapshots
//!
//! Writers always emit [`SNAPSHOT_FORMAT_VERSION`]. Readers accept any
//! version in `[SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION..=SNAPSHOT_FORMAT_VERSION]`
//! and dispatch payload decoding through [`decode_payload_for_version`]. When
//! a breaking change bumps `SNAPSHOT_FORMAT_VERSION`, keep the old struct
//! layout around (e.g. `SnapshotPayloadV1`) and add a `From` impl that
//! upgrades it to the current [`SnapshotPayload`]; legacy files then load
//! transparently through the same `load_snapshot` entry point.
//!
//! `SnapshotPayload` itself is deliberately layout-portable — it is just a
//! list of `NodeRecord` + `RelationshipRecord` plus the two ID counters. Any
//! backend that implements [`Snapshotable`] can produce or consume the same
//! format.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{NodeId, NodeRecord, RelationshipId, RelationshipRecord};

/// Magic bytes at the head of every snapshot file.
pub const SNAPSHOT_MAGIC: &[u8; 8] = b"LORASNAP";

/// Current snapshot format version. Bump on any payload-structure change.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

/// Oldest snapshot format version the current reader accepts. Files with a
/// `format` below this — or above [`SNAPSHOT_FORMAT_VERSION`] — are rejected
/// with [`SnapshotError::UnsupportedVersion`].
///
/// Raising this constant is a deliberate act: it drops support for a legacy
/// on-disk format. Until then, older snapshots continue to load through the
/// per-version dispatch in [`decode_payload_for_version`].
pub const SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION: u32 = 1;

const _: () = assert!(
    SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION <= SNAPSHOT_FORMAT_VERSION,
    "SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION must not exceed SNAPSHOT_FORMAT_VERSION",
);

/// Header byte length, including the magic + format + flags + wal_lsn +
/// reserved region. Must stay fixed per format version.
pub(crate) const HEADER_LEN: usize = 40;

/// Bit in `header_flags` that marks the `wal_lsn` field as meaningful.
pub const HEADER_FLAG_HAS_WAL_LSN: u32 = 1 << 0;

/// Portable representation of an entire store state.
///
/// Every [`Snapshotable`] backend produces and consumes this struct, so
/// snapshots are readable across backends as long as they agree on the
/// record shape (they do — all records are defined in `lora-store`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotPayload {
    pub next_node_id: NodeId,
    pub next_rel_id: RelationshipId,
    pub nodes: Vec<NodeRecord>,
    pub relationships: Vec<RelationshipRecord>,
}

impl SnapshotPayload {
    pub fn empty() -> Self {
        Self {
            next_node_id: 0,
            next_rel_id: 0,
            nodes: Vec::new(),
            relationships: Vec::new(),
        }
    }
}

/// Metadata reported by `save_snapshot` / `load_snapshot`. Kept small and
/// stable so callers can log / diff it without reflecting on the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotMeta {
    /// Format version the payload is written in.
    pub format_version: u32,
    /// Number of nodes in the snapshot.
    pub node_count: usize,
    /// Number of relationships in the snapshot.
    pub relationship_count: usize,
    /// WAL log position captured alongside the snapshot, if any. `None` for
    /// pure (non-checkpoint) snapshots.
    pub wal_lsn: Option<u64>,
}

/// Errors produced by the snapshot codec.
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("snapshot I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("snapshot is not a LORASNAP file (bad magic)")]
    BadMagic,

    #[error("unsupported snapshot format version: {0}")]
    UnsupportedVersion(u32),

    #[error("snapshot header too short (expected {expected} bytes, got {actual})")]
    TruncatedHeader { expected: usize, actual: usize },

    #[error("snapshot CRC mismatch: expected 0x{expected:08x}, got 0x{actual:08x}")]
    CrcMismatch { expected: u32, actual: u32 },

    #[error("snapshot payload could not be decoded: {0}")]
    Decode(String),

    #[error("snapshot payload could not be encoded: {0}")]
    Encode(String),
}

/// A backend that can serialize its state to a byte stream and restore from
/// one.
///
/// The trait is deliberately orthogonal to [`GraphStorage`] /
/// [`GraphStorageMut`]: a backend opts into durability independently of the
/// core read/write contract. Future hooks (WAL, incremental checkpoints)
/// will land alongside this trait, not inside it — keeping `Snapshotable`
/// narrow makes it easy to compose (e.g. `SnapshotOverS3`, or a wrapper that
/// also appends to a WAL on every mutation).
pub trait Snapshotable {
    fn save_snapshot<W: Write>(&self, writer: W) -> Result<SnapshotMeta, SnapshotError>;

    fn load_snapshot<R: Read>(&mut self, reader: R) -> Result<SnapshotMeta, SnapshotError>;

    /// Save a snapshot stamped with a WAL log position, suitable as a
    /// checkpoint fence. The fence is the LSN past which the WAL is
    /// the source of truth on recovery; replay skips records at or
    /// below it.
    ///
    /// Required (no default) because a fence-less default would
    /// silently break recovery for any backend that opted into a
    /// WAL — every backend that implements `Snapshotable` must be
    /// able to produce a checkpoint. The only in-tree impl
    /// (`InMemoryGraph`) just calls `write_snapshot` with
    /// `Some(wal_lsn)`.
    fn save_checkpoint<W: Write>(
        &self,
        writer: W,
        wal_lsn: u64,
    ) -> Result<SnapshotMeta, SnapshotError>;
}

// ---------------------------------------------------------------------------
// Header codec
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub(crate) struct SnapshotHeader {
    pub format_version: u32,
    pub header_flags: u32,
    pub wal_lsn: u64,
}

impl SnapshotHeader {
    pub(crate) fn new(format_version: u32, wal_lsn: Option<u64>) -> Self {
        let (flags, lsn) = match wal_lsn {
            Some(lsn) => (HEADER_FLAG_HAS_WAL_LSN, lsn),
            None => (0, 0),
        };
        Self {
            format_version,
            header_flags: flags,
            wal_lsn: lsn,
        }
    }

    pub(crate) fn wal_lsn_if_set(&self) -> Option<u64> {
        if self.header_flags & HEADER_FLAG_HAS_WAL_LSN != 0 {
            Some(self.wal_lsn)
        } else {
            None
        }
    }

    pub(crate) fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..8].copy_from_slice(SNAPSHOT_MAGIC);
        out[8..12].copy_from_slice(&self.format_version.to_le_bytes());
        out[12..16].copy_from_slice(&self.header_flags.to_le_bytes());
        out[16..24].copy_from_slice(&self.wal_lsn.to_le_bytes());
        // [24..40) stays zeroed.
        out
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, SnapshotError> {
        if bytes.len() < HEADER_LEN {
            return Err(SnapshotError::TruncatedHeader {
                expected: HEADER_LEN,
                actual: bytes.len(),
            });
        }
        if &bytes[0..8] != SNAPSHOT_MAGIC {
            return Err(SnapshotError::BadMagic);
        }
        let format_version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if format_version < SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION
            || format_version > SNAPSHOT_FORMAT_VERSION
        {
            return Err(SnapshotError::UnsupportedVersion(format_version));
        }
        let header_flags = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let wal_lsn = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        Ok(Self {
            format_version,
            header_flags,
            wal_lsn,
        })
    }
}

// ---------------------------------------------------------------------------
// Top-level codec
// ---------------------------------------------------------------------------

/// Serialize a payload to `writer` with an optional WAL LSN. Returns the
/// `SnapshotMeta` describing what was written.
pub(crate) fn write_snapshot<W: Write>(
    mut writer: W,
    payload: &SnapshotPayload,
    wal_lsn: Option<u64>,
) -> Result<SnapshotMeta, SnapshotError> {
    let header = SnapshotHeader::new(SNAPSHOT_FORMAT_VERSION, wal_lsn);
    let header_bytes = header.encode();

    // Encode payload to a buffer first so we can CRC + length-prefix it.
    // bincode 1.x has no built-in streaming check, and for a snapshot you
    // want the whole file to be atomic anyway.
    let payload_bytes =
        bincode::serialize(payload).map_err(|e| SnapshotError::Encode(e.to_string()))?;

    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&header_bytes);
    hasher.update(&payload_bytes);
    let crc = hasher.finalize();

    writer.write_all(&header_bytes)?;
    writer.write_all(&payload_bytes)?;
    writer.write_all(&crc.to_le_bytes())?;

    Ok(SnapshotMeta {
        format_version: SNAPSHOT_FORMAT_VERSION,
        node_count: payload.nodes.len(),
        relationship_count: payload.relationships.len(),
        wal_lsn: header.wal_lsn_if_set(),
    })
}

/// Dispatch table from on-disk format version to the current
/// [`SnapshotPayload`]. Today v1 is the only format, and its wire layout is
/// identical to `SnapshotPayload` — so the arm is a direct bincode decode.
///
/// When `SNAPSHOT_FORMAT_VERSION` is bumped:
///
/// 1. Capture the *old* struct layout under a versioned alias (e.g.
///    `SnapshotPayloadV1`) so bincode can still deserialize legacy bytes.
/// 2. Implement `From<SnapshotPayloadVN> for SnapshotPayload` that fills in
///    any new fields with sensible defaults.
/// 3. Add an arm here that deserializes into the versioned struct and
///    `.into()`s it to the current payload.
///
/// Callers never see the legacy struct — everything above this function
/// operates on the current `SnapshotPayload` only.
fn decode_payload_for_version(
    format_version: u32,
    bytes: &[u8],
) -> Result<SnapshotPayload, SnapshotError> {
    match format_version {
        1 => bincode::deserialize::<SnapshotPayload>(bytes)
            .map_err(|e| SnapshotError::Decode(e.to_string())),
        other => Err(SnapshotError::UnsupportedVersion(other)),
    }
}

/// Parse a snapshot file from `reader` into a payload plus metadata.
pub(crate) fn read_snapshot<R: Read>(
    mut reader: R,
) -> Result<(SnapshotPayload, SnapshotMeta), SnapshotError> {
    // Read everything up-front. Snapshots fit in memory by definition
    // (they mirror the in-memory graph); and bincode 1.x is happiest with a
    // contiguous buffer.
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf)?;

    if buf.len() < HEADER_LEN + 4 {
        return Err(SnapshotError::TruncatedHeader {
            expected: HEADER_LEN + 4,
            actual: buf.len(),
        });
    }

    let header = SnapshotHeader::decode(&buf[..HEADER_LEN])?;

    let crc_offset = buf.len() - 4;
    let stored_crc = u32::from_le_bytes(buf[crc_offset..].try_into().unwrap());

    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&buf[..crc_offset]);
    let actual_crc = hasher.finalize();
    if stored_crc != actual_crc {
        return Err(SnapshotError::CrcMismatch {
            expected: stored_crc,
            actual: actual_crc,
        });
    }

    let payload = decode_payload_for_version(header.format_version, &buf[HEADER_LEN..crc_offset])?;

    let meta = SnapshotMeta {
        format_version: header.format_version,
        node_count: payload.nodes.len(),
        relationship_count: payload.relationships.len(),
        wal_lsn: header.wal_lsn_if_set(),
    };
    Ok((payload, meta))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeRecord, Properties, PropertyValue, RelationshipRecord};

    fn sample_payload() -> SnapshotPayload {
        let mut props = Properties::new();
        props.insert("name".into(), PropertyValue::String("alice".into()));
        let nodes = vec![
            NodeRecord {
                id: 0,
                labels: vec!["Person".into()],
                properties: props.clone(),
            },
            NodeRecord {
                id: 1,
                labels: vec!["Person".into()],
                properties: Properties::new(),
            },
        ];
        let relationships = vec![RelationshipRecord {
            id: 0,
            src: 0,
            dst: 1,
            rel_type: "KNOWS".into(),
            properties: Properties::new(),
        }];
        SnapshotPayload {
            next_node_id: 2,
            next_rel_id: 1,
            nodes,
            relationships,
        }
    }

    #[test]
    fn roundtrip_without_wal_lsn() {
        let payload = sample_payload();
        let mut buf = Vec::new();
        let meta = write_snapshot(&mut buf, &payload, None).unwrap();

        assert_eq!(meta.format_version, SNAPSHOT_FORMAT_VERSION);
        assert_eq!(meta.node_count, 2);
        assert_eq!(meta.relationship_count, 1);
        assert_eq!(meta.wal_lsn, None);

        let (decoded, decoded_meta) = read_snapshot(&buf[..]).unwrap();
        assert_eq!(decoded, payload);
        assert_eq!(decoded_meta, meta);
    }

    #[test]
    fn roundtrip_with_wal_lsn() {
        let payload = sample_payload();
        let mut buf = Vec::new();
        let meta = write_snapshot(&mut buf, &payload, Some(42)).unwrap();
        assert_eq!(meta.wal_lsn, Some(42));

        let (decoded, decoded_meta) = read_snapshot(&buf[..]).unwrap();
        assert_eq!(decoded, payload);
        assert_eq!(decoded_meta.wal_lsn, Some(42));
    }

    #[test]
    fn bad_magic_rejected() {
        let payload = sample_payload();
        let mut buf = Vec::new();
        write_snapshot(&mut buf, &payload, None).unwrap();
        buf[0] = b'X';
        let err = read_snapshot(&buf[..]).unwrap_err();
        assert!(matches!(err, SnapshotError::BadMagic));
    }

    #[test]
    fn future_version_rejected() {
        let payload = sample_payload();
        let mut buf = Vec::new();
        write_snapshot(&mut buf, &payload, None).unwrap();
        // Bump the format version byte to something newer than the current
        // reader knows about. The version check must fire before the CRC
        // check (the CRC is now stale because we tampered with the header).
        buf[8] = 99;
        let err = read_snapshot(&buf[..]).unwrap_err();
        assert!(matches!(err, SnapshotError::UnsupportedVersion(99)));
    }

    #[test]
    fn below_min_version_rejected() {
        let payload = sample_payload();
        let mut buf = Vec::new();
        write_snapshot(&mut buf, &payload, None).unwrap();
        // Version 0 is below SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION and must
        // be rejected — we no longer support whatever pre-v1 shape existed.
        buf[8] = 0;
        let err = read_snapshot(&buf[..]).unwrap_err();
        assert!(matches!(err, SnapshotError::UnsupportedVersion(0)));
    }

    #[test]
    fn crc_mismatch_rejected() {
        let payload = sample_payload();
        let mut buf = Vec::new();
        write_snapshot(&mut buf, &payload, None).unwrap();
        // Flip a byte in the middle of the payload, leaving the CRC stale.
        let mid = HEADER_LEN + 4;
        buf[mid] ^= 0xff;
        let err = read_snapshot(&buf[..]).unwrap_err();
        assert!(matches!(err, SnapshotError::CrcMismatch { .. }));
    }

    #[test]
    fn truncated_file_rejected() {
        let payload = sample_payload();
        let mut buf = Vec::new();
        write_snapshot(&mut buf, &payload, None).unwrap();
        buf.truncate(10);
        let err = read_snapshot(&buf[..]).unwrap_err();
        assert!(matches!(err, SnapshotError::TruncatedHeader { .. }));
    }
}
