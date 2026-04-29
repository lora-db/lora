//! Segmented binary value support.

use std::hash::{Hash, Hasher};

/// A logical binary/blob value stored as one or more byte segments.
///
/// Small values usually have a single segment. Larger values can preserve
/// producer-side chunking so codecs can write/read length-prefixed segments
/// without building one large temporary buffer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoraBinary {
    segments: Vec<Vec<u8>>,
    len: usize,
}

impl LoraBinary {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let len = bytes.len();
        Self {
            segments: if bytes.is_empty() {
                Vec::new()
            } else {
                vec![bytes]
            },
            len,
        }
    }

    pub fn from_segments(segments: Vec<Vec<u8>>) -> Self {
        let len = segments.iter().map(Vec::len).sum();
        Self { segments, len }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn segments(&self) -> &[Vec<u8>] {
        &self.segments
    }

    pub fn chunks(&self) -> impl Iterator<Item = &[u8]> + '_ {
        self.segments.iter().map(Vec::as_slice)
    }

    pub fn into_segments(self) -> Vec<Vec<u8>> {
        self.segments
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len);
        for segment in self.chunks() {
            out.extend_from_slice(segment);
        }
        out
    }
}

impl PartialEq for LoraBinary {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self
                .chunks()
                .flat_map(|segment| segment.iter())
                .eq(other.chunks().flat_map(|segment| segment.iter()))
    }
}

impl Eq for LoraBinary {}

impl Hash for LoraBinary {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.len.hash(state);
        for segment in self.chunks() {
            state.write(segment);
        }
    }
}

impl From<Vec<u8>> for LoraBinary {
    fn from(value: Vec<u8>) -> Self {
        Self::from_bytes(value)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use super::*;

    #[test]
    fn equality_and_hash_use_logical_bytes_not_chunking() {
        let contiguous = LoraBinary::from_bytes(vec![1, 2, 3, 4]);
        let segmented = LoraBinary::from_segments(vec![vec![1, 2], vec![3], vec![4]]);

        assert_eq!(contiguous, segmented);
        assert_eq!(hash(&contiguous), hash(&segmented));
    }

    fn hash(value: &LoraBinary) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }
}
