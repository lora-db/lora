//! Compact payload codec for WAL mutation records.
//!
//! The outer WAL record framing still owns length, LSNs, and CRC. This
//! module stores mutation events as a small tagged binary vocabulary
//! that mirrors `lora-store::MutationEvent`.
//!
//! Layout:
//! - `format` — on-disk constants (magic, tag/value/vector enums) shared
//!   between the encode and decode halves.
//! - `encode` — write path: `encode_event`, `encode_events`, framing
//!   helpers, and pre-flight size computation.
//! - `decode` — read path: `decode_event`, `decode_events`, and the
//!   `PayloadReader` cursor that consumes the framed bytes.
//! - `tests` — round-trip tests covering every value variant.

mod decode;
mod encode;
mod format;

#[cfg(test)]
mod tests;

/// Deepest nesting of lists and maps a property value may have in a WAL
/// record. Both halves enforce it: the encoder refuses to write a deeper
/// value and the decoder refuses to read one, because both recurse per
/// level and an unbounded value would overflow the stack during replay.
/// The same limit as the store and snapshot codecs.
pub(crate) const MAX_VALUE_DEPTH: usize = lora_store::codec::MAX_VALUE_NESTING;

pub(crate) use decode::{decode_event, decode_events, for_each_event, validate_events};
pub(crate) use encode::{
    batch_payload_len, encode_event_into, encode_events_into, encoded_batch_event_len,
    encoded_event_len, encoded_events_len,
};
