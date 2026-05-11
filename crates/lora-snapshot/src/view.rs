use crate::body::BodyReader;
use crate::codec::SnapshotInfo;
use crate::errors::{Result, SnapshotCodecError};
use crate::format::BODY_FORMAT_VERSION;

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

    #[must_use]
    pub fn info(&self) -> &SnapshotInfo {
        &self.info
    }

    #[must_use]
    pub fn next_node_id(&self) -> u64 {
        self.body.next_node_id
    }

    #[must_use]
    pub fn next_rel_id(&self) -> u64 {
        self.body.next_rel_id
    }

    #[must_use]
    pub fn node_ids(&self) -> U64ColumnView<'a> {
        self.body.node_ids
    }

    #[must_use]
    pub fn relationship_ids(&self) -> U64ColumnView<'a> {
        self.body.rel_ids
    }

    #[must_use]
    pub fn relationship_sources(&self) -> U64ColumnView<'a> {
        self.body.rel_src
    }

    #[must_use]
    pub fn relationship_targets(&self) -> U64ColumnView<'a> {
        self.body.rel_dst
    }

    #[must_use]
    pub fn relationship_type_ids(&self) -> U32ColumnView<'a> {
        self.body.rel_type_ids
    }

    pub fn labels_for_node_index(
        &self,
        index: usize,
    ) -> Result<impl Iterator<Item = &'a str> + '_> {
        let start = u32_to_usize(
            self.body.node_label_offsets.get(index).ok_or_else(|| {
                SnapshotCodecError::Decode("node label offset out of bounds".into())
            })?,
            "node label offset",
        )?;
        let end = u32_to_usize(
            self.body.node_label_offsets.get(index + 1).ok_or_else(|| {
                SnapshotCodecError::Decode("node label offset out of bounds".into())
            })?,
            "node label offset",
        )?;
        if start > end || end > self.body.node_label_ids.len() {
            return Err(SnapshotCodecError::Decode(
                "invalid node label offset".into(),
            ));
        }
        Ok(self.body.node_label_ids.slice(start, end).map(move |id| {
            u32_to_usize(id, "label id")
                .ok()
                .and_then(|index| self.body.label_dictionary.get(index))
                .unwrap_or("<invalid-label>")
        }))
    }

    #[must_use]
    pub fn relationship_type(&self, type_id: u32) -> Option<&'a str> {
        self.body
            .rel_type_dictionary
            .get(usize::try_from(type_id).ok()?)
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
        let view = Self {
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
        };
        view.validate()?;
        Ok(view)
    }

    fn validate(&self) -> Result<()> {
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
        let mut previous = 0usize;
        for index in 0..self.node_label_offsets.len() {
            let offset = u32_to_usize(
                self.node_label_offsets.get(index).ok_or_else(|| {
                    SnapshotCodecError::Decode("node label offset out of bounds".into())
                })?,
                "node label offset",
            )?;
            if offset < previous || offset > self.node_label_ids.len() {
                return Err(SnapshotCodecError::Decode(
                    "invalid node label offset".into(),
                ));
            }
            previous = offset;
        }
        if previous != self.node_label_ids.len() {
            return Err(SnapshotCodecError::Decode(
                "node label offsets do not cover all label ids".into(),
            ));
        }
        for type_id in self.rel_type_ids.iter() {
            let type_id = u32_to_usize(type_id, "relationship type id")?;
            if self.rel_type_dictionary.get(type_id).is_none() {
                return Err(SnapshotCodecError::Decode(
                    "invalid relationship type id".into(),
                ));
            }
        }
        for label_id in self.node_label_ids.iter() {
            let label_id = u32_to_usize(label_id, "label id")?;
            if self.label_dictionary.get(label_id).is_none() {
                return Err(SnapshotCodecError::Decode("invalid label id".into()));
            }
        }
        Ok(())
    }
}

fn u32_to_usize(value: u32, label: &str) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| SnapshotCodecError::Decode(format!("{label} does not fit in usize")))
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

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<u64> {
        if index >= self.len {
            return None;
        }
        let start = index.checked_mul(8)?;
        let end = start.checked_add(8)?;
        Some(u64::from_le_bytes(
            self.bytes.get(start..end)?.try_into().ok()?,
        ))
    }

    pub fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        (0..self.len).filter_map(|index| self.get(index))
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

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<u32> {
        if index >= self.len {
            return None;
        }
        let start = index.checked_mul(4)?;
        let end = start.checked_add(4)?;
        Some(u32::from_le_bytes(
            self.bytes.get(start..end)?.try_into().ok()?,
        ))
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.len).filter_map(|index| self.get(index))
    }

    pub(crate) fn slice(&self, start: usize, end: usize) -> impl Iterator<Item = u32> + '_ {
        (start..end).filter_map(|index| self.get(index))
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

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&'a str> {
        self.entries.get(index).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.entries.iter().copied()
    }
}
