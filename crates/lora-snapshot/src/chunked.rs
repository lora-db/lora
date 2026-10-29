//! The framed snapshot body (envelope format 3).
//!
//! The column-major body (format 2) is one stream, compressed whole: a
//! reader inflates and decodes it on one thread, and a graph of tens of
//! gigabytes took minutes to open. Here the records travel in frames of
//! a few thousand, each compressed on its own, so frames are written and
//! read on every core.
//!
//! ```text
//! body:    u32 CHUNKED_BODY_VERSION
//!          u64 next node id, u64 next relationship id
//!          names: labels, types, keys        (u64 count, then u64 length + UTF-8 each)
//!          value dictionary: strings         (as names)
//!                            u64 count, then per key: u32 key number, u8 state, u16 misses
//!          frame*                            node frames, then relationship frames,
//!                                            then one catalog frame
//!          u8 0                              end
//! frame:   u8 kind, u64 records, u64 raw length, u64 stored length, stored bytes
//! ```
//!
//! A frame's stored bytes are its raw bytes, raw-deflated when the
//! manifest says gzip. Raw bytes, with integers as LEB128 varints:
//!
//! ```text
//! node:          id (the first in a frame whole, then the step from the last),
//!                label count, label numbers, properties
//! relationship:  id (as a node's), source, target, type number, properties
//! properties:    count, then per property: key number, value
//! value:         tag 0 null | 1 false | 2 true | 3 int (zigzag) | 4 float (8 bytes LE)
//!                | 5 string (length, UTF-8) | 6 other (length, `lora_store::codec` bytes)
//! catalog:       index definitions, constraint definitions, vector index
//!                trailer, each u64 length + bytes as in format 2
//! ```
//!
//! Names are numbered as the graph numbers them ([`SnapshotNames`]), so a
//! writer copies the numbers out of the stored records, and a reader
//! primes the graph it builds with the same names and encodes each frame
//! into the stored form on a worker thread
//! ([`lora_store::SnapshotEncoder`]). The builder's thread only files the
//! encoded records, in frame order, while the workers decode the next
//! frames.

use std::io::{Read, Write};
use std::sync::Arc;

use flate2::bufread::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression as DeflateLevel;
use lora_store::codec::{
    decode_constraint_definitions, decode_index_definitions, decode_property_value,
    encode_property_value,
};
use lora_store::scratch::ScratchBytes;
use lora_store::{
    InMemoryGraph, Name, NodeBatch, PropertyValue, RelationshipBatch, SnapshotEncoder,
    SnapshotGraphBuilder, SnapshotNames, ValueDictionary, ValueKeyState, ValueRef,
};

use crate::errors::{Result, SnapshotCodecError};
use crate::graph::{put_bytes, put_len, put_u32, put_u64, store_error, BodyStream, Counts};
use crate::graph::{write_trailers, LoadedBody};
use crate::options::Compression;

/// Version of the framed body, inside envelope format 3.
pub(crate) const CHUNKED_BODY_VERSION: u32 = 1;

const FRAME_END: u8 = 0;
const FRAME_NODES: u8 = 1;
const FRAME_RELATIONSHIPS: u8 = 2;
const FRAME_CATALOG: u8 = 3;

/// Ids a frame covers. A frame of 4,096 typical records is a hundred
/// kilobytes or two: enough to amortize a frame's cost, small enough that
/// a graph of a million records is spread over every core and the frames
/// in flight (two or three rounds of them) stay a few megabytes.
const IDS_PER_FRAME: u64 = 4096;

/// [`IDS_PER_FRAME`], unless a benchmark set another with
/// [`set_ids_per_frame`].
static FRAME_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(IDS_PER_FRAME);

/// Ids per frame of the snapshots this process writes next. For
/// benchmarks; readers take any frame size.
#[doc(hidden)]
pub fn set_ids_per_frame(ids: u64) {
    FRAME_IDS.store(ids.max(1), std::sync::atomic::Ordering::Relaxed);
}

/// Largest allocation made up front for a length read from a frame.
const MAX_PREALLOC: usize = 1 << 24;

const TAG_NULL: u8 = 0;
const TAG_FALSE: u8 = 1;
const TAG_TRUE: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_FLOAT: u8 = 4;
const TAG_STRING: u8 = 5;
const TAG_OTHER: u8 = 6;

const VALUE_KEY_NUMBERED: u8 = 0;
const VALUE_KEY_STORED: u8 = 1;

// ---------------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------------

/// Frames handled per round: enough to keep every thread busy.
fn frames_per_round() -> usize {
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    {
        rayon::current_num_threads().max(1) * 2
    }
    #[cfg(not(all(feature = "parallel", not(target_arch = "wasm32"))))]
    {
        1
    }
}

/// `f` over every item, on every core when the build allows, in order.
fn map_all<T: Send, U: Send>(items: Vec<T>, f: impl Fn(T) -> U + Sync + Send) -> Vec<U> {
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    {
        use rayon::prelude::*;
        items.into_par_iter().map(f).collect()
    }
    #[cfg(not(all(feature = "parallel", not(target_arch = "wasm32"))))]
    {
        items.into_iter().map(f).collect()
    }
}

/// Run `background` on the thread pool while this thread runs
/// `foreground`; return both results. Without threads, one after the
/// other.
fn overlap<A: Send, B>(
    background: impl FnOnce() -> A + Send,
    foreground: impl FnOnce() -> B,
) -> (A, B) {
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    {
        let mut a = None;
        let b = rayon::in_place_scope(|scope| {
            scope.spawn(|_| a = Some(background()));
            foreground()
        });
        (a.expect("the scope joined its task"), b)
    }
    #[cfg(not(all(feature = "parallel", not(target_arch = "wasm32"))))]
    {
        let a = background();
        (a, foreground())
    }
}

// ---------------------------------------------------------------------------
// Varints
// ---------------------------------------------------------------------------

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn unzigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

/// A frame's raw bytes being decoded.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn u8(&mut self) -> Result<u8> {
        let byte = *self.bytes.get(self.pos).ok_or_else(truncated_frame)?;
        self.pos += 1;
        Ok(byte)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        if len > self.remaining() {
            return Err(truncated_frame());
        }
        let bytes = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(bytes)
    }

    fn varint(&mut self) -> Result<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            let bits = u64::from(byte & 0x7f);
            if shift == 63 && bits > 1 {
                break;
            }
            value |= bits << shift;
            if byte < 0x80 {
                return Ok(value);
            }
        }
        Err(SnapshotCodecError::Decode(
            "invalid varint in snapshot frame".into(),
        ))
    }

    fn number(&mut self) -> Result<u32> {
        u32::try_from(self.varint()?)
            .map_err(|_| SnapshotCodecError::Decode("dictionary number out of range".into()))
    }

    /// A count of items each at least one byte long, so a corrupt count
    /// fails here instead of sizing an allocation.
    fn count(&mut self) -> Result<usize> {
        let count = self.varint()?;
        if count > self.remaining() as u64 {
            return Err(truncated_frame());
        }
        Ok(count as usize)
    }

    fn len(&mut self) -> Result<usize> {
        let len = self.varint()?;
        if len > self.remaining() as u64 {
            return Err(truncated_frame());
        }
        Ok(len as usize)
    }

    fn value(&mut self) -> Result<PropertyValue> {
        Ok(match self.u8()? {
            TAG_NULL => PropertyValue::Null,
            TAG_FALSE => PropertyValue::Bool(false),
            TAG_TRUE => PropertyValue::Bool(true),
            TAG_INT => PropertyValue::Int(unzigzag(self.varint()?)),
            TAG_FLOAT => {
                let bytes = self.take(8)?;
                PropertyValue::Float(f64::from_le_bytes(bytes.try_into().expect("8 bytes")))
            }
            TAG_STRING => {
                let len = self.len()?;
                let bytes = self.take(len)?;
                PropertyValue::String(
                    std::str::from_utf8(bytes)
                        .map_err(|e| {
                            SnapshotCodecError::Decode(format!("invalid UTF-8 string: {e}"))
                        })?
                        .to_owned(),
                )
            }
            TAG_OTHER => {
                let len = self.len()?;
                decode_property_value(self.take(len)?)
                    .map_err(|e| SnapshotCodecError::Decode(e.to_string()))?
            }
            tag => {
                return Err(SnapshotCodecError::Decode(format!(
                    "invalid property value tag {tag}"
                )));
            }
        })
    }

    fn properties(&mut self, out: &mut Vec<(u32, PropertyValue)>) -> Result<()> {
        let count = self.count()?;
        for _ in 0..count {
            let key = self.number()?;
            let value = self.value()?;
            out.push((key, value));
        }
        Ok(())
    }

    /// The next record id: the first of a frame whole, then the step from
    /// the last, which must move forward.
    fn id(&mut self, last: Option<u64>) -> Result<u64> {
        let raw = self.varint()?;
        match last {
            None => Ok(raw),
            Some(last) if raw > 0 => last
                .checked_add(raw)
                .ok_or_else(|| SnapshotCodecError::Decode("record id overflows u64".into())),
            Some(_) => Err(SnapshotCodecError::Decode(
                "record ids in a snapshot frame must ascend".into(),
            )),
        }
    }
}

fn truncated_frame() -> SnapshotCodecError {
    SnapshotCodecError::Decode("truncated snapshot frame".into())
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

fn put_value(out: &mut Vec<u8>, value: ValueRef<'_>) -> Result<()> {
    match value {
        ValueRef::Null => out.push(TAG_NULL),
        ValueRef::Bool(false) => out.push(TAG_FALSE),
        ValueRef::Bool(true) => out.push(TAG_TRUE),
        ValueRef::Int(value) => {
            out.push(TAG_INT);
            push_varint(out, zigzag(value));
        }
        ValueRef::Float(value) => {
            out.push(TAG_FLOAT);
            out.extend_from_slice(&value.to_le_bytes());
        }
        ValueRef::String(value) => {
            out.push(TAG_STRING);
            push_varint(out, value.len() as u64);
            out.extend_from_slice(value.as_bytes());
        }
        ValueRef::Other(value) => {
            let bytes = encode_property_value(&value.get())
                .map_err(|e| SnapshotCodecError::Encode(e.to_string()))?;
            out.push(TAG_OTHER);
            push_varint(out, bytes.len() as u64);
            out.extend_from_slice(&bytes);
        }
    }
    Ok(())
}

/// One frame ready to write.
struct Frame {
    kind: u8,
    records: u64,
    raw_len: u64,
    stored: Vec<u8>,
}

fn compress_frame(raw: Vec<u8>, compression: Compression) -> Result<(u64, Vec<u8>)> {
    let raw_len = raw.len() as u64;
    match compression {
        Compression::None => Ok((raw_len, raw)),
        Compression::Gzip { level } => {
            let mut encoder = DeflateEncoder::new(
                Vec::with_capacity(raw.len() / 4),
                DeflateLevel::new(level.min(9)),
            );
            encoder.write_all(&raw)?;
            Ok((raw_len, encoder.finish()?))
        }
        #[cfg(not(target_arch = "wasm32"))]
        Compression::Zstd { level } => {
            let level = i32::try_from(level.clamp(1, 22)).expect("small level");
            Ok((raw_len, zstd::bulk::compress(&raw, level)?))
        }
        #[cfg(target_arch = "wasm32")]
        Compression::Zstd { .. } => Err(SnapshotCodecError::UnsupportedCompression(
            "zstd is not available on wasm",
        )),
    }
}

/// Raw bytes after which the writer starts a new frame, so a range of
/// large records is stored as several frames a reader can bound.
const FRAME_SPLIT_BYTES: usize = 64 << 20;

/// Largest raw frame a reader inflates. The writer splits frames at
/// [`FRAME_SPLIT_BYTES`], so only a single record this large exceeds it,
/// and the writer refuses that record instead of writing a snapshot no
/// reader would load.
const MAX_FRAME_RAW: u64 = 1 << 30;

/// Raw bytes of the frames one round reads ahead. Up to three rounds are
/// alive at once, so this, with [`MAX_FRAME_RAW`], bounds what a snapshot
/// can make a reader hold before its records are checked, however far
/// its frames claim to inflate.
const ROUND_RAW_BUDGET: u64 = 256 << 20;

/// Collects one id range's records into frames of about
/// [`FRAME_SPLIT_BYTES`].
struct FrameSink {
    kind: u8,
    compression: Compression,
    raw: Vec<u8>,
    records: u64,
    frames: Vec<Frame>,
}

impl FrameSink {
    fn new(kind: u8, compression: Compression) -> Self {
        Self {
            kind,
            compression,
            raw: Vec::new(),
            records: 0,
            frames: Vec::new(),
        }
    }

    /// Delta-encode `id` against the frame's previous record; the first
    /// record of a frame is written whole.
    fn start_record(&mut self, id: u64, last: &mut Option<u64>) {
        push_varint(&mut self.raw, last.map_or(id, |last| id - last));
        *last = Some(id);
    }

    /// Count the record just written, and close the frame once it is large.
    fn end_record(&mut self, last: &mut Option<u64>) -> Result<()> {
        self.records += 1;
        if self.raw.len() >= FRAME_SPLIT_BYTES {
            self.flush()?;
            *last = None;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.records == 0 {
            return Ok(());
        }
        if self.raw.len() as u64 > MAX_FRAME_RAW {
            return Err(SnapshotCodecError::Encode(format!(
                "a snapshot record takes more than {MAX_FRAME_RAW} bytes"
            )));
        }
        let raw = std::mem::take(&mut self.raw);
        let (raw_len, stored) = compress_frame(raw, self.compression)?;
        self.frames.push(Frame {
            kind: self.kind,
            records: std::mem::take(&mut self.records),
            raw_len,
            stored,
        });
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<Frame>> {
        self.flush()?;
        Ok(self.frames)
    }
}

fn node_frame(
    graph: &InMemoryGraph,
    ids: std::ops::Range<u64>,
    compression: Compression,
) -> Result<Vec<Frame>> {
    let mut sink = FrameSink::new(FRAME_NODES, compression);
    let mut last = None;
    for node in graph.snapshot_nodes_in(ids) {
        sink.start_record(node.id(), &mut last);
        let raw = &mut sink.raw;
        let labels = node.label_ids();
        push_varint(raw, labels.len() as u64);
        for label in labels {
            push_varint(raw, u64::from(label));
        }
        push_varint(raw, node.property_count() as u64);
        for (key, value) in node.properties() {
            push_varint(raw, u64::from(key));
            put_value(raw, value)?;
        }
        sink.end_record(&mut last)?;
    }
    sink.finish()
}

fn relationship_frame(
    graph: &InMemoryGraph,
    ids: std::ops::Range<u64>,
    compression: Compression,
) -> Result<Vec<Frame>> {
    let mut sink = FrameSink::new(FRAME_RELATIONSHIPS, compression);
    let mut last = None;
    for rel in graph.snapshot_relationships_in(ids) {
        sink.start_record(rel.id(), &mut last);
        let raw = &mut sink.raw;
        push_varint(raw, rel.src());
        push_varint(raw, rel.dst());
        push_varint(raw, u64::from(rel.type_id()));
        push_varint(raw, rel.property_count() as u64);
        for (key, value) in rel.properties() {
            push_varint(raw, u64::from(key));
            put_value(raw, value)?;
        }
        sink.end_record(&mut last)?;
    }
    sink.finish()
}

fn write_frame(out: &mut impl Write, frame: &Frame) -> Result<()> {
    out.write_all(&[frame.kind])?;
    put_u64(out, frame.records)?;
    put_u64(out, frame.raw_len)?;
    put_u64(out, frame.stored.len() as u64)?;
    out.write_all(&frame.stored)?;
    Ok(())
}

fn put_strings<'a>(
    out: &mut impl Write,
    values: impl ExactSizeIterator<Item = &'a str>,
) -> Result<()> {
    put_len(out, values.len())?;
    for value in values {
        put_bytes(out, value.as_bytes())?;
    }
    Ok(())
}

fn write_names(out: &mut impl Write, names: &SnapshotNames) -> Result<()> {
    put_strings(out, names.labels.iter().map(Name::as_str))?;
    put_strings(out, names.types.iter().map(Name::as_str))?;
    put_strings(out, names.keys.iter().map(Name::as_str))?;
    put_strings(out, names.values.strings.iter().map(|s| &**s))?;
    let numbers: std::collections::HashMap<&str, usize> = names
        .keys
        .iter()
        .enumerate()
        .map(|(number, name)| (name.as_str(), number))
        .collect();
    put_len(out, names.values.keys.len())?;
    for (key, state) in &names.values.keys {
        let number = *numbers.get(key.as_str()).ok_or_else(|| {
            SnapshotCodecError::Encode("value dictionary names an unknown key".into())
        })?;
        put_u32(out, number as u32)?;
        match state {
            ValueKeyState::Numbered { misses } => {
                out.write_all(&[VALUE_KEY_NUMBERED])?;
                out.write_all(&misses.to_le_bytes())?;
            }
            ValueKeyState::Stored => {
                out.write_all(&[VALUE_KEY_STORED])?;
                out.write_all(&0u16.to_le_bytes())?;
            }
        }
    }
    Ok(())
}

/// Write `graph` as a framed body. Frames are built and compressed on
/// every core, a round at a time, while this thread writes the round
/// before.
pub(crate) fn write_chunked_body<W: Write>(
    out: &mut W,
    graph: &InMemoryGraph,
    compression: Compression,
) -> Result<Counts> {
    let catalogs = graph.snapshot_catalogs();
    put_u32(out, CHUNKED_BODY_VERSION)?;
    put_u64(out, catalogs.next_node_id)?;
    put_u64(out, catalogs.next_rel_id)?;
    write_names(out, &graph.snapshot_names())?;

    let frame_ids = FRAME_IDS.load(std::sync::atomic::Ordering::Relaxed);
    let ranges = |slots: u64| {
        (0..slots.div_ceil(frame_ids))
            .map(|i| (i * frame_ids)..((i + 1) * frame_ids).min(slots))
            .map(|ids| (true, ids))
            .collect::<Vec<_>>()
    };
    let mut work: Vec<(bool, std::ops::Range<u64>)> = ranges(graph.snapshot_node_slots());
    work.extend(
        ranges(graph.snapshot_relationship_slots())
            .into_iter()
            .map(|(_, ids)| (false, ids)),
    );

    let build = |(nodes, ids): (bool, std::ops::Range<u64>)| {
        if nodes {
            node_frame(graph, ids, compression)
        } else {
            relationship_frame(graph, ids, compression)
        }
    };
    let mut counts = Counts {
        nodes: 0,
        relationships: 0,
    };
    let round = frames_per_round();
    let mut rounds = work.chunks(round).map(<[_]>::to_vec);
    let mut ready = map_all(rounds.next().unwrap_or_default(), build);
    loop {
        let next = rounds.next();
        let done = next.is_none();
        let (built, written) = overlap(
            || map_all(next.unwrap_or_default(), build),
            || -> Result<()> {
                for frames in ready.drain(..) {
                    for frame in frames? {
                        match frame.kind {
                            FRAME_NODES => counts.nodes += frame.records,
                            _ => counts.relationships += frame.records,
                        }
                        write_frame(out, &frame)?;
                    }
                }
                Ok(())
            },
        );
        written?;
        ready = built;
        if done {
            break;
        }
    }

    let mut trailer = Vec::new();
    write_trailers(&mut trailer, &catalogs)?;
    let (raw_len, stored) = compress_frame(trailer, compression)?;
    write_frame(
        out,
        &Frame {
            kind: FRAME_CATALOG,
            records: 0,
            raw_len,
            stored,
        },
    )?;
    out.write_all(&[FRAME_END])?;
    Ok(counts)
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// A frame as read, before decoding.
#[derive(Clone)]
struct StoredFrame {
    kind: u8,
    records: u64,
    raw_len: u64,
    stored: Vec<u8>,
}

enum Decoded {
    Nodes(NodeBatch),
    Relationships(RelationshipBatch),
}

fn inflate(frame: &StoredFrame, compression: Compression) -> Result<ScratchBytes> {
    match compression {
        Compression::None => {
            if frame.stored.len() as u64 != frame.raw_len {
                return Err(SnapshotCodecError::Decode(
                    "snapshot frame length mismatch".into(),
                ));
            }
            Ok(ScratchBytes::from_vec(frame.stored.clone()))
        }
        Compression::Gzip { .. } => {
            // The buffered decoder reads no further than its stream ends.
            let mut stored = frame.stored.as_slice();
            let raw = read_inflated(DeflateDecoder::new(&mut stored), frame.raw_len)?;
            expect_consumed(stored)?;
            Ok(raw)
        }
        #[cfg(not(target_arch = "wasm32"))]
        Compression::Zstd { .. } => {
            // Streamed, as gzip is: the output grows as bytes really
            // inflate, never to the size the frame claims up front.
            let mut stored = frame.stored.as_slice();
            let decoder = zstd::stream::read::Decoder::with_buffer(&mut stored)
                .map_err(|e| SnapshotCodecError::Decode(format!("snapshot frame: {e}")))?
                .single_frame();
            let raw = read_inflated(decoder, frame.raw_len)?;
            expect_consumed(stored)?;
            Ok(raw)
        }
        #[cfg(target_arch = "wasm32")]
        Compression::Zstd { .. } => inflate_zstd_portable(frame),
    }
}

/// Inflate exactly `raw_len` bytes from `decoder`: a frame that inflates
/// to more or fewer fails, having allocated no more than it produced.
///
/// A frame of up to [`MAX_PREALLOC`] bytes inflates into a buffer of its
/// size, which on macOS is pages of its own when it is large, unmapped
/// when the frame is decoded: a `Vec` per frame left the open holding
/// those malloc kept for reuse (G-64). Mapped pages cost memory only as
/// they are written, so a frame that claims more than it inflates to
/// costs no more than it produced either.
fn read_inflated(mut decoder: impl Read, raw_len: u64) -> Result<ScratchBytes> {
    let mismatch = || SnapshotCodecError::Decode("snapshot frame length mismatch".into());
    let read_error = |e: std::io::Error| SnapshotCodecError::Decode(format!("snapshot frame: {e}"));
    let len = usize::try_from(raw_len).unwrap_or(usize::MAX);
    #[cfg(target_os = "macos")]
    let exact = (ScratchBytes::MAPPED_MIN..=MAX_PREALLOC).contains(&len);
    #[cfg(not(target_os = "macos"))]
    let exact = false;
    if exact {
        let mut raw = ScratchBytes::zeroed(len).ok_or_else(|| {
            SnapshotCodecError::Decode(format!("cannot allocate a {len}-byte snapshot frame"))
        })?;
        let mut filled = 0;
        while filled < len {
            match decoder.read(&mut raw[filled..]) {
                Ok(0) => return Err(mismatch()),
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(read_error(e)),
            }
        }
        let mut probe = [0u8; 1];
        loop {
            match decoder.read(&mut probe) {
                Ok(0) => return Ok(raw),
                Ok(_) => return Err(mismatch()),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(read_error(e)),
            }
        }
    }
    let mut raw = Vec::with_capacity(
        usize::try_from(raw_len)
            .unwrap_or(usize::MAX)
            .min(MAX_PREALLOC),
    );
    decoder
        .take(raw_len.saturating_add(1))
        .read_to_end(&mut raw)
        .map_err(read_error)?;
    if raw.len() as u64 != raw_len {
        return Err(mismatch());
    }
    Ok(ScratchBytes::from_vec(raw))
}

/// A compressed frame holds one compressed stream and nothing after it, so
/// every decoder accepts and refuses the same frames.
fn expect_consumed(rest: &[u8]) -> Result<()> {
    if rest.is_empty() {
        Ok(())
    } else {
        Err(SnapshotCodecError::Decode(
            "snapshot frame has bytes after its compressed data".into(),
        ))
    }
}

/// A zstd frame inflated by the pure-Rust decoder, where the C library is
/// not built (wasm).
#[cfg(any(target_arch = "wasm32", test))]
fn inflate_zstd_portable(frame: &StoredFrame) -> Result<ScratchBytes> {
    let mut stored = frame.stored.as_slice();
    let decoder = ruzstd::streaming_decoder::StreamingDecoder::new(&mut stored)
        .map_err(|e| SnapshotCodecError::Decode(format!("snapshot frame: {e}")))?;
    let raw = read_inflated(decoder, frame.raw_len)?;
    expect_consumed(stored)?;
    Ok(raw)
}

fn decode_frame(
    encoder: &SnapshotEncoder,
    frame: StoredFrame,
    compression: Compression,
) -> Result<Decoded> {
    let inflated;
    let raw: &[u8] = match compression {
        Compression::None if frame.stored.len() as u64 == frame.raw_len => &frame.stored,
        _ => {
            inflated = inflate(&frame, compression)?;
            &inflated
        }
    };
    let mut cursor = Cursor::new(raw);
    // Each record takes at least a byte.
    if frame.records > raw.len() as u64 {
        return Err(truncated_frame());
    }
    let records = frame.records as usize;
    let mut properties = Vec::new();
    let mut last = None;
    let decoded = match frame.kind {
        FRAME_NODES => {
            let mut batch = encoder.node_batch(records);
            let mut labels = Vec::new();
            for _ in 0..records {
                let id = cursor.id(last)?;
                last = Some(id);
                labels.clear();
                for _ in 0..cursor.count()? {
                    labels.push(cursor.number()?);
                }
                cursor.properties(&mut properties)?;
                encoder
                    .push_node(&mut batch, id, &labels, &mut properties)
                    .map_err(store_error)?;
            }
            Decoded::Nodes(batch)
        }
        _ => {
            let mut batch = encoder.relationship_batch(records);
            for _ in 0..records {
                let id = cursor.id(last)?;
                last = Some(id);
                let src = cursor.varint()?;
                let dst = cursor.varint()?;
                let rel_type = cursor.number()?;
                cursor.properties(&mut properties)?;
                encoder
                    .push_relationship(&mut batch, id, src, dst, rel_type, &mut properties)
                    .map_err(store_error)?;
            }
            Decoded::Relationships(batch)
        }
    };
    if cursor.remaining() != 0 {
        return Err(SnapshotCodecError::Decode(
            "trailing bytes in snapshot frame".into(),
        ));
    }
    Ok(decoded)
}

fn read_names<R: Read>(body: &mut BodyStream<R>) -> Result<SnapshotNames> {
    let labels: Vec<Name> = body.string_vec()?.iter().map(Name::from).collect();
    let types: Vec<Name> = body.string_vec()?.iter().map(Name::from).collect();
    let keys: Vec<Name> = body
        .string_vec()?
        .into_iter()
        .map(|key| Name::from_arc(lora_store::intern_owned(key)))
        .collect();
    let strings: Vec<Arc<str>> = body.string_vec()?.into_iter().map(Arc::from).collect();
    let count = body.len()?;
    let mut value_keys = Vec::with_capacity(count.min(keys.len()));
    for _ in 0..count {
        let key = keys.get(body.u32()? as usize).cloned().ok_or_else(|| {
            SnapshotCodecError::Decode("value dictionary names an unknown key".into())
        })?;
        let state = body.u8()?;
        let misses = u16::from_le_bytes(body.array()?);
        let state = match state {
            VALUE_KEY_NUMBERED => ValueKeyState::Numbered { misses },
            VALUE_KEY_STORED => ValueKeyState::Stored,
            other => {
                return Err(SnapshotCodecError::Decode(format!(
                    "invalid value dictionary key state {other}"
                )));
            }
        };
        value_keys.push((key, state));
    }
    Ok(SnapshotNames {
        labels,
        types,
        keys,
        values: ValueDictionary {
            strings,
            keys: value_keys,
        },
    })
}

/// What [`read_round`] read: record frames, and whether the body ended.
struct Round {
    frames: Vec<StoredFrame>,
    catalog: Option<Vec<u8>>,
    ended: bool,
}

/// Read up to `limit` record frames, stopping early at the catalog frame
/// (which is read too) or the end. `last_kind` enforces the order: node
/// frames, then relationship frames, then the catalog.
fn read_round<R: Read>(
    body: &mut BodyStream<R>,
    limit: usize,
    last_kind: &mut u8,
    compression: Compression,
) -> Result<Round> {
    let mut round = Round {
        frames: Vec::new(),
        catalog: None,
        ended: false,
    };
    let mut raw_bytes = 0u64;
    while round.frames.len() < limit && raw_bytes < ROUND_RAW_BUDGET {
        let kind = body.u8()?;
        if kind == FRAME_END {
            if *last_kind != FRAME_CATALOG {
                return Err(SnapshotCodecError::Decode(
                    "snapshot body ends without its catalog".into(),
                ));
            }
            round.ended = true;
            break;
        }
        if !(FRAME_NODES..=FRAME_CATALOG).contains(&kind) {
            return Err(SnapshotCodecError::Decode(format!(
                "invalid snapshot frame kind {kind}"
            )));
        }
        if kind < *last_kind || *last_kind == FRAME_CATALOG {
            return Err(SnapshotCodecError::Decode(
                "snapshot frames are out of order".into(),
            ));
        }
        *last_kind = kind;
        let records = body.u64()?;
        let raw_len = body.u64()?;
        if raw_len > MAX_FRAME_RAW {
            return Err(SnapshotCodecError::Decode(format!(
                "snapshot frame claims {raw_len} bytes, more than {MAX_FRAME_RAW}"
            )));
        }
        raw_bytes += raw_len;
        let stored = body.bytes()?;
        let frame = StoredFrame {
            kind,
            records,
            raw_len,
            stored,
        };
        if kind == FRAME_CATALOG {
            round.catalog = Some(inflate(&frame, compression)?.to_vec());
            continue;
        }
        round.frames.push(frame);
    }
    Ok(round)
}

/// File a round's decoded frames in order. The relationship frames go
/// together, so their adjacency is linked in one parallel pass.
fn install(builder: &mut SnapshotGraphBuilder, decoded: Vec<Result<Decoded>>) -> Result<()> {
    let mut relationships = Vec::new();
    for batch in decoded {
        match batch? {
            Decoded::Nodes(batch) => {
                // Node frames come before every relationship frame.
                debug_assert!(relationships.is_empty());
                builder.push_nodes(batch).map_err(store_error)?;
            }
            Decoded::Relationships(batch) => relationships.push(batch),
        }
    }
    if !relationships.is_empty() {
        builder
            .push_relationship_batches(relationships)
            .map_err(store_error)?;
    }
    Ok(())
}

/// Read a framed body, after its version, into a graph builder. The
/// frames of one round decode on the thread pool while this thread files
/// the round before and reads the round after.
pub(crate) fn read_chunked_body<R: Read>(
    body: &mut BodyStream<R>,
    compression: Compression,
) -> Result<LoadedBody> {
    let version = body.u32()?;
    if version != CHUNKED_BODY_VERSION {
        return Err(SnapshotCodecError::Decode(format!(
            "unsupported framed snapshot body version {version}"
        )));
    }
    let next_node_id = body.u64()?;
    let next_rel_id = body.u64()?;
    let mut builder = SnapshotGraphBuilder::new(next_node_id, next_rel_id).map_err(store_error)?;
    let names = read_names(body)?;
    let encoder = builder.prime(&names).map_err(store_error)?;
    drop(names);

    let limit = frames_per_round();
    let mut last_kind = FRAME_NODES;
    let decode = |frames: Vec<StoredFrame>| {
        map_all(frames, |frame| decode_frame(&encoder, frame, compression))
    };

    let mut catalog = None;
    let first = read_round(body, limit, &mut last_kind, compression)?;
    let mut ended = first.ended;
    catalog = first.catalog.or(catalog);
    let mut ready = decode(first.frames);
    let mut queued = if ended {
        Vec::new()
    } else {
        let round = read_round(body, limit, &mut last_kind, compression)?;
        ended = round.ended;
        catalog = round.catalog.or(catalog);
        round.frames
    };
    loop {
        let idle = queued.is_empty();
        let (decoded, (installed, round)) = overlap(
            || decode(queued),
            || {
                let installed = install(&mut builder, std::mem::take(&mut ready));
                let round = (!ended && installed.is_ok())
                    .then(|| read_round(body, limit, &mut last_kind, compression));
                (installed, round)
            },
        );
        installed?;
        ready = decoded;
        queued = match round.transpose()? {
            Some(round) => {
                ended = round.ended;
                catalog = round.catalog.or(catalog);
                round.frames
            }
            None => Vec::new(),
        };
        if idle && ready.is_empty() {
            break;
        }
    }
    if !ended {
        return Err(SnapshotCodecError::Decode(
            "snapshot body ends without its end frame".into(),
        ));
    }
    drop(encoder);

    let catalog = catalog.ok_or_else(|| {
        SnapshotCodecError::Decode("snapshot body ends without its catalog".into())
    })?;
    let mut trailer = BodyStream::new(catalog.as_slice());
    let indexes = decode_index_definitions(&trailer.bytes()?)
        .map_err(|e| SnapshotCodecError::Decode(format!("catalog: {e}")))?;
    let constraints = decode_constraint_definitions(&trailer.bytes()?)
        .map_err(|e| SnapshotCodecError::Decode(format!("constraint catalog: {e}")))?;
    let vector_bytes = trailer.bytes()?;
    trailer.expect_end()?;
    let vector_indexes = if vector_bytes.is_empty() {
        Vec::new()
    } else {
        serde_json::from_slice(&vector_bytes)
            .map_err(|e| SnapshotCodecError::Decode(format!("vector index trailer: {e}")))?
    };
    Ok(LoadedBody {
        builder,
        indexes,
        constraints,
        vector_indexes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of `raw`, stored with `compression`.
    fn frame(raw: &[u8], compression: Compression) -> StoredFrame {
        let (raw_len, stored) = compress_frame(raw.to_vec(), compression).unwrap();
        StoredFrame {
            kind: FRAME_NODES,
            records: 1,
            raw_len,
            stored,
        }
    }

    fn sample() -> Vec<u8> {
        (0..200_000u32)
            .flat_map(|i| (i % 977).to_le_bytes())
            .collect()
    }

    #[test]
    fn every_codec_round_trips_a_frame() {
        let raw = sample();
        for compression in [
            Compression::None,
            Compression::Gzip { level: 1 },
            Compression::Zstd { level: 3 },
        ] {
            let frame = frame(&raw, compression);
            assert_eq!(
                &inflate(&frame, compression).unwrap()[..],
                raw,
                "{compression:?}"
            );
        }
    }

    /// What wasm builds run: the pure-Rust decoder reads what the C
    /// library wrote.
    #[test]
    fn the_portable_zstd_decoder_reads_the_native_encoder() {
        let raw = sample();
        let frame = frame(&raw, Compression::Zstd { level: 3 });
        assert_eq!(&inflate_zstd_portable(&frame).unwrap()[..], &raw[..]);
    }

    #[test]
    fn a_frame_longer_or_shorter_than_declared_is_refused() {
        let raw = sample();
        for compression in [
            Compression::Zstd { level: 3 },
            Compression::Gzip { level: 1 },
        ] {
            for wrong in [raw.len() as u64 - 1, raw.len() as u64 + 1] {
                let mut frame = frame(&raw, compression);
                frame.raw_len = wrong;
                assert!(
                    inflate(&frame, compression).is_err(),
                    "{compression:?} {wrong}"
                );
            }
            let mut frame = frame(&raw, compression);
            frame.stored.truncate(frame.stored.len() / 2);
            assert!(
                inflate(&frame, compression).is_err(),
                "{compression:?} truncated"
            );
        }
    }

    /// A length read from the file never sizes an allocation: a frame
    /// claiming 64 TiB fails on its real bytes instead of aborting the
    /// process.
    #[test]
    fn a_frame_claiming_a_huge_length_fails_without_reserving_it() {
        let raw = sample();
        for compression in [
            Compression::Zstd { level: 3 },
            Compression::Gzip { level: 1 },
        ] {
            let mut frame = frame(&raw, compression);
            frame.raw_len = 1 << 46;
            assert!(inflate(&frame, compression).is_err(), "{compression:?}");
        }
        let mut zstd = frame(&raw, Compression::Zstd { level: 3 });
        zstd.raw_len = 1 << 46;
        assert!(inflate_zstd_portable(&zstd).is_err());
    }

    /// Native and wasm builds accept the same frames: bytes after the
    /// compressed stream are refused by both.
    #[test]
    fn bytes_after_a_compressed_frame_are_refused_by_every_decoder() {
        let raw = sample();
        for compression in [
            Compression::Zstd { level: 3 },
            Compression::Gzip { level: 1 },
        ] {
            let mut frame = frame(&raw, compression);
            frame.stored.extend_from_slice(b"trailing");
            assert!(inflate(&frame, compression).is_err(), "{compression:?}");
        }
        let mut zstd = frame(&raw, Compression::Zstd { level: 3 });
        let clean = zstd.clone();
        zstd.stored.extend_from_slice(b"trailing");
        assert!(inflate_zstd_portable(&zstd).is_err());
        assert_eq!(&inflate_zstd_portable(&clean).unwrap()[..], &raw[..]);
    }

    /// A frame header claiming more than any writer produces is refused as
    /// it is read, before a byte of it is inflated.
    #[test]
    fn a_round_refuses_a_frame_over_the_cap() {
        let mut body = Vec::new();
        write_frame(
            &mut body,
            &Frame {
                kind: FRAME_NODES,
                records: 1,
                raw_len: MAX_FRAME_RAW + 1,
                stored: vec![0; 8],
            },
        )
        .unwrap();
        let mut stream = BodyStream::new(body.as_slice());
        let mut last_kind = FRAME_NODES;
        let err = read_round(&mut stream, 4, &mut last_kind, Compression::None)
            .err()
            .expect("an oversized frame is refused");
        assert!(err.to_string().contains("claims"), "{err}");
    }
}
