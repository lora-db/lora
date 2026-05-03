//! Decode side of the WAL mutation payload codec.

use std::collections::BTreeMap;

use lora_store::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint, LoraTime,
    LoraVector, MutationEvent, Properties, PropertyValue, VectorValues,
};

use super::format::*;
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
    let len = reader.read_len()?;
    let mut events = Vec::with_capacity(len);
    for _ in 0..len {
        events.push(reader.read_event()?);
    }
    reader.finish()?;
    Ok(events)
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
