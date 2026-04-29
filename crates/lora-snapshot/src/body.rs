use crate::error::{Result, SnapshotCodecError};
use crate::view::{StringTableView, U32ColumnView, U64ColumnView};

pub(crate) fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn write_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn write_len(out: &mut Vec<u8>, len: usize) -> Result<()> {
    write_u64(
        out,
        u64::try_from(len)
            .map_err(|_| SnapshotCodecError::Encode("length does not fit in u64".into()))?,
    );
    Ok(())
}

pub(crate) fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    write_len(out, bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

pub(crate) fn write_string(out: &mut Vec<u8>, value: &str) -> Result<()> {
    write_bytes(out, value.as_bytes())
}

pub(crate) fn write_string_vec(out: &mut Vec<u8>, values: &[String]) -> Result<()> {
    write_len(out, values.len())?;
    for value in values {
        write_string(out, value)?;
    }
    Ok(())
}

pub(crate) fn write_u32_vec(out: &mut Vec<u8>, values: &[u32]) {
    write_u64(out, values.len() as u64);
    out.reserve(values.len() * std::mem::size_of::<u32>());
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

pub(crate) fn write_u64_vec(out: &mut Vec<u8>, values: &[u64]) {
    write_u64(out, values.len() as u64);
    out.reserve(values.len() * std::mem::size_of::<u64>());
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

pub(crate) struct BodyReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> BodyReader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub(crate) fn finish(&self) -> Result<()> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(SnapshotCodecError::Decode(format!(
                "trailing bytes in snapshot body: {}",
                self.bytes.len() - self.offset
            )))
        }
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| SnapshotCodecError::Decode("snapshot body offset overflow".into()))?;
        if end > self.bytes.len() {
            return Err(SnapshotCodecError::Decode("truncated snapshot body".into()));
        }
        let out = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(out)
    }

    pub(crate) fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_exact(1)?[0])
    }

    pub(crate) fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read_exact(4)?.try_into().unwrap()))
    }

    pub(crate) fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.read_exact(8)?.try_into().unwrap()))
    }

    pub(crate) fn read_i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.read_exact(8)?.try_into().unwrap()))
    }

    pub(crate) fn read_len(&mut self) -> Result<usize> {
        usize::try_from(self.read_u64()?)
            .map_err(|_| SnapshotCodecError::Decode("length overflows usize".into()))
    }

    pub(crate) fn read_bytes(&mut self) -> Result<&'a [u8]> {
        let len = self.read_len()?;
        self.read_exact(len)
    }

    pub(crate) fn read_string(&mut self) -> Result<String> {
        let bytes = self.read_bytes()?;
        std::str::from_utf8(bytes)
            .map(|value| value.to_string())
            .map_err(|e| SnapshotCodecError::Decode(format!("invalid UTF-8 string: {e}")))
    }

    pub(crate) fn read_string_vec(&mut self) -> Result<Vec<String>> {
        let len = self.read_len()?;
        let mut values = Vec::with_capacity(len);
        for _ in 0..len {
            values.push(self.read_string()?);
        }
        Ok(values)
    }

    pub(crate) fn read_string_table_view(&mut self) -> Result<StringTableView<'a>> {
        let len = self.read_len()?;
        let mut entries = Vec::with_capacity(len);
        for _ in 0..len {
            let bytes = self.read_bytes()?;
            let value = std::str::from_utf8(bytes)
                .map_err(|e| SnapshotCodecError::Decode(format!("invalid UTF-8 string: {e}")))?;
            entries.push(value);
        }
        Ok(StringTableView::new(entries))
    }

    pub(crate) fn read_u32_vec(&mut self) -> Result<Vec<u32>> {
        let len = self.read_len()?;
        let mut values = Vec::with_capacity(len);
        for _ in 0..len {
            values.push(self.read_u32()?);
        }
        Ok(values)
    }

    pub(crate) fn read_u32_column_view(&mut self) -> Result<U32ColumnView<'a>> {
        let len = self.read_len()?;
        let byte_len = len
            .checked_mul(4)
            .ok_or_else(|| SnapshotCodecError::Decode("u32 column byte length overflow".into()))?;
        let bytes = self.read_exact(byte_len)?;
        Ok(U32ColumnView::new(bytes, len))
    }

    pub(crate) fn read_u64_vec(&mut self) -> Result<Vec<u64>> {
        let len = self.read_len()?;
        let mut values = Vec::with_capacity(len);
        for _ in 0..len {
            values.push(self.read_u64()?);
        }
        Ok(values)
    }

    pub(crate) fn read_u64_column_view(&mut self) -> Result<U64ColumnView<'a>> {
        let len = self.read_len()?;
        let byte_len = len
            .checked_mul(8)
            .ok_or_else(|| SnapshotCodecError::Decode("u64 column byte length overflow".into()))?;
        let bytes = self.read_exact(byte_len)?;
        Ok(U64ColumnView::new(bytes, len))
    }
}
