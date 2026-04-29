use crate::body::BodyReader;
use crate::error::{Result, SnapshotCodecError};
use crate::{SnapshotInfo, BODY_FORMAT_VERSION};

#[derive(Debug, Clone)]
pub struct SnapshotView<'a> {
    info: SnapshotInfo,
    body: BodyView<'a>,
}

impl<'a> SnapshotView<'a> {
    pub(crate) fn parse(info: SnapshotInfo, bytes: &'a [u8]) -> Result<Self> {
        Ok(Self {
            info,
            body: BodyView::parse(bytes)?,
        })
    }

    pub fn info(&self) -> &SnapshotInfo {
        &self.info
    }

    pub fn next_node_id(&self) -> u64 {
        self.body.next_node_id
    }

    pub fn next_rel_id(&self) -> u64 {
        self.body.next_rel_id
    }

    pub fn node_ids(&self) -> U64ColumnView<'a> {
        self.body.node_ids
    }

    pub fn relationship_ids(&self) -> U64ColumnView<'a> {
        self.body.rel_ids
    }

    pub fn relationship_sources(&self) -> U64ColumnView<'a> {
        self.body.rel_src
    }

    pub fn relationship_targets(&self) -> U64ColumnView<'a> {
        self.body.rel_dst
    }

    pub fn relationship_type_ids(&self) -> U32ColumnView<'a> {
        self.body.rel_type_ids
    }

    pub fn labels_for_node_index(
        &self,
        index: usize,
    ) -> Result<impl Iterator<Item = &'a str> + '_> {
        let start =
            self.body.node_label_offsets.get(index).ok_or_else(|| {
                SnapshotCodecError::Decode("node label offset out of bounds".into())
            })? as usize;
        let end =
            self.body.node_label_offsets.get(index + 1).ok_or_else(|| {
                SnapshotCodecError::Decode("node label offset out of bounds".into())
            })? as usize;
        if start > end || end > self.body.node_label_ids.len() {
            return Err(SnapshotCodecError::Decode(
                "invalid node label offset".into(),
            ));
        }
        Ok(self.body.node_label_ids.slice(start, end).map(move |id| {
            self.body
                .label_dictionary
                .get(id as usize)
                .unwrap_or("<invalid-label>")
        }))
    }

    pub fn relationship_type(&self, type_id: u32) -> Option<&'a str> {
        self.body.rel_type_dictionary.get(type_id as usize)
    }
}

#[derive(Debug, Clone)]
struct BodyView<'a> {
    next_node_id: u64,
    next_rel_id: u64,
    node_ids: U64ColumnView<'a>,
    label_dictionary: StringTableView<'a>,
    node_label_offsets: U32ColumnView<'a>,
    node_label_ids: U32ColumnView<'a>,
    rel_ids: U64ColumnView<'a>,
    rel_src: U64ColumnView<'a>,
    rel_dst: U64ColumnView<'a>,
    rel_type_dictionary: StringTableView<'a>,
    rel_type_ids: U32ColumnView<'a>,
}

impl<'a> BodyView<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self> {
        let mut reader = BodyReader::new(bytes);
        let version = reader.read_u32()?;
        if version != BODY_FORMAT_VERSION {
            return Err(SnapshotCodecError::Decode(format!(
                "unsupported snapshot body format version {version}"
            )));
        }
        let next_node_id = reader.read_u64()?;
        let next_rel_id = reader.read_u64()?;
        let node_ids = reader.read_u64_column_view()?;
        let label_dictionary = reader.read_string_table_view()?;
        let node_label_offsets = reader.read_u32_column_view()?;
        let node_label_ids = reader.read_u32_column_view()?;
        let rel_ids = reader.read_u64_column_view()?;
        let rel_src = reader.read_u64_column_view()?;
        let rel_dst = reader.read_u64_column_view()?;
        let rel_type_dictionary = reader.read_string_table_view()?;
        let rel_type_ids = reader.read_u32_column_view()?;
        Ok(Self {
            next_node_id,
            next_rel_id,
            node_ids,
            label_dictionary,
            node_label_offsets,
            node_label_ids,
            rel_ids,
            rel_src,
            rel_dst,
            rel_type_dictionary,
            rel_type_ids,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct U64ColumnView<'a> {
    bytes: &'a [u8],
    len: usize,
}

impl<'a> U64ColumnView<'a> {
    pub(crate) fn new(bytes: &'a [u8], len: usize) -> Self {
        Self { bytes, len }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<u64> {
        if index >= self.len {
            return None;
        }
        let start = index * 8;
        Some(u64::from_le_bytes(
            self.bytes[start..start + 8].try_into().unwrap(),
        ))
    }

    pub fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        (0..self.len).map(|index| self.get(index).unwrap())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct U32ColumnView<'a> {
    bytes: &'a [u8],
    len: usize,
}

impl<'a> U32ColumnView<'a> {
    pub(crate) fn new(bytes: &'a [u8], len: usize) -> Self {
        Self { bytes, len }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<u32> {
        if index >= self.len {
            return None;
        }
        let start = index * 4;
        Some(u32::from_le_bytes(
            self.bytes[start..start + 4].try_into().unwrap(),
        ))
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.len).map(|index| self.get(index).unwrap())
    }

    pub(crate) fn slice(&self, start: usize, end: usize) -> impl Iterator<Item = u32> + '_ {
        (start..end).map(|index| self.get(index).unwrap())
    }
}

#[derive(Debug, Clone)]
pub struct StringTableView<'a> {
    entries: Vec<&'a str>,
}

impl<'a> StringTableView<'a> {
    pub(crate) fn new(entries: Vec<&'a str>) -> Self {
        Self { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Option<&'a str> {
        self.entries.get(index).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.entries.iter().copied()
    }
}
