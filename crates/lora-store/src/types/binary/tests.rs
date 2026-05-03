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
