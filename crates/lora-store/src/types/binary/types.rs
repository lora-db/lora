//! `LoraBinary` definition + constructor / accessor surface.

/// A logical binary/blob value stored as one or more byte segments.
///
/// Small values usually have a single segment. Larger values can preserve
/// producer-side chunking so codecs can write/read length-prefixed segments
/// without building one large temporary buffer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoraBinary {
    pub(super) segments: Vec<Vec<u8>>,
    pub(super) len: usize,
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
