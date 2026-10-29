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
//! check and replay stops cleanly at that boundary. A failing record is
//! treated as the new tail of the log only when nothing valid follows it;
//! see [`crate::replay`].
//!
//! Records are written by the append paths on `Wal` and read back by replay.
//! Both paths funnel through [`WalRecord::encode`] and [`WalRecord::decode`] so
//! the framing only lives in one place.

use std::io::{self, Read};

use lora_store::MutationEvent;

use crate::codec;
use crate::errors::WalError;
use crate::lsn::Lsn;

/// Length of the fixed prefix that precedes the compact payload:
/// `length` + `kind` + `lsn` + `tx_begin_lsn`.
pub const RECORD_HEADER_LEN: usize = 4 + 1 + 8 + 8;

/// Length of the trailing CRC32.
pub const RECORD_TRAILER_LEN: usize = 4;

/// Maximum accepted WAL record size, including the length prefix and CRC.
///
/// A corrupt length prefix is otherwise enough to make replay allocate up to
/// 4 GiB before it can discover the record is truncated or has a bad CRC.
pub const MAX_RECORD_LEN: usize = 256 * 1024 * 1024;

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
    #[cfg(test)]
    pub fn encode<W: std::io::Write>(&self, mut out: W) -> Result<u32, WalError> {
        let mut framed = Vec::new();
        let length = self.encode_into(&mut framed)?;
        out.write_all(&framed)?;
        Ok(length)
    }

    /// Append this record's framed bytes to `out` without an intermediate
    /// buffer. On error `out` is left exactly as it was.
    pub(crate) fn encode_into(&self, out: &mut Vec<u8>) -> Result<u32, WalError> {
        let payload_len = self.encoded_payload_len()?;
        encode_frame(
            out,
            self.kind(),
            self.lsn(),
            self.tx_begin_lsn(),
            payload_len,
            |out| self.encode_payload_into(out),
        )
    }

    /// Decode a single record from `reader`.
    ///
    /// Returns `Ok(None)` only on a *clean* EOF before any byte of the
    /// next record has been consumed — this is the legitimate end of a
    /// well-formed log. A partial read in the middle of a record is
    /// treated as a torn write and surfaces as
    /// [`WalError::Truncated`]; the caller is expected to truncate the
    /// segment at the position of the last successful decode.
    #[cfg(test)]
    pub fn decode<R: Read>(reader: R) -> Result<Option<Self>, WalError> {
        match RawRecord::read(reader)? {
            Some(raw) => raw.decode().map(Some),
            None => Ok(None),
        }
    }
}

/// Append one framed record (`length`, `kind`, LSNs, payload, CRC) to
/// `out`. `payload_len` must be the exact length `payload` appends. On
/// error `out` is truncated back to its original length, so a failed
/// encode never leaves half a record in a pending buffer.
pub(crate) fn encode_frame(
    out: &mut Vec<u8>,
    kind: RecordKind,
    lsn: Lsn,
    tx_begin_lsn: Lsn,
    payload_len: usize,
    payload: impl FnOnce(&mut Vec<u8>) -> Result<(), WalError>,
) -> Result<u32, WalError> {
    let total = framed_len(payload_len)?;
    let length =
        u32::try_from(total).map_err(|_| WalError::Encode("record larger than 4 GiB".into()))?;

    let start = out.len();
    out.try_reserve(total)
        .map_err(|_| WalError::Encode(format!("record length {total} is too large to allocate")))?;
    out.extend_from_slice(&length.to_le_bytes());
    out.push(kind as u8);
    out.extend_from_slice(&lsn.raw().to_le_bytes());
    out.extend_from_slice(&tx_begin_lsn.raw().to_le_bytes());
    if let Err(err) = payload(out) {
        out.truncate(start);
        return Err(err);
    }
    if out.len() - start != total - RECORD_TRAILER_LEN {
        out.truncate(start);
        return Err(WalError::Encode(
            "record payload length disagrees with its size estimate".into(),
        ));
    }

    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&out[start..]);
    let crc = hasher.finalize();
    out.extend_from_slice(&crc.to_le_bytes());
    Ok(length)
}

/// Total framed size of a record carrying `payload_len` payload bytes,
/// rejecting records over [`MAX_RECORD_LEN`].
pub(crate) fn framed_len(payload_len: usize) -> Result<usize, WalError> {
    let total = RECORD_HEADER_LEN
        .checked_add(payload_len)
        .and_then(|n| n.checked_add(RECORD_TRAILER_LEN))
        .ok_or_else(|| WalError::Encode("record larger than usize::MAX".into()))?;
    if total > MAX_RECORD_LEN {
        return Err(WalError::Encode(format!(
            "record length {total} exceeds maximum {MAX_RECORD_LEN}"
        )));
    }
    Ok(total)
}

/// A record as replay reads it: a mutation batch still encoded (see
/// [`RawRecord::decode_for_replay`]), anything else decoded.
pub(crate) enum ReplayRecord {
    Batch {
        lsn: Lsn,
        tx_begin_lsn: Lsn,
        batch: EncodedBatch,
    },
    Decoded(WalRecord),
}

/// A mutation batch's payload, checked to decode, and its event count.
#[derive(Debug)]
pub(crate) struct EncodedBatch {
    /// The frame after its length prefix.
    rest: FrameBuf,
    /// The payload's range in `rest`.
    start: usize,
    end: usize,
    count: usize,
}

impl EncodedBatch {
    pub(crate) fn len(&self) -> usize {
        self.count
    }

    /// Decode the events one at a time, handing each to `f`.
    pub(crate) fn for_each<E: From<WalError>>(
        &self,
        f: impl FnMut(MutationEvent) -> Result<(), E>,
    ) -> Result<(), E> {
        codec::for_each_event(&self.rest[self.start..self.end], f)
    }
}

/// The bytes of one frame as replay reads it. A transaction of a few
/// thousand writes is a frame of megabytes, and replay frees one per
/// mutation batch: on macOS such a frame gets pages of its own, so the
/// open does not keep them resident (see [`lora_store::scratch`]).
type FrameBuf = lora_store::scratch::ScratchBytes;

/// A record whose framing checked out (length in bounds, every byte
/// present, CRC matches) but whose payload is not decoded yet.
///
/// Splitting the two steps lets replay tell a torn or corrupt *frame*,
/// which can be the tail of an interrupted write, from a CRC-valid
/// record whose payload does not decode, which can only be a bug or
/// tampering and must never be cut off as a torn tail.
pub(crate) struct RawRecord {
    kind: u8,
    lsn: Lsn,
    tx_begin_lsn: Lsn,
    /// Everything after the length prefix, CRC trailer included.
    rest: FrameBuf,
}

impl RawRecord {
    /// Read one frame. `Ok(None)` on a clean EOF; [`WalError::Truncated`],
    /// [`WalError::CrcMismatch`] or [`WalError::Decode`] (length out of
    /// bounds) when the frame is torn or corrupt; [`WalError::Io`] when
    /// the read itself failed.
    pub(crate) fn read<R: Read>(mut reader: R) -> Result<Option<Self>, WalError> {
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
        let raw_length = u32::from_le_bytes(len_buf);
        let length = usize::try_from(raw_length).map_err(|_| {
            WalError::Decode(format!(
                "record length {raw_length} does not fit in usize on this platform"
            ))
        })?;
        if length < RECORD_HEADER_LEN + RECORD_TRAILER_LEN {
            return Err(WalError::Decode(format!(
                "record length {length} smaller than fixed framing"
            )));
        }
        if length > MAX_RECORD_LEN {
            return Err(WalError::Decode(format!(
                "record length {length} exceeds maximum {MAX_RECORD_LEN}"
            )));
        }

        // We have already consumed the length prefix. Read the rest of
        // the record into a single buffer so the CRC can be checked over
        // [length..crc) without seeking back.
        let remaining = length - 4;
        let mut rest = FrameBuf::zeroed(remaining).ok_or_else(|| {
            WalError::Decode(format!("record length {length} is too large to allocate"))
        })?;
        match read_exact_or_eof(&mut reader, &mut rest)? {
            ReadOutcome::Full => {}
            ReadOutcome::Eof => {
                return Err(WalError::Truncated {
                    expected: remaining,
                    actual: 0,
                });
            }
            ReadOutcome::Partial(actual) => {
                return Err(WalError::Truncated {
                    expected: remaining,
                    actual,
                });
            }
        }

        let crc_offset = remaining - RECORD_TRAILER_LEN;
        let stored_crc = le_u32(&rest[crc_offset..])?;
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&len_buf);
        hasher.update(&rest[..crc_offset]);
        let actual_crc = hasher.finalize();

        let lsn = Lsn::new(le_u64(&rest[1..9])?);
        if stored_crc != actual_crc {
            return Err(WalError::CrcMismatch {
                lsn,
                expected: stored_crc,
                actual: actual_crc,
            });
        }
        let tx_begin_lsn = Lsn::new(le_u64(&rest[9..17])?);
        Ok(Some(Self {
            kind: rest[0],
            lsn,
            tx_begin_lsn,
            rest,
        }))
    }

    /// Size of the whole frame, length prefix and checksum included.
    pub(crate) fn encoded_len(&self) -> usize {
        4 + self.rest.len()
    }

    /// [`Self::decode`] for replay, which keeps a mutation batch encoded
    /// until its transaction is applied: the batch is checked here, an
    /// event at a time, and decoded again an event at a time as it is
    /// applied, so neither step holds the batch decoded. Errors are those
    /// of [`Self::decode`], at the same record.
    pub(crate) fn decode_for_replay(self) -> Result<ReplayRecord, WalError> {
        if self.kind != RecordKind::MutationBatch as u8 {
            return self.decode().map(ReplayRecord::Decoded);
        }
        let crc_offset = self.rest.len() - RECORD_TRAILER_LEN;
        let start = RECORD_HEADER_LEN - 4;
        let count = codec::validate_events(&self.rest[start..crc_offset])?;
        Ok(ReplayRecord::Batch {
            lsn: self.lsn,
            tx_begin_lsn: self.tx_begin_lsn,
            batch: EncodedBatch {
                rest: self.rest,
                start,
                end: crc_offset,
                count,
            },
        })
    }

    /// Decode the payload. Errors here mean the frame was intact, so the
    /// bytes are what was written: they are never a torn tail.
    pub(crate) fn decode(self) -> Result<WalRecord, WalError> {
        let Self {
            kind,
            lsn,
            tx_begin_lsn,
            rest,
        } = self;
        let kind = RecordKind::from_byte(kind)?;
        let crc_offset = rest.len() - RECORD_TRAILER_LEN;
        let payload = &rest[(RECORD_HEADER_LEN - 4)..crc_offset];

        Ok(match kind {
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
        })
    }
}

/// True when `bytes` starts with a complete, CRC-valid record frame of a
/// known kind whose LSN is above `after`. Used to decide whether a bad
/// frame is followed by more log (corruption) or is the end of it (a torn
/// tail).
pub(crate) fn is_valid_frame_at(bytes: &[u8], after: Lsn) -> bool {
    if bytes.len() < RECORD_HEADER_LEN + RECORD_TRAILER_LEN {
        return false;
    }
    let length = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let max = MAX_RECORD_LEN.min(bytes.len());
    if !(RECORD_HEADER_LEN + RECORD_TRAILER_LEN..=max).contains(&length) {
        return false;
    }
    if RecordKind::from_byte(bytes[4]).is_err() {
        return false;
    }
    let mut lsn = [0u8; 8];
    lsn.copy_from_slice(&bytes[5..13]);
    if u64::from_le_bytes(lsn) <= after.raw() {
        return false;
    }
    let crc_offset = length - RECORD_TRAILER_LEN;
    let stored = u32::from_le_bytes([
        bytes[crc_offset],
        bytes[crc_offset + 1],
        bytes[crc_offset + 2],
        bytes[crc_offset + 3],
    ]);
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&bytes[..crc_offset]);
    hasher.finalize() == stored
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

fn le_u32(bytes: &[u8]) -> Result<u32, WalError> {
    let array = fixed_bytes::<4>(bytes)?;
    Ok(u32::from_le_bytes(array))
}

fn le_u64(bytes: &[u8]) -> Result<u64, WalError> {
    let array = fixed_bytes::<8>(bytes)?;
    Ok(u64::from_le_bytes(array))
}

fn fixed_bytes<const N: usize>(bytes: &[u8]) -> Result<[u8; N], WalError> {
    if bytes.len() != N {
        return Err(WalError::Decode(format!(
            "expected {N} bytes in WAL record framing, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(bytes);
    Ok(out)
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
    fn oversized_record_length_is_rejected_before_allocation() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&u32::MAX.to_le_bytes());

        let err = WalRecord::decode(&buf[..]).unwrap_err();
        assert!(matches!(err, WalError::Decode(_)));
        assert!(err.to_string().contains("exceeds maximum"));
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
        let mut cursor = io::Cursor::new(&buf);
        let mut out = Vec::new();
        while let Some(r) = WalRecord::decode(&mut cursor).unwrap() {
            out.push(r);
        }
        assert_eq!(out, records);
    }
}
