//! Per-record framing for the write-ahead log.
//!
//! On-disk layout for a single record:
//!
//! ```text
//! [0..4)   length        u32 LE — total bytes from `length` through `crc` inclusive
//! [4..5)   kind          u8     — see `RecordKind`
//! [5..13)  lsn           u64 LE — monotonic record id
//! [13..21) tx_begin_lsn  u64 LE — owning tx's begin lsn, or 0 for non-tx records
//! [21..N)  payload       compact tagged bytes (kind-specific; empty for marker variants)
//! [N..N+4) crc           u32 LE — IEEE CRC32 over [length..crc)
//! ```
//!
//! The CRC covers the length prefix too so that a torn write — where the
//! length got persisted but the rest of the record did not — fails the
//! check and replay stops cleanly at that boundary. The first failing
//! record is treated as the new tail of the log.
//!
//! Records are written by the append paths on `Wal` and read back by replay.
//! Both paths funnel through [`WalRecord::encode`] and [`WalRecord::decode`] so
//! the framing only lives in one place.

use std::io::{self, Read, Write};

use lora_store::MutationEvent;

use crate::codec;
use crate::error::WalError;
use crate::lsn::Lsn;

/// Length of the fixed prefix that precedes the compact payload:
/// `length` + `kind` + `lsn` + `tx_begin_lsn`.
pub const RECORD_HEADER_LEN: usize = 4 + 1 + 8 + 8;

/// Length of the trailing CRC32.
pub const RECORD_TRAILER_LEN: usize = 4;

/// Discriminant byte for the record kind.
///
/// `Mutation` carries one `MutationEvent`, `MutationBatch` carries a vector of
/// events, and the marker variants carry no payload. The numbering is
/// deliberately stable — bumping it would orphan existing on-disk WALs, so
/// additions go at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordKind {
    Mutation = 1,
    TxBegin = 2,
    TxCommit = 3,
    TxAbort = 4,
    Checkpoint = 5,
    MutationBatch = 6,
}

impl RecordKind {
    fn from_byte(b: u8) -> Result<Self, WalError> {
        match b {
            1 => Ok(Self::Mutation),
            2 => Ok(Self::TxBegin),
            3 => Ok(Self::TxCommit),
            4 => Ok(Self::TxAbort),
            5 => Ok(Self::Checkpoint),
            6 => Ok(Self::MutationBatch),
            other => Err(WalError::UnknownKind(other)),
        }
    }
}

/// A single record on the wire.
///
/// `tx_begin_lsn` ties a `Mutation` record back to the transaction that
/// owns it. Markers carry their own LSN in `lsn`; for `TxCommit` /
/// `TxAbort`, `tx_begin_lsn` references the matching `TxBegin`.
#[derive(Debug, Clone, PartialEq)]
pub enum WalRecord {
    Mutation {
        lsn: Lsn,
        tx_begin_lsn: Lsn,
        event: MutationEvent,
    },
    MutationBatch {
        lsn: Lsn,
        tx_begin_lsn: Lsn,
        events: Vec<MutationEvent>,
    },
    TxBegin {
        lsn: Lsn,
    },
    TxCommit {
        lsn: Lsn,
        tx_begin_lsn: Lsn,
    },
    TxAbort {
        lsn: Lsn,
        tx_begin_lsn: Lsn,
    },
    /// Marker emitted after a successful checkpoint. `snapshot_lsn` is
    /// the LSN that was written into the snapshot header, i.e. the
    /// fence past which the WAL can be truncated.
    Checkpoint {
        lsn: Lsn,
        snapshot_lsn: Lsn,
    },
}

impl WalRecord {
    pub fn lsn(&self) -> Lsn {
        match self {
            Self::Mutation { lsn, .. }
            | Self::MutationBatch { lsn, .. }
            | Self::TxBegin { lsn }
            | Self::TxCommit { lsn, .. }
            | Self::TxAbort { lsn, .. }
            | Self::Checkpoint { lsn, .. } => *lsn,
        }
    }

    fn kind(&self) -> RecordKind {
        match self {
            Self::Mutation { .. } => RecordKind::Mutation,
            Self::MutationBatch { .. } => RecordKind::MutationBatch,
            Self::TxBegin { .. } => RecordKind::TxBegin,
            Self::TxCommit { .. } => RecordKind::TxCommit,
            Self::TxAbort { .. } => RecordKind::TxAbort,
            Self::Checkpoint { .. } => RecordKind::Checkpoint,
        }
    }

    fn tx_begin_lsn(&self) -> Lsn {
        match self {
            Self::Mutation { tx_begin_lsn, .. }
            | Self::MutationBatch { tx_begin_lsn, .. }
            | Self::TxCommit { tx_begin_lsn, .. }
            | Self::TxAbort { tx_begin_lsn, .. } => *tx_begin_lsn,
            // `TxBegin` has no parent; `Checkpoint` reuses the slot to
            // carry the snapshot fence LSN, which the decoder pulls back
            // into the right field.
            Self::TxBegin { .. } => Lsn::ZERO,
            Self::Checkpoint { snapshot_lsn, .. } => *snapshot_lsn,
        }
    }

    fn encoded_payload_len(&self) -> Result<usize, WalError> {
        match self {
            Self::Mutation { event, .. } => codec::encoded_event_len(event),
            Self::MutationBatch { events, .. } => codec::encoded_events_len(events),
            // Marker records carry no payload; the LSN + tx_begin_lsn in
            // the fixed header is the entire record.
            Self::TxBegin { .. }
            | Self::TxCommit { .. }
            | Self::TxAbort { .. }
            | Self::Checkpoint { .. } => Ok(0),
        }
    }

    fn encode_payload_into(&self, out: &mut Vec<u8>) -> Result<(), WalError> {
        match self {
            Self::Mutation { event, .. } => codec::encode_event_into(out, event),
            Self::MutationBatch { events, .. } => codec::encode_events_into(out, events),
            // Marker records carry no payload; the LSN + tx_begin_lsn in
            // the fixed header is the entire record.
            Self::TxBegin { .. }
            | Self::TxCommit { .. }
            | Self::TxAbort { .. }
            | Self::Checkpoint { .. } => Ok(()),
        }
    }

    /// Encode this record into `out`. Returns the number of bytes
    /// written, which equals the value stored in the `length` prefix.
    pub fn encode<W: Write>(&self, mut out: W) -> Result<u32, WalError> {
        let payload_len = self.encoded_payload_len()?;
        let total = RECORD_HEADER_LEN
            .checked_add(payload_len)
            .and_then(|n| n.checked_add(RECORD_TRAILER_LEN))
            .ok_or_else(|| WalError::Encode("record larger than usize::MAX".into()))?;
        let length = u32::try_from(total)
            .map_err(|_| WalError::Encode("record larger than 4 GiB".into()))?;

        let mut framed = Vec::with_capacity(total);
        framed.extend_from_slice(&length.to_le_bytes());
        framed.push(self.kind() as u8);
        framed.extend_from_slice(&self.lsn().raw().to_le_bytes());
        framed.extend_from_slice(&self.tx_begin_lsn().raw().to_le_bytes());
        self.encode_payload_into(&mut framed)?;
        debug_assert_eq!(framed.len(), total - RECORD_TRAILER_LEN);

        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&framed);
        let crc = hasher.finalize();
        framed.extend_from_slice(&crc.to_le_bytes());

        out.write_all(&framed)?;
        Ok(length)
    }

    /// Decode a single record from `reader`.
    ///
    /// Returns `Ok(None)` only on a *clean* EOF before any byte of the
    /// next record has been consumed — this is the legitimate end of a
    /// well-formed log. A partial read in the middle of a record is
    /// treated as a torn write and surfaces as
    /// [`WalError::Truncated`]; the caller is expected to truncate the
    /// segment at the position of the last successful decode.
    pub fn decode<R: Read>(mut reader: R) -> Result<Option<Self>, WalError> {
        let mut len_buf = [0u8; 4];
        match read_exact_or_eof(&mut reader, &mut len_buf)? {
            ReadOutcome::Eof => return Ok(None),
            ReadOutcome::Partial(actual) => {
                return Err(WalError::Truncated {
                    expected: 4,
                    actual,
                });
            }
            ReadOutcome::Full => {}
        }
        let length = u32::from_le_bytes(len_buf) as usize;
        if length < RECORD_HEADER_LEN + RECORD_TRAILER_LEN {
            return Err(WalError::Decode(format!(
                "record length {length} smaller than fixed framing"
            )));
        }

        // We have already consumed the length prefix. Read the rest of
        // the record into a single buffer so the CRC can be checked over
        // [length..crc) without seeking back.
        let remaining = length - 4;
        let mut rest = vec![0u8; remaining];
        match read_exact_or_eof(&mut reader, &mut rest)? {
            ReadOutcome::Full => {}
            ReadOutcome::Eof | ReadOutcome::Partial(_) => {
                return Err(WalError::Truncated {
                    expected: remaining,
                    actual: 0,
                });
            }
        }

        let crc_offset = remaining - 4;
        let stored_crc = u32::from_le_bytes(rest[crc_offset..].try_into().unwrap());

        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&len_buf);
        hasher.update(&rest[..crc_offset]);
        let actual_crc = hasher.finalize();

        // Reconstruct fields from the buffer.
        let kind = RecordKind::from_byte(rest[0])?;
        let lsn = Lsn::new(u64::from_le_bytes(rest[1..9].try_into().unwrap()));
        let tx_begin_lsn = Lsn::new(u64::from_le_bytes(rest[9..17].try_into().unwrap()));
        let payload = &rest[17..crc_offset];

        if stored_crc != actual_crc {
            return Err(WalError::CrcMismatch {
                lsn,
                expected: stored_crc,
                actual: actual_crc,
            });
        }

        Ok(Some(match kind {
            RecordKind::Mutation => {
                let event = codec::decode_event(payload)?;
                WalRecord::Mutation {
                    lsn,
                    tx_begin_lsn,
                    event,
                }
            }
            RecordKind::MutationBatch => {
                let events = codec::decode_events(payload)?;
                WalRecord::MutationBatch {
                    lsn,
                    tx_begin_lsn,
                    events,
                }
            }
            RecordKind::TxBegin => {
                if !payload.is_empty() {
                    return Err(WalError::Decode("TxBegin has unexpected payload".into()));
                }
                WalRecord::TxBegin { lsn }
            }
            RecordKind::TxCommit => {
                if !payload.is_empty() {
                    return Err(WalError::Decode("TxCommit has unexpected payload".into()));
                }
                WalRecord::TxCommit { lsn, tx_begin_lsn }
            }
            RecordKind::TxAbort => {
                if !payload.is_empty() {
                    return Err(WalError::Decode("TxAbort has unexpected payload".into()));
                }
                WalRecord::TxAbort { lsn, tx_begin_lsn }
            }
            RecordKind::Checkpoint => {
                if !payload.is_empty() {
                    return Err(WalError::Decode("Checkpoint has unexpected payload".into()));
                }
                WalRecord::Checkpoint {
                    lsn,
                    snapshot_lsn: tx_begin_lsn,
                }
            }
        }))
    }
}

enum ReadOutcome {
    Full,
    Partial(usize),
    Eof,
}

fn read_exact_or_eof<R: Read>(mut reader: R, buf: &mut [u8]) -> Result<ReadOutcome, WalError> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => {
                return Ok(if filled == 0 {
                    ReadOutcome::Eof
                } else {
                    ReadOutcome::Partial(filled)
                });
            }
            Ok(n) => filled += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(WalError::Io(e)),
        }
    }
    Ok(ReadOutcome::Full)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lora_store::{Properties, PropertyValue};

    fn sample_event() -> MutationEvent {
        let mut props = Properties::new();
        props.insert("name".into(), PropertyValue::String("alice".into()));
        MutationEvent::CreateNode {
            id: 7,
            labels: vec!["Person".into()],
            properties: props,
        }
    }

    fn round_trip(record: WalRecord) {
        let mut buf = Vec::new();
        record.encode(&mut buf).unwrap();
        let decoded = WalRecord::decode(&buf[..]).unwrap().expect("record");
        assert_eq!(decoded, record);
    }

    #[test]
    fn mutation_round_trip() {
        round_trip(WalRecord::Mutation {
            lsn: Lsn::new(42),
            tx_begin_lsn: Lsn::new(40),
            event: sample_event(),
        });
    }

    #[test]
    fn mutation_batch_round_trip() {
        round_trip(WalRecord::MutationBatch {
            lsn: Lsn::new(43),
            tx_begin_lsn: Lsn::new(40),
            events: vec![sample_event(), sample_event()],
        });
    }

    #[test]
    fn marker_round_trip() {
        round_trip(WalRecord::TxBegin { lsn: Lsn::new(40) });
        round_trip(WalRecord::TxCommit {
            lsn: Lsn::new(50),
            tx_begin_lsn: Lsn::new(40),
        });
        round_trip(WalRecord::TxAbort {
            lsn: Lsn::new(60),
            tx_begin_lsn: Lsn::new(40),
        });
        round_trip(WalRecord::Checkpoint {
            lsn: Lsn::new(70),
            snapshot_lsn: Lsn::new(50),
        });
    }

    #[test]
    fn clean_eof_returns_none() {
        let buf: &[u8] = &[];
        assert!(WalRecord::decode(buf).unwrap().is_none());
    }

    #[test]
    fn truncated_length_prefix_is_torn_write() {
        // Three bytes — not enough for the length prefix.
        let buf: &[u8] = &[1, 2, 3];
        let err = WalRecord::decode(buf).unwrap_err();
        assert!(matches!(err, WalError::Truncated { .. }));
    }

    #[test]
    fn truncated_payload_is_torn_write() {
        let mut buf = Vec::new();
        WalRecord::Mutation {
            lsn: Lsn::new(1),
            tx_begin_lsn: Lsn::new(1),
            event: sample_event(),
        }
        .encode(&mut buf)
        .unwrap();
        // Drop the last few bytes including the CRC trailer.
        buf.truncate(buf.len() - 8);
        let err = WalRecord::decode(&buf[..]).unwrap_err();
        assert!(matches!(err, WalError::Truncated { .. }));
    }

    #[test]
    fn flipped_byte_fails_crc() {
        let mut buf = Vec::new();
        WalRecord::Mutation {
            lsn: Lsn::new(1),
            tx_begin_lsn: Lsn::new(1),
            event: sample_event(),
        }
        .encode(&mut buf)
        .unwrap();
        // Flip a byte in the middle of the payload.
        let mid = buf.len() / 2;
        buf[mid] ^= 0xff;
        let err = WalRecord::decode(&buf[..]).unwrap_err();
        assert!(matches!(err, WalError::CrcMismatch { .. }));
    }

    #[test]
    fn unknown_kind_rejected() {
        let mut buf = Vec::new();
        WalRecord::TxBegin { lsn: Lsn::new(1) }
            .encode(&mut buf)
            .unwrap();
        // Position 4 is the kind byte (after the 4-byte length prefix).
        buf[4] = 99;
        // Recompute the CRC so we exercise the kind check rather than
        // tripping the CRC check first.
        let crc_offset = buf.len() - 4;
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&buf[..crc_offset]);
        let crc = hasher.finalize();
        buf[crc_offset..].copy_from_slice(&crc.to_le_bytes());
        let err = WalRecord::decode(&buf[..]).unwrap_err();
        assert!(matches!(err, WalError::UnknownKind(99)));
    }

    #[test]
    fn many_records_back_to_back() {
        // The decoder must handle a stream of records with no separator
        // beyond their own length prefixes.
        let records = vec![
            WalRecord::TxBegin { lsn: Lsn::new(1) },
            WalRecord::Mutation {
                lsn: Lsn::new(2),
                tx_begin_lsn: Lsn::new(1),
                event: sample_event(),
            },
            WalRecord::TxCommit {
                lsn: Lsn::new(3),
                tx_begin_lsn: Lsn::new(1),
            },
        ];
        let mut buf = Vec::new();
        for r in &records {
            r.encode(&mut buf).unwrap();
        }
        let mut cursor = std::io::Cursor::new(&buf);
        let mut out = Vec::new();
        while let Some(r) = WalRecord::decode(&mut cursor).unwrap() {
            out.push(r);
        }
        assert_eq!(out, records);
    }
}
