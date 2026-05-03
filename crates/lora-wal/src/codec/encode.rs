//! Encode side of the WAL mutation payload codec.

use lora_store::{LoraVector, MutationEvent, Properties, PropertyValue, VectorValues};

use super::format::*;
use crate::errors::WalError;

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

// ---------- Size pre-computation ----------

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
