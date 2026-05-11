//! Dependency-free binary codecs for store-owned values that must cross
//! crate boundaries.
//!
//! `lora-wal` and `lora-snapshot` own their container formats, but the
//! byte shape for core store types belongs here so catalog DDL, property
//! values, and snapshot index metadata do not drift apart.

use std::collections::BTreeMap;
use std::fmt;

use crate::{
    ConstraintDefinition, ConstraintRequest, IndexConfigValue, IndexDefinition, IndexRequest,
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, PropertyValue, StoredConstraintKind, StoredIndexEntity, StoredIndexKind,
    StoredIndexState, StoredPropertyType, StoredPropertyTypeTerm, StoredScalarType,
    StoredVectorCoordType, VectorValues,
};

type Result<T> = std::result::Result<T, StoreCodecError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreCodecError {
    Encode(String),
    Decode(String),
}

impl fmt::Display for StoreCodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(message) => write!(f, "store value encode failed: {message}"),
            Self::Decode(message) => write!(f, "store value decode failed: {message}"),
        }
    }
}

impl std::error::Error for StoreCodecError {}

pub fn encode_property_value(value: &PropertyValue) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_property_value(&mut out, value)?;
    Ok(out)
}

pub fn decode_property_value(bytes: &[u8]) -> Result<PropertyValue> {
    let mut reader = Reader::new(bytes);
    let value = reader.read_property_value()?;
    reader.finish()?;
    Ok(value)
}

pub fn encode_index_request(request: &IndexRequest) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_index_request(&mut out, request)?;
    Ok(out)
}

pub fn decode_index_request(bytes: &[u8]) -> Result<IndexRequest> {
    let mut reader = Reader::new(bytes);
    let request = reader.read_index_request()?;
    reader.finish()?;
    Ok(request)
}

pub fn encode_constraint_request(request: &ConstraintRequest) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_constraint_request(&mut out, request)?;
    Ok(out)
}

pub fn decode_constraint_request(bytes: &[u8]) -> Result<ConstraintRequest> {
    let mut reader = Reader::new(bytes);
    let request = reader.read_constraint_request()?;
    reader.finish()?;
    Ok(request)
}

pub fn encode_constraint_definitions(defs: &[ConstraintDefinition]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_len(&mut out, defs.len())?;
    for def in defs {
        write_constraint_definition(&mut out, def)?;
    }
    Ok(out)
}

pub fn decode_constraint_definitions(bytes: &[u8]) -> Result<Vec<ConstraintDefinition>> {
    let mut reader = Reader::new(bytes);
    let len = reader.read_len_bounded("constraint definition")?;
    let mut defs = reader.vec_with_capacity(len, "constraint definition")?;
    for _ in 0..len {
        defs.push(reader.read_constraint_definition()?);
    }
    reader.finish()?;
    Ok(defs)
}

pub fn encode_index_definitions(defs: &[IndexDefinition]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_len(&mut out, defs.len())?;
    for def in defs {
        write_index_definition(&mut out, def)?;
    }
    Ok(out)
}

pub fn decode_index_definitions(bytes: &[u8]) -> Result<Vec<IndexDefinition>> {
    let mut reader = Reader::new(bytes);
    let len = reader.read_len_bounded("index definition")?;
    let mut defs = reader.vec_with_capacity(len, "index definition")?;
    for _ in 0..len {
        defs.push(reader.read_index_definition()?);
    }
    reader.finish()?;
    Ok(defs)
}

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

const INDEX_KIND_RANGE: u8 = 1;
const INDEX_KIND_TEXT: u8 = 2;
const INDEX_KIND_POINT: u8 = 3;
const INDEX_KIND_LOOKUP: u8 = 4;
const INDEX_KIND_VECTOR: u8 = 5;
const INDEX_KIND_FULLTEXT: u8 = 6;

const INDEX_ENTITY_NODE: u8 = 1;
const INDEX_ENTITY_RELATIONSHIP: u8 = 2;

const INDEX_STATE_ONLINE: u8 = 1;
const INDEX_STATE_POPULATING: u8 = 2;

const CONFIG_NUMBER: u8 = 1;
const CONFIG_INTEGER: u8 = 2;
const CONFIG_STRING: u8 = 3;
const CONFIG_BOOL: u8 = 4;
const CONFIG_LIST: u8 = 5;
const CONFIG_MAP: u8 = 6;
const CONFIG_NULL: u8 = 7;

const CONSTRAINT_KIND_UNIQUE: u8 = 1;
const CONSTRAINT_KIND_EXISTENCE: u8 = 2;
const CONSTRAINT_KIND_NODE_KEY: u8 = 3;
const CONSTRAINT_KIND_RELATIONSHIP_KEY: u8 = 4;
const CONSTRAINT_KIND_PROPERTY_TYPE: u8 = 5;

const PROPERTY_TYPE_TERM_SCALAR: u8 = 1;
const PROPERTY_TYPE_TERM_LIST: u8 = 2;
const PROPERTY_TYPE_TERM_VECTOR: u8 = 3;

const SCALAR_BOOLEAN: u8 = 1;
const SCALAR_STRING: u8 = 2;
const SCALAR_INTEGER: u8 = 3;
const SCALAR_FLOAT: u8 = 4;
const SCALAR_DATE: u8 = 5;
const SCALAR_LOCAL_TIME: u8 = 6;
const SCALAR_ZONED_TIME: u8 = 7;
const SCALAR_LOCAL_DATETIME: u8 = 8;
const SCALAR_ZONED_DATETIME: u8 = 9;
const SCALAR_DURATION: u8 = 10;
const SCALAR_POINT: u8 = 11;
const SCALAR_MAP: u8 = 12;
const SCALAR_ANY: u8 = 13;

const VCOORD_INT8: u8 = 1;
const VCOORD_INT16: u8 = 2;
const VCOORD_INT32: u8 = 3;
const VCOORD_INT64: u8 = 4;
const VCOORD_FLOAT32: u8 = 5;
const VCOORD_FLOAT64: u8 = 6;

fn write_index_request(out: &mut Vec<u8>, request: &IndexRequest) -> Result<()> {
    write_optional_string(out, request.explicit_name.as_deref())?;
    write_index_kind(out, request.kind);
    write_index_entity(out, request.entity);
    write_optional_string(out, request.label.as_deref())?;
    write_string_vec(out, &request.additional_labels)?;
    write_string_vec(out, &request.properties)?;
    write_config_map(out, &request.options)
}

fn write_index_definition(out: &mut Vec<u8>, def: &IndexDefinition) -> Result<()> {
    write_string(out, &def.name)?;
    write_index_kind(out, def.kind);
    write_index_entity(out, def.entity);
    write_optional_string(out, def.label.as_deref())?;
    write_string_vec(out, &def.additional_labels)?;
    write_string_vec(out, &def.properties)?;
    write_config_map(out, &def.options)?;
    write_index_state(out, def.state);
    Ok(())
}

fn write_index_kind(out: &mut Vec<u8>, kind: StoredIndexKind) {
    out.push(match kind {
        StoredIndexKind::Range => INDEX_KIND_RANGE,
        StoredIndexKind::Text => INDEX_KIND_TEXT,
        StoredIndexKind::Point => INDEX_KIND_POINT,
        StoredIndexKind::Lookup => INDEX_KIND_LOOKUP,
        StoredIndexKind::Vector => INDEX_KIND_VECTOR,
        StoredIndexKind::Fulltext => INDEX_KIND_FULLTEXT,
    });
}

fn write_index_entity(out: &mut Vec<u8>, entity: StoredIndexEntity) {
    out.push(match entity {
        StoredIndexEntity::Node => INDEX_ENTITY_NODE,
        StoredIndexEntity::Relationship => INDEX_ENTITY_RELATIONSHIP,
    });
}

fn write_index_state(out: &mut Vec<u8>, state: StoredIndexState) {
    out.push(match state {
        StoredIndexState::Online => INDEX_STATE_ONLINE,
        StoredIndexState::Populating => INDEX_STATE_POPULATING,
    });
}

fn write_constraint_request(out: &mut Vec<u8>, request: &ConstraintRequest) -> Result<()> {
    write_string(out, &request.name)?;
    write_constraint_kind(out, &request.kind)?;
    write_index_entity(out, request.entity);
    write_string(out, &request.label)?;
    write_string_vec(out, &request.properties)
}

fn write_constraint_definition(out: &mut Vec<u8>, def: &ConstraintDefinition) -> Result<()> {
    write_string(out, &def.name)?;
    write_constraint_kind(out, &def.kind)?;
    write_index_entity(out, def.entity);
    write_string(out, &def.label)?;
    write_string_vec(out, &def.properties)?;
    write_optional_string(out, def.owned_index.as_deref())
}

fn write_constraint_kind(out: &mut Vec<u8>, kind: &StoredConstraintKind) -> Result<()> {
    match kind {
        StoredConstraintKind::Unique => out.push(CONSTRAINT_KIND_UNIQUE),
        StoredConstraintKind::Existence => out.push(CONSTRAINT_KIND_EXISTENCE),
        StoredConstraintKind::NodeKey => out.push(CONSTRAINT_KIND_NODE_KEY),
        StoredConstraintKind::RelationshipKey => out.push(CONSTRAINT_KIND_RELATIONSHIP_KEY),
        StoredConstraintKind::PropertyType(t) => {
            out.push(CONSTRAINT_KIND_PROPERTY_TYPE);
            write_property_type(out, t)?;
        }
    }
    Ok(())
}

fn write_property_type(out: &mut Vec<u8>, t: &StoredPropertyType) -> Result<()> {
    write_len(out, t.alternatives.len())?;
    for term in &t.alternatives {
        write_property_type_term(out, term)?;
    }
    Ok(())
}

fn write_property_type_term(out: &mut Vec<u8>, term: &StoredPropertyTypeTerm) -> Result<()> {
    match term {
        StoredPropertyTypeTerm::Scalar(s) => {
            out.push(PROPERTY_TYPE_TERM_SCALAR);
            out.push(scalar_tag(*s));
        }
        StoredPropertyTypeTerm::List { inner, not_null } => {
            out.push(PROPERTY_TYPE_TERM_LIST);
            out.push(u8::from(*not_null));
            write_property_type_term(out, inner)?;
        }
        StoredPropertyTypeTerm::Vector { coord, dimension } => {
            out.push(PROPERTY_TYPE_TERM_VECTOR);
            out.push(vector_coord_tag(*coord));
            write_u32(out, *dimension);
        }
    }
    Ok(())
}

fn scalar_tag(s: StoredScalarType) -> u8 {
    match s {
        StoredScalarType::Boolean => SCALAR_BOOLEAN,
        StoredScalarType::String => SCALAR_STRING,
        StoredScalarType::Integer => SCALAR_INTEGER,
        StoredScalarType::Float => SCALAR_FLOAT,
        StoredScalarType::Date => SCALAR_DATE,
        StoredScalarType::LocalTime => SCALAR_LOCAL_TIME,
        StoredScalarType::ZonedTime => SCALAR_ZONED_TIME,
        StoredScalarType::LocalDateTime => SCALAR_LOCAL_DATETIME,
        StoredScalarType::ZonedDateTime => SCALAR_ZONED_DATETIME,
        StoredScalarType::Duration => SCALAR_DURATION,
        StoredScalarType::Point => SCALAR_POINT,
        StoredScalarType::Map => SCALAR_MAP,
        StoredScalarType::Any => SCALAR_ANY,
    }
}

fn vector_coord_tag(c: StoredVectorCoordType) -> u8 {
    match c {
        StoredVectorCoordType::Int8 => VCOORD_INT8,
        StoredVectorCoordType::Int16 => VCOORD_INT16,
        StoredVectorCoordType::Int32 => VCOORD_INT32,
        StoredVectorCoordType::Int64 => VCOORD_INT64,
        StoredVectorCoordType::Float32 => VCOORD_FLOAT32,
        StoredVectorCoordType::Float64 => VCOORD_FLOAT64,
    }
}

fn write_config_map(out: &mut Vec<u8>, values: &BTreeMap<String, IndexConfigValue>) -> Result<()> {
    write_len(out, values.len())?;
    for (key, value) in values {
        write_string(out, key)?;
        write_config_value(out, value)?;
    }
    Ok(())
}

fn write_config_value(out: &mut Vec<u8>, value: &IndexConfigValue) -> Result<()> {
    match value {
        IndexConfigValue::Number(value) => {
            out.push(CONFIG_NUMBER);
            write_f64(out, *value);
        }
        IndexConfigValue::Integer(value) => {
            out.push(CONFIG_INTEGER);
            write_i64(out, *value);
        }
        IndexConfigValue::String(value) => {
            out.push(CONFIG_STRING);
            write_string(out, value)?;
        }
        IndexConfigValue::Bool(value) => {
            out.push(CONFIG_BOOL);
            out.push(u8::from(*value));
        }
        IndexConfigValue::List(values) => {
            out.push(CONFIG_LIST);
            write_len(out, values.len())?;
            for value in values {
                write_config_value(out, value)?;
            }
        }
        IndexConfigValue::Map(values) => {
            out.push(CONFIG_MAP);
            write_config_map(out, values)?;
        }
        IndexConfigValue::Null => out.push(CONFIG_NULL),
    }
    Ok(())
}

fn write_property_value(out: &mut Vec<u8>, value: &PropertyValue) -> Result<()> {
    match value {
        PropertyValue::Null => out.push(VALUE_NULL),
        PropertyValue::Bool(value) => {
            out.push(VALUE_BOOL);
            out.push(u8::from(*value));
        }
        PropertyValue::Int(value) => {
            out.push(VALUE_INT);
            write_i64(out, *value);
        }
        PropertyValue::Float(value) => {
            out.push(VALUE_FLOAT);
            write_f64(out, *value);
        }
        PropertyValue::String(value) => {
            out.push(VALUE_STRING);
            write_string(out, value)?;
        }
        PropertyValue::List(values) => {
            out.push(VALUE_LIST);
            write_len(out, values.len())?;
            for value in values {
                write_property_value(out, value)?;
            }
        }
        PropertyValue::Map(values) => {
            out.push(VALUE_MAP);
            write_len(out, values.len())?;
            for (key, value) in values {
                write_string(out, key)?;
                write_property_value(out, value)?;
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
        PropertyValue::Binary(value) => {
            out.push(VALUE_BINARY);
            write_len(out, value.segments().len())?;
            for segment in value.segments() {
                write_bytes(out, segment)?;
            }
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

fn write_vector(out: &mut Vec<u8>, vector: &LoraVector) -> Result<()> {
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
                out.push(*value as u8);
            }
        }
    }
    Ok(())
}

fn write_optional_string(out: &mut Vec<u8>, value: Option<&str>) -> Result<()> {
    match value {
        Some(value) => {
            out.push(1);
            write_string(out, value)?;
        }
        None => out.push(0),
    }
    Ok(())
}

fn write_len(out: &mut Vec<u8>, len: usize) -> Result<()> {
    write_u64(
        out,
        u64::try_from(len)
            .map_err(|_| StoreCodecError::Encode("length does not fit in u64".into()))?,
    );
    Ok(())
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    write_len(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn write_string(out: &mut Vec<u8>, value: &str) -> Result<()> {
    write_bytes(out, value.as_bytes())
}

fn write_string_vec(out: &mut Vec<u8>, values: &[String]) -> Result<()> {
    write_len(out, values.len())?;
    for value in values {
        write_string(out, value)?;
    }
    Ok(())
}

fn write_i16(out: &mut Vec<u8>, value: i16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_bits().to_le_bytes());
}

fn write_f64(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&value.to_bits().to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn finish(&self) -> Result<()> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(StoreCodecError::Decode(format!(
                "trailing bytes: {}",
                self.bytes.len() - self.offset
            )))
        }
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| StoreCodecError::Decode("offset overflow".into()))?;
        if end > self.bytes.len() {
            return Err(StoreCodecError::Decode("truncated input".into()));
        }
        let out = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(out)
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.read_exact(N)?
            .try_into()
            .map_err(|_| StoreCodecError::Decode("fixed-width field truncated".into()))
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_i8(&mut self) -> Result<i8> {
        Ok(self.read_u8()? as i8)
    }

    fn read_i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.read_array()?))
    }

    fn read_i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.read_array()?))
    }

    fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read_array()?))
    }

    fn read_i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.read_array()?))
    }

    fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.read_array()?))
    }

    fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.read_u32()?))
    }

    fn read_f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.read_u64()?))
    }

    fn read_len(&mut self) -> Result<usize> {
        usize::try_from(self.read_u64()?)
            .map_err(|_| StoreCodecError::Decode("length overflows usize".into()))
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn read_len_bounded(&mut self, label: &str) -> Result<usize> {
        let len = self.read_len()?;
        if len > self.remaining() {
            return Err(StoreCodecError::Decode(format!(
                "{label} count {len} exceeds remaining input"
            )));
        }
        Ok(len)
    }

    fn vec_with_capacity<T>(&self, len: usize, label: &str) -> Result<Vec<T>> {
        let mut values = Vec::new();
        values.try_reserve(len).map_err(|_| {
            StoreCodecError::Decode(format!("{label} count {len} is too large to allocate"))
        })?;
        Ok(values)
    }

    fn read_bytes(&mut self) -> Result<&'a [u8]> {
        let len = self.read_len()?;
        self.read_exact(len)
    }

    fn read_string(&mut self) -> Result<String> {
        let bytes = self.read_bytes()?;
        std::str::from_utf8(bytes)
            .map(|value| value.to_string())
            .map_err(|e| StoreCodecError::Decode(format!("invalid UTF-8 string: {e}")))
    }

    fn read_string_vec(&mut self) -> Result<Vec<String>> {
        let len = self.read_len_bounded("string")?;
        let mut values = self.vec_with_capacity(len, "string")?;
        for _ in 0..len {
            values.push(self.read_string()?);
        }
        Ok(values)
    }

    fn read_optional_string(&mut self) -> Result<Option<String>> {
        match self.read_u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.read_string()?)),
            tag => Err(StoreCodecError::Decode(format!(
                "invalid optional string tag {tag}"
            ))),
        }
    }

    fn read_index_request(&mut self) -> Result<IndexRequest> {
        Ok(IndexRequest {
            explicit_name: self.read_optional_string()?,
            kind: self.read_index_kind()?,
            entity: self.read_index_entity()?,
            label: self.read_optional_string()?,
            additional_labels: self.read_string_vec()?,
            properties: self.read_string_vec()?,
            options: self.read_config_map()?,
        })
    }

    fn read_index_definition(&mut self) -> Result<IndexDefinition> {
        Ok(IndexDefinition {
            name: self.read_string()?,
            kind: self.read_index_kind()?,
            entity: self.read_index_entity()?,
            label: self.read_optional_string()?,
            additional_labels: self.read_string_vec()?,
            properties: self.read_string_vec()?,
            options: self.read_config_map()?,
            state: self.read_index_state()?,
        })
    }

    fn read_index_kind(&mut self) -> Result<StoredIndexKind> {
        match self.read_u8()? {
            INDEX_KIND_RANGE => Ok(StoredIndexKind::Range),
            INDEX_KIND_TEXT => Ok(StoredIndexKind::Text),
            INDEX_KIND_POINT => Ok(StoredIndexKind::Point),
            INDEX_KIND_LOOKUP => Ok(StoredIndexKind::Lookup),
            INDEX_KIND_VECTOR => Ok(StoredIndexKind::Vector),
            INDEX_KIND_FULLTEXT => Ok(StoredIndexKind::Fulltext),
            tag => Err(StoreCodecError::Decode(format!(
                "invalid index kind tag {tag}"
            ))),
        }
    }

    fn read_index_entity(&mut self) -> Result<StoredIndexEntity> {
        match self.read_u8()? {
            INDEX_ENTITY_NODE => Ok(StoredIndexEntity::Node),
            INDEX_ENTITY_RELATIONSHIP => Ok(StoredIndexEntity::Relationship),
            tag => Err(StoreCodecError::Decode(format!(
                "invalid index entity tag {tag}"
            ))),
        }
    }

    fn read_index_state(&mut self) -> Result<StoredIndexState> {
        match self.read_u8()? {
            INDEX_STATE_ONLINE => Ok(StoredIndexState::Online),
            INDEX_STATE_POPULATING => Ok(StoredIndexState::Populating),
            tag => Err(StoreCodecError::Decode(format!(
                "invalid index state tag {tag}"
            ))),
        }
    }

    fn read_constraint_request(&mut self) -> Result<ConstraintRequest> {
        Ok(ConstraintRequest {
            name: self.read_string()?,
            kind: self.read_constraint_kind()?,
            entity: self.read_index_entity()?,
            label: self.read_string()?,
            properties: self.read_string_vec()?,
        })
    }

    fn read_constraint_definition(&mut self) -> Result<ConstraintDefinition> {
        Ok(ConstraintDefinition {
            name: self.read_string()?,
            kind: self.read_constraint_kind()?,
            entity: self.read_index_entity()?,
            label: self.read_string()?,
            properties: self.read_string_vec()?,
            owned_index: self.read_optional_string()?,
        })
    }

    fn read_constraint_kind(&mut self) -> Result<StoredConstraintKind> {
        match self.read_u8()? {
            CONSTRAINT_KIND_UNIQUE => Ok(StoredConstraintKind::Unique),
            CONSTRAINT_KIND_EXISTENCE => Ok(StoredConstraintKind::Existence),
            CONSTRAINT_KIND_NODE_KEY => Ok(StoredConstraintKind::NodeKey),
            CONSTRAINT_KIND_RELATIONSHIP_KEY => Ok(StoredConstraintKind::RelationshipKey),
            CONSTRAINT_KIND_PROPERTY_TYPE => Ok(StoredConstraintKind::PropertyType(
                self.read_property_type()?,
            )),
            tag => Err(StoreCodecError::Decode(format!(
                "invalid constraint kind tag {tag}"
            ))),
        }
    }

    fn read_property_type(&mut self) -> Result<StoredPropertyType> {
        let len = self.read_len_bounded("property type alternative")?;
        let mut alternatives = self.vec_with_capacity(len, "property type alternative")?;
        for _ in 0..len {
            alternatives.push(self.read_property_type_term()?);
        }
        Ok(StoredPropertyType { alternatives })
    }

    fn read_property_type_term(&mut self) -> Result<StoredPropertyTypeTerm> {
        match self.read_u8()? {
            PROPERTY_TYPE_TERM_SCALAR => {
                Ok(StoredPropertyTypeTerm::Scalar(self.read_scalar_type()?))
            }
            PROPERTY_TYPE_TERM_LIST => {
                let not_null = self.read_u8()? != 0;
                let inner = Box::new(self.read_property_type_term()?);
                Ok(StoredPropertyTypeTerm::List { inner, not_null })
            }
            PROPERTY_TYPE_TERM_VECTOR => {
                let coord = self.read_vector_coord_type()?;
                let dimension = self.read_u32()?;
                Ok(StoredPropertyTypeTerm::Vector { coord, dimension })
            }
            tag => Err(StoreCodecError::Decode(format!(
                "invalid property-type term tag {tag}"
            ))),
        }
    }

    fn read_scalar_type(&mut self) -> Result<StoredScalarType> {
        match self.read_u8()? {
            SCALAR_BOOLEAN => Ok(StoredScalarType::Boolean),
            SCALAR_STRING => Ok(StoredScalarType::String),
            SCALAR_INTEGER => Ok(StoredScalarType::Integer),
            SCALAR_FLOAT => Ok(StoredScalarType::Float),
            SCALAR_DATE => Ok(StoredScalarType::Date),
            SCALAR_LOCAL_TIME => Ok(StoredScalarType::LocalTime),
            SCALAR_ZONED_TIME => Ok(StoredScalarType::ZonedTime),
            SCALAR_LOCAL_DATETIME => Ok(StoredScalarType::LocalDateTime),
            SCALAR_ZONED_DATETIME => Ok(StoredScalarType::ZonedDateTime),
            SCALAR_DURATION => Ok(StoredScalarType::Duration),
            SCALAR_POINT => Ok(StoredScalarType::Point),
            SCALAR_MAP => Ok(StoredScalarType::Map),
            SCALAR_ANY => Ok(StoredScalarType::Any),
            tag => Err(StoreCodecError::Decode(format!(
                "invalid scalar type tag {tag}"
            ))),
        }
    }

    fn read_vector_coord_type(&mut self) -> Result<StoredVectorCoordType> {
        match self.read_u8()? {
            VCOORD_INT8 => Ok(StoredVectorCoordType::Int8),
            VCOORD_INT16 => Ok(StoredVectorCoordType::Int16),
            VCOORD_INT32 => Ok(StoredVectorCoordType::Int32),
            VCOORD_INT64 => Ok(StoredVectorCoordType::Int64),
            VCOORD_FLOAT32 => Ok(StoredVectorCoordType::Float32),
            VCOORD_FLOAT64 => Ok(StoredVectorCoordType::Float64),
            tag => Err(StoreCodecError::Decode(format!(
                "invalid vector coord type tag {tag}"
            ))),
        }
    }

    fn read_config_map(&mut self) -> Result<BTreeMap<String, IndexConfigValue>> {
        let len = self.read_len_bounded("index config map entry")?;
        let mut values = BTreeMap::new();
        for _ in 0..len {
            values.insert(self.read_string()?, self.read_config_value()?);
        }
        Ok(values)
    }

    fn read_config_value(&mut self) -> Result<IndexConfigValue> {
        Ok(match self.read_u8()? {
            CONFIG_NUMBER => IndexConfigValue::Number(self.read_f64()?),
            CONFIG_INTEGER => IndexConfigValue::Integer(self.read_i64()?),
            CONFIG_STRING => IndexConfigValue::String(self.read_string()?),
            CONFIG_BOOL => IndexConfigValue::Bool(self.read_u8()? != 0),
            CONFIG_LIST => {
                let len = self.read_len_bounded("index config list value")?;
                let mut values = self.vec_with_capacity(len, "index config list value")?;
                for _ in 0..len {
                    values.push(self.read_config_value()?);
                }
                IndexConfigValue::List(values)
            }
            CONFIG_MAP => IndexConfigValue::Map(self.read_config_map()?),
            CONFIG_NULL => IndexConfigValue::Null,
            tag => {
                return Err(StoreCodecError::Decode(format!(
                    "invalid index config value tag {tag}"
                )));
            }
        })
    }

    fn read_property_value(&mut self) -> Result<PropertyValue> {
        Ok(match self.read_u8()? {
            VALUE_NULL => PropertyValue::Null,
            VALUE_BOOL => PropertyValue::Bool(self.read_u8()? != 0),
            VALUE_INT => PropertyValue::Int(self.read_i64()?),
            VALUE_FLOAT => PropertyValue::Float(self.read_f64()?),
            VALUE_STRING => PropertyValue::String(self.read_string()?),
            VALUE_LIST => {
                let len = self.read_len_bounded("property list value")?;
                let mut values = self.vec_with_capacity(len, "property list value")?;
                for _ in 0..len {
                    values.push(self.read_property_value()?);
                }
                PropertyValue::List(values)
            }
            VALUE_MAP => {
                let len = self.read_len_bounded("property map entry")?;
                let mut values = BTreeMap::new();
                for _ in 0..len {
                    values.insert(self.read_string()?, self.read_property_value()?);
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
                        return Err(StoreCodecError::Decode(format!(
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
            VALUE_BINARY => PropertyValue::Binary(self.read_binary()?),
            tag => {
                return Err(StoreCodecError::Decode(format!(
                    "invalid property value tag {tag}"
                )));
            }
        })
    }

    fn read_vector(&mut self) -> Result<LoraVector> {
        let dimension = self.read_len()?;
        let values = match self.read_u8()? {
            VECTOR_FLOAT64 => {
                read_vec(self, |reader| reader.read_f64()).map(VectorValues::Float64)?
            }
            VECTOR_FLOAT32 => {
                read_vec(self, |reader| reader.read_f32()).map(VectorValues::Float32)?
            }
            VECTOR_INTEGER64 => {
                read_vec(self, |reader| reader.read_i64()).map(VectorValues::Integer64)?
            }
            VECTOR_INTEGER32 => {
                read_vec(self, |reader| reader.read_i32()).map(VectorValues::Integer32)?
            }
            VECTOR_INTEGER16 => {
                read_vec(self, |reader| reader.read_i16()).map(VectorValues::Integer16)?
            }
            VECTOR_INTEGER8 => {
                read_vec(self, |reader| reader.read_i8()).map(VectorValues::Integer8)?
            }
            tag => {
                return Err(StoreCodecError::Decode(format!(
                    "unknown vector value tag {tag}"
                )))
            }
        };
        if values.len() != dimension {
            return Err(StoreCodecError::Decode(format!(
                "vector dimension mismatch: declared {dimension}, got {}",
                values.len()
            )));
        }
        Ok(LoraVector { dimension, values })
    }

    fn read_binary(&mut self) -> Result<LoraBinary> {
        let len = self.read_len_bounded("binary segment")?;
        let mut segments = self.vec_with_capacity(len, "binary segment")?;
        for _ in 0..len {
            segments.push(self.read_bytes()?.to_vec());
        }
        Ok(LoraBinary::from_segments(segments))
    }
}

fn read_vec<T>(
    reader: &mut Reader<'_>,
    mut read_one: impl FnMut(&mut Reader<'_>) -> Result<T>,
) -> Result<Vec<T>> {
    let len = reader.read_len()?;
    if len > reader.remaining() {
        return Err(StoreCodecError::Decode(format!(
            "vector value count {len} exceeds remaining input"
        )));
    }
    let mut values = reader.vec_with_capacity(len, "vector value")?;
    for _ in 0..len {
        values.push(read_one(reader)?);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IndexConfigValue, StoredIndexEntity, StoredIndexKind};

    #[test]
    fn property_value_roundtrips_nested_values() {
        let value = PropertyValue::Map(BTreeMap::from([
            ("name".into(), PropertyValue::String("Ada".into())),
            (
                "scores".into(),
                PropertyValue::List(vec![PropertyValue::Int(1), PropertyValue::Float(-2.5)]),
            ),
        ]));

        let bytes = encode_property_value(&value).unwrap();
        assert_eq!(decode_property_value(&bytes).unwrap(), value);
    }

    #[test]
    fn property_value_rejects_oversized_list_length() {
        let mut bytes = vec![VALUE_LIST];
        bytes.extend_from_slice(&u64::MAX.to_le_bytes());

        let err = decode_property_value(&bytes).unwrap_err();
        assert!(matches!(err, StoreCodecError::Decode(_)));
        assert!(err.to_string().contains("exceeds remaining input"));
    }

    #[test]
    fn index_request_roundtrips_options() {
        let request = IndexRequest {
            explicit_name: Some("idx_person_name".into()),
            kind: StoredIndexKind::Text,
            entity: StoredIndexEntity::Node,
            label: Some("Person".into()),
            additional_labels: Vec::new(),
            properties: vec!["name".into()],
            options: BTreeMap::from([(
                "indexConfig".into(),
                IndexConfigValue::Map(BTreeMap::from([(
                    "trigram.min".into(),
                    IndexConfigValue::Integer(3),
                )])),
            )]),
        };

        let bytes = encode_index_request(&request).unwrap();
        assert_eq!(decode_index_request(&bytes).unwrap(), request);
    }

    #[test]
    fn index_definition_vec_roundtrips_state() {
        let defs = vec![IndexDefinition {
            name: "idx_person_age".into(),
            kind: StoredIndexKind::Range,
            entity: StoredIndexEntity::Node,
            label: Some("Person".into()),
            additional_labels: Vec::new(),
            properties: vec!["age".into()],
            options: BTreeMap::new(),
            state: StoredIndexState::Online,
        }];

        let bytes = encode_index_definitions(&defs).unwrap();
        assert_eq!(decode_index_definitions(&bytes).unwrap(), defs);
    }
}
