//! A copy-on-write set of ids, stored like a Roaring bitmap.
//!
//! Backs the label and relationship-type indexes, which hold every node
//! and relationship id once per label or type. A flat list costs 8 bytes
//! per id; here ids are grouped by their high bits into blocks of
//! [`BLOCK`] ids, and each block is either a sorted array of the low 16
//! bits (2 bytes per id) or, once it holds more than [`ARRAY_MAX`] ids, a
//! bitmap of [`BLOCK`] bits (8 KB, an eighth of a byte per possible id).
//! A label every node carries costs about 0.13 bytes per node.
//!
//! ```text
//! IdBitmap: Arc<[block entry]>  →  entry: (high bits, count, Arc<Block>)
//!                                   Block: Array(Vec<u16>) | Bits([u64; 1024])
//! ```
//!
//! Cloning bumps one refcount. The first write after a clone copies the
//! entry table (one entry per 65,536 ids of range) and the one block it
//! touches, at most 8 KB. Ids iterate in ascending order, and inserting
//! or removing an id is a binary search, not a scan of the list.

use super::ARC_HEADER;
use std::sync::Arc;

/// Ids per block: the low 16 bits of an id address it within its block.
const BLOCK_SHIFT: u32 = 16;
/// Bitmap words per block.
const WORDS: usize = (1 << BLOCK_SHIFT) / 64;
/// A block holding more ids than this is a bitmap: past 4096 ids the
/// sorted array would outgrow the 8 KB bitmap.
const ARRAY_MAX: usize = 4096;
/// A bitmap block that falls below this many ids becomes an array again.
/// Lower than [`ARRAY_MAX`] so a block hovering at the boundary does not
/// convert on every write.
const ARRAY_MIN: usize = 2048;

#[derive(Clone)]
enum Block {
    /// Low bits, ascending and distinct.
    Array(Vec<u16>),
    Bits(Box<[u64; WORDS]>),
}

#[derive(Clone)]
struct Entry {
    high: u64,
    count: u32,
    block: Arc<Block>,
}

#[derive(Clone, Default)]
pub(super) struct IdBitmap {
    /// Ascending by `high`; no entry is empty.
    blocks: Arc<Vec<Entry>>,
    len: usize,
}

#[inline]
fn split(id: u64) -> (u64, u16) {
    (id >> BLOCK_SHIFT, id as u16)
}

impl Block {
    /// Insert `low`; returns whether it was absent. `count` is the block's
    /// size before the insert.
    fn insert(&mut self, low: u16, count: usize) -> bool {
        match self {
            Block::Array(lows) => {
                // Creates hand out ascending ids, so this is the common case.
                let pos = match lows.last() {
                    Some(&last) if last < low => lows.len(),
                    None => 0,
                    _ => match lows.binary_search(&low) {
                        Ok(_) => return false,
                        Err(pos) => pos,
                    },
                };
                if count == ARRAY_MAX {
                    let mut bits = Box::new([0u64; WORDS]);
                    for &l in lows.iter() {
                        bits[l as usize >> 6] |= 1 << (l & 63);
                    }
                    bits[low as usize >> 6] |= 1 << (low & 63);
                    *self = Block::Bits(bits);
                } else {
                    lows.insert(pos, low);
                }
                true
            }
            Block::Bits(bits) => {
                let word = &mut bits[low as usize >> 6];
                let mask = 1 << (low & 63);
                let absent = *word & mask == 0;
                *word |= mask;
                absent
            }
        }
    }

    /// Remove `low`; returns whether it was present. `count` is the
    /// block's size before the remove.
    fn remove(&mut self, low: u16, count: usize) -> bool {
        match self {
            Block::Array(lows) => match lows.binary_search(&low) {
                Ok(pos) => {
                    lows.remove(pos);
                    true
                }
                Err(_) => false,
            },
            Block::Bits(bits) => {
                let word = &mut bits[low as usize >> 6];
                let mask = 1 << (low & 63);
                if *word & mask == 0 {
                    return false;
                }
                *word &= !mask;
                if count - 1 < ARRAY_MIN {
                    let lows: Vec<u16> = BlockIter::new(self, 0).map(|l| l as u16).collect();
                    *self = Block::Array(lows);
                }
                true
            }
        }
    }

    fn heap_bytes(&self) -> usize {
        match self {
            Block::Array(lows) => lows.capacity() * size_of::<u16>(),
            Block::Bits(_) => WORDS * size_of::<u64>(),
        }
    }
}

/// The low bits of one block's ids from `from` on, ascending.
enum BlockIter<'a> {
    Array(std::slice::Iter<'a, u16>),
    Bits {
        bits: &'a [u64; WORDS],
        word: usize,
        current: u64,
    },
}

impl<'a> BlockIter<'a> {
    fn new(block: &'a Block, from: u16) -> Self {
        match block {
            Block::Array(lows) => {
                let start = lows.partition_point(|&l| l < from);
                BlockIter::Array(lows[start..].iter())
            }
            Block::Bits(bits) => {
                let word = from as usize >> 6;
                BlockIter::Bits {
                    bits,
                    word,
                    current: bits[word] & (u64::MAX << (from & 63)),
                }
            }
        }
    }
}

impl Iterator for BlockIter<'_> {
    type Item = u64;

    #[inline]
    fn next(&mut self) -> Option<u64> {
        match self {
            BlockIter::Array(iter) => iter.next().map(|&l| l as u64),
            BlockIter::Bits {
                bits,
                word,
                current,
            } => {
                while *current == 0 {
                    *word += 1;
                    if *word == WORDS {
                        return None;
                    }
                    *current = bits[*word];
                }
                let bit = current.trailing_zeros() as u64;
                *current &= *current - 1;
                Some(((*word as u64) << 6) | bit)
            }
        }
    }
}

/// Ids of an [`IdBitmap`] in ascending order.
pub(super) struct IdBitmapIter<'a> {
    rest: std::slice::Iter<'a, Entry>,
    base: u64,
    block: Option<BlockIter<'a>>,
}

impl Iterator for IdBitmapIter<'_> {
    type Item = u64;

    #[inline]
    fn next(&mut self) -> Option<u64> {
        loop {
            if let Some(low) = self.block.as_mut()?.next() {
                return Some(self.base | low);
            }
            let entry = self.rest.next();
            self.block = entry.map(|e| BlockIter::new(&e.block, 0));
            self.base = entry.map_or(0, |e| e.high << BLOCK_SHIFT);
        }
    }
}

impl IdBitmap {
    pub(super) fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub(super) fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Insert `id`; returns whether it was absent.
    pub(super) fn insert(&mut self, id: u64) -> bool {
        let (high, low) = split(id);
        // Look the id up before copying anything: inserting an id that is
        // already there must not copy a table or block a clone shares.
        let found = match self.blocks.last() {
            Some(last) if last.high == high => Ok(self.blocks.len() - 1),
            Some(last) if last.high > high => self.blocks.binary_search_by_key(&high, |e| e.high),
            _ => Err(self.blocks.len()),
        };
        if found.is_ok_and(|index| self.blocks[index].contains(low)) {
            return false;
        }
        let blocks = Arc::make_mut(&mut self.blocks);
        let index = found.unwrap_or_else(|index| {
            blocks.insert(index, Entry::empty(high));
            index
        });
        let entry = &mut blocks[index];
        let inserted = Arc::make_mut(&mut entry.block).insert(low, entry.count as usize);
        debug_assert!(inserted, "absent above");
        entry.count += 1;
        self.len += 1;
        true
    }

    /// Remove `id`; returns whether it was present.
    pub(super) fn remove(&mut self, id: u64) -> bool {
        let (high, low) = split(id);
        let Ok(index) = self.blocks.binary_search_by_key(&high, |e| e.high) else {
            return false;
        };
        if !self.blocks[index].contains(low) {
            return false;
        }
        let blocks = Arc::make_mut(&mut self.blocks);
        let entry = &mut blocks[index];
        if entry.count == 1 {
            blocks.remove(index);
        } else {
            Arc::make_mut(&mut entry.block).remove(low, entry.count as usize);
            entry.count -= 1;
        }
        self.len -= 1;
        true
    }

    #[cfg(test)]
    pub(super) fn contains(&self, id: u64) -> bool {
        let (high, low) = split(id);
        self.blocks
            .binary_search_by_key(&high, |e| e.high)
            .is_ok_and(|index| self.blocks[index].contains(low))
    }

    pub(super) fn iter(&self) -> IdBitmapIter<'_> {
        self.iter_from(0)
    }

    /// The ids `>= from`, ascending.
    pub(super) fn iter_from(&self, from: u64) -> IdBitmapIter<'_> {
        let (high, low) = split(from);
        let start = self.blocks.partition_point(|e| e.high < high);
        let mut rest = self.blocks[start..].iter();
        let first = rest.next();
        let block = first.map(|e| BlockIter::new(&e.block, if e.high == high { low } else { 0 }));
        IdBitmapIter {
            rest,
            base: first.map_or(0, |e| e.high << BLOCK_SHIFT),
            block,
        }
    }

    pub(super) fn to_vec(&self) -> Vec<u64> {
        let mut out = Vec::with_capacity(self.len);
        out.extend(self.iter());
        out
    }

    /// Heap bytes: the entry table and every block.
    pub(super) fn heap_bytes(&self) -> usize {
        ARC_HEADER
            + size_of::<Vec<Entry>>()
            + self.blocks.capacity() * size_of::<Entry>()
            + self
                .blocks
                .iter()
                .map(|e| ARC_HEADER + size_of::<Block>() + e.block.heap_bytes())
                .sum::<usize>()
    }
}

impl Entry {
    fn empty(high: u64) -> Self {
        Entry {
            high,
            count: 0,
            block: Arc::new(Block::Array(Vec::new())),
        }
    }

    #[inline]
    fn contains(&self, low: u16) -> bool {
        match &*self.block {
            // Creates hand out ascending ids: past the last one is common.
            Block::Array(lows) => match lows.last() {
                Some(&last) if last < low => false,
                _ => lows.binary_search(&low).is_ok(),
            },
            Block::Bits(bits) => bits[low as usize >> 6] & (1 << (low & 63)) != 0,
        }
    }
}

impl FromIterator<u64> for IdBitmap {
    fn from_iter<I: IntoIterator<Item = u64>>(iter: I) -> Self {
        let mut out = Self::new();
        for id in iter {
            out.insert(id);
        }
        out
    }
}

impl std::fmt::Debug for IdBitmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl PartialEq for IdBitmap {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

#[cfg(test)]
impl IdBitmap {
    /// Panic unless the blocks are ascending, non-empty, counted right and
    /// in the representation their size calls for.
    fn check_invariants(&self) {
        assert!(
            self.blocks.windows(2).all(|w| w[0].high < w[1].high),
            "ascending blocks"
        );
        let mut len = 0;
        for entry in self.blocks.iter() {
            let count = match &*entry.block {
                Block::Array(lows) => {
                    assert!(
                        lows.windows(2).all(|w| w[0] < w[1]),
                        "sorted, distinct lows"
                    );
                    assert!(lows.len() <= ARRAY_MAX, "an array past ARRAY_MAX");
                    lows.len()
                }
                Block::Bits(bits) => {
                    let count = bits.iter().map(|w| w.count_ones() as usize).sum();
                    assert!(count >= ARRAY_MIN, "a bitmap below ARRAY_MIN");
                    count
                }
            };
            assert!(count > 0, "an empty block");
            assert_eq!(count, entry.count as usize, "block count");
            len += count;
        }
        assert_eq!(len, self.len, "len");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::test_util::splitmix;
    use std::collections::BTreeSet;

    fn check(set: &IdBitmap, model: &BTreeSet<u64>) {
        set.check_invariants();
        assert_eq!(set.len(), model.len());
        assert!(set.iter().eq(model.iter().copied()));
        for from in [0, 1, 4095, 65_535, 65_536, 70_000, 200_000, u64::MAX] {
            assert!(
                set.iter_from(from).eq(model.range(from..).copied()),
                "from {from}"
            );
        }
    }

    #[test]
    fn matches_a_btreeset_through_array_and_bitmap_blocks() {
        let mut set = IdBitmap::new();
        let mut model = BTreeSet::new();
        // Dense ascending ids fill block 0 past ARRAY_MAX (a bitmap), sparse
        // ones keep later blocks as arrays.
        for id in 0..10_000 {
            assert_eq!(set.insert(id), model.insert(id));
        }
        for i in 0..5_000 {
            let id = splitmix(i) % 400_000;
            assert_eq!(set.insert(id), model.insert(id));
        }
        check(&set, &model);
        let snapshot = set.clone();
        let snapshot_model = model.clone();
        for i in 0..12_000 {
            let id = splitmix(i ^ 0xABCD) % 400_000;
            if i % 3 == 0 {
                assert_eq!(set.insert(id), model.insert(id));
            } else {
                assert_eq!(set.remove(id), model.remove(&id));
            }
            assert_eq!(set.contains(id), model.contains(&id));
            if i % 97 == 0 {
                set.check_invariants();
            }
        }
        // Remove most of the dense block so it turns back into an array.
        for id in 0..9_000 {
            assert_eq!(set.remove(id), model.remove(&id));
        }
        check(&set, &model);
        // The clone taken earlier is unaffected.
        check(&snapshot, &snapshot_model);
        // The ends of the id space.
        for id in [u64::MAX, u64::MAX - 1, 1 << 63] {
            assert_eq!(set.insert(id), model.insert(id));
        }
        check(&set, &model);
        for id in model.clone() {
            assert!(set.remove(id));
        }
        assert!(set.is_empty() && set.blocks.is_empty());
    }

    #[test]
    fn inserting_a_present_id_copies_nothing_a_clone_shares() {
        let mut set: IdBitmap = (0..10_000).chain([1 << 20]).collect();
        let copy = set.clone();
        for id in [0, 4_097, 9_999, 1 << 20] {
            assert!(!set.insert(id));
        }
        assert!(Arc::ptr_eq(&set.blocks, &copy.blocks));
        assert!(set.insert(10_000));
        assert!(!Arc::ptr_eq(&set.blocks, &copy.blocks));
        assert_eq!((set.len(), copy.len()), (10_002, 10_001));
    }

    #[test]
    fn a_bitmap_block_converts_back_below_array_min() {
        let mut set: IdBitmap = (0..ARRAY_MAX as u64 + 1).collect();
        assert!(matches!(*set.blocks[0].block, Block::Bits(_)));
        for id in 0..(ARRAY_MAX - ARRAY_MIN + 2) as u64 {
            set.remove(id);
        }
        assert!(matches!(*set.blocks[0].block, Block::Array(_)));
        assert!(set
            .iter()
            .eq((ARRAY_MAX - ARRAY_MIN + 2) as u64..ARRAY_MAX as u64 + 1));
    }

    #[test]
    fn a_dense_set_costs_an_eighth_of_a_byte_per_id() {
        let set: IdBitmap = (0..1_000_000).collect();
        assert!(set.heap_bytes() < 1_000_000 / 7, "{}", set.heap_bytes());
    }
}
