//! Decode side of the WAL mutation payload codec.

use std::collections::BTreeMap;

use lora_store::{
    codec::{decode_constraint_request, decode_index_request},
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint, LoraTime,
    LoraVector, MutationEvent, Properties, PropertyValue, VectorValues, ZoneId,
};

use super::format::*;
use super::MAX_VALUE_DEPTH;
use crate::errors::WalError;

pub(crate) fn decode_event(bytes: &[u8]) -> Result<MutationEvent, WalError> {
    if !bytes.starts_with(PAYLOAD_MAGIC) {
        return Err(WalError::Decode(
            "WAL mutation payload has bad magic".into(),
        ));
    }
    let mut reader = PayloadReader::new(&bytes[PAYLOAD_MAGIC.len()..]);
    let event = reader.read_event()?;
    reader.finish()?;
    Ok(event)
}

pub(crate) fn decode_events(bytes: &[u8]) -> Result<Vec<MutationEvent>, WalError> {
    if !bytes.starts_with(PAYLOAD_MAGIC) {
        return Err(WalError::Decode(
            "WAL mutation payload has bad magic".into(),
        ));
    }
    let mut reader = PayloadReader::new(&bytes[PAYLOAD_MAGIC.len()..]);
    let len = reader.read_len_bounded("WAL event")?;
    let mut events = reader.vec_with_capacity(len, "WAL event")?;
    for _ in 0..len {
        events.push(reader.read_event()?);
    }
    reader.finish()?;
    Ok(events)
}

/// Check that a mutation-batch payload decodes, decoding its events one at
/// a time and dropping each: replay validates a batch where it reads it
/// without holding the batch decoded. Returns the number of events.
pub(crate) fn validate_events(bytes: &[u8]) -> Result<usize, WalError> {
    if !bytes.starts_with(PAYLOAD_MAGIC) {
        return Err(WalError::Decode(
            "WAL mutation payload has bad magic".into(),
        ));
    }
    let mut reader = PayloadReader::new(&bytes[PAYLOAD_MAGIC.len()..]);
    let len = reader.read_len_bounded("WAL event")?;
    for _ in 0..len {
        reader.check_event()?;
    }
    reader.finish()?;
    Ok(len)
}

/// Decode a mutation-batch payload's events one at a time, handing each
/// to `f` before the next is decoded.
pub(crate) fn for_each_event<E: From<WalError>>(
    bytes: &[u8],
    mut f: impl FnMut(MutationEvent) -> Result<(), E>,
) -> Result<(), E> {
    if !bytes.starts_with(PAYLOAD_MAGIC) {
        return Err(WalError::Decode("WAL mutation payload has bad magic".into()).into());
    }
    let mut reader = PayloadReader::new(&bytes[PAYLOAD_MAGIC.len()..]);
    let len = reader.read_len_bounded("WAL event")?;
    for _ in 0..len {
        f(reader.read_event()?)?;
    }
    reader.finish()?;
    Ok(())
}

struct PayloadReader<'a> {
    bytes: &'a [u8],
    offset: usize,
    /// Lists and maps enclosing the value being read. Bounded by
    /// [`MAX_VALUE_DEPTH`] so a hostile or corrupt payload cannot recurse
    /// the decoder off the end of the stack.
    depth: usize,
}

impl<'a> PayloadReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            depth: 0,
        }
    }

    fn finish(&self) -> Result<(), WalError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(WalError::Decode(format!(
                "trailing bytes in WAL mutation payload: {}",
                self.bytes.len() - self.offset
            )))
        }
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], WalError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| WalError::Decode("WAL mutation payload offset overflow".into()))?;
        if end > self.bytes.len() {
            return Err(WalError::Decode("truncated WAL mutation payload".into()));
        }
        let out = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(out)
    }

    fn read_u8(&mut self) -> Result<u8, WalError> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_i8(&mut self) -> Result<i8, WalError> {
        Ok(self.read_u8()? as i8)
    }

    fn read_i16(&mut self) -> Result<i16, WalError> {
        Ok(i16::from_le_bytes(self.read_array()?))
    }

    fn read_i32(&mut self) -> Result<i32, WalError> {
        Ok(i32::from_le_bytes(self.read_array()?))
    }

    fn read_u32(&mut self) -> Result<u32, WalError> {
        Ok(u32::from_le_bytes(self.read_array()?))
    }

    fn read_u64(&mut self) -> Result<u64, WalError> {
        Ok(u64::from_le_bytes(self.read_array()?))
    }

    fn read_i64(&mut self) -> Result<i64, WalError> {
        Ok(i64::from_le_bytes(self.read_array()?))
    }

    fn read_f32(&mut self) -> Result<f32, WalError> {
        Ok(f32::from_bits(self.read_u32()?))
    }

    fn read_f64(&mut self) -> Result<f64, WalError> {
        Ok(f64::from_bits(self.read_u64()?))
    }

    fn read_len(&mut self) -> Result<usize, WalError> {
        usize::try_from(self.read_u64()?)
            .map_err(|_| WalError::Decode("length overflows usize".into()))
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn read_len_bounded(&mut self, label: &str) -> Result<usize, WalError> {
        let len = self.read_len()?;
        if len > self.remaining() {
            return Err(WalError::Decode(format!(
                "{label} count {len} exceeds remaining WAL payload"
            )));
        }
        Ok(len)
    }

    fn vec_with_capacity<T>(&self, len: usize, label: &str) -> Result<Vec<T>, WalError> {
        let mut values = Vec::new();
        values.try_reserve(len).map_err(|_| {
            WalError::Decode(format!("{label} count {len} is too large to allocate"))
        })?;
        Ok(values)
    }

    fn read_bytes(&mut self) -> Result<&'a [u8], WalError> {
        let len = self.read_len()?;
        self.read_exact(len)
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], WalError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.read_exact(N)?);
        Ok(out)
    }

    fn read_string(&mut self) -> Result<String, WalError> {
        let bytes = self.read_bytes()?;
        std::str::from_utf8(bytes)
            .map(|value| value.to_string())
            .map_err(|e| WalError::Decode(format!("invalid UTF-8 in WAL payload: {e}")))
    }

    fn read_string_vec(&mut self) -> Result<Vec<String>, WalError> {
        let len = self.read_len_bounded("string")?;
        let mut values = self.vec_with_capacity(len, "string")?;
        for _ in 0..len {
            values.push(self.read_string()?);
        }
        Ok(values)
    }

    fn read_properties(&mut self) -> Result<Properties, WalError> {
        let len = self.read_len_bounded("property")?;
        let mut properties = Properties::with_capacity(len);
        for _ in 0..len {
            let key = self.read_string()?;
            let value = self.read_value()?;
            properties.insert(lora_store::intern_owned(key), value);
        }
        Ok(properties)
    }

    fn enter_container(&mut self) -> Result<(), WalError> {
        if self.depth >= MAX_VALUE_DEPTH {
            return Err(WalError::Decode(format!(
                "property value is nested more than {MAX_VALUE_DEPTH} lists or maps deep"
            )));
        }
        self.depth += 1;
        Ok(())
    }

    /// Recursive for lists and maps only; scalars are read by
    /// [`Self::read_scalar`] so each nesting level costs a small stack
    /// frame even in unoptimised builds.
    fn read_value(&mut self) -> Result<PropertyValue, WalError> {
        Ok(match self.read_u8()? {
            VALUE_LIST => {
                self.enter_container()?;
                let len = self.read_len_bounded("property list value")?;
                let mut values = self.vec_with_capacity(len, "property list value")?;
                for _ in 0..len {
                    values.push(self.read_value()?);
                }
                self.depth -= 1;
                PropertyValue::List(values)
            }
            VALUE_MAP => {
                self.enter_container()?;
                let len = self.read_len_bounded("property map entry")?;
                let mut values = BTreeMap::new();
                for _ in 0..len {
                    let key = self.read_string()?;
                    let value = self.read_value()?;
                    values.insert(key, value);
                }
                self.depth -= 1;
                PropertyValue::Map(values)
            }
            tag => self.read_scalar(tag)?,
        })
    }

    fn read_scalar(&mut self, tag: u8) -> Result<PropertyValue, WalError> {
        Ok(match tag {
            VALUE_NULL => PropertyValue::Null,
            VALUE_BOOL => PropertyValue::Bool(self.read_u8()? != 0),
            VALUE_INT => PropertyValue::Int(self.read_i64()?),
            VALUE_FLOAT => PropertyValue::Float(f64::from_bits(self.read_u64()?)),
            VALUE_STRING => PropertyValue::String(self.read_string()?),
            VALUE_BINARY => PropertyValue::Binary(self.read_binary()?),
            VALUE_DATE => PropertyValue::Date(LoraDate {
                year: self.read_i32()?,
                month: self.read_u32()?,
                day: self.read_u32()?,
            }),
            VALUE_TIME => PropertyValue::Time(LoraTime {
                hour: self.read_u32()?,
                minute: self.read_u32()?,
                second: self.read_u32()?,
                nanosecond: self.read_u32()?,
                offset_seconds: self.read_i32()?,
            }),
            VALUE_LOCAL_TIME => PropertyValue::LocalTime(LoraLocalTime {
                hour: self.read_u32()?,
                minute: self.read_u32()?,
                second: self.read_u32()?,
                nanosecond: self.read_u32()?,
            }),
            tag @ (VALUE_DATE_TIME | VALUE_ZONED_DATE_TIME) => {
                let mut value = LoraDateTime {
                    year: self.read_i32()?,
                    month: self.read_u32()?,
                    day: self.read_u32()?,
                    hour: self.read_u32()?,
                    minute: self.read_u32()?,
                    second: self.read_u32()?,
                    nanosecond: self.read_u32()?,
                    offset_seconds: self.read_i32()?,
                    zone: None,
                };
                if tag == VALUE_ZONED_DATE_TIME {
                    let name = self.read_string()?;
                    value.zone =
                        Some(ZoneId::lookup(&name).ok_or_else(|| {
                            WalError::Decode(format!("unknown time zone `{name}`"))
                        })?);
                }
                PropertyValue::DateTime(value)
            }
            VALUE_LOCAL_DATE_TIME => PropertyValue::LocalDateTime(LoraLocalDateTime {
                year: self.read_i32()?,
                month: self.read_u32()?,
                day: self.read_u32()?,
                hour: self.read_u32()?,
                minute: self.read_u32()?,
                second: self.read_u32()?,
                nanosecond: self.read_u32()?,
            }),
            VALUE_DURATION => PropertyValue::Duration(LoraDuration {
                months: self.read_i64()?,
                days: self.read_i64()?,
                seconds: self.read_i64()?,
                nanoseconds: self.read_i64()?,
            }),
            VALUE_POINT => {
                let x = self.read_f64()?;
                let y = self.read_f64()?;
                let z = match self.read_u8()? {
                    0 => None,
                    1 => Some(self.read_f64()?),
                    tag => {
                        return Err(WalError::Decode(format!(
                            "invalid point z-presence tag {tag}"
                        )));
                    }
                };
                PropertyValue::Point(LoraPoint {
                    x,
                    y,
                    z,
                    srid: self.read_u32()?,
                })
            }
            VALUE_VECTOR => PropertyValue::Vector(self.read_vector()?),
            tag => {
                return Err(WalError::Decode(format!(
                    "unknown WAL property value tag {tag}"
                )));
            }
        })
    }

    fn read_vector(&mut self) -> Result<LoraVector, WalError> {
        let dimension = self.read_len()?;
        let values = match self.read_u8()? {
            VECTOR_FLOAT64 => {
                let len = self.read_len_bounded("float64 vector value")?;
                let mut values = self.vec_with_capacity(len, "float64 vector value")?;
                for _ in 0..len {
                    values.push(self.read_f64()?);
                }
                VectorValues::Float64(values)
            }
            VECTOR_FLOAT32 => {
                let len = self.read_len_bounded("float32 vector value")?;
                let mut values = self.vec_with_capacity(len, "float32 vector value")?;
                for _ in 0..len {
                    values.push(self.read_f32()?);
                }
                VectorValues::Float32(values)
            }
            VECTOR_INTEGER64 => {
                let len = self.read_len_bounded("int64 vector value")?;
                let mut values = self.vec_with_capacity(len, "int64 vector value")?;
                for _ in 0..len {
                    values.push(self.read_i64()?);
                }
                VectorValues::Integer64(values)
            }
            VECTOR_INTEGER32 => {
                let len = self.read_len_bounded("int32 vector value")?;
                let mut values = self.vec_with_capacity(len, "int32 vector value")?;
                for _ in 0..len {
                    values.push(self.read_i32()?);
                }
                VectorValues::Integer32(values)
            }
            VECTOR_INTEGER16 => {
                let len = self.read_len_bounded("int16 vector value")?;
                let mut values = self.vec_with_capacity(len, "int16 vector value")?;
                for _ in 0..len {
                    values.push(self.read_i16()?);
                }
                VectorValues::Integer16(values)
            }
            VECTOR_INTEGER8 => {
                let len = self.read_len_bounded("int8 vector value")?;
                let mut values = self.vec_with_capacity(len, "int8 vector value")?;
                for _ in 0..len {
                    values.push(self.read_i8()?);
                }
                VectorValues::Integer8(values)
            }
            tag => return Err(WalError::Decode(format!("unknown vector value tag {tag}"))),
        };
        if values.len() != dimension {
            return Err(WalError::Decode(format!(
                "vector dimension mismatch: declared {dimension}, got {}",
                values.len()
            )));
        }
        Ok(LoraVector { dimension, values })
    }

    fn read_binary(&mut self) -> Result<lora_store::LoraBinary, WalError> {
        let len = self.read_len_bounded("binary segment")?;
        let mut segments = self.vec_with_capacity(len, "binary segment")?;
        for _ in 0..len {
            segments.push(self.read_bytes()?.to_vec());
        }
        Ok(lora_store::LoraBinary::from_segments(segments))
    }

    // Checking: the `read_*` methods' parse, building nothing, for
    // [`validate_events`]. Each accepts exactly what its `read_*`
    // counterpart accepts (`check_matches_decode` in the tests).

    fn check_str(&mut self) -> Result<(), WalError> {
        let bytes = self.read_bytes()?;
        std::str::from_utf8(bytes)
            .map(drop)
            .map_err(|e| WalError::Decode(format!("invalid UTF-8 in WAL payload: {e}")))
    }

    fn check_string_vec(&mut self) -> Result<(), WalError> {
        let len = self.read_len_bounded("string")?;
        for _ in 0..len {
            self.check_str()?;
        }
        Ok(())
    }

    fn check_properties(&mut self) -> Result<(), WalError> {
        let len = self.read_len_bounded("property")?;
        for _ in 0..len {
            self.check_str()?;
            self.check_value()?;
        }
        Ok(())
    }

    fn check_value(&mut self) -> Result<(), WalError> {
        match self.read_u8()? {
            VALUE_LIST => {
                self.enter_container()?;
                let len = self.read_len_bounded("property list value")?;
                for _ in 0..len {
                    self.check_value()?;
                }
                self.depth -= 1;
            }
            VALUE_MAP => {
                self.enter_container()?;
                let len = self.read_len_bounded("property map entry")?;
                for _ in 0..len {
                    self.check_str()?;
                    self.check_value()?;
                }
                self.depth -= 1;
            }
            VALUE_STRING => self.check_str()?,
            tag @ (VALUE_DATE_TIME | VALUE_ZONED_DATE_TIME) => {
                // year, month .. nanosecond, offset: eight 4-byte fields.
                self.read_exact(32)?;
                if tag == VALUE_ZONED_DATE_TIME {
                    let bytes = self.read_bytes()?;
                    let name = std::str::from_utf8(bytes).map_err(|e| {
                        WalError::Decode(format!("invalid UTF-8 in WAL payload: {e}"))
                    })?;
                    if ZoneId::lookup(name).is_none() {
                        return Err(WalError::Decode(format!("unknown time zone `{name}`")));
                    }
                }
            }
            // The other scalars hold no strings or collections: reading
            // one builds nothing on the heap but vectors and binaries,
            // which are rare and bounded by the payload.
            tag => drop(self.read_scalar(tag)?),
        }
        Ok(())
    }

    fn check_event(&mut self) -> Result<(), WalError> {
        match self.read_u8()? {
            TAG_CREATE_NODE => {
                self.read_u64()?;
                self.check_string_vec()?;
                self.check_properties()?;
            }
            TAG_CREATE_RELATIONSHIP => {
                self.read_exact(24)?;
                self.check_str()?;
                self.check_properties()?;
            }
            TAG_SET_NODE_PROPERTY | TAG_SET_RELATIONSHIP_PROPERTY => {
                self.read_u64()?;
                self.check_str()?;
                self.check_value()?;
            }
            TAG_REMOVE_NODE_PROPERTY
            | TAG_ADD_NODE_LABEL
            | TAG_REMOVE_NODE_LABEL
            | TAG_REMOVE_RELATIONSHIP_PROPERTY => {
                self.read_u64()?;
                self.check_str()?;
            }
            TAG_DELETE_RELATIONSHIP | TAG_DELETE_NODE | TAG_DETACH_DELETE_NODE => {
                self.read_u64()?;
            }
            TAG_CLEAR => {}
            // Schema events are rare, and an unknown tag fails as
            // decoding fails: decode them.
            _ => {
                self.offset -= 1;
                drop(self.read_event()?);
            }
        }
        Ok(())
    }

    fn read_event(&mut self) -> Result<MutationEvent, WalError> {
        Ok(match self.read_u8()? {
            TAG_CREATE_NODE => MutationEvent::CreateNode {
                id: self.read_u64()?,
                labels: self.read_string_vec()?,
                properties: self.read_properties()?,
            },
            TAG_CREATE_RELATIONSHIP => MutationEvent::CreateRelationship {
                id: self.read_u64()?,
                src: self.read_u64()?,
                dst: self.read_u64()?,
                rel_type: self.read_string()?,
                properties: self.read_properties()?,
            },
            TAG_SET_NODE_PROPERTY => MutationEvent::SetNodeProperty {
                node_id: self.read_u64()?,
                key: self.read_string()?,
                value: self.read_value()?,
            },
            TAG_REMOVE_NODE_PROPERTY => MutationEvent::RemoveNodeProperty {
                node_id: self.read_u64()?,
                key: self.read_string()?,
            },
            TAG_ADD_NODE_LABEL => MutationEvent::AddNodeLabel {
                node_id: self.read_u64()?,
                label: self.read_string()?,
            },
            TAG_REMOVE_NODE_LABEL => MutationEvent::RemoveNodeLabel {
                node_id: self.read_u64()?,
                label: self.read_string()?,
            },
            TAG_SET_RELATIONSHIP_PROPERTY => MutationEvent::SetRelationshipProperty {
                rel_id: self.read_u64()?,
                key: self.read_string()?,
                value: self.read_value()?,
            },
            TAG_REMOVE_RELATIONSHIP_PROPERTY => MutationEvent::RemoveRelationshipProperty {
                rel_id: self.read_u64()?,
                key: self.read_string()?,
            },
            TAG_DELETE_RELATIONSHIP => MutationEvent::DeleteRelationship {
                rel_id: self.read_u64()?,
            },
            TAG_DELETE_NODE => MutationEvent::DeleteNode {
                node_id: self.read_u64()?,
            },
            TAG_DETACH_DELETE_NODE => MutationEvent::DetachDeleteNode {
                node_id: self.read_u64()?,
            },
            TAG_CLEAR => MutationEvent::Clear,
            TAG_CREATE_INDEX => {
                let bytes = self.read_bytes()?;
                let request = decode_index_request(bytes)
                    .map_err(|e| WalError::Decode(format!("CreateIndex decode failed: {e}")))?;
                let if_not_exists = self.read_u8()? != 0;
                MutationEvent::CreateIndex {
                    request,
                    if_not_exists,
                }
            }
            TAG_DROP_INDEX => MutationEvent::DropIndex {
                name: self.read_string()?,
                if_exists: self.read_u8()? != 0,
            },
            TAG_CREATE_CONSTRAINT => {
                let bytes = self.read_bytes()?;
                let request = decode_constraint_request(bytes).map_err(|e| {
                    WalError::Decode(format!("CreateConstraint decode failed: {e}"))
                })?;
                let if_not_exists = self.read_u8()? != 0;
                MutationEvent::CreateConstraint {
                    request,
                    if_not_exists,
                }
            }
            TAG_DROP_CONSTRAINT => MutationEvent::DropConstraint {
                name: self.read_string()?,
                if_exists: self.read_u8()? != 0,
            },
            tag => return Err(WalError::Decode(format!("unknown WAL mutation tag {tag}"))),
        })
    }
}
