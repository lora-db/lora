use std::collections::BTreeMap;

use lora_store::{
    codec::{
        decode_constraint_definitions, decode_index_definitions, decode_property_value,
        encode_constraint_definitions, encode_index_definitions, encode_property_value,
    },
    ConstraintDefinition, IndexDefinition, NodeRecord, PropertyValue, RelationshipRecord,
    SnapshotPayload, VectorIndexSnapshot,
};
use serde::{Deserialize, Serialize};

use crate::body::{
    write_bytes, write_len, write_string, write_string_vec, write_u32, write_u32_vec, write_u64,
    write_u64_vec, BodyReader,
};
use crate::errors::{Result, SnapshotCodecError};
use crate::format::{
    BODY_FORMAT_VERSION, BODY_FORMAT_VERSION_V2, BODY_FORMAT_VERSION_V3, BODY_FORMAT_VERSION_V4,
};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ColumnarSnapshot {
    next_node_id: u64,
    next_rel_id: u64,
    node_ids: Vec<u64>,
    node_label_offsets: Vec<u32>,
    node_labels: Vec<String>,
    rel_ids: Vec<u64>,
    rel_src: Vec<u64>,
    rel_dst: Vec<u64>,
    rel_type_ids: Vec<u32>,
    rel_type_dictionary: Vec<String>,
    properties: PropertyColumns,
    /// Catalog of explicitly-declared indexes. Defaulted to empty so
    /// older `BODY_FORMAT_VERSION_V2` snapshots load with no entries.
    #[serde(default)]
    indexes: Vec<IndexDefinition>,
    /// Catalog of explicitly-declared constraints. Defaulted to empty
    /// so snapshots from before the constraint trailer was added still
    /// round-trip.
    #[serde(default)]
    constraints: Vec<ConstraintDefinition>,
    /// Persisted HNSW backend state. Defaulted to empty so snapshots
    /// from before V5 round-trip with the existing rebuild-on-load
    /// path.
    #[serde(default)]
    vector_indexes: Vec<VectorIndexSnapshot>,
}

impl ColumnarSnapshot {
    pub(crate) fn from_payload(payload: &SnapshotPayload, _wal_lsn: Option<u64>) -> Result<Self> {
        let (node_label_offsets, node_labels) = node_label_columns(&payload.nodes)?;
        let (rel_type_ids, rel_type_dictionary) =
            relationship_type_columns(&payload.relationships)?;
        let properties = property_columns_from_payload(payload);

        Ok(Self {
            next_node_id: payload.next_node_id,
            next_rel_id: payload.next_rel_id,
            node_ids: payload.nodes.iter().map(|node| node.id).collect(),
            node_label_offsets,
            node_labels,
            rel_ids: payload.relationships.iter().map(|rel| rel.id).collect(),
            rel_src: payload.relationships.iter().map(|rel| rel.src).collect(),
            rel_dst: payload.relationships.iter().map(|rel| rel.dst).collect(),
            rel_type_ids,
            rel_type_dictionary,
            properties,
            indexes: payload.indexes.clone(),
            constraints: payload.constraints.clone(),
            vector_indexes: payload.vector_indexes.clone(),
        })
    }

    pub(crate) fn into_payload(self) -> Result<SnapshotPayload> {
        self.validate_payload_columns()?;
        let mut nodes = self.node_records_from_columns()?;
        let mut relationships = self.relationship_records_from_columns()?;

        self.properties
            .attach_to_entities(&mut nodes, &mut relationships)?;

        Ok(SnapshotPayload {
            next_node_id: self.next_node_id,
            next_rel_id: self.next_rel_id,
            nodes,
            relationships,
            indexes: self.indexes,
            constraints: self.constraints,
            vector_indexes: self.vector_indexes,
        })
    }

    fn validate_payload_columns(&self) -> Result<()> {
        let expected_offsets = self
            .node_ids
            .len()
            .checked_add(1)
            .ok_or_else(|| SnapshotCodecError::Decode("node column length overflow".into()))?;
        if self.node_label_offsets.len() != expected_offsets {
            return Err(SnapshotCodecError::Decode(
                "node label offset length mismatch".into(),
            ));
        }
        if self.rel_ids.len() != self.rel_src.len()
            || self.rel_ids.len() != self.rel_dst.len()
            || self.rel_ids.len() != self.rel_type_ids.len()
        {
            return Err(SnapshotCodecError::Decode(
                "relationship column length mismatch".into(),
            ));
        }
        Ok(())
    }

    fn node_records_from_columns(&self) -> Result<Vec<NodeRecord>> {
        let mut nodes = Vec::with_capacity(self.node_ids.len());
        for (index, id) in self.node_ids.iter().copied().enumerate() {
            let start = u32_to_usize(self.node_label_offsets[index], "node label offset")?;
            let end = u32_to_usize(self.node_label_offsets[index + 1], "node label offset")?;
            if start > end || end > self.node_labels.len() {
                return Err(SnapshotCodecError::Decode(
                    "invalid node label offset".into(),
                ));
            }
            nodes.push(NodeRecord {
                id,
                labels: self.node_labels[start..end].to_vec(),
                properties: BTreeMap::new(),
            });
        }
        Ok(nodes)
    }

    fn relationship_records_from_columns(&self) -> Result<Vec<RelationshipRecord>> {
        let mut relationships = Vec::with_capacity(self.rel_ids.len());
        for index in 0..self.rel_ids.len() {
            let type_id = u32_to_usize(self.rel_type_ids[index], "relationship type id")?;
            let rel_type = self
                .rel_type_dictionary
                .get(type_id)
                .ok_or_else(|| SnapshotCodecError::Decode("invalid relationship type id".into()))?
                .clone();
            relationships.push(RelationshipRecord {
                id: self.rel_ids[index],
                src: self.rel_src[index],
                dst: self.rel_dst[index],
                rel_type,
                properties: BTreeMap::new(),
            });
        }
        Ok(relationships)
    }

    pub(crate) fn encode_binary(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        write_u32(&mut out, BODY_FORMAT_VERSION);
        write_u64(&mut out, self.next_node_id);
        write_u64(&mut out, self.next_rel_id);

        write_u64_vec(&mut out, &self.node_ids);

        let (label_dictionary, label_ids) = dictionary_encode_strings(&self.node_labels)?;
        write_string_vec(&mut out, &label_dictionary)?;
        write_u32_vec(&mut out, &self.node_label_offsets);
        write_u32_vec(&mut out, &label_ids);

        write_u64_vec(&mut out, &self.rel_ids);
        write_u64_vec(&mut out, &self.rel_src);
        write_u64_vec(&mut out, &self.rel_dst);
        write_string_vec(&mut out, &self.rel_type_dictionary)?;
        write_u32_vec(&mut out, &self.rel_type_ids);

        self.properties.encode_binary(&mut out)?;

        // v3 trailer: catalog of explicitly-declared indexes. The catalog
        // uses the store-owned binary codec so WAL and snapshots agree on
        // the shape without pulling in a general-purpose serializer.
        let catalog_bytes = encode_index_definitions(&self.indexes)
            .map_err(|e| SnapshotCodecError::Encode(format!("catalog: {e}")))?;
        write_bytes(&mut out, &catalog_bytes)?;

        // v4 trailer: catalog of explicitly-declared constraints.
        let constraint_bytes = encode_constraint_definitions(&self.constraints)
            .map_err(|e| SnapshotCodecError::Encode(format!("constraint catalog: {e}")))?;
        write_bytes(&mut out, &constraint_bytes)?;

        // v5 trailer: persisted HNSW backend state. JSON-encoded for
        // first iteration — a follow-up can swap to a hand-rolled
        // binary format inside the same length-prefixed bytes slot
        // without bumping the format version (only the encoder/
        // decoder pair changes, framing is stable).
        let vector_index_bytes = serde_json::to_vec(&self.vector_indexes).map_err(|e| {
            SnapshotCodecError::Encode(format!("vector index trailer: {e}"))
        })?;
        write_bytes(&mut out, &vector_index_bytes)?;
        Ok(out)
    }

    pub(crate) fn decode_binary(bytes: &[u8]) -> Result<Self> {
        let mut reader = BodyReader::new(bytes);
        let version = reader.read_u32()?;
        if version != BODY_FORMAT_VERSION
            && version != BODY_FORMAT_VERSION_V4
            && version != BODY_FORMAT_VERSION_V3
            && version != BODY_FORMAT_VERSION_V2
        {
            return Err(SnapshotCodecError::Decode(format!(
                "unsupported snapshot body format version {version}"
            )));
        }
        let has_catalog = version >= BODY_FORMAT_VERSION_V3;
        let has_constraints = version >= BODY_FORMAT_VERSION_V4;
        let has_vector_indexes = version >= BODY_FORMAT_VERSION;
        let next_node_id = reader.read_u64()?;
        let next_rel_id = reader.read_u64()?;
        let node_ids = reader.read_u64_vec()?;

        let label_dictionary = reader.read_string_vec()?;
        let node_label_offsets = reader.read_u32_vec()?;
        let label_ids = reader.read_u32_vec()?;
        let node_labels =
            decode_dictionary_strings(&label_dictionary, label_ids, "label dictionary")?;

        let rel_ids = reader.read_u64_vec()?;
        let rel_src = reader.read_u64_vec()?;
        let rel_dst = reader.read_u64_vec()?;
        let rel_type_dictionary = reader.read_string_vec()?;
        let rel_type_ids = reader.read_u32_vec()?;
        let properties = PropertyColumns::decode_binary(&mut reader)?;

        let indexes = if has_catalog {
            let catalog_bytes = reader.read_bytes()?;
            decode_index_definitions(catalog_bytes)
                .map_err(|e| SnapshotCodecError::Decode(format!("catalog: {e}")))?
        } else {
            Vec::new()
        };

        let constraints = if has_constraints {
            let bytes = reader.read_bytes()?;
            decode_constraint_definitions(bytes)
                .map_err(|e| SnapshotCodecError::Decode(format!("constraint catalog: {e}")))?
        } else {
            Vec::new()
        };

        let vector_indexes = if has_vector_indexes {
            let bytes = reader.read_bytes()?;
            if bytes.is_empty() {
                Vec::new()
            } else {
                serde_json::from_slice(bytes).map_err(|e| {
                    SnapshotCodecError::Decode(format!("vector index trailer: {e}"))
                })?
            }
        } else {
            Vec::new()
        };

        reader.finish()?;

        Ok(Self {
            next_node_id,
            next_rel_id,
            node_ids,
            node_label_offsets,
            node_labels,
            rel_ids,
            rel_src,
            rel_dst,
            rel_type_ids,
            rel_type_dictionary,
            properties,
            indexes,
            constraints,
            vector_indexes,
        })
    }
}

fn decode_dictionary_strings(
    dictionary: &[String],
    ids: Vec<u32>,
    name: &str,
) -> Result<Vec<String>> {
    let mut values = Vec::with_capacity(ids.len());
    for id in ids {
        let index = u32_to_usize(id, name)?;
        let value = dictionary
            .get(index)
            .ok_or_else(|| SnapshotCodecError::Decode(format!("invalid {name} id")))?;
        values.push(value.clone());
    }
    Ok(values)
}

fn node_label_columns(nodes: &[NodeRecord]) -> Result<(Vec<u32>, Vec<String>)> {
    let total_label_count = nodes.iter().try_fold(0usize, |total, node| {
        total
            .checked_add(node.labels.len())
            .ok_or_else(|| SnapshotCodecError::Encode("node label count overflows usize".into()))
    })?;
    let offset_count = nodes
        .len()
        .checked_add(1)
        .ok_or_else(|| SnapshotCodecError::Encode("node count overflows usize".into()))?;
    let mut offsets = Vec::new();
    offsets.try_reserve_exact(offset_count).map_err(|_| {
        SnapshotCodecError::Encode("node label offsets are too large to allocate".into())
    })?;
    let mut labels = Vec::new();
    labels
        .try_reserve_exact(total_label_count)
        .map_err(|_| SnapshotCodecError::Encode("node labels are too large to allocate".into()))?;
    offsets.push(0);
    for node in nodes {
        labels.extend(node.labels.iter().cloned());
        offsets.push(u32::try_from(labels.len()).map_err(|_| {
            SnapshotCodecError::Encode("node label offset exceeds u32 range".into())
        })?);
    }
    Ok((offsets, labels))
}

fn relationship_type_columns(
    relationships: &[RelationshipRecord],
) -> Result<(Vec<u32>, Vec<String>)> {
    let mut ids = Vec::new();
    ids.try_reserve_exact(relationships.len()).map_err(|_| {
        SnapshotCodecError::Encode("relationship type ids are too large to allocate".into())
    })?;
    let mut dictionary = Vec::new();
    let mut index = BTreeMap::<&str, u32>::new();

    for rel in relationships {
        let rel_type = rel.rel_type.as_str();
        let id = if let Some(id) = index.get(rel_type) {
            *id
        } else {
            let id = u32::try_from(dictionary.len()).map_err(|_| {
                SnapshotCodecError::Encode("relationship type dictionary too large".into())
            })?;
            dictionary.push(rel.rel_type.clone());
            index.insert(rel_type, id);
            id
        };
        ids.push(id);
    }

    Ok((ids, dictionary))
}

fn property_columns_from_payload(payload: &SnapshotPayload) -> PropertyColumns {
    let total_property_count = payload
        .nodes
        .iter()
        .map(|node| node.properties.len())
        .sum::<usize>()
        + payload
            .relationships
            .iter()
            .map(|rel| rel.properties.len())
            .sum::<usize>();
    let mut properties = PropertyColumns::with_capacity(total_property_count);

    for (owner_index, node) in payload.nodes.iter().enumerate() {
        properties.push_entity(EntityKind::Node, owner_index as u64, &node.properties);
    }
    for (owner_index, rel) in payload.relationships.iter().enumerate() {
        properties.push_entity(
            EntityKind::Relationship,
            owner_index as u64,
            &rel.properties,
        );
    }

    properties
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PropertyColumns {
    owner_kind: Vec<EntityKind>,
    owner_index: Vec<u64>,
    key: Vec<String>,
    value: Vec<ValueCell>,
}

impl PropertyColumns {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            owner_kind: Vec::with_capacity(capacity),
            owner_index: Vec::with_capacity(capacity),
            key: Vec::with_capacity(capacity),
            value: Vec::with_capacity(capacity),
        }
    }

    fn push_entity(
        &mut self,
        owner_kind: EntityKind,
        owner_index: u64,
        properties: &BTreeMap<String, PropertyValue>,
    ) {
        for (key, value) in properties {
            self.owner_kind.push(owner_kind);
            self.owner_index.push(owner_index);
            self.key.push(key.clone());
            self.value.push(ValueCell::from(value.clone()));
        }
    }

    fn attach_to_entities(
        self,
        nodes: &mut [NodeRecord],
        relationships: &mut [RelationshipRecord],
    ) -> Result<()> {
        let len = self.owner_kind.len();
        if len != self.owner_index.len() || len != self.key.len() || len != self.value.len() {
            return Err(SnapshotCodecError::Decode(
                "property column length mismatch".into(),
            ));
        }

        for (((owner_kind, owner_index), key), value) in self
            .owner_kind
            .into_iter()
            .zip(self.owner_index)
            .zip(self.key)
            .zip(self.value)
        {
            let value: PropertyValue = value.into();
            match owner_kind {
                EntityKind::Node => {
                    let owner_index = u64_to_usize(owner_index, "node property owner")?;
                    let node = nodes.get_mut(owner_index).ok_or_else(|| {
                        SnapshotCodecError::Decode("invalid node property owner".into())
                    })?;
                    node.properties.insert(key, value);
                }
                EntityKind::Relationship => {
                    let owner_index = u64_to_usize(owner_index, "relationship property owner")?;
                    let rel = relationships.get_mut(owner_index).ok_or_else(|| {
                        SnapshotCodecError::Decode("invalid relationship property owner".into())
                    })?;
                    rel.properties.insert(key, value);
                }
            }
        }
        Ok(())
    }

    fn encode_binary(&self, out: &mut Vec<u8>) -> Result<()> {
        let len = self.owner_kind.len();
        if len != self.owner_index.len() || len != self.key.len() || len != self.value.len() {
            return Err(SnapshotCodecError::Encode(
                "property column length mismatch".into(),
            ));
        }

        write_len(out, len)?;
        for kind in &self.owner_kind {
            out.push(match kind {
                EntityKind::Node => 0,
                EntityKind::Relationship => 1,
            });
        }
        write_u64_vec(out, &self.owner_index);

        let (key_dictionary, key_ids) = dictionary_encode_strings(&self.key)?;
        write_string_vec(out, &key_dictionary)?;
        write_u32_vec(out, &key_ids);

        write_len(out, self.value.len())?;
        for value in &self.value {
            value.encode_binary(out)?;
        }
        Ok(())
    }

    fn decode_binary(reader: &mut BodyReader<'_>) -> Result<Self> {
        let len = reader.read_len_bounded("property owner")?;
        let mut owner_kind = reader.vec_with_capacity(len, "property owner")?;
        for _ in 0..len {
            owner_kind.push(match reader.read_u8()? {
                0 => EntityKind::Node,
                1 => EntityKind::Relationship,
                tag => {
                    return Err(SnapshotCodecError::Decode(format!(
                        "invalid property owner kind tag {tag}"
                    )));
                }
            });
        }
        let owner_index = reader.read_u64_vec()?;
        if owner_index.len() != len {
            return Err(SnapshotCodecError::Decode(
                "property owner index length mismatch".into(),
            ));
        }

        let key_dictionary = reader.read_string_vec()?;
        let key_ids = reader.read_u32_vec()?;
        if key_ids.len() != len {
            return Err(SnapshotCodecError::Decode(
                "property key id length mismatch".into(),
            ));
        }
        let mut key = reader.vec_with_capacity(len, "property key")?;
        for id in key_ids {
            let index = u32_to_usize(id, "property key dictionary id")?;
            let value = key_dictionary.get(index).ok_or_else(|| {
                SnapshotCodecError::Decode("invalid property key dictionary id".into())
            })?;
            key.push(value.clone());
        }

        let value_len = reader.read_len()?;
        if value_len != len {
            return Err(SnapshotCodecError::Decode(
                "property value length mismatch".into(),
            ));
        }
        let mut value = reader.vec_with_capacity(len, "property value")?;
        for _ in 0..value_len {
            value.push(ValueCell::decode_binary(reader)?);
        }

        Ok(Self {
            owner_kind,
            owner_index,
            key,
            value,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum EntityKind {
    Node,
    Relationship,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ValueCell {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Binary(Vec<Vec<u8>>),
    List(Vec<ValueCell>),
    Map(Vec<(String, ValueCell)>),
    Extension(PropertyValue),
}

impl From<PropertyValue> for ValueCell {
    fn from(value: PropertyValue) -> Self {
        match value {
            PropertyValue::Null => Self::Null,
            PropertyValue::Bool(value) => Self::Bool(value),
            PropertyValue::Int(value) => Self::Int(value),
            PropertyValue::Float(value) => Self::Float(value),
            PropertyValue::String(value) => Self::String(value),
            PropertyValue::Binary(value) => Self::Binary(value.into_segments()),
            PropertyValue::List(values) => Self::List(values.into_iter().map(Self::from).collect()),
            PropertyValue::Map(values) => Self::Map(
                values
                    .into_iter()
                    .map(|(k, v)| (k, Self::from(v)))
                    .collect(),
            ),
            other => Self::Extension(other),
        }
    }
}

impl From<ValueCell> for PropertyValue {
    fn from(value: ValueCell) -> Self {
        match value {
            ValueCell::Null => Self::Null,
            ValueCell::Bool(value) => Self::Bool(value),
            ValueCell::Int(value) => Self::Int(value),
            ValueCell::Float(value) => Self::Float(value),
            ValueCell::String(value) => Self::String(value),
            ValueCell::Binary(value) => Self::Binary(lora_store::LoraBinary::from_segments(value)),
            ValueCell::List(values) => Self::List(values.into_iter().map(Self::from).collect()),
            ValueCell::Map(values) => Self::Map(
                values
                    .into_iter()
                    .map(|(k, v)| (k, Self::from(v)))
                    .collect(),
            ),
            ValueCell::Extension(value) => value,
        }
    }
}

impl ValueCell {
    fn encode_binary(&self, out: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::Null => out.push(0),
            Self::Bool(value) => {
                out.push(1);
                out.push(u8::from(*value));
            }
            Self::Int(value) => {
                out.push(2);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Self::Float(value) => {
                out.push(3);
                out.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            Self::String(value) => {
                out.push(4);
                write_string(out, value)?;
            }
            Self::Binary(segments) => {
                out.push(8);
                write_len(out, segments.len())?;
                for segment in segments {
                    write_bytes(out, segment)?;
                }
            }
            Self::List(values) => {
                out.push(5);
                write_len(out, values.len())?;
                for value in values {
                    value.encode_binary(out)?;
                }
            }
            Self::Map(values) => {
                out.push(6);
                write_len(out, values.len())?;
                for (key, value) in values {
                    write_string(out, key)?;
                    value.encode_binary(out)?;
                }
            }
            Self::Extension(value) => {
                out.push(7);
                let bytes = encode_property_value(value)
                    .map_err(|e| SnapshotCodecError::Encode(e.to_string()))?;
                write_bytes(out, &bytes)?;
            }
        }
        Ok(())
    }

    fn decode_binary(reader: &mut BodyReader<'_>) -> Result<Self> {
        match reader.read_u8()? {
            0 => Ok(Self::Null),
            1 => Ok(Self::Bool(reader.read_u8()? != 0)),
            2 => Ok(Self::Int(reader.read_i64()?)),
            3 => Ok(Self::Float(f64::from_bits(reader.read_u64()?))),
            4 => Ok(Self::String(reader.read_string()?)),
            8 => {
                let len = reader.read_len_bounded("binary segment")?;
                let mut segments = reader.vec_with_capacity(len, "binary segment")?;
                for _ in 0..len {
                    segments.push(reader.read_bytes()?.to_vec());
                }
                Ok(Self::Binary(segments))
            }
            5 => {
                let len = reader.read_len_bounded("list value")?;
                let mut values = reader.vec_with_capacity(len, "list value")?;
                for _ in 0..len {
                    values.push(Self::decode_binary(reader)?);
                }
                Ok(Self::List(values))
            }
            6 => {
                let len = reader.read_len_bounded("map entry")?;
                let mut values = reader.vec_with_capacity(len, "map entry")?;
                for _ in 0..len {
                    values.push((reader.read_string()?, Self::decode_binary(reader)?));
                }
                Ok(Self::Map(values))
            }
            7 => {
                let bytes = reader.read_bytes()?;
                let value = decode_property_value(bytes)
                    .map_err(|e| SnapshotCodecError::Decode(e.to_string()))?;
                Ok(Self::Extension(value))
            }
            tag => Err(SnapshotCodecError::Decode(format!(
                "invalid property value tag {tag}"
            ))),
        }
    }
}

fn u32_to_usize(value: u32, label: &str) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| SnapshotCodecError::Decode(format!("{label} does not fit in usize")))
}

fn u64_to_usize(value: u64, label: &str) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| SnapshotCodecError::Decode(format!("{label} does not fit in usize")))
}

fn dictionary_encode_strings(values: &[String]) -> Result<(Vec<String>, Vec<u32>)> {
    let mut dictionary = Vec::new();
    let mut index = BTreeMap::<&str, u32>::new();
    let mut ids = Vec::new();
    ids.try_reserve_exact(values.len()).map_err(|_| {
        SnapshotCodecError::Encode("string dictionary ids are too large to allocate".into())
    })?;
    for value in values {
        let id = if let Some(id) = index.get(value.as_str()) {
            *id
        } else {
            let id = u32::try_from(dictionary.len())
                .map_err(|_| SnapshotCodecError::Encode("string dictionary too large".into()))?;
            dictionary.push(value.clone());
            index.insert(value.as_str(), id);
            id
        };
        ids.push(id);
    }
    Ok((dictionary, ids))
}
