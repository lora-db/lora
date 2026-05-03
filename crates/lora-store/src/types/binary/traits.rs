//! Equality, hashing, and conversion traits for [`LoraBinary`].
//!
//! Equality and hash are *content-based* — two `LoraBinary` values that
//! hold the same logical bytes compare equal and hash the same, even if
//! one is segmented and the other isn't. That makes producer-side
//! chunking transparent to consumers.

use std::hash::{Hash, Hasher};

use super::types::LoraBinary;

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
