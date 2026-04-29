//! Compact payload codec for WAL mutation records.
//!
//! The outer WAL record framing still owns length, LSNs, and CRC. This module
//! stores mutation events as a small tagged binary vocabulary that mirrors
//! `lora-store::MutationEvent`.

use std::collections::BTreeMap;

use lora_store::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint, LoraTime,
    LoraVector, MutationEvent, Properties, PropertyValue, VectorValues,
};

use crate::error::WalError;

const PAYLOAD_MAGIC: &[u8; 4] = b"LW1\0";
const TAG_CREATE_NODE: u8 = 1;
const TAG_CREATE_RELATIONSHIP: u8 = 2;
const TAG_SET_NODE_PROPERTY: u8 = 3;
const TAG_REMOVE_NODE_PROPERTY: u8 = 4;
const TAG_ADD_NODE_LABEL: u8 = 5;
const TAG_REMOVE_NODE_LABEL: u8 = 6;
const TAG_SET_RELATIONSHIP_PROPERTY: u8 = 7;
const TAG_REMOVE_RELATIONSHIP_PROPERTY: u8 = 8;
const TAG_DELETE_RELATIONSHIP: u8 = 9;
const TAG_DELETE_NODE: u8 = 10;
const TAG_DETACH_DELETE_NODE: u8 = 11;
const TAG_CLEAR: u8 = 12;

const VALUE_NULL: u8 = 0;
const VALUE_BOOL: u8 = 1;
const VALUE_INT: u8 = 2;
const VALUE_FLOAT: u8 = 3;
const VALUE_STRING: u8 = 4;
const VALUE_LIST: u8 = 5;
const VALUE_MAP: u8 = 6;
const VALUE_DATE: u8 = 7;
const VALUE_TIME: u8 = 8;
const VALUE_LOCAL_TIME: u8 = 9;
const VALUE_DATE_TIME: u8 = 10;
const VALUE_LOCAL_DATE_TIME: u8 = 11;
const VALUE_DURATION: u8 = 12;
const VALUE_POINT: u8 = 13;
const VALUE_VECTOR: u8 = 14;
const VALUE_BINARY: u8 = 15;

const VECTOR_FLOAT64: u8 = 1;
const VECTOR_FLOAT32: u8 = 2;
const VECTOR_INTEGER64: u8 = 3;
const VECTOR_INTEGER32: u8 = 4;
const VECTOR_INTEGER16: u8 = 5;
const VECTOR_INTEGER8: u8 = 6;

#[cfg(test)]
pub(crate) fn encode_event(event: &MutationEvent) -> Result<Vec<u8>, WalError> {
    let mut out = Vec::with_capacity(encoded_event_len(event)?);
    encode_event_into(&mut out, event)?;
    Ok(out)
}

pub(crate) fn encode_event_into(out: &mut Vec<u8>, event: &MutationEvent) -> Result<(), WalError> {
    out.extend_from_slice(PAYLOAD_MAGIC);
    write_event(out, event)?;
    Ok(())
}

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

#[cfg(test)]
pub(crate) fn encode_events(events: &[MutationEvent]) -> Result<Vec<u8>, WalError> {
    let mut out = Vec::with_capacity(encoded_events_len(events)?);
    encode_events_into(&mut out, events)?;
    Ok(out)
}

pub(crate) fn encode_events_into(
    out: &mut Vec<u8>,
    events: &[MutationEvent],
) -> Result<(), WalError> {
    out.extend_from_slice(PAYLOAD_MAGIC);
    write_len(out, events.len())?;
    for event in events {
        write_event(out, event)?;
    }
    Ok(())
}

pub(crate) fn decode_events(bytes: &[u8]) -> Result<Vec<MutationEvent>, WalError> {
    if !bytes.starts_with(PAYLOAD_MAGIC) {
        return Err(WalError::Decode(
            "WAL mutation payload has bad magic".into(),
        ));
    }
    let mut reader = PayloadReader::new(&bytes[PAYLOAD_MAGIC.len()..]);
    let len = reader.read_len()?;
    let mut events = Vec::with_capacity(len);
    for _ in 0..len {
        events.push(reader.read_event()?);
    }
    reader.finish()?;
    Ok(events)
}

pub(crate) fn encoded_event_len(event: &MutationEvent) -> Result<usize, WalError> {
    let mut size = PAYLOAD_MAGIC.len();
    size_event(&mut size, event)?;
    Ok(size)
}

pub(crate) fn encoded_events_len(events: &[MutationEvent]) -> Result<usize, WalError> {
    let mut size = PAYLOAD_MAGIC.len();
    size_len(&mut size, events.len())?;
    for event in events {
        size_event(&mut size, event)?;
    }
    Ok(size)
}

fn write_event(out: &mut Vec<u8>, event: &MutationEvent) -> Result<(), WalError> {
    match event {
        MutationEvent::CreateNode {
            id,
            labels,
            properties,
        } => {
            out.push(TAG_CREATE_NODE);
            write_u64(out, *id);
            write_string_vec(out, labels)?;
            write_properties(out, properties)?;
        }
        MutationEvent::CreateRelationship {
            id,
            src,
            dst,
            rel_type,
            properties,
        } => {
            out.push(TAG_CREATE_RELATIONSHIP);
            write_u64(out, *id);
            write_u64(out, *src);
            write_u64(out, *dst);
            write_string(out, rel_type)?;
            write_properties(out, properties)?;
        }
        MutationEvent::SetNodeProperty {
            node_id,
            key,
            value,
        } => {
            out.push(TAG_SET_NODE_PROPERTY);
            write_u64(out, *node_id);
            write_string(out, key)?;
            write_value(out, value)?;
        }
        MutationEvent::RemoveNodeProperty { node_id, key } => {
            out.push(TAG_REMOVE_NODE_PROPERTY);
            write_u64(out, *node_id);
            write_string(out, key)?;
        }
        MutationEvent::AddNodeLabel { node_id, label } => {
            out.push(TAG_ADD_NODE_LABEL);
            write_u64(out, *node_id);
            write_string(out, label)?;
        }
        MutationEvent::RemoveNodeLabel { node_id, label } => {
            out.push(TAG_REMOVE_NODE_LABEL);
            write_u64(out, *node_id);
            write_string(out, label)?;
        }
        MutationEvent::SetRelationshipProperty { rel_id, key, value } => {
            out.push(TAG_SET_RELATIONSHIP_PROPERTY);
            write_u64(out, *rel_id);
            write_string(out, key)?;
            write_value(out, value)?;
        }
        MutationEvent::RemoveRelationshipProperty { rel_id, key } => {
            out.push(TAG_REMOVE_RELATIONSHIP_PROPERTY);
            write_u64(out, *rel_id);
            write_string(out, key)?;
        }
        MutationEvent::DeleteRelationship { rel_id } => {
            out.push(TAG_DELETE_RELATIONSHIP);
            write_u64(out, *rel_id);
        }
        MutationEvent::DeleteNode { node_id } => {
            out.push(TAG_DELETE_NODE);
            write_u64(out, *node_id);
        }
        MutationEvent::DetachDeleteNode { node_id } => {
            out.push(TAG_DETACH_DELETE_NODE);
            write_u64(out, *node_id);
        }
        MutationEvent::Clear => out.push(TAG_CLEAR),
    }
    Ok(())
}

fn write_properties(out: &mut Vec<u8>, properties: &Properties) -> Result<(), WalError> {
    write_len(out, properties.len())?;
    for (key, value) in properties {
        write_string(out, key)?;
        write_value(out, value)?;
    }
    Ok(())
}

fn write_value(out: &mut Vec<u8>, value: &PropertyValue) -> Result<(), WalError> {
    match value {
        PropertyValue::Null => out.push(VALUE_NULL),
        PropertyValue::Bool(value) => {
            out.push(VALUE_BOOL);
            out.push(u8::from(*value));
        }
        PropertyValue::Int(value) => {
            out.push(VALUE_INT);
            out.extend_from_slice(&value.to_le_bytes());
        }
        PropertyValue::Float(value) => {
            out.push(VALUE_FLOAT);
            out.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        PropertyValue::String(value) => {
            out.push(VALUE_STRING);
            write_string(out, value)?;
        }
        PropertyValue::Binary(value) => {
            out.push(VALUE_BINARY);
            write_binary_segments(out, value.segments())?;
        }
        PropertyValue::List(values) => {
            out.push(VALUE_LIST);
            write_len(out, values.len())?;
            for value in values {
                write_value(out, value)?;
            }
        }
        PropertyValue::Map(values) => {
            out.push(VALUE_MAP);
            write_len(out, values.len())?;
            for (key, value) in values {
                write_string(out, key)?;
                write_value(out, value)?;
            }
        }
        PropertyValue::Date(value) => {
            out.push(VALUE_DATE);
            write_i32(out, value.year);
            write_u32(out, value.month);
            write_u32(out, value.day);
        }
        PropertyValue::Time(value) => {
            out.push(VALUE_TIME);
            write_time_fields(
                out,
                value.hour,
                value.minute,
                value.second,
                value.nanosecond,
            );
            write_i32(out, value.offset_seconds);
        }
        PropertyValue::LocalTime(value) => {
            out.push(VALUE_LOCAL_TIME);
            write_time_fields(
                out,
                value.hour,
                value.minute,
                value.second,
                value.nanosecond,
            );
        }
        PropertyValue::DateTime(value) => {
            out.push(VALUE_DATE_TIME);
            write_date_fields(out, value.year, value.month, value.day);
            write_time_fields(
                out,
                value.hour,
                value.minute,
                value.second,
                value.nanosecond,
            );
            write_i32(out, value.offset_seconds);
        }
        PropertyValue::LocalDateTime(value) => {
            out.push(VALUE_LOCAL_DATE_TIME);
            write_date_fields(out, value.year, value.month, value.day);
            write_time_fields(
                out,
                value.hour,
                value.minute,
                value.second,
                value.nanosecond,
            );
        }
        PropertyValue::Duration(value) => {
            out.push(VALUE_DURATION);
            write_i64(out, value.months);
            write_i64(out, value.days);
            write_i64(out, value.seconds);
            write_i64(out, value.nanoseconds);
        }
        PropertyValue::Point(value) => {
            out.push(VALUE_POINT);
            write_f64(out, value.x);
            write_f64(out, value.y);
            match value.z {
                Some(z) => {
                    out.push(1);
                    write_f64(out, z);
                }
                None => out.push(0),
            }
            write_u32(out, value.srid);
        }
        PropertyValue::Vector(value) => {
            out.push(VALUE_VECTOR);
            write_vector(out, value)?;
        }
    }
    Ok(())
}

fn write_date_fields(out: &mut Vec<u8>, year: i32, month: u32, day: u32) {
    write_i32(out, year);
    write_u32(out, month);
    write_u32(out, day);
}

fn write_time_fields(out: &mut Vec<u8>, hour: u32, minute: u32, second: u32, nanosecond: u32) {
    write_u32(out, hour);
    write_u32(out, minute);
    write_u32(out, second);
    write_u32(out, nanosecond);
}

fn write_vector(out: &mut Vec<u8>, vector: &LoraVector) -> Result<(), WalError> {
    write_len(out, vector.dimension)?;
    match &vector.values {
        VectorValues::Float64(values) => {
            out.push(VECTOR_FLOAT64);
            write_len(out, values.len())?;
            for value in values {
                write_f64(out, *value);
            }
        }
        VectorValues::Float32(values) => {
            out.push(VECTOR_FLOAT32);
            write_len(out, values.len())?;
            for value in values {
                write_f32(out, *value);
            }
        }
        VectorValues::Integer64(values) => {
            out.push(VECTOR_INTEGER64);
            write_len(out, values.len())?;
            for value in values {
                write_i64(out, *value);
            }
        }
        VectorValues::Integer32(values) => {
            out.push(VECTOR_INTEGER32);
            write_len(out, values.len())?;
            for value in values {
                write_i32(out, *value);
            }
        }
        VectorValues::Integer16(values) => {
            out.push(VECTOR_INTEGER16);
            write_len(out, values.len())?;
            for value in values {
                write_i16(out, *value);
            }
        }
        VectorValues::Integer8(values) => {
            out.push(VECTOR_INTEGER8);
            write_len(out, values.len())?;
            for value in values {
                write_i8(out, *value);
            }
        }
    }
    Ok(())
}

fn write_binary_segments(out: &mut Vec<u8>, segments: &[Vec<u8>]) -> Result<(), WalError> {
    write_len(out, segments.len())?;
    for segment in segments {
        write_bytes(out, segment)?;
    }
    Ok(())
}

fn write_i8(out: &mut Vec<u8>, value: i8) {
    out.push(value as u8);
}

fn write_i16(out: &mut Vec<u8>, value: i16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_bits().to_le_bytes());
}

fn write_f64(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&value.to_bits().to_le_bytes());
}

fn write_len(out: &mut Vec<u8>, len: usize) -> Result<(), WalError> {
    write_u64(
        out,
        u64::try_from(len).map_err(|_| WalError::Encode("length does not fit in u64".into()))?,
    );
    Ok(())
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), WalError> {
    write_len(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn write_string(out: &mut Vec<u8>, value: &str) -> Result<(), WalError> {
    write_bytes(out, value.as_bytes())
}

fn write_string_vec(out: &mut Vec<u8>, values: &[String]) -> Result<(), WalError> {
    write_len(out, values.len())?;
    for value in values {
        write_string(out, value)?;
    }
    Ok(())
}

struct PayloadReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> PayloadReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
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
        Ok(i16::from_le_bytes(self.read_exact(2)?.try_into().unwrap()))
    }

    fn read_i32(&mut self) -> Result<i32, WalError> {
        Ok(i32::from_le_bytes(self.read_exact(4)?.try_into().unwrap()))
    }

    fn read_u32(&mut self) -> Result<u32, WalError> {
        Ok(u32::from_le_bytes(self.read_exact(4)?.try_into().unwrap()))
    }

    fn read_u64(&mut self) -> Result<u64, WalError> {
        Ok(u64::from_le_bytes(self.read_exact(8)?.try_into().unwrap()))
    }

    fn read_i64(&mut self) -> Result<i64, WalError> {
        Ok(i64::from_le_bytes(self.read_exact(8)?.try_into().unwrap()))
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

    fn read_bytes(&mut self) -> Result<&'a [u8], WalError> {
        let len = self.read_len()?;
        self.read_exact(len)
    }

    fn read_string(&mut self) -> Result<String, WalError> {
        let bytes = self.read_bytes()?;
        std::str::from_utf8(bytes)
            .map(|value| value.to_string())
            .map_err(|e| WalError::Decode(format!("invalid UTF-8 in WAL payload: {e}")))
    }

    fn read_string_vec(&mut self) -> Result<Vec<String>, WalError> {
        let len = self.read_len()?;
        let mut values = Vec::with_capacity(len);
        for _ in 0..len {
            values.push(self.read_string()?);
        }
        Ok(values)
    }

    fn read_properties(&mut self) -> Result<Properties, WalError> {
        let len = self.read_len()?;
        let mut properties = BTreeMap::new();
        for _ in 0..len {
            let key = self.read_string()?;
            let value = self.read_value()?;
            properties.insert(key, value);
        }
        Ok(properties)
    }

    fn read_value(&mut self) -> Result<PropertyValue, WalError> {
        Ok(match self.read_u8()? {
            VALUE_NULL => PropertyValue::Null,
            VALUE_BOOL => PropertyValue::Bool(self.read_u8()? != 0),
            VALUE_INT => PropertyValue::Int(self.read_i64()?),
            VALUE_FLOAT => PropertyValue::Float(f64::from_bits(self.read_u64()?)),
            VALUE_STRING => PropertyValue::String(self.read_string()?),
            VALUE_BINARY => PropertyValue::Binary(self.read_binary()?),
            VALUE_LIST => {
                let len = self.read_len()?;
                let mut values = Vec::with_capacity(len);
                for _ in 0..len {
                    values.push(self.read_value()?);
                }
                PropertyValue::List(values)
            }
            VALUE_MAP => {
                let len = self.read_len()?;
                let mut values = BTreeMap::new();
                for _ in 0..len {
                    let key = self.read_string()?;
                    let value = self.read_value()?;
                    values.insert(key, value);
                }
                PropertyValue::Map(values)
            }
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
            VALUE_DATE_TIME => PropertyValue::DateTime(LoraDateTime {
                year: self.read_i32()?,
                month: self.read_u32()?,
                day: self.read_u32()?,
                hour: self.read_u32()?,
                minute: self.read_u32()?,
                second: self.read_u32()?,
                nanosecond: self.read_u32()?,
                offset_seconds: self.read_i32()?,
            }),
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
                let len = self.read_len()?;
                let mut values = Vec::with_capacity(len);
                for _ in 0..len {
                    values.push(self.read_f64()?);
                }
                VectorValues::Float64(values)
            }
            VECTOR_FLOAT32 => {
                let len = self.read_len()?;
                let mut values = Vec::with_capacity(len);
                for _ in 0..len {
                    values.push(self.read_f32()?);
                }
                VectorValues::Float32(values)
            }
            VECTOR_INTEGER64 => {
                let len = self.read_len()?;
                let mut values = Vec::with_capacity(len);
                for _ in 0..len {
                    values.push(self.read_i64()?);
                }
                VectorValues::Integer64(values)
            }
            VECTOR_INTEGER32 => {
                let len = self.read_len()?;
                let mut values = Vec::with_capacity(len);
                for _ in 0..len {
                    values.push(self.read_i32()?);
                }
                VectorValues::Integer32(values)
            }
            VECTOR_INTEGER16 => {
                let len = self.read_len()?;
                let mut values = Vec::with_capacity(len);
                for _ in 0..len {
                    values.push(self.read_i16()?);
                }
                VectorValues::Integer16(values)
            }
            VECTOR_INTEGER8 => {
                let len = self.read_len()?;
                let mut values = Vec::with_capacity(len);
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
        let len = self.read_len()?;
        let mut segments = Vec::with_capacity(len);
        for _ in 0..len {
            segments.push(self.read_bytes()?.to_vec());
        }
        Ok(lora_store::LoraBinary::from_segments(segments))
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
            tag => return Err(WalError::Decode(format!("unknown WAL mutation tag {tag}"))),
        })
    }
}

fn size_event(size: &mut usize, event: &MutationEvent) -> Result<(), WalError> {
    size_tag(size)?;
    match event {
        MutationEvent::CreateNode {
            labels, properties, ..
        } => {
            size_u64(size)?;
            size_string_vec(size, labels)?;
            size_properties(size, properties)?;
        }
        MutationEvent::CreateRelationship {
            rel_type,
            properties,
            ..
        } => {
            size_u64(size)?;
            size_u64(size)?;
            size_u64(size)?;
            size_string(size, rel_type)?;
            size_properties(size, properties)?;
        }
        MutationEvent::SetNodeProperty { key, value, .. }
        | MutationEvent::SetRelationshipProperty { key, value, .. } => {
            size_u64(size)?;
            size_string(size, key)?;
            size_value(size, value)?;
        }
        MutationEvent::RemoveNodeProperty { key, .. }
        | MutationEvent::AddNodeLabel { label: key, .. }
        | MutationEvent::RemoveNodeLabel { label: key, .. }
        | MutationEvent::RemoveRelationshipProperty { key, .. } => {
            size_u64(size)?;
            size_string(size, key)?;
        }
        MutationEvent::DeleteRelationship { .. }
        | MutationEvent::DeleteNode { .. }
        | MutationEvent::DetachDeleteNode { .. } => {
            size_u64(size)?;
        }
        MutationEvent::Clear => {}
    }
    Ok(())
}

fn size_properties(size: &mut usize, properties: &Properties) -> Result<(), WalError> {
    size_len(size, properties.len())?;
    for (key, value) in properties {
        size_string(size, key)?;
        size_value(size, value)?;
    }
    Ok(())
}

fn size_value(size: &mut usize, value: &PropertyValue) -> Result<(), WalError> {
    size_tag(size)?;
    match value {
        PropertyValue::Null => {}
        PropertyValue::Bool(_) => add_size(size, 1)?,
        PropertyValue::Int(_) | PropertyValue::Float(_) => add_size(size, 8)?,
        PropertyValue::String(value) => size_string(size, value)?,
        PropertyValue::Binary(value) => size_binary_segments(size, value.segments())?,
        PropertyValue::List(values) => {
            size_len(size, values.len())?;
            for value in values {
                size_value(size, value)?;
            }
        }
        PropertyValue::Map(values) => {
            size_len(size, values.len())?;
            for (key, value) in values {
                size_string(size, key)?;
                size_value(size, value)?;
            }
        }
        PropertyValue::Date(_) => add_size(size, 12)?,
        PropertyValue::Time(_) => add_size(size, 20)?,
        PropertyValue::LocalTime(_) => add_size(size, 16)?,
        PropertyValue::DateTime(_) => add_size(size, 32)?,
        PropertyValue::LocalDateTime(_) => add_size(size, 28)?,
        PropertyValue::Duration(_) => add_size(size, 32)?,
        PropertyValue::Point(value) => {
            add_size(size, 8 + 8 + 1)?;
            if value.z.is_some() {
                add_size(size, 8)?;
            }
            add_size(size, 4)?;
        }
        PropertyValue::Vector(value) => size_vector(size, value)?,
    }
    Ok(())
}

fn size_vector(size: &mut usize, vector: &LoraVector) -> Result<(), WalError> {
    size_len(size, vector.dimension)?;
    size_tag(size)?;
    match &vector.values {
        VectorValues::Float64(values) => size_numeric_slice(size, values.len(), 8),
        VectorValues::Float32(values) => size_numeric_slice(size, values.len(), 4),
        VectorValues::Integer64(values) => size_numeric_slice(size, values.len(), 8),
        VectorValues::Integer32(values) => size_numeric_slice(size, values.len(), 4),
        VectorValues::Integer16(values) => size_numeric_slice(size, values.len(), 2),
        VectorValues::Integer8(values) => size_numeric_slice(size, values.len(), 1),
    }
}

fn size_numeric_slice(size: &mut usize, len: usize, element_size: usize) -> Result<(), WalError> {
    size_len(size, len)?;
    add_size(
        size,
        len.checked_mul(element_size)
            .ok_or_else(|| WalError::Encode("payload length overflows usize".into()))?,
    )
}

fn size_binary_segments(size: &mut usize, segments: &[Vec<u8>]) -> Result<(), WalError> {
    size_len(size, segments.len())?;
    for segment in segments {
        size_bytes(size, segment.len())?;
    }
    Ok(())
}

fn size_string_vec(size: &mut usize, values: &[String]) -> Result<(), WalError> {
    size_len(size, values.len())?;
    for value in values {
        size_string(size, value)?;
    }
    Ok(())
}

fn size_string(size: &mut usize, value: &str) -> Result<(), WalError> {
    size_bytes(size, value.len())
}

fn size_bytes(size: &mut usize, len: usize) -> Result<(), WalError> {
    size_len(size, len)?;
    add_size(size, len)
}

fn size_len(size: &mut usize, len: usize) -> Result<(), WalError> {
    u64::try_from(len).map_err(|_| WalError::Encode("length does not fit in u64".into()))?;
    size_u64(size)
}

fn size_tag(size: &mut usize) -> Result<(), WalError> {
    add_size(size, 1)
}

fn size_u64(size: &mut usize) -> Result<(), WalError> {
    add_size(size, 8)
}

fn add_size(size: &mut usize, bytes: usize) -> Result<(), WalError> {
    *size = size
        .checked_add(bytes)
        .ok_or_else(|| WalError::Encode("payload length overflows usize".into()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lora_store::{Properties, PropertyValue};

    fn sample_event() -> MutationEvent {
        let mut props = Properties::new();
        props.insert("name".into(), PropertyValue::String("alice".into()));
        props.insert("age".into(), PropertyValue::Int(42));
        props.insert(
            "blob".into(),
            PropertyValue::Binary(lora_store::LoraBinary::from_segments(vec![
                vec![0, 1, 2],
                vec![3, 4, 255],
            ])),
        );
        props.insert(
            "nested".into(),
            PropertyValue::List(vec![PropertyValue::Bool(true), PropertyValue::Null]),
        );
        MutationEvent::CreateNode {
            id: 7,
            labels: vec!["Person".into(), "Admin".into()],
            properties: props,
        }
    }

    fn all_extension_values_event() -> MutationEvent {
        let mut props = Properties::new();
        props.insert(
            "date".into(),
            PropertyValue::Date(LoraDate {
                year: 2026,
                month: 4,
                day: 27,
            }),
        );
        props.insert(
            "time".into(),
            PropertyValue::Time(LoraTime {
                hour: 12,
                minute: 34,
                second: 56,
                nanosecond: 789,
                offset_seconds: 3600,
            }),
        );
        props.insert(
            "localtime".into(),
            PropertyValue::LocalTime(LoraLocalTime {
                hour: 1,
                minute: 2,
                second: 3,
                nanosecond: 4,
            }),
        );
        props.insert(
            "datetime".into(),
            PropertyValue::DateTime(LoraDateTime {
                year: 2026,
                month: 4,
                day: 27,
                hour: 12,
                minute: 34,
                second: 56,
                nanosecond: 789,
                offset_seconds: -1800,
            }),
        );
        props.insert(
            "localdatetime".into(),
            PropertyValue::LocalDateTime(LoraLocalDateTime {
                year: 2026,
                month: 4,
                day: 27,
                hour: 12,
                minute: 34,
                second: 56,
                nanosecond: 789,
            }),
        );
        props.insert(
            "duration".into(),
            PropertyValue::Duration(LoraDuration {
                months: 14,
                days: 3,
                seconds: 4,
                nanoseconds: 5,
            }),
        );
        props.insert(
            "point".into(),
            PropertyValue::Point(LoraPoint {
                x: 4.9,
                y: 52.37,
                z: Some(7.0),
                srid: 4979,
            }),
        );
        props.insert(
            "vector_f64".into(),
            PropertyValue::Vector(LoraVector {
                dimension: 2,
                values: VectorValues::Float64(vec![1.5, 2.5]),
            }),
        );
        props.insert(
            "vector_f32".into(),
            PropertyValue::Vector(LoraVector {
                dimension: 2,
                values: VectorValues::Float32(vec![1.5, 2.5]),
            }),
        );
        props.insert(
            "vector_i64".into(),
            PropertyValue::Vector(LoraVector {
                dimension: 2,
                values: VectorValues::Integer64(vec![1, -2]),
            }),
        );
        props.insert(
            "vector_i32".into(),
            PropertyValue::Vector(LoraVector {
                dimension: 2,
                values: VectorValues::Integer32(vec![1, -2]),
            }),
        );
        props.insert(
            "vector_i16".into(),
            PropertyValue::Vector(LoraVector {
                dimension: 2,
                values: VectorValues::Integer16(vec![1, -2]),
            }),
        );
        props.insert(
            "vector_i8".into(),
            PropertyValue::Vector(LoraVector {
                dimension: 2,
                values: VectorValues::Integer8(vec![1, -2]),
            }),
        );
        MutationEvent::SetNodeProperty {
            node_id: 7,
            key: "extensions".into(),
            value: PropertyValue::Map(props),
        }
    }

    #[test]
    fn event_round_trip() {
        let event = sample_event();
        let encoded = encode_event(&event).unwrap();
        assert_eq!(encoded_event_len(&event).unwrap(), encoded.len());
        assert_eq!(decode_event(&encoded).unwrap(), event);
    }

    #[test]
    fn event_batch_round_trip() {
        let events = vec![sample_event(), MutationEvent::Clear];
        let encoded = encode_events(&events).unwrap();
        assert_eq!(encoded_events_len(&events).unwrap(), encoded.len());
        assert_eq!(decode_events(&encoded).unwrap(), events);
    }

    #[test]
    fn non_compact_payload_is_rejected() {
        assert!(matches!(
            decode_event(b"not-lora-wal"),
            Err(WalError::Decode(_))
        ));
    }

    #[test]
    fn add_label_round_trip() {
        let event = MutationEvent::AddNodeLabel {
            node_id: 99,
            label: "User".into(),
        };
        let encoded = encode_event(&event).unwrap();
        assert_eq!(decode_event(&encoded).unwrap(), event);
    }

    #[test]
    fn all_extension_values_round_trip() {
        let event = all_extension_values_event();
        let encoded = encode_event(&event).unwrap();
        assert_eq!(encoded_event_len(&event).unwrap(), encoded.len());
        assert_eq!(decode_event(&encoded).unwrap(), event);
    }
}
