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

pub(crate) use decode::{decode_event, decode_events};
pub(crate) use encode::{
    encode_event_into, encode_events_into, encoded_event_len, encoded_events_len,
};
