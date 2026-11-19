//! Compact stored form of nodes and relationships.
//!
//! The in-memory store keeps each record as one byte string, packed with
//! the records of neighbouring ids into a shared buffer (see
//! `memory::record_slab`), where an `Arc<NodeRecord>` cost a 16-byte
//! header, a 56-byte struct and 64 bytes per property before the value's
//! own heap. Readers get views over the bytes ([`crate::NodeRef`],
//! [`crate::RelRef`]); nothing is decoded until it is asked for.
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
//!               numbered string       its number in the value dictionary
//!               other                 length, `codec` bytes
//! ```
//!
//! Label, type and key ids come from the graph's [`Dicts`], and so do the
//! numbers of short string values a key repeats (see
//! [`crate::dict::ValueDict`]). A record's own id is its slot index and
//! is not stored.

use crate::dict::{Dicts, NameDict, ValueDict};
use crate::varint;
use crate::{
    Labels, NodeId, NodeRecord, OtherValue, Properties, PropertyValue, RelationshipId,
    RelationshipRecord, ValueRef,
};

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
const TAG_NUMBERED: u8 = 7;
/// A temporal value in [`temporal`]'s compact form, behind its length.
const TAG_TEMPORAL: u8 = 8;

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
        TAG_INT | TAG_NUMBERED => {
            while bytes[*pos] >= 0x80 {
                *pos += 1;
            }
            *pos += 1;
        }
        TAG_FLOAT => *pos += 8,
        TAG_NULL | TAG_FALSE | TAG_TRUE => {}
        // TAG_STRING, TAG_OTHER and TAG_TEMPORAL, as `read_value` reads
        // them.
        _ => {
            let len = varint::read(bytes, pos) as usize;
            *pos += len;
        }
    }
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// A stored value of a kind [`crate::ValueRef`] does not carry inline:
/// its bytes, decoded on request.
#[derive(Clone, Copy)]
pub(crate) struct EncodedOther<'a> {
    bytes: &'a [u8],
    /// [`temporal`]'s form rather than `codec`'s.
    temporal: bool,
}

impl EncodedOther<'_> {
    pub(crate) fn decode(self) -> PropertyValue {
        if self.temporal {
            return temporal::read(self.bytes)
                .expect("a stored temporal value decodes: it was encoded by this process");
        }
        crate::codec::decode_stored_property_value(self.bytes)
            .expect("a stored property value decodes: it was encoded by this process")
    }
}

#[inline]
fn read_value<'a>(
    bytes: &'a [u8],
    tag: u8,
    pos: &mut usize,
    values: &'a ValueDict,
) -> ValueRef<'a> {
    match tag {
        TAG_NUMBERED => ValueRef::String(values.string(varint::read(bytes, pos) as u32)),
        TAG_NULL => ValueRef::Null,
        TAG_FALSE => ValueRef::Bool(false),
        TAG_TRUE => ValueRef::Bool(true),
        TAG_INT => ValueRef::Int(unzigzag(varint::read(bytes, pos))),
        TAG_FLOAT => {
            let raw: [u8; 8] = bytes[*pos..*pos + 8].try_into().unwrap();
            *pos += 8;
            ValueRef::Float(f64::from_le_bytes(raw))
        }
        TAG_STRING => {
            let len = varint::read(bytes, pos) as usize;
            let raw = &bytes[*pos..*pos + len];
            *pos += len;
            // SAFETY: written by `write_value` from a `String`'s bytes.
            ValueRef::String(unsafe { std::str::from_utf8_unchecked(raw) })
        }
        _ => {
            let len = varint::read(bytes, pos) as usize;
            let raw = &bytes[*pos..*pos + len];
            *pos += len;
            ValueRef::Other(OtherValue::encoded(EncodedOther {
                bytes: raw,
                temporal: tag == TAG_TEMPORAL,
            }))
        }
    }
}

/// Write `value`, the value of key `key`, numbering it in `values` when
/// its key repeats a few short strings.
fn write_value(out: &mut Vec<u8>, key: u32, value: &PropertyValue, values: &mut ValueDict) {
    write_value_numbered(out, value, |v| values.number(key, v));
}

/// [`write_value`] with the numbering of a short string left to `number`.
fn write_value_numbered(
    out: &mut Vec<u8>,
    value: &PropertyValue,
    number: impl FnOnce(&str) -> Option<u32>,
) {
    match value {
        PropertyValue::String(v) => match number(v) {
            Some(id) => {
                out.push(TAG_NUMBERED);
                varint::push(out, u64::from(id));
            }
            None => {
                out.push(TAG_STRING);
                varint::push(out, v.len() as u64);
                out.extend_from_slice(v.as_bytes());
            }
        },
        PropertyValue::Null => out.push(TAG_NULL),
        PropertyValue::Bool(false) => out.push(TAG_FALSE),
        PropertyValue::Bool(true) => out.push(TAG_TRUE),
        PropertyValue::Int(v) => {
            out.push(TAG_INT);
            varint::push(out, zigzag(*v));
        }
        PropertyValue::Float(v) => {
            out.push(TAG_FLOAT);
            out.extend_from_slice(&v.to_le_bytes());
        }
        other => {
            if let Some(compact) = temporal::write(other) {
                debug_assert_eq!(temporal::read(&compact).as_ref(), Some(other));
                out.push(TAG_TEMPORAL);
                varint::push(out, compact.len() as u64);
                out.extend_from_slice(&compact);
                return;
            }
            let encoded = crate::codec::encode_stored_property_value(other)
                .expect("a property value encodes: lengths fit in u64");
            // Reads decode these bytes and treat a failure as a bug, so
            // every debug build checks each value on its way in.
            debug_assert!(
                !vectors_consistent(other)
                    || crate::codec::decode_stored_property_value(&encoded).as_ref() == Ok(other),
                "the codec does not round-trip {other:?}"
            );
            out.push(TAG_OTHER);
            varint::push(out, encoded.len() as u64);
            out.extend_from_slice(&encoded);
        }
    }
}

/// Temporal values in records, in a form private to this process (a zone
/// is its handle): the `codec` form spends four bytes on every field, 33
/// on a date-time, which made a `DATETIME` property cost more than the
/// same instant as a string. Here a date-time takes 9 to 13 bytes.
///
/// ```text
/// kind byte, then by kind (varints; zz = zigzag):
///   date            zz year, month * 32 + day
///   local time      second of day, fraction
///   time            second of day, fraction, zz offset
///   local datetime  zz year, month * 32 + day, second of day, fraction
///   datetime        ... as local datetime, zz offset
///   zoned datetime  ... as datetime, zone handle
///   duration        zz months, zz days, zz seconds, zz nanoseconds
/// fraction: millis * 2 when whole milliseconds, else nanos * 2 + 1
/// ```
///
/// A value with a field outside its calendar range (which no constructor
/// makes) keeps the `codec` form, so every value round-trips.
mod temporal {
    use super::{unzigzag, zigzag};
    use crate::varint;
    use crate::{
        LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraTime,
        PropertyValue, ZoneId,
    };

    const DATE: u8 = 0;
    const LOCAL_TIME: u8 = 1;
    const TIME: u8 = 2;
    const LOCAL_DATE_TIME: u8 = 3;
    const DATE_TIME: u8 = 4;
    const ZONED_DATE_TIME: u8 = 5;
    const DURATION: u8 = 6;

    pub(super) fn write(value: &PropertyValue) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(16);
        match value {
            PropertyValue::Date(d) => {
                out.push(DATE);
                date(&mut out, d.year, d.month, d.day)?;
            }
            PropertyValue::LocalTime(t) => {
                out.push(LOCAL_TIME);
                time(&mut out, t.hour, t.minute, t.second, t.nanosecond)?;
            }
            PropertyValue::Time(t) => {
                out.push(TIME);
                time(&mut out, t.hour, t.minute, t.second, t.nanosecond)?;
                varint::push(&mut out, zigzag(i64::from(t.offset_seconds)));
            }
            PropertyValue::LocalDateTime(t) => {
                out.push(LOCAL_DATE_TIME);
                date(&mut out, t.year, t.month, t.day)?;
                time(&mut out, t.hour, t.minute, t.second, t.nanosecond)?;
            }
            PropertyValue::DateTime(t) => {
                out.push(if t.zone.is_some() {
                    ZONED_DATE_TIME
                } else {
                    DATE_TIME
                });
                date(&mut out, t.year, t.month, t.day)?;
                time(&mut out, t.hour, t.minute, t.second, t.nanosecond)?;
                varint::push(&mut out, zigzag(i64::from(t.offset_seconds)));
                if let Some(zone) = t.zone {
                    varint::push(&mut out, u64::from(zone.handle()));
                }
            }
            PropertyValue::Duration(d) => {
                out.push(DURATION);
                for field in [d.months, d.days, d.seconds, d.nanoseconds] {
                    varint::push(&mut out, zigzag(field));
                }
            }
            _ => return None,
        }
        Some(out)
    }

    fn date(out: &mut Vec<u8>, year: i32, month: u32, day: u32) -> Option<()> {
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        varint::push(out, zigzag(i64::from(year)));
        varint::push(out, u64::from(month * 32 + day));
        Some(())
    }

    fn time(out: &mut Vec<u8>, hour: u32, minute: u32, second: u32, nanos: u32) -> Option<()> {
        if hour > 23 || minute > 59 || second > 59 || nanos > 999_999_999 {
            return None;
        }
        varint::push(out, u64::from(hour * 3600 + minute * 60 + second));
        let fraction = if nanos.is_multiple_of(1_000_000) {
            u64::from(nanos / 1_000_000) * 2
        } else {
            u64::from(nanos) * 2 + 1
        };
        varint::push(out, fraction);
        Some(())
    }

    /// The value [`write`] encoded, or `None` on bytes it did not write.
    pub(super) fn read(bytes: &[u8]) -> Option<PropertyValue> {
        let mut r = Reader { bytes, pos: 1 };
        let value = match *bytes.first()? {
            DATE => {
                let (year, month, day) = r.date()?;
                PropertyValue::Date(LoraDate { year, month, day })
            }
            LOCAL_TIME => {
                let (hour, minute, second, nanosecond) = r.time()?;
                PropertyValue::LocalTime(LoraLocalTime {
                    hour,
                    minute,
                    second,
                    nanosecond,
                })
            }
            TIME => {
                let (hour, minute, second, nanosecond) = r.time()?;
                PropertyValue::Time(LoraTime {
                    hour,
                    minute,
                    second,
                    nanosecond,
                    offset_seconds: r.i32()?,
                })
            }
            LOCAL_DATE_TIME => {
                let (year, month, day) = r.date()?;
                let (hour, minute, second, nanosecond) = r.time()?;
                PropertyValue::LocalDateTime(LoraLocalDateTime {
                    year,
                    month,
                    day,
                    hour,
                    minute,
                    second,
                    nanosecond,
                })
            }
            kind @ (DATE_TIME | ZONED_DATE_TIME) => {
                let (year, month, day) = r.date()?;
                let (hour, minute, second, nanosecond) = r.time()?;
                let offset_seconds = r.i32()?;
                let zone = if kind == ZONED_DATE_TIME {
                    Some(ZoneId::from_handle(u16::try_from(r.u64()?).ok()?)?)
                } else {
                    None
                };
                PropertyValue::DateTime(LoraDateTime {
                    year,
                    month,
                    day,
                    hour,
                    minute,
                    second,
                    nanosecond,
                    offset_seconds,
                    zone,
                })
            }
            DURATION => PropertyValue::Duration(LoraDuration {
                months: r.i64()?,
                days: r.i64()?,
                seconds: r.i64()?,
                nanoseconds: r.i64()?,
            }),
            _ => return None,
        };
        (r.pos == bytes.len()).then_some(value)
    }

    struct Reader<'a> {
        bytes: &'a [u8],
        pos: usize,
    }

    impl Reader<'_> {
        fn u64(&mut self) -> Option<u64> {
            // `varint::read` indexes past the end on a truncated varint.
            let last = self.bytes[self.pos..].iter().position(|b| b & 0x80 == 0)?;
            (last < 10).then(|| varint::read(self.bytes, &mut self.pos))
        }

        fn i64(&mut self) -> Option<i64> {
            self.u64().map(unzigzag)
        }

        fn i32(&mut self) -> Option<i32> {
            i32::try_from(self.i64()?).ok()
        }

        fn date(&mut self) -> Option<(i32, u32, u32)> {
            let year = self.i32()?;
            let packed = u32::try_from(self.u64()?).ok()?;
            Some((year, packed / 32, packed % 32))
        }

        fn time(&mut self) -> Option<(u32, u32, u32, u32)> {
            let second_of_day = u32::try_from(self.u64()?).ok()?;
            let fraction = self.u64()?;
            let nanos = if fraction % 2 == 0 {
                fraction / 2 * 1_000_000
            } else {
                fraction / 2
            };
            Some((
                second_of_day / 3600,
                second_of_day / 60 % 60,
                second_of_day % 60,
                u32::try_from(nanos).ok()?,
            ))
        }
    }
}

/// Whether every vector in `value` has the dimension of its coordinates.
/// The codec stores the coordinates' count, so only such values round-trip
/// exactly.
fn vectors_consistent(value: &PropertyValue) -> bool {
    match value {
        PropertyValue::Vector(v) => v.dimension == v.values.len(),
        PropertyValue::List(items) => items.iter().all(vectors_consistent),
        PropertyValue::Map(entries) => entries.values().all(vectors_consistent),
        _ => true,
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
    values: &'a ValueDict,
}

impl<'a> StoredProps<'a> {
    #[inline]
    pub(crate) fn len(self) -> usize {
        let mut pos = 0;
        varint::read(self.bytes, &mut pos) as usize
    }

    #[inline]
    pub(crate) fn get(self, key: &str) -> Option<ValueRef<'a>> {
        // A key no record ever had is not in the dictionary.
        self.get_id(self.keys.id_of(key)?)
    }

    /// [`Self::get`] for a key already numbered: a scan reading one key
    /// of many records looks its number up once.
    #[inline]
    pub(crate) fn get_id(self, key: u32) -> Option<ValueRef<'a>> {
        let wanted = u64::from(key);
        let bytes = self.bytes;
        let mut pos = 0;
        let count = varint::read(bytes, &mut pos);
        for _ in 0..count {
            let id = varint::read(bytes, &mut pos);
            let tag = bytes[pos];
            pos += 1;
            if id == wanted {
                return Some(read_value(bytes, tag, &mut pos, self.values));
            }
            skip_payload(bytes, tag, &mut pos);
        }
        None
    }

    #[inline]
    pub(crate) fn iter(self) -> StoredPropsIter<'a> {
        let mut pos = 0;
        let remaining = varint::read(self.bytes, &mut pos) as usize;
        StoredPropsIter {
            bytes: self.bytes,
            keys: self.keys,
            values: self.values,
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
    values: &'a ValueDict,
    pos: usize,
    remaining: usize,
}

impl<'a> Iterator for StoredPropsIter<'a> {
    type Item = (&'a crate::Name, ValueRef<'a>);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let id = varint::read(self.bytes, &mut self.pos) as u32;
        let tag = self.bytes[self.pos];
        self.pos += 1;
        let value = read_value(self.bytes, tag, &mut self.pos, self.values);
        Some((self.keys.name(id), value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for StoredPropsIter<'_> {}

impl<'a> StoredProps<'a> {
    /// The key ids, in stored (key-name) order, stepping over the values:
    /// which keys a record holds, for a scan that only asks that.
    #[inline]
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    pub(crate) fn key_ids(self) -> impl Iterator<Item = u32> + 'a {
        let bytes = self.bytes;
        let mut pos = 0;
        let count = varint::read(bytes, &mut pos);
        (0..count).map(move |_| {
            let id = varint::read(bytes, &mut pos) as u32;
            let tag = bytes[pos];
            pos += 1;
            skip_payload(bytes, tag, &mut pos);
            id
        })
    }

    /// The properties by key id, in stored (key-name) order.
    #[inline]
    pub(crate) fn iter_ids(self) -> impl Iterator<Item = (u32, ValueRef<'a>)> + 'a {
        let bytes = self.bytes;
        let values = self.values;
        let mut pos = 0;
        let count = varint::read(bytes, &mut pos);
        (0..count).map(move |_| {
            let id = varint::read(bytes, &mut pos) as u32;
            let tag = bytes[pos];
            pos += 1;
            (id, read_value(bytes, tag, &mut pos, values))
        })
    }
}

/// Encode `properties` (key ids in key-name order, each once) as
/// [`write_props`] does, numbering only the values `dicts` already
/// numbers, so encoders on several threads can share `dicts` unchanged.
fn write_props_existing(out: &mut Vec<u8>, properties: &[(u32, PropertyValue)], dicts: &Dicts) {
    varint::push(out, properties.len() as u64);
    for (key, value) in properties {
        varint::push(out, u64::from(*key));
        write_value_numbered(out, value, |v| dicts.values.number_existing(*key, v));
    }
}

/// Encode a node from dictionary ids into `out`, replacing what it held.
/// See [`write_props_existing`] for `properties`.
pub(crate) fn encode_node_ids(
    labels: &[u32],
    properties: &[(u32, PropertyValue)],
    dicts: &Dicts,
    out: &mut Vec<u8>,
) {
    out.clear();
    varint::push(out, labels.len() as u64);
    for label in labels {
        varint::push(out, u64::from(*label));
    }
    write_props_existing(out, properties, dicts);
}

/// Encode a relationship from dictionary ids into `out`, replacing what
/// it held. See [`write_props_existing`] for `properties`.
pub(crate) fn encode_rel_ids(
    src: NodeId,
    dst: NodeId,
    type_id: u32,
    properties: &[(u32, PropertyValue)],
    dicts: &Dicts,
    out: &mut Vec<u8>,
) {
    out.clear();
    varint::push(out, src);
    varint::push(out, dst);
    varint::push(out, u64::from(type_id));
    write_props_existing(out, properties, dicts);
}

fn write_props(out: &mut Vec<u8>, properties: &Properties, dicts: &mut Dicts) {
    varint::push(out, properties.len() as u64);
    // `Properties` iterates in key order, which is the stored order.
    for (key, value) in properties.iter() {
        let id = dicts
            .keys
            .id_or_insert_with(key, || crate::Name::from_arc(key.clone()));
        varint::push(out, u64::from(id));
        write_value(out, id, value, &mut dicts.values);
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
    pub(crate) fn ids(self) -> impl Iterator<Item = u32> + Clone + 'a {
        let bytes = self.bytes;
        let mut pos = 0;
        (0..self.count).map(move |_| varint::read(bytes, &mut pos) as u32)
    }

    #[inline]
    pub(crate) fn iter(self) -> impl ExactSizeIterator<Item = &'a crate::Name> + Clone + 'a {
        let dict = self.labels;
        let bytes = self.bytes;
        let mut pos = 0;
        (0..self.count).map(move |_| dict.name(varint::read(bytes, &mut pos) as u32))
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
    pub(crate) fn new(id: NodeId, bytes: &'a [u8], dicts: &'a Dicts) -> Self {
        Self { id, bytes, dicts }
    }

    /// `(labels, offset of the properties)`.
    #[inline]
    fn split(self) -> (StoredLabels<'a>, usize) {
        let mut pos = 0;
        let count = varint::read(self.bytes, &mut pos) as usize;
        let start = pos;
        for _ in 0..count {
            varint::read(self.bytes, &mut pos);
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
            values: &self.dicts.values,
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
    pub(crate) fn new(id: RelationshipId, bytes: &'a [u8], dicts: &'a Dicts) -> Self {
        let mut pos = 0;
        let src = varint::read(bytes, &mut pos);
        let dst = varint::read(bytes, &mut pos);
        let type_id = varint::read(bytes, &mut pos) as u32;
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
    pub(crate) fn type_id(self) -> u32 {
        self.type_id
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
            values: &self.dicts.values,
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

/// A change to one property of a stored record.
pub(crate) enum PropEdit<'v> {
    Set(&'v PropertyValue),
    Remove,
}

/// Which kind of record a byte string holds: where its properties start.
#[derive(Clone, Copy)]
pub(crate) enum RecordKind {
    Node,
    Relationship,
}

/// Offset of the property block (its count) in a record.
fn props_offset(bytes: &[u8], kind: RecordKind) -> usize {
    let mut pos = 0;
    let skip = match kind {
        RecordKind::Node => varint::read(bytes, &mut pos),
        // Source, target, type.
        RecordKind::Relationship => 3,
    };
    for _ in 0..skip {
        varint::read(bytes, &mut pos);
    }
    pos
}

/// Apply `edit` to property `key` of the record `bytes`, writing the new
/// record to `out` and returning the property's previous value.
///
/// Only the edited entry is rewritten: the rest of the record is copied
/// as bytes, so the cost does not grow with what the other properties
/// hold. `None` when there is nothing to do: removing a key the record
/// does not have.
pub(crate) fn edit_property(
    bytes: &[u8],
    kind: RecordKind,
    dicts: &mut Dicts,
    key: &str,
    edit: PropEdit<'_>,
    out: &mut Vec<u8>,
) -> Option<Option<PropertyValue>> {
    let keys = &dicts.keys;
    let props_at = props_offset(bytes, kind);
    let mut pos = props_at;
    let count = varint::read(bytes, &mut pos);
    let entries_at = pos;

    // Find the entry, or where a new one goes to keep key-name order.
    let wanted = keys.id_of(key);
    let mut found: Option<(usize, usize, PropertyValue)> = None;
    let mut insert_at = None;
    for _ in 0..count {
        let start = pos;
        let id = varint::read(bytes, &mut pos) as u32;
        let tag = bytes[pos];
        pos += 1;
        if Some(id) == wanted {
            let old = read_value(bytes, tag, &mut pos, &dicts.values).to_owned();
            found = Some((start, pos, old));
            break;
        }
        if insert_at.is_none() && keys.name(id).as_str() > key {
            insert_at = Some(start);
            // A later entry cannot be `key`: entries are in name order.
            break;
        }
        skip_payload(bytes, tag, &mut pos);
    }

    let (start, end, old) = match found {
        Some((start, end, old)) => (start, end, Some(old)),
        None => {
            if matches!(edit, PropEdit::Remove) {
                return None;
            }
            // `pos` is the end of the record when no later key was met.
            let at = insert_at.unwrap_or(pos);
            (at, at, None)
        }
    };
    let new_count = match (&edit, old.is_some()) {
        (PropEdit::Set(_), true) => count,
        (PropEdit::Set(_), false) => count + 1,
        (PropEdit::Remove, _) => count - 1,
    };
    out.clear();
    {
        out.extend_from_slice(&bytes[..props_at]);
        varint::push(out, new_count);
        out.extend_from_slice(&bytes[entries_at..start]);
        if let PropEdit::Set(value) = edit {
            let id = match wanted {
                Some(id) => id,
                None => dicts
                    .keys
                    .id_or_insert_with(key, || crate::Name::from_arc(crate::intern(key))),
            };
            varint::push(out, u64::from(id));
            write_value(out, id, value, &mut dicts.values);
        }
        out.extend_from_slice(&bytes[end..]);
    }
    Some(old)
}

/// `(source, target)` of a stored relationship, without the rest.
#[inline]
pub(crate) fn rel_endpoints(bytes: &[u8]) -> (NodeId, NodeId) {
    let mut pos = 0;
    let src = varint::read(bytes, &mut pos);
    (src, varint::read(bytes, &mut pos))
}

thread_local! {
    /// Scratch buffer for encoding: one record is built here, then copied
    /// into the record store.
    static SCRATCH: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The thread's encoding buffer, empty, until dropped. Taken out of the
/// thread-local so the caller can hold it across other borrows.
pub(crate) struct Scratch(Vec<u8>);

impl Scratch {
    pub(crate) fn take() -> Self {
        let mut buf = SCRATCH.with(|scratch| std::mem::take(&mut *scratch.borrow_mut()));
        buf.clear();
        Scratch(buf)
    }
}

impl std::ops::Deref for Scratch {
    type Target = Vec<u8>;

    fn deref(&self) -> &Vec<u8> {
        &self.0
    }
}

impl std::ops::DerefMut for Scratch {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Don't let one huge record pin its size in every thread.
        if self.0.capacity() <= 1 << 20 {
            let buf = std::mem::take(&mut self.0);
            SCRATCH.with(|scratch| *scratch.borrow_mut() = buf);
        }
    }
}

/// Encode `node` into `out`, replacing what it held.
pub(crate) fn encode_node(node: &NodeRecord, dicts: &mut Dicts, out: &mut Vec<u8>) {
    out.clear();
    varint::push(out, node.labels.len() as u64);
    for label in node.labels.iter() {
        varint::push(out, u64::from(dicts.labels.id_or_insert(label)));
    }
    write_props(out, &node.properties, dicts);
}

/// Encode `rel` into `out`, replacing what it held.
pub(crate) fn encode_rel(rel: &RelationshipRecord, dicts: &mut Dicts, out: &mut Vec<u8>) {
    out.clear();
    varint::push(out, rel.src);
    varint::push(out, rel.dst);
    varint::push(out, u64::from(dicts.types.id_or_insert(&rel.rel_type)));
    write_props(out, &rel.properties, dicts);
}

/// `node` encoded into a new buffer.
#[cfg(test)]
pub(crate) fn node_bytes(node: &NodeRecord, dicts: &mut Dicts) -> Vec<u8> {
    let mut out = Vec::new();
    encode_node(node, dicts, &mut out);
    out
}

/// `rel` encoded into a new buffer.
#[cfg(test)]
pub(crate) fn rel_bytes(rel: &RelationshipRecord, dicts: &mut Dicts) -> Vec<u8> {
    let mut out = Vec::new();
    encode_rel(rel, dicts, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_node(node: &NodeRecord, dicts: &mut Dicts) -> Vec<u8> {
        node_bytes(node, dicts)
    }

    fn encode_rel(rel: &RelationshipRecord, dicts: &mut Dicts) -> Vec<u8> {
        rel_bytes(rel, dicts)
    }

    fn edit_property(
        bytes: &[u8],
        kind: RecordKind,
        dicts: &mut Dicts,
        key: &str,
        edit: PropEdit<'_>,
    ) -> Option<(Vec<u8>, Option<PropertyValue>)> {
        let mut out = Vec::new();
        let old = super::edit_property(bytes, kind, dicts, key, edit, &mut out)?;
        Some((out, old))
    }
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
        .into_iter()
        .chain(temporals())
        .collect()
    }

    /// Every temporal kind, at the edges of the compact form.
    fn temporals() -> Vec<PropertyValue> {
        let dt = |year, nanosecond, offset_seconds, zone: Option<&str>| {
            PropertyValue::DateTime(crate::LoraDateTime {
                year,
                month: 12,
                day: 31,
                hour: 23,
                minute: 59,
                second: 59,
                nanosecond,
                offset_seconds,
                zone: zone.map(|z| crate::ZoneId::lookup(z).unwrap()),
            })
        };
        vec![
            PropertyValue::Date(LoraDate {
                year: -999_999_999,
                month: 1,
                day: 1,
            }),
            dt(2026, 0, 0, None),
            dt(2026, 123_000_000, 3600, None),
            dt(-44, 1, -50_400, None),
            dt(i32::MAX, 999_999_999, 64_800, Some("Europe/Amsterdam")),
            PropertyValue::LocalDateTime(crate::LoraLocalDateTime {
                year: 1970,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                nanosecond: 500,
            }),
            PropertyValue::Time(crate::LoraTime {
                hour: 12,
                minute: 30,
                second: 5,
                nanosecond: 7_000_000,
                offset_seconds: -3600,
            }),
            PropertyValue::LocalTime(crate::LoraLocalTime {
                hour: 0,
                minute: 0,
                second: 0,
                nanosecond: 0,
            }),
            PropertyValue::Duration(crate::LoraDuration {
                months: -14,
                days: 3,
                seconds: i64::MIN,
                nanoseconds: 999_999_999,
            }),
        ]
    }

    #[test]
    fn a_temporal_is_compact_and_round_trips() {
        for value in temporals() {
            let compact = temporal::write(&value).expect("in range");
            assert_eq!(temporal::read(&compact), Some(value.clone()), "{value:?}");
            let codec = crate::codec::encode_stored_property_value(&value).unwrap();
            assert!(
                compact.len() < codec.len(),
                "{value:?}: {} >= {}",
                compact.len(),
                codec.len()
            );
        }
        // A date-time to the millisecond, the common case: 11 bytes, where
        // the codec took 33.
        let at = crate::LoraDateTime::parse("2026-11-18T12:00:00.123Z").unwrap();
        assert_eq!(
            temporal::write(&PropertyValue::DateTime(at)).unwrap().len(),
            11
        );
        // Bytes it did not write are refused, not misread.
        assert_eq!(temporal::read(&[]), None);
        assert_eq!(temporal::read(&[99]), None);
        assert_eq!(temporal::read(&[0, 0x80]), None);
    }

    #[test]
    fn a_temporal_out_of_its_calendar_keeps_the_codec_form() {
        let odd = PropertyValue::Date(LoraDate {
            year: 2026,
            month: 13,
            day: 40,
        });
        assert_eq!(temporal::write(&odd), None);
        let mut dicts = Dicts::default();
        let node = NodeRecord {
            id: 1,
            labels: Labels::new(),
            properties: props(&[("d", odd.clone())]),
        };
        let blob = encode_node(&node, &mut dicts);
        let stored = StoredNode::new(1, &blob, &dicts);
        assert_eq!(stored.properties().get("d").unwrap().to_owned(), odd);
    }

    #[test]
    fn zigzag_round_trips() {
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

    /// Editing a record in place must give the bytes a fresh encoding of
    /// the edited record would.
    #[test]
    fn editing_one_property_matches_re_encoding_the_record() {
        let all = values();
        let mut dicts = Dicts::default();
        let mut node = NodeRecord {
            id: 1,
            labels: ["A", "B"].into(),
            properties: Properties::new(),
        };
        let mut rel = RelationshipRecord {
            id: 2,
            src: 300,
            dst: 70_000,
            rel_type: "T".into(),
            properties: Properties::new(),
        };
        let mut node_blob = encode_node(&node, &mut dicts);
        let mut rel_blob = encode_rel(&rel, &mut dicts);
        // Keys arrive out of name order; each is set, overwritten with
        // another kind of value, and some removed again.
        let keys = ["m", "c", "x", "a", "q", "zz", "b"];
        let mut step = 0usize;
        let mut check = |key: &str, edit: Option<&PropertyValue>, dicts: &mut Dicts| {
            let expected_old = match edit {
                Some(value) => node.properties.insert(intern(key), value.clone()),
                None => node.properties.remove(key),
            };
            match edit {
                Some(value) => rel.properties.insert(intern(key), value.clone()),
                None => rel.properties.remove(key),
            };
            let op = || match edit {
                Some(value) => PropEdit::Set(value),
                None => PropEdit::Remove,
            };
            let edited = edit_property(&node_blob, RecordKind::Node, dicts, key, op());
            let edited_rel = edit_property(&rel_blob, RecordKind::Relationship, dicts, key, op());
            if edit.is_none() && expected_old.is_none() {
                assert!(edited.is_none() && edited_rel.is_none());
                return;
            }
            let (blob, old) = edited.unwrap();
            assert_eq!(old, expected_old, "{key}");
            assert_eq!(
                blob.as_slice(),
                encode_node(&node, dicts).as_slice(),
                "{key}"
            );
            node_blob = blob;
            let (blob, old) = edited_rel.unwrap();
            assert_eq!(old, expected_old, "{key}");
            assert_eq!(blob.as_slice(), encode_rel(&rel, dicts).as_slice(), "{key}");
            rel_blob = blob;
        };
        for round in 0..3 {
            for key in keys {
                let value = &all[step % all.len()];
                step += 1;
                check(key, Some(value), &mut dicts);
                if (step + round).is_multiple_of(3) {
                    check(key, None, &mut dicts);
                    check(key, None, &mut dicts);
                }
            }
        }
        for key in keys {
            check(key, None, &mut dicts);
        }
        assert_eq!(StoredNode::new(1, &node_blob, &dicts).to_record(), node);
        assert_eq!(StoredRel::new(2, &rel_blob, &dicts).to_record(), rel);
        assert!(node.properties.is_empty());
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
