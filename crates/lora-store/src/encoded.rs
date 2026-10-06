//! Compact stored form of nodes and relationships.
//!
//! The in-memory store keeps each record as one immutable byte string
//! behind a thin, reference-counted pointer ([`Blob`]): an 8-byte slot
//! and an 8-byte header per record, where an `Arc<NodeRecord>` cost a
//! 16-byte header, a 56-byte struct and 64 bytes per property before the
//! value's own heap. Readers get views over the bytes
//! ([`crate::NodeRef`], [`crate::RelRef`]); nothing is decoded until it
//! is asked for.
//!
//! Layout (integers are LEB128 varints unless noted):
//!
//! ```text
//! node:         label count, label ids...,  properties
//! relationship: source id, target id, type id,  properties
//! properties:   count, then per property in key-name order:
//!                 key id, tag byte, payload
//! payload:      null / false / true   nothing
//!               int                   zigzag varint
//!               float                 8 bytes, little-endian
//!               string                length, UTF-8 bytes
//!               other                 length, `codec` bytes
//! ```
//!
//! Label, type and key ids come from the graph's [`Dicts`]. A record's
//! own id is its slot index and is not stored.

use std::alloc::{self, Layout};
use std::ptr::NonNull;
use std::sync::atomic::{self, AtomicU32, Ordering};

use crate::dict::{Dicts, NameDict};
use crate::{
    Labels, NodeId, NodeRecord, Properties, PropertyValue, RelationshipId, RelationshipRecord,
};

// ---------------------------------------------------------------------------
// Blob: a thin, shared, immutable byte string
// ---------------------------------------------------------------------------

#[repr(C)]
struct Header {
    refs: AtomicU32,
    /// Byte length, or [`WIDE`] when a `u64` length follows the header.
    len: u32,
}

/// `Header::len` value marking a byte string too long for a `u32`.
const WIDE: u32 = u32::MAX;
const HEADER: usize = std::mem::size_of::<Header>();
const ALIGN: usize = 8;
const _: () = assert!(HEADER == 8 && std::mem::align_of::<Header>() <= ALIGN);

/// An immutable byte string behind one pointer. Cloning bumps a count.
pub(crate) struct Blob {
    ptr: NonNull<Header>,
}

// SAFETY: the bytes are immutable after construction and the count is
// atomic, as with `Arc<[u8]>`.
unsafe impl Send for Blob {}
unsafe impl Sync for Blob {}

impl Blob {
    pub(crate) fn new(bytes: &[u8]) -> Self {
        Self::with_width(bytes, bytes.len() >= WIDE as usize)
    }

    fn with_width(bytes: &[u8], wide: bool) -> Self {
        let prefix = if wide { HEADER + 8 } else { HEADER };
        let layout = Self::layout(prefix + bytes.len());
        // SAFETY: the layout has a non-zero size (it includes the header).
        let raw = unsafe { alloc::alloc(layout) };
        let Some(ptr) = NonNull::new(raw.cast::<Header>()) else {
            alloc::handle_alloc_error(layout)
        };
        // SAFETY: `raw` is valid for `prefix + bytes.len()` bytes and
        // aligned for `Header` and `u64`.
        unsafe {
            ptr.as_ptr().write(Header {
                refs: AtomicU32::new(1),
                len: if wide { WIDE } else { bytes.len() as u32 },
            });
            if wide {
                raw.add(HEADER).cast::<u64>().write(bytes.len() as u64);
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), raw.add(prefix), bytes.len());
        }
        Self { ptr }
    }

    fn layout(size: usize) -> Layout {
        Layout::from_size_align(size, ALIGN).expect("record size fits the address space")
    }

    /// `(offset of the bytes, their length)`.
    #[inline]
    fn extent(&self) -> (usize, usize) {
        // SAFETY: `ptr` points at a live header written by `with_width`.
        let len = unsafe { (*self.ptr.as_ptr()).len };
        if len != WIDE {
            (HEADER, len as usize)
        } else {
            // SAFETY: a wide blob stores its length right after the header.
            let len = unsafe {
                self.ptr
                    .as_ptr()
                    .cast::<u8>()
                    .add(HEADER)
                    .cast::<u64>()
                    .read()
            };
            (HEADER + 8, len as usize)
        }
    }

    #[inline]
    pub(crate) fn as_slice(&self) -> &[u8] {
        let (offset, len) = self.extent();
        // SAFETY: the allocation holds `len` initialized bytes at `offset`
        // and lives as long as `self`.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr().cast::<u8>().add(offset), len) }
    }

    /// Bytes this blob's allocation occupies.
    pub(crate) fn alloc_bytes(&self) -> usize {
        let (offset, len) = self.extent();
        offset + len
    }
}

impl Clone for Blob {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: `ptr` points at a live header.
        let old = unsafe { (*self.ptr.as_ptr()).refs.fetch_add(1, Ordering::Relaxed) };
        // Far more references than slots could hold them: something is
        // leaking clones. Stop before the count wraps.
        if old > u32::MAX / 2 {
            std::process::abort();
        }
        Self { ptr: self.ptr }
    }
}

impl Drop for Blob {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: `ptr` points at a live header; after the last reference
        // is released nothing else can reach the allocation.
        unsafe {
            if (*self.ptr.as_ptr()).refs.fetch_sub(1, Ordering::Release) != 1 {
                return;
            }
            atomic::fence(Ordering::Acquire);
            let (offset, len) = self.extent();
            alloc::dealloc(self.ptr.as_ptr().cast::<u8>(), Self::layout(offset + len));
        }
    }
}

impl std::fmt::Debug for Blob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Blob({} bytes)", self.as_slice().len())
    }
}

// ---------------------------------------------------------------------------
// Primitive readers and writers
// ---------------------------------------------------------------------------

const TAG_NULL: u8 = 0;
const TAG_FALSE: u8 = 1;
const TAG_TRUE: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_FLOAT: u8 = 4;
const TAG_STRING: u8 = 5;
const TAG_OTHER: u8 = 6;

#[inline]
fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Read a varint at `*pos`, advancing it. The bytes are ones this module
/// wrote, so a malformed varint is a bug, reported by the slice index.
#[inline]
fn get_varint(bytes: &[u8], pos: &mut usize) -> u64 {
    let first = bytes[*pos];
    *pos += 1;
    if first < 0x80 {
        return u64::from(first);
    }
    let mut value = u64::from(first & 0x7f);
    let mut shift = 7;
    loop {
        let byte = bytes[*pos];
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return value;
        }
        shift += 7;
    }
}

#[inline]
fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

#[inline]
fn unzigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

/// Step over one property payload.
#[inline]
fn skip_payload(bytes: &[u8], tag: u8, pos: &mut usize) {
    match tag {
        TAG_INT => {
            while bytes[*pos] >= 0x80 {
                *pos += 1;
            }
            *pos += 1;
        }
        TAG_FLOAT => *pos += 8,
        TAG_STRING | TAG_OTHER => {
            let len = get_varint(bytes, pos) as usize;
            *pos += len;
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// A stored value of a kind [`crate::ValueRef`] does not carry inline:
/// its `codec` bytes, decoded on request.
#[derive(Clone, Copy)]
pub(crate) struct EncodedOther<'a> {
    bytes: &'a [u8],
}

impl EncodedOther<'_> {
    pub(crate) fn decode(self) -> PropertyValue {
        crate::codec::decode_property_value(self.bytes)
            .expect("a stored property value decodes: it was encoded by this process")
    }
}

/// A property value as stored. `crate::types::view` turns it into a
/// public [`crate::ValueRef`].
#[derive(Clone, Copy)]
pub(crate) enum StoredValue<'a> {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(&'a str),
    Other(EncodedOther<'a>),
}

#[inline]
fn read_value<'a>(bytes: &'a [u8], tag: u8, pos: &mut usize) -> StoredValue<'a> {
    match tag {
        TAG_NULL => StoredValue::Null,
        TAG_FALSE => StoredValue::Bool(false),
        TAG_TRUE => StoredValue::Bool(true),
        TAG_INT => StoredValue::Int(unzigzag(get_varint(bytes, pos))),
        TAG_FLOAT => {
            let raw: [u8; 8] = bytes[*pos..*pos + 8].try_into().unwrap();
            *pos += 8;
            StoredValue::Float(f64::from_le_bytes(raw))
        }
        TAG_STRING => {
            let len = get_varint(bytes, pos) as usize;
            let raw = &bytes[*pos..*pos + len];
            *pos += len;
            // SAFETY: written by `write_value` from a `String`'s bytes.
            StoredValue::String(unsafe { std::str::from_utf8_unchecked(raw) })
        }
        _ => {
            let len = get_varint(bytes, pos) as usize;
            let raw = &bytes[*pos..*pos + len];
            *pos += len;
            StoredValue::Other(EncodedOther { bytes: raw })
        }
    }
}

fn write_value(out: &mut Vec<u8>, value: &PropertyValue) {
    match value {
        PropertyValue::Null => out.push(TAG_NULL),
        PropertyValue::Bool(false) => out.push(TAG_FALSE),
        PropertyValue::Bool(true) => out.push(TAG_TRUE),
        PropertyValue::Int(v) => {
            out.push(TAG_INT);
            put_varint(out, zigzag(*v));
        }
        PropertyValue::Float(v) => {
            out.push(TAG_FLOAT);
            out.extend_from_slice(&v.to_le_bytes());
        }
        PropertyValue::String(v) => {
            out.push(TAG_STRING);
            put_varint(out, v.len() as u64);
            out.extend_from_slice(v.as_bytes());
        }
        other => {
            let encoded = crate::codec::encode_property_value(other)
                .expect("a property value encodes: lengths fit in u64");
            out.push(TAG_OTHER);
            put_varint(out, encoded.len() as u64);
            out.extend_from_slice(&encoded);
        }
    }
}

impl StoredValue<'_> {
    pub(crate) fn to_owned(self) -> PropertyValue {
        match self {
            StoredValue::Null => PropertyValue::Null,
            StoredValue::Bool(v) => PropertyValue::Bool(v),
            StoredValue::Int(v) => PropertyValue::Int(v),
            StoredValue::Float(v) => PropertyValue::Float(v),
            StoredValue::String(v) => PropertyValue::String(v.to_owned()),
            StoredValue::Other(v) => v.decode(),
        }
    }
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

/// The encoded properties of one record, with the dictionary that names
/// their keys.
#[derive(Clone, Copy)]
pub(crate) struct StoredProps<'a> {
    /// From the property count to the end of the record.
    bytes: &'a [u8],
    keys: &'a NameDict,
}

impl<'a> StoredProps<'a> {
    #[inline]
    pub(crate) fn len(self) -> usize {
        let mut pos = 0;
        get_varint(self.bytes, &mut pos) as usize
    }

    #[inline]
    pub(crate) fn get(self, key: &str) -> Option<StoredValue<'a>> {
        // A key no record ever had is not in the dictionary.
        let wanted = u64::from(self.keys.id_of(key)?);
        let bytes = self.bytes;
        let mut pos = 0;
        let count = get_varint(bytes, &mut pos);
        for _ in 0..count {
            let id = get_varint(bytes, &mut pos);
            let tag = bytes[pos];
            pos += 1;
            if id == wanted {
                return Some(read_value(bytes, tag, &mut pos));
            }
            skip_payload(bytes, tag, &mut pos);
        }
        None
    }

    #[inline]
    pub(crate) fn iter(self) -> StoredPropsIter<'a> {
        let mut pos = 0;
        let remaining = get_varint(self.bytes, &mut pos) as usize;
        StoredPropsIter {
            bytes: self.bytes,
            keys: self.keys,
            pos,
            remaining,
        }
    }

    pub(crate) fn to_owned(self) -> Properties {
        let mut out = Properties::with_capacity(self.len());
        // Stored in key order, so each insert appends.
        for (key, value) in self.iter() {
            out.insert(key.as_arc().clone(), value.to_owned());
        }
        out
    }
}

pub(crate) struct StoredPropsIter<'a> {
    bytes: &'a [u8],
    keys: &'a NameDict,
    pos: usize,
    remaining: usize,
}

impl<'a> Iterator for StoredPropsIter<'a> {
    type Item = (&'a crate::Name, StoredValue<'a>);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let id = get_varint(self.bytes, &mut self.pos) as u32;
        let tag = self.bytes[self.pos];
        self.pos += 1;
        let value = read_value(self.bytes, tag, &mut self.pos);
        Some((self.keys.name(id), value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for StoredPropsIter<'_> {}

fn write_props(out: &mut Vec<u8>, properties: &Properties, keys: &mut NameDict) {
    put_varint(out, properties.len() as u64);
    // `Properties` iterates in key order, which is the stored order.
    for (key, value) in properties.iter() {
        let id = match keys.id_of(key) {
            Some(id) => id,
            None => keys.id_or_insert(&crate::Name::from_arc(key.clone())),
        };
        put_varint(out, u64::from(id));
        write_value(out, value);
    }
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

/// The encoded labels of one node.
#[derive(Clone, Copy)]
pub(crate) struct StoredLabels<'a> {
    /// The label ids, without the count.
    bytes: &'a [u8],
    count: usize,
    labels: &'a NameDict,
}

impl<'a> StoredLabels<'a> {
    #[inline]
    pub(crate) fn len(self) -> usize {
        self.count
    }

    #[inline]
    pub(crate) fn has(self, label: &str) -> bool {
        let Some(wanted) = self.labels.id_of(label) else {
            return false;
        };
        self.ids().any(|id| id == wanted)
    }

    #[inline]
    fn ids(self) -> impl Iterator<Item = u32> + Clone + 'a {
        let bytes = self.bytes;
        let mut pos = 0;
        (0..self.count).map(move |_| get_varint(bytes, &mut pos) as u32)
    }

    #[inline]
    pub(crate) fn iter(self) -> impl ExactSizeIterator<Item = &'a crate::Name> + Clone + 'a {
        let dict = self.labels;
        let bytes = self.bytes;
        let mut pos = 0;
        (0..self.count).map(move |_| dict.name(get_varint(bytes, &mut pos) as u32))
    }

    pub(crate) fn to_owned(self) -> Labels {
        self.iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// A node as stored: its bytes and the dictionaries that name its ids.
#[derive(Clone, Copy)]
pub(crate) struct StoredNode<'a> {
    pub(crate) id: NodeId,
    bytes: &'a [u8],
    dicts: &'a Dicts,
}

impl<'a> StoredNode<'a> {
    #[inline]
    pub(crate) fn new(id: NodeId, blob: &'a Blob, dicts: &'a Dicts) -> Self {
        Self {
            id,
            bytes: blob.as_slice(),
            dicts,
        }
    }

    /// `(labels, offset of the properties)`.
    #[inline]
    fn split(self) -> (StoredLabels<'a>, usize) {
        let mut pos = 0;
        let count = get_varint(self.bytes, &mut pos) as usize;
        let start = pos;
        for _ in 0..count {
            get_varint(self.bytes, &mut pos);
        }
        (
            StoredLabels {
                bytes: &self.bytes[start..pos],
                count,
                labels: &self.dicts.labels,
            },
            pos,
        )
    }

    #[inline]
    pub(crate) fn labels(self) -> StoredLabels<'a> {
        self.split().0
    }

    #[inline]
    pub(crate) fn properties(self) -> StoredProps<'a> {
        let (_, pos) = self.split();
        StoredProps {
            bytes: &self.bytes[pos..],
            keys: &self.dicts.keys,
        }
    }

    pub(crate) fn to_record(self) -> NodeRecord {
        NodeRecord {
            id: self.id,
            labels: self.labels().to_owned(),
            properties: self.properties().to_owned(),
        }
    }
}

/// A relationship as stored.
#[derive(Clone, Copy)]
pub(crate) struct StoredRel<'a> {
    pub(crate) id: RelationshipId,
    pub(crate) src: NodeId,
    pub(crate) dst: NodeId,
    type_id: u32,
    /// The properties: the rest of the record.
    props: &'a [u8],
    dicts: &'a Dicts,
}

impl<'a> StoredRel<'a> {
    #[inline]
    pub(crate) fn new(id: RelationshipId, blob: &'a Blob, dicts: &'a Dicts) -> Self {
        let bytes = blob.as_slice();
        let mut pos = 0;
        let src = get_varint(bytes, &mut pos);
        let dst = get_varint(bytes, &mut pos);
        let type_id = get_varint(bytes, &mut pos) as u32;
        Self {
            id,
            src,
            dst,
            type_id,
            props: &bytes[pos..],
            dicts,
        }
    }

    #[inline]
    pub(crate) fn rel_type(self) -> &'a crate::Name {
        self.dicts.types.name(self.type_id)
    }

    #[inline]
    pub(crate) fn properties(self) -> StoredProps<'a> {
        StoredProps {
            bytes: self.props,
            keys: &self.dicts.keys,
        }
    }

    pub(crate) fn to_record(self) -> RelationshipRecord {
        RelationshipRecord {
            id: self.id,
            src: self.src,
            dst: self.dst,
            rel_type: self.rel_type().clone(),
            properties: self.properties().to_owned(),
        }
    }
}

/// `(source, target)` of a stored relationship, without the rest.
#[inline]
pub(crate) fn rel_endpoints(blob: &Blob) -> (NodeId, NodeId) {
    let bytes = blob.as_slice();
    let mut pos = 0;
    let src = get_varint(bytes, &mut pos);
    (src, get_varint(bytes, &mut pos))
}

thread_local! {
    /// Scratch buffer for encoding: one record is built here, then copied
    /// into an exactly-sized [`Blob`].
    static SCRATCH: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn with_scratch(build: impl FnOnce(&mut Vec<u8>)) -> Blob {
    SCRATCH.with(|scratch| {
        let mut buf = scratch.borrow_mut();
        buf.clear();
        build(&mut buf);
        let blob = Blob::new(&buf);
        // Don't let one huge record pin its size in every thread.
        if buf.capacity() > 1 << 20 {
            *buf = Vec::new();
        }
        blob
    })
}

pub(crate) fn encode_node(node: &NodeRecord, dicts: &mut Dicts) -> Blob {
    with_scratch(|out| {
        put_varint(out, node.labels.len() as u64);
        for label in node.labels.iter() {
            put_varint(out, u64::from(dicts.labels.id_or_insert(label)));
        }
        write_props(out, &node.properties, &mut dicts.keys);
    })
}

pub(crate) fn encode_rel(rel: &RelationshipRecord, dicts: &mut Dicts) -> Blob {
    with_scratch(|out| {
        put_varint(out, rel.src);
        put_varint(out, rel.dst);
        put_varint(out, u64::from(dicts.types.id_or_insert(&rel.rel_type)));
        write_props(out, &rel.properties, &mut dicts.keys);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{intern, LoraDate, LoraPoint};

    fn props(pairs: &[(&str, PropertyValue)]) -> Properties {
        pairs.iter().map(|(k, v)| (intern(k), v.clone())).collect()
    }

    fn values() -> Vec<PropertyValue> {
        vec![
            PropertyValue::Null,
            PropertyValue::Bool(true),
            PropertyValue::Bool(false),
            PropertyValue::Int(0),
            PropertyValue::Int(-1),
            PropertyValue::Int(63),
            PropertyValue::Int(-64),
            PropertyValue::Int(i64::MAX),
            PropertyValue::Int(i64::MIN),
            PropertyValue::Float(0.5),
            PropertyValue::Float(-0.0),
            PropertyValue::Float(f64::INFINITY),
            PropertyValue::String(String::new()),
            PropertyValue::String("héllo wörld".into()),
            PropertyValue::String("x".repeat(300)),
            PropertyValue::List(vec![
                PropertyValue::Int(1),
                PropertyValue::String("two".into()),
                PropertyValue::List(vec![PropertyValue::Null]),
            ]),
            PropertyValue::Map(
                [("k".to_string(), PropertyValue::Float(1.5))]
                    .into_iter()
                    .collect(),
            ),
            PropertyValue::Date(LoraDate {
                year: 2026,
                month: 10,
                day: 6,
            }),
            PropertyValue::Point(LoraPoint {
                x: 4.9,
                y: 52.3,
                z: None,
                srid: crate::SRID_WGS84,
            }),
        ]
    }

    #[test]
    fn blobs_hold_their_bytes_and_free_on_the_last_drop() {
        for len in [0usize, 1, 7, 8, 9, 4096] {
            let bytes: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let blob = Blob::new(&bytes);
            let copy = blob.clone();
            assert_eq!(blob.as_slice(), bytes);
            drop(blob);
            assert_eq!(copy.as_slice(), bytes);
            assert_eq!(copy.alloc_bytes(), 8 + len);
        }
        assert_eq!(std::mem::size_of::<Option<Blob>>(), 8);
    }

    #[test]
    fn a_wide_blob_reads_like_a_narrow_one() {
        let bytes: Vec<u8> = (0..100u8).collect();
        let blob = Blob::with_width(&bytes, true);
        let copy = blob.clone();
        assert_eq!(blob.as_slice(), bytes);
        assert_eq!(blob.alloc_bytes(), 16 + 100);
        drop(blob);
        assert_eq!(copy.as_slice(), bytes);
    }

    #[test]
    fn varints_and_zigzag_round_trip() {
        for value in [0u64, 1, 127, 128, 16_383, 16_384, u32::MAX as u64, u64::MAX] {
            let mut out = Vec::new();
            put_varint(&mut out, value);
            let mut pos = 0;
            assert_eq!(get_varint(&out, &mut pos), value);
            assert_eq!(pos, out.len());
        }
        for value in [0i64, 1, -1, 63, -64, 64, i64::MAX, i64::MIN] {
            assert_eq!(unzigzag(zigzag(value)), value);
        }
    }

    #[test]
    fn a_node_round_trips_and_reads_in_place() {
        let mut dicts = Dicts::default();
        // More keys than the dictionary's linear-lookup limit.
        let all = values();
        let pairs: Vec<(String, PropertyValue)> = all
            .iter()
            .enumerate()
            .map(|(i, v)| (format!("key{i:02}"), v.clone()))
            .collect();
        let node = NodeRecord {
            id: 42,
            labels: ["Person", "Admin"].into(),
            properties: pairs.iter().map(|(k, v)| (intern(k), v.clone())).collect(),
        };
        let blob = encode_node(&node, &mut dicts);
        let stored = StoredNode::new(42, &blob, &dicts);

        assert_eq!(stored.to_record(), node);
        assert_eq!(stored.labels().len(), 2);
        assert!(stored.labels().has("Admin") && !stored.labels().has("Nope"));
        let props = stored.properties();
        assert_eq!(props.len(), pairs.len());
        for (key, value) in &pairs {
            assert_eq!(&props.get(key).unwrap().to_owned(), value, "{key}");
        }
        assert!(props.get("missing").is_none());
        let keys: Vec<&str> = props.iter().map(|(k, _)| k.as_str()).collect();
        let expected: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn keys_are_stored_in_name_order_whatever_their_numbers() {
        let mut dicts = Dicts::default();
        // `z` is numbered before `a`.
        let first = NodeRecord {
            id: 0,
            labels: Labels::new(),
            properties: props(&[("z", PropertyValue::Int(1))]),
        };
        encode_node(&first, &mut dicts);
        let second = NodeRecord {
            id: 1,
            labels: Labels::new(),
            properties: props(&[("z", PropertyValue::Int(2)), ("a", PropertyValue::Int(3))]),
        };
        let blob = encode_node(&second, &mut dicts);
        let stored = StoredNode::new(1, &blob, &dicts);
        let keys: Vec<&str> = stored
            .properties()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, ["a", "z"]);
        assert_eq!(stored.to_record(), second);
    }

    #[test]
    fn a_relationship_round_trips() {
        let mut dicts = Dicts::default();
        for (src, dst) in [(0u64, 0u64), (1, 300), (u64::MAX, 1 << 40)] {
            let rel = RelationshipRecord {
                id: 9,
                src,
                dst,
                rel_type: "KNOWS".into(),
                properties: props(&[("since", PropertyValue::Int(2020))]),
            };
            let blob = encode_rel(&rel, &mut dicts);
            assert_eq!(rel_endpoints(&blob), (src, dst));
            let stored = StoredRel::new(9, &blob, &dicts);
            assert_eq!(stored.rel_type().as_str(), "KNOWS");
            assert_eq!(stored.to_record(), rel);
        }
    }

    #[test]
    fn a_small_relationship_is_a_few_bytes() {
        let mut dicts = Dicts::default();
        let rel = RelationshipRecord {
            id: 0,
            src: 1_000_000,
            dst: 1_999_999,
            rel_type: "KNOWS".into(),
            properties: props(&[("w", PropertyValue::Int(7))]),
        };
        // 3 + 3 (endpoints) + 1 (type) + 1 (count) + 1 + 1 + 1 (property).
        assert_eq!(encode_rel(&rel, &mut dicts).as_slice().len(), 11);
    }
}
