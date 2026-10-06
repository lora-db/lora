//! Copy-on-write collections for secondary indexes.
//!
//! Writes that cannot run in place work on a clone of the graph, so an
//! index clone has to be cheap, and a write should copy only the part of
//! an index it changes. Both shapes here are persistent trees of bounded
//! nodes behind `Arc`: cloning bumps one refcount, and a mutation copies
//! the path from the root to the entry it changes (`Arc::make_mut` at each
//! level), a bounded number of bounded nodes whatever the size.
//!
//! A flat table of shards is not enough: with a fixed shard count a write
//! copies one shard, which grows with the map (8k entries per shard at
//! 2M keys), and with a fixed shard size it copies the shard table, which
//! grows instead. Either way a staged single-node CREATE on a 2M-node
//! graph spent ~200 µs copying index shards.
//!
//! - [`CowMap`] is a hash trie: a leaf is a small hash table, and one that
//!   outgrows [`LEAF_MAX`] entries splits into [`FAN`] children by the next
//!   [`BITS`] bits of the key's hash. A write copies one branch of [`FAN`]
//!   pointers per level (three levels at 2M keys) and one leaf of 14 to
//!   448 entries. No key order (the trigram, point-cell, hash-bucket and
//!   per-entity maps only need lookups).
//! - Posting lists keyed by entity id ([`super::id_map::CowIdMap`],
//!   [`super::id_set::IdSet`]) are sorted chunks under two levels of
//!   tables instead: ids mostly arrive in ascending order, so an insert is
//!   an append, and readers get them sorted.
//! - [`CowOrdMap`] keeps key order, for range and prefix scans: sorted
//!   partitions of at most [`PART_MAX`] keys, grouped [`GROUP_MAX`] to a
//!   group, under one root table. A write copies the root (one entry per
//!   8k to 32k keys), one group and one partition.
//!
//! Both are also cheaper to read than the flat tables were: lookups
//! binary-search contiguous minimum keys (ordered) or probe small tables
//! that keys spread over evenly (hashed).

use std::borrow::Borrow;
use std::collections::{hash_map, HashMap};
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::sync::Arc;

/// `std::sync::Arc<T>`: strong + weak refcount header before the value.
const ARC_HEADER: usize = 2 * std::mem::size_of::<usize>();

/// The hasher for every key here: deterministic, and cheap for the short
/// keys indexes hold (integers, grid cells, trigrams, short strings).
/// Byte strings go through FNV-1a; integers (an `Int` key, an enum
/// discriminant, a length prefix) are mixed in a word at a time instead of
/// eight FNV rounds, and `finish` avalanches the state (murmur3's
/// finalizer) so the low bits a hash table indexes by depend on every
/// input bit: a `Float` key's bits differ only at the top.
#[derive(Default)]
pub(super) struct KeyHasher(u64);

impl Hasher for KeyHasher {
    #[inline]
    fn finish(&self) -> u64 {
        let mut h = self.0;
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^ (h >> 33)
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            self.0 = 0xcbf2_9ce4_8422_2325;
        }
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    #[inline]
    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    #[inline]
    fn write_u8(&mut self, x: u8) {
        self.write_u64(u64::from(x));
    }

    #[inline]
    fn write_u16(&mut self, x: u16) {
        self.write_u64(u64::from(x));
    }

    #[inline]
    fn write_u32(&mut self, x: u32) {
        self.write_u64(u64::from(x));
    }

    #[inline]
    fn write_usize(&mut self, x: usize) {
        self.write_u64(x as u64);
    }

    #[inline]
    fn write_i64(&mut self, x: i64) {
        self.write_u64(x as u64);
    }

    #[inline]
    fn write_i32(&mut self, x: i32) {
        self.write_u64(x as u32 as u64);
    }

    #[inline]
    fn write_isize(&mut self, x: isize) {
        self.write_u64(x as u64);
    }
}

/// A `HashMap` with [`KeyHasher`]: the leaves of a [`CowMap`], and the
/// small key and scope tables in front of the hash property index, where
/// SipHash cost more than the lookup behind it.
pub(super) type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<KeyHasher>>;
type LeafMap<K, V> = FastHashMap<K, V>;

/// Hash bits consumed per trie level.
const BITS: usize = 5;
/// Children per branch. A write copies one branch per level, so this is
/// the per-level cost; 32 keeps the trie three levels deep at 2M keys.
const FAN: usize = 1 << BITS;
/// A leaf splits into a branch once it would grow past this many
/// entries. The children start at ~`LEAF_MAX / FAN` entries each, small
/// enough to copy cheaply and large enough that per-leaf overhead (an
/// `Arc` and a table header, ~100 bytes) stays a few percent of the
/// entries. 448 is the most a 512-slot table holds; one more entry would
/// double the table at under half full.
const LEAF_MAX: usize = 448;
/// A branch whose subtree drops below this many entries collapses back
/// into one leaf. Well under `LEAF_MAX`, so a key added and removed at
/// the boundary does not split and merge on every write.
const LEAF_MIN: usize = LEAF_MAX / 4;
/// Deepest level that still splits; the routing hash has 64 bits. Keys
/// that share all of them share a leaf, however large.
const MAX_DEPTH: usize = 64 / BITS - 1;

/// The key's routing hash. The hash's low bits index each leaf's hash
/// table and its top seven bits tag the slots, so the trie routes on a
/// multiplicative remix instead: keys sharing a leaf then share neither
/// (sharing low bits would pile them into a fraction of the table's slots,
/// sharing the tag would make every probe compare keys).
#[inline]
pub(super) fn route<Q: Hash + ?Sized>(key: &Q) -> u64 {
    let mut hasher = KeyHasher::default();
    key.hash(&mut hasher);
    hasher.finish().wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

/// The child slot `route` takes at `depth`: successive [`BITS`]-bit
/// fields from the top.
#[inline]
fn slot(route: u64, depth: usize) -> usize {
    (route >> (64 - BITS * (depth + 1))) as usize & (FAN - 1)
}

enum Node<K, V> {
    Empty,
    Leaf(Arc<LeafMap<K, V>>),
    Branch(Arc<Branch<K, V>>),
}

struct Branch<K, V> {
    kids: [Node<K, V>; FAN],
}

impl<K, V> Clone for Node<K, V> {
    fn clone(&self) -> Self {
        match self {
            Node::Empty => Node::Empty,
            Node::Leaf(map) => Node::Leaf(Arc::clone(map)),
            Node::Branch(branch) => Node::Branch(Arc::clone(branch)),
        }
    }
}

impl<K, V> Clone for Branch<K, V> {
    fn clone(&self) -> Self {
        Self {
            kids: self.kids.clone(),
        }
    }
}

impl<K, V> Branch<K, V> {
    /// Entries under this branch when every child is a leaf (or empty);
    /// `None` when a child is itself a branch.
    fn leaf_total(&self) -> Option<usize> {
        self.kids.iter().try_fold(0, |total, kid| match kid {
            Node::Empty => Some(total),
            Node::Leaf(map) => Some(total + map.len()),
            Node::Branch(_) => None,
        })
    }
}

impl<K: Eq + Hash + Clone, V: Clone> Node<K, V> {
    /// A full leaf at `depth`, redistributed over a branch's children.
    fn split(map: Arc<LeafMap<K, V>>, depth: usize) -> Self {
        let mut maps: [LeafMap<K, V>; FAN] = std::array::from_fn(|_| LeafMap::default());
        for (key, value) in Arc::unwrap_or_clone(map) {
            maps[slot(route(&key), depth)].insert(key, value);
        }
        let kids = maps.map(|map| {
            if map.is_empty() {
                Node::Empty
            } else {
                Node::Leaf(Arc::new(map))
            }
        });
        Node::Branch(Arc::new(Branch { kids }))
    }

    /// Every entry of the subtree, moved out where unshared and cloned
    /// where a graph copy still holds it.
    fn drain_into(self, out: &mut LeafMap<K, V>) {
        match self {
            Node::Empty => {}
            Node::Leaf(map) => match Arc::try_unwrap(map) {
                Ok(map) => out.extend(map),
                Err(map) => out.extend(map.iter().map(|(k, v)| (k.clone(), v.clone()))),
            },
            Node::Branch(branch) => {
                for kid in Arc::unwrap_or_clone(branch).kids {
                    kid.drain_into(out);
                }
            }
        }
    }
}

/// A hash map as a persistent hash trie of small hash tables (see the
/// module docs). Empty maps allocate nothing; maps up to [`LEAF_MAX`]
/// entries are a single table.
pub(super) struct CowMap<K, V> {
    root: Node<K, V>,
    /// Entries in the map. Branches do not count theirs, so an insert
    /// learns whether the key is new at the leaf, in one walk.
    len: usize,
}

impl<K, V> Default for CowMap<K, V> {
    fn default() -> Self {
        Self {
            root: Node::Empty,
            len: 0,
        }
    }
}

impl<K, V> Clone for CowMap<K, V> {
    /// O(1): one refcount bump on the root.
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            len: self.len,
        }
    }
}

impl<K: std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug for CowMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K: Eq + Hash, V: PartialEq> PartialEq for CowMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.lookup(k) == Some(v))
    }
}

impl<K, V> CowMap<K, V> {
    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Entries in no particular order.
    pub(super) fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            stack: vec![std::slice::from_ref(&self.root).iter()],
            leaf: None,
        }
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &V> + '_ {
        self.iter().map(|(_, v)| v)
    }

    /// Heap bytes of the trie itself (branches, and each leaf's `Arc` and
    /// table header) for the memory report, which charges the entries
    /// separately. Nodes shared with another graph copy are charged to
    /// both, like the other copy-on-write structures.
    pub(super) fn structure_bytes(&self) -> usize {
        fn walk<K, V>(node: &Node<K, V>) -> usize {
            match node {
                Node::Empty => 0,
                Node::Leaf(_) => ARC_HEADER + std::mem::size_of::<LeafMap<K, V>>(),
                Node::Branch(branch) => {
                    ARC_HEADER
                        + std::mem::size_of::<Branch<K, V>>()
                        + branch.kids.iter().map(walk).sum::<usize>()
                }
            }
        }
        walk(&self.root)
    }
}

impl<K: Eq + Hash, V> CowMap<K, V> {
    fn lookup<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.lookup_routed(route(key), key)
    }

    fn lookup_routed<Q>(&self, route: u64, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let mut node = &self.root;
        let mut depth = 0;
        loop {
            match node {
                Node::Empty => return None,
                Node::Leaf(map) => return map.get(key),
                Node::Branch(branch) => {
                    node = &branch.kids[slot(route, depth)];
                    depth += 1;
                }
            }
        }
    }
}

impl<K: Eq + Hash + Clone, V: Clone> CowMap<K, V> {
    pub(super) fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.lookup(key)
    }

    pub(super) fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.lookup(key).is_some()
    }

    /// Mutable access to an existing entry; copies the path to it where
    /// shared. A miss copies nothing.
    pub(super) fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.get_mut_routed(route(key), key)
    }

    fn get_mut_routed<Q>(&mut self, route: u64, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.lookup_routed(route, key)?;
        let mut node = &mut self.root;
        let mut depth = 0;
        loop {
            match node {
                Node::Empty => return None,
                Node::Leaf(map) => return Arc::make_mut(map).get_mut(key),
                Node::Branch(branch) => {
                    node = &mut Arc::make_mut(branch).kids[slot(route, depth)];
                    depth += 1;
                }
            }
        }
    }

    /// Mutable access to `key`'s value, inserting `make()` first if absent.
    /// Copies the path to it where shared.
    pub(super) fn get_or_insert_with(&mut self, key: K, make: impl FnOnce() -> V) -> &mut V {
        match self.entry(key) {
            (hash_map::Entry::Occupied(entry), _) => entry.into_mut(),
            (hash_map::Entry::Vacant(entry), len) => {
                *len += 1;
                entry.insert(make())
            }
        }
    }

    /// Update `key`'s value with `update`, or insert `make()` if absent:
    /// [`Self::get_or_insert_with`] for callers that do different things
    /// in the two cases.
    pub(super) fn upsert(&mut self, key: K, make: impl FnOnce() -> V, update: impl FnOnce(&mut V)) {
        match self.entry(key) {
            (hash_map::Entry::Occupied(entry), _) => update(entry.into_mut()),
            (hash_map::Entry::Vacant(entry), len) => {
                *len += 1;
                entry.insert(make());
            }
        }
    }

    /// The leaf entry for `key` (and the map's length, for the caller to
    /// count an insert), in one walk that copies the path where shared and
    /// splits a full leaf the key would be added to.
    fn entry(&mut self, key: K) -> (hash_map::Entry<'_, K, V>, &mut usize) {
        let route = route(&key);
        let len = &mut self.len;
        let mut node = &mut self.root;
        let mut depth = 0;
        loop {
            if depth <= MAX_DEPTH
                && matches!(node, Node::Leaf(map) if map.len() >= LEAF_MAX && !map.contains_key(&key))
            {
                let Node::Leaf(map) = std::mem::replace(node, Node::Empty) else {
                    unreachable!("matched a leaf");
                };
                *node = Node::split(map, depth);
            }
            match node {
                Node::Empty => *node = Node::Leaf(Arc::new(LeafMap::default())),
                Node::Leaf(map) => return (Arc::make_mut(map).entry(key), len),
                Node::Branch(branch) => {
                    node = &mut Arc::make_mut(branch).kids[slot(route, depth)];
                    depth += 1;
                }
            }
        }
    }

    /// An empty map shaped to hold the keys whose [`route`]s are given
    /// (one per item; repeats are fine), so that inserting those keys next
    /// neither splits a leaf nor grows its table. For indexing existing
    /// data: inserted one by one into an unshaped map, every leaf table
    /// grows through each smaller size first, and the allocator keeps the
    /// tables freed on the way (at 2M keys ~90% more resident memory than
    /// live). Only the routes (8 bytes a key) are held meanwhile.
    pub(super) fn with_shape(mut routes: Vec<u64>) -> Self {
        // Equal keys share a route and distinct keys almost never do, so
        // the distinct routes size each leaf.
        routes.sort_unstable();
        routes.dedup();
        Self {
            root: Self::shape(&routes, 0),
            len: 0,
        }
    }

    /// Empty subtree at `depth` for sorted distinct `routes` that share
    /// their first `depth` slots (so each child's are a contiguous run).
    fn shape(routes: &[u64], depth: usize) -> Node<K, V> {
        if routes.is_empty() {
            return Node::Empty;
        }
        if depth > MAX_DEPTH || routes.len() <= LEAF_MAX {
            let map = LeafMap::with_capacity_and_hasher(routes.len(), Default::default());
            return Node::Leaf(Arc::new(map));
        }
        let mut rest = routes;
        let kids = std::array::from_fn(|s| {
            let (mine, after) = rest.split_at(rest.partition_point(|r| slot(*r, depth) == s));
            rest = after;
            Self::shape(mine, depth + 1)
        });
        Node::Branch(Arc::new(Branch { kids }))
    }

    pub(super) fn insert(&mut self, key: K, value: V) -> Option<V> {
        let mut value = Some(value);
        let slot = self.get_or_insert_with(key, || value.take().expect("value unused"));
        value.map(|v| std::mem::replace(slot, v))
    }

    pub(super) fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let route = route(key);
        self.lookup_routed(route, key)?;
        self.len -= 1;
        if self.len == 0 {
            // Drops whatever branches are left.
            return match std::mem::replace(&mut self.root, Node::Empty) {
                Node::Empty => None,
                other => {
                    let mut map = LeafMap::default();
                    other.drain_into(&mut map);
                    map.remove(key)
                }
            };
        }
        let mut node = &mut self.root;
        let mut depth = 0;
        loop {
            let s = slot(route, depth);
            if matches!(node, Node::Branch(branch) if matches!(branch.kids[s], Node::Leaf(_))) {
                let Node::Branch(branch) = &mut *node else {
                    unreachable!("matched a branch");
                };
                let branch = Arc::make_mut(branch);
                let Node::Leaf(map) = &mut branch.kids[s] else {
                    unreachable!("matched a leaf");
                };
                let map = Arc::make_mut(map);
                let removed = map.remove(key);
                let left = map.len();
                if left == 0 {
                    branch.kids[s] = Node::Empty;
                }
                // A leaf this small may leave the branch too small to
                // keep: fold it into one leaf once all its children are
                // leaves holding under `LEAF_MIN` entries together. (Only
                // then: summing the children touches all of them.)
                let fold = (left <= LEAF_MIN / FAN)
                    .then(|| branch.leaf_total())
                    .flatten()
                    .filter(|total| *total < LEAF_MIN);
                if let Some(total) = fold {
                    let mut map = LeafMap::default();
                    map.reserve(total);
                    std::mem::replace(node, Node::Empty).drain_into(&mut map);
                    if !map.is_empty() {
                        *node = Node::Leaf(Arc::new(map));
                    }
                }
                return removed;
            }
            match node {
                Node::Empty => return None,
                Node::Leaf(map) => return Arc::make_mut(map).remove(key),
                Node::Branch(branch) => {
                    node = &mut Arc::make_mut(branch).kids[s];
                    depth += 1;
                }
            }
        }
    }
}

/// Iterator over a [`CowMap`]: a depth-first walk of the trie.
pub(super) struct Iter<'a, K, V> {
    stack: Vec<std::slice::Iter<'a, Node<K, V>>>,
    leaf: Option<hash_map::Iter<'a, K, V>>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(entry) = self.leaf.as_mut().and_then(Iterator::next) {
                return Some(entry);
            }
            self.leaf = None;
            match self.stack.last_mut()?.next() {
                None => {
                    self.stack.pop();
                }
                Some(Node::Empty) => {}
                Some(Node::Leaf(map)) => self.leaf = Some(map.iter()),
                Some(Node::Branch(branch)) => self.stack.push(branch.kids.iter()),
            }
        }
    }
}

/// Largest partition of a [`CowOrdMap`] before it splits in two. A write
/// copies one partition, so this bounds its cost (a `BTreeMap` rebuild of
/// up to this many entries).
const PART_MAX: usize = 256;
/// A partition that shrinks below this merges into a neighbour when the
/// two fit in three quarters of [`PART_MAX`].
const PART_MIN: usize = PART_MAX / 4;
/// Most partitions in one group before it splits in two. A write copies
/// one group (up to this many key + pointer pairs) and the root table (one
/// pair per group: 61 at 2M keys inserted in ascending order, up to four
/// times that for keys arriving in random order).
const GROUP_MAX: usize = 128;
/// A group with fewer partitions merges into a neighbour when the two fit
/// in three quarters of [`GROUP_MAX`].
const GROUP_MIN: usize = GROUP_MAX / 4;

/// A partition: a sorted run of keys in one array. A `BTreeMap` here
/// spent about twice the entries' size on half-empty nodes, and copying
/// one (a write's first touch of a shared partition) walked every node;
/// an array is copied in one pass and searched without pointer chasing.
/// An insert shifts the entries after it, at most [`PART_MAX`] of them.
#[derive(Clone)]
struct SortedPart<K, V> {
    entries: Vec<(K, V)>,
}

impl<K: Ord, V> SortedPart<K, V> {
    fn single(key: K, value: V) -> Self {
        Self {
            entries: vec![(key, value)],
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn position<Q>(&self, key: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.entries.binary_search_by(|(k, _)| k.borrow().cmp(key))
    }

    fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.position(key).ok().map(|i| &self.entries[i].1)
    }

    fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.position(key).ok().map(|i| &mut self.entries[i].1)
    }

    fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.position(key).is_ok()
    }

    #[cfg(test)]
    fn keys(&self) -> impl DoubleEndedIterator<Item = &K> + '_ {
        self.entries.iter().map(|(k, _)| k)
    }

    /// Insert `key`, replacing its value if the partition holds it.
    fn insert(&mut self, key: K, value: V) {
        // Ascending loads append.
        if self.entries.last().is_none_or(|(last, _)| *last < key) {
            self.entries.push((key, value));
            return;
        }
        match self.position(&key) {
            Ok(i) => self.entries[i].1 = value,
            Err(i) => self.entries.insert(i, (key, value)),
        }
    }

    fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let i = self.position(key).ok()?;
        Some(self.entries.remove(i).1)
    }

    fn first_key(&self) -> Option<&K> {
        self.entries.first().map(|(k, _)| k)
    }

    fn last_key(&self) -> Option<&K> {
        self.entries.last().map(|(k, _)| k)
    }

    fn key_at(&self, index: usize) -> Option<&K> {
        self.entries.get(index).map(|(k, _)| k)
    }

    /// Move the entries with keys `>= key` into a new partition. Both
    /// halves end up exactly sized: this one just outgrew its capacity
    /// and would otherwise keep twice what it holds.
    fn split_off(&mut self, key: &K) -> Self {
        let at = self.entries.partition_point(|(k, _)| k < key);
        let upper = self.entries.split_off(at);
        self.entries.shrink_to_fit();
        Self { entries: upper }
    }

    /// Append `other`, whose keys all follow this partition's.
    fn append(&mut self, mut other: Self) {
        self.entries.append(&mut other.entries);
    }

    fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> + '_ {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    fn range(
        &self,
        lower: &std::ops::Bound<K>,
        upper: &std::ops::Bound<K>,
    ) -> impl DoubleEndedIterator<Item = (&K, &V)> + '_ {
        use std::ops::Bound;
        let from = match lower {
            Bound::Included(l) => self.entries.partition_point(|(k, _)| k < l),
            Bound::Excluded(l) => self.entries.partition_point(|(k, _)| k <= l),
            Bound::Unbounded => 0,
        };
        let to = match upper {
            Bound::Included(u) => self.entries.partition_point(|(k, _)| k <= u),
            Bound::Excluded(u) => self.entries.partition_point(|(k, _)| k < u),
            Bound::Unbounded => self.entries.len(),
        };
        self.entries[from..to.max(from)].iter().map(|(k, v)| (k, v))
    }

    /// Slots allocated, for the memory report.
    fn capacity(&self) -> usize {
        self.entries.capacity()
    }
}

type Part<K, V> = Arc<SortedPart<K, V>>;
/// Partitions in key order, each with its smallest key so a lookup binary
/// searches a contiguous array instead of the partitions themselves.
type Group<K, V> = Arc<Vec<(K, Part<K, V>)>>;

/// An ordered map as a three-level B-tree of `Arc`-shared nodes: a root
/// table of groups, each a table of partitions, each a sorted array (see
/// the module docs). Unlike [`CowMap`] it keeps key order, so range scans
/// (the sorted property index behind RANGE indexes and uniqueness
/// constraints, fulltext prefix terms) work in both directions.
pub(super) struct CowOrdMap<K, V> {
    /// Non-empty groups of non-empty partitions, each with its smallest
    /// key, in ascending key order.
    groups: Arc<Vec<(K, Group<K, V>)>>,
    len: usize,
}

impl<K, V> Default for CowOrdMap<K, V> {
    fn default() -> Self {
        Self {
            groups: Arc::new(Vec::new()),
            len: 0,
        }
    }
}

impl<K, V> Clone for CowOrdMap<K, V> {
    /// O(1): one refcount bump on the root.
    fn clone(&self) -> Self {
        Self {
            groups: Arc::clone(&self.groups),
            len: self.len,
        }
    }
}

impl<K: std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug for CowOrdMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(
                self.groups
                    .iter()
                    .flat_map(|(_, group)| group.iter())
                    .flat_map(|(_, part)| part.entries.iter().map(|(k, v)| (k, v))),
            )
            .finish()
    }
}

/// Index of the last slot whose smallest key is `<= key` (0 when `key`
/// precedes them all): the slot that holds, or would receive, `key`.
fn slot_for<K: Borrow<Q>, Q: Ord + ?Sized, X>(slots: &[(K, X)], key: &Q) -> usize {
    slots
        .partition_point(|(min, _)| min.borrow() <= key)
        .saturating_sub(1)
}

/// Fold the undersized slot at `at` into a neighbour when the two fit in
/// `fit` entries (as measured by `size`), with `append` moving the
/// second's contents onto the end of the first. Returns whether it merged.
fn merge_slot<K: Clone, X: Clone>(
    slots: &mut Vec<(K, Arc<X>)>,
    at: usize,
    fit: usize,
    size: impl Fn(&X) -> usize,
    append: impl Fn(&mut X, X),
) -> bool {
    let small = size(&slots[at].1);
    let target = if at + 1 < slots.len() && small + size(&slots[at + 1].1) <= fit {
        at
    } else if at > 0 && small + size(&slots[at - 1].1) <= fit {
        at - 1
    } else {
        return false;
    };
    let (_, next) = slots.remove(target + 1);
    append(
        Arc::make_mut(&mut slots[target].1),
        Arc::unwrap_or_clone(next),
    );
    true
}

impl<K: Ord + Clone, V: Clone> CowOrdMap<K, V> {
    pub(super) fn len(&self) -> usize {
        self.len
    }

    /// Group and partition that hold (or would hold) `key`. The map must
    /// not be empty.
    fn locate<Q>(&self, key: &Q) -> (usize, usize)
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let g = slot_for(&self.groups, key);
        (g, slot_for(&self.groups[g].1, key))
    }

    pub(super) fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        if self.groups.is_empty() {
            return None;
        }
        let (g, p) = self.locate(key);
        self.groups[g].1[p].1.get(key)
    }

    pub(super) fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Mutable access to an existing entry; copies the path to it where
    /// shared. A miss copies nothing.
    pub(super) fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        if !self.contains_key(key) {
            return None;
        }
        let (g, p) = self.locate(key);
        let group = Arc::make_mut(&mut Arc::make_mut(&mut self.groups)[g].1);
        Arc::make_mut(&mut group[p].1).get_mut(key)
    }

    /// Mutable access to `key`'s value, inserting `make()` first if absent.
    pub(super) fn get_or_insert_with(&mut self, key: K, make: impl FnOnce() -> V) -> &mut V {
        if !self.contains_key(&key) {
            self.insert_new(key.clone(), make());
        }
        self.get_mut(&key).expect("present after insert")
    }

    /// Update `key`'s value with `update`, or insert `make()` if absent,
    /// with one search for an existing key instead of two.
    pub(super) fn upsert(&mut self, key: K, make: impl FnOnce() -> V, update: impl FnOnce(&mut V)) {
        if !self.groups.is_empty() {
            let (g, p) = self.locate(&key);
            if self.groups[g].1[p].1.contains_key(&key) {
                let group = Arc::make_mut(&mut Arc::make_mut(&mut self.groups)[g].1);
                let part = Arc::make_mut(&mut group[p].1);
                update(part.get_mut(&key).expect("present"));
                return;
            }
        }
        self.insert_new(key, make());
    }

    fn insert_new(&mut self, key: K, value: V) {
        self.len += 1;
        let root = Arc::make_mut(&mut self.groups);
        if root.is_empty() {
            let part = Arc::new(SortedPart::single(key.clone(), value));
            root.push((key.clone(), Arc::new(vec![(key, part)])));
            return;
        }
        let g = slot_for(root, &key);
        // A new smallest key lands in the first partition and becomes
        // the minimum at every level.
        let lowest = key < root[g].0;
        if lowest {
            root[g].0 = key.clone();
        }
        let last_group = g + 1 == root.len();
        let group = Arc::make_mut(&mut root[g].1);
        let p = slot_for(group, &key);
        if lowest {
            group[p].0 = key.clone();
        }
        // Keys mostly arrive in ascending order (ids, timestamps). An
        // append past the last key opens a new partition (and group) for
        // itself instead of splitting the full one in half, so ascending
        // loads leave full partitions and a root a quarter the size.
        let append = last_group
            && p + 1 == group.len()
            && group[p].1.last_key().is_some_and(|last| *last < key);
        let part = Arc::make_mut(&mut group[p].1);
        let split_at = if append { Some(key.clone()) } else { None };
        part.insert(key, value);
        if part.len() <= PART_MAX {
            return;
        }
        let split_at =
            split_at.unwrap_or_else(|| part.key_at(part.len() / 2).cloned().expect("non-empty"));
        let upper = part.split_off(&split_at);
        group.insert(p + 1, (split_at, Arc::new(upper)));
        if group.len() > GROUP_MAX {
            let at = if append {
                group.len() - 1
            } else {
                group.len() / 2
            };
            let upper = group.split_off(at);
            let min = upper[0].0.clone();
            root.insert(g + 1, (min, Arc::new(upper)));
        }
    }

    pub(super) fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        if !self.contains_key(key) {
            return None;
        }
        let (g, p) = self.locate(key);
        self.len -= 1;
        let root = Arc::make_mut(&mut self.groups);
        let group = Arc::make_mut(&mut root[g].1);
        let part = Arc::make_mut(&mut group[p].1);
        let removed = part.remove(key);
        if part.is_empty() {
            group.remove(p);
        } else {
            if part.len() < PART_MIN {
                merge_slot(group, p, PART_MAX * 3 / 4, SortedPart::len, |a, b| {
                    a.append(b)
                });
            }
            // Removing a partition's smallest key moves its minimum.
            let p = slot_for(group, key);
            group[p].0 = group[p].1.first_key().expect("non-empty").clone();
        }
        if group.is_empty() {
            root.remove(g);
            return removed;
        }
        if group.len() < GROUP_MIN {
            merge_slot(root, g, GROUP_MAX * 3 / 4, Vec::len, |a, b| a.extend(b));
        }
        let g = slot_for(root, key);
        root[g].0 = root[g].1[0].0.clone();
        removed
    }

    /// Entries with keys in `(lower, upper)`, in key order (reversible).
    /// An empty or inverted range (`x > 5 AND x < 5`) yields nothing:
    /// `BTreeMap::range` would panic on it, and a panic aborts the host.
    pub(super) fn range(
        &self,
        lower: std::ops::Bound<K>,
        upper: std::ops::Bound<K>,
    ) -> impl DoubleEndedIterator<Item = (&K, &V)> + '_ {
        use std::ops::Bound;
        let empty = match (&lower, &upper) {
            (Bound::Included(l), Bound::Included(u)) => l > u,
            (Bound::Included(l) | Bound::Excluded(l), Bound::Included(u) | Bound::Excluded(u)) => {
                l >= u
            }
            _ => false,
        };
        // First and last (group, partition) the range touches; an empty
        // span is (1, 0)..=(0, 0).
        let span = if empty || self.groups.is_empty() {
            None
        } else {
            let first = match &lower {
                Bound::Included(k) | Bound::Excluded(k) => self.locate(k),
                Bound::Unbounded => (0, 0),
            };
            let last = match &upper {
                Bound::Included(k) | Bound::Excluded(k) => self.locate(k),
                Bound::Unbounded => {
                    let g = self.groups.len() - 1;
                    (g, self.groups[g].1.len() - 1)
                }
            };
            (first <= last).then_some((first, last))
        };
        let ((gf, pf), (gl, pl)) = span.unwrap_or(((1, 0), (0, 0)));
        let groups = &self.groups[..];
        (gf..=gl)
            .flat_map(move |g| {
                let group = &groups[g].1;
                let from = if g == gf { pf } else { 0 };
                let to = if g == gl { pl } else { group.len() - 1 };
                group[from..=to].iter()
            })
            .flat_map(move |(_, part)| part.range(&lower, &upper))
    }

    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> + '_ {
        self.groups
            .iter()
            .flat_map(|(_, group)| group.iter())
            .flat_map(|(_, part)| part.iter())
    }

    /// Heap bytes of the tables above the partitions (root and groups,
    /// each slot a minimum key and a pointer, and each partition's `Arc`
    /// and map header) for the memory report, which charges the entries
    /// separately. Minimum keys' own heap data is shared with the entries
    /// (`Arc<str>` strings) or small, and not counted.
    pub(super) fn structure_bytes(&self) -> usize {
        let slot = std::mem::size_of::<(K, Group<K, V>)>();
        let parts: usize = self.groups.iter().map(|(_, group)| group.len()).sum();
        ARC_HEADER
            + self.groups.capacity() * slot
            + self
                .groups
                .iter()
                .map(|(_, group)| ARC_HEADER + group.capacity() * slot)
                .sum::<usize>()
            + parts * (ARC_HEADER + std::mem::size_of::<SortedPart<K, V>>())
    }

    /// Entry slots the partitions have allocated: at least [`Self::len`].
    /// The memory report charges one entry per slot.
    pub(super) fn entry_slots(&self) -> usize {
        self.groups
            .iter()
            .flat_map(|(_, group)| group.iter())
            .map(|(_, part)| part.capacity())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    /// Deterministic xorshift so the model tests need no extra crate.
    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    /// Depth of the deepest leaf and the number of branches.
    fn shape<K, V>(m: &CowMap<K, V>) -> (usize, usize) {
        fn walk<K, V>(node: &Node<K, V>, depth: usize) -> (usize, usize) {
            match node {
                Node::Empty | Node::Leaf(_) => (depth, 0),
                Node::Branch(b) => b.kids.iter().fold((depth, 1), |(d, n), kid| {
                    let (kd, kn) = walk(kid, depth + 1);
                    (d.max(kd), n + kn)
                }),
            }
        }
        walk(&m.root, 0)
    }

    /// The map's `len` matches its entries; every leaf is non-empty and
    /// within `LEAF_MAX`, and every branch holds something.
    fn check_trie<K: Eq + Hash, V>(m: &CowMap<K, V>) {
        fn walk<K: Eq + Hash, V>(node: &Node<K, V>) -> usize {
            match node {
                Node::Empty => 0,
                Node::Leaf(map) => {
                    assert!(!map.is_empty(), "empty leaf");
                    assert!(map.len() <= LEAF_MAX, "oversized leaf {}", map.len());
                    map.len()
                }
                Node::Branch(b) => {
                    let n: usize = b.kids.iter().map(walk).sum();
                    assert!(n > 0, "empty branch");
                    n
                }
            }
        }
        assert_eq!(walk(&m.root), m.len(), "len");
    }

    /// Every leaf holds only keys routed to it.
    fn check_routing(m: &CowMap<u64, u64>) {
        check_routing_of(m)
    }

    fn check_routing_of<V>(m: &CowMap<u64, V>) {
        fn walk<V>(node: &Node<u64, V>, path: &mut Vec<usize>) {
            match node {
                Node::Empty => {}
                Node::Leaf(map) => {
                    for k in map.keys() {
                        let r = route(k);
                        for (d, s) in path.iter().enumerate() {
                            assert_eq!(slot(r, d), *s, "key {k} misrouted at depth {d}");
                        }
                    }
                }
                Node::Branch(b) => {
                    for (i, kid) in b.kids.iter().enumerate() {
                        path.push(i);
                        walk(kid, path);
                        path.pop();
                    }
                }
            }
        }
        walk(&m.root, &mut Vec::new());
    }

    fn arc_ptrs<K, V>(m: &CowMap<K, V>) -> Vec<usize> {
        fn walk<K, V>(node: &Node<K, V>, out: &mut Vec<usize>) {
            match node {
                Node::Empty => {}
                Node::Leaf(map) => out.push(Arc::as_ptr(map) as *const u8 as usize),
                Node::Branch(b) => {
                    out.push(Arc::as_ptr(b) as *const u8 as usize);
                    for kid in &b.kids {
                        walk(kid, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&m.root, &mut out);
        out
    }

    #[test]
    fn ord_map_empty_and_inverted_ranges_yield_nothing() {
        use std::ops::Bound::{Excluded, Included, Unbounded};
        let mut m: CowOrdMap<i64, ()> = CowOrdMap::default();
        for i in 0..2000 {
            m.get_or_insert_with(i, || ());
        }
        for (lo, hi) in [
            (Excluded(5), Excluded(5)),
            (Included(5), Excluded(5)),
            (Excluded(5), Included(5)),
            (Included(9), Included(3)),
            (Excluded(1500), Excluded(10)),
        ] {
            assert_eq!(m.range(lo, hi).count(), 0);
            assert_eq!(m.range(lo, hi).rev().count(), 0);
        }
        assert_eq!(m.range(Included(5), Included(5)).count(), 1);
        assert_eq!(m.range(Unbounded, Excluded(0)).count(), 0);
        let empty: CowOrdMap<i64, ()> = CowOrdMap::default();
        assert_eq!(empty.range(Unbounded, Unbounded).count(), 0);
        assert_eq!(empty.range(Included(1), Included(4)).rev().count(), 0);
    }

    #[test]
    fn cow_map_behaves_like_a_map_and_shares_on_clone() {
        let mut m: CowMap<String, u32> = CowMap::default();
        for i in 0..500u32 {
            assert!(m.insert(format!("k{i}"), i).is_none());
        }
        assert_eq!(m.insert("k7".into(), 70), Some(7));
        assert_eq!(m.len(), 500);
        let snapshot = m.clone();
        *m.get_mut("k1").unwrap() = 11;
        assert_eq!(m.remove("k2"), Some(2));
        assert_eq!(snapshot.get("k1"), Some(&1));
        assert_eq!(snapshot.get("k2"), Some(&2));
        assert_eq!(m.get("k1"), Some(&11));
        assert!(!m.contains_key("k2"));
        assert_eq!(m.len(), 499);
        assert_eq!(m.remove("missing"), None);
        assert!(m.get_mut("missing").is_none());
    }

    #[test]
    fn cow_map_empty_allocates_nothing_and_small_is_one_leaf() {
        let mut m: CowMap<u64, u64> = CowMap::default();
        assert!(matches!(m.root, Node::Empty));
        assert_eq!(m.structure_bytes(), 0);
        assert_eq!(m.iter().count(), 0);
        for k in 0..LEAF_MAX as u64 {
            m.insert(k, k);
        }
        assert!(matches!(m.root, Node::Leaf(_)), "a full leaf has not split");
        m.insert(u64::MAX, 0);
        assert!(matches!(m.root, Node::Branch(_)), "one more splits it");
        check_trie(&m);
        check_routing(&m);
        for k in 0..LEAF_MAX as u64 {
            m.remove(&k);
        }
        assert_eq!(m.len(), 1);
        assert!(matches!(m.root, Node::Leaf(_)), "collapsed back to a leaf");
        assert_eq!(m.remove(&u64::MAX), Some(0));
        assert!(
            matches!(m.root, Node::Empty),
            "the last remove frees the leaf"
        );
        assert!(m.is_empty());
    }

    #[test]
    fn cow_map_matches_a_hashmap_through_splits_and_collapses() {
        let mut seed = 0x2545_f491_4f6c_dd1d;
        let mut model: BTreeMap<u64, u64> = BTreeMap::new();
        let mut m: CowMap<u64, u64> = CowMap::default();
        let mut snapshots: Vec<(CowMap<u64, u64>, BTreeMap<u64, u64>)> = Vec::new();
        // Grow to ~40k keys (three levels), shrink to a few hundred, grow
        // again: every split and collapse threshold is crossed both ways.
        for step in 0..160_000u64 {
            let phase = step / 40_000;
            let grow = phase % 2 == 0;
            let k = rng(&mut seed) % 60_000;
            let roll = rng(&mut seed) % 100;
            if (grow && roll < 80) || (!grow && roll < 3) {
                assert_eq!(m.insert(k, step), model.insert(k, step));
            } else if roll < 90 {
                let k = if grow {
                    k
                } else {
                    *model.keys().next().unwrap_or(&k)
                };
                assert_eq!(m.remove(&k), model.remove(&k));
            } else if let Some(v) = m.get_mut(&k) {
                *v += 1;
                *model.get_mut(&k).unwrap() += 1;
            } else {
                assert!(!model.contains_key(&k));
            }
            assert_eq!(m.len(), model.len());
            if step % 4_999 == 0 {
                check_trie(&m);
                check_routing(&m);
                let got: BTreeMap<u64, u64> = m.iter().map(|(k, v)| (*k, *v)).collect();
                assert_eq!(got, model, "step {step}");
                snapshots.push((m.clone(), model.clone()));
            }
        }
        // Every clone kept its contents while the original moved on.
        for (snap, want) in &snapshots {
            assert_eq!(snap.len(), want.len());
            for (k, v) in want {
                assert_eq!(snap.get(k), Some(v));
            }
            let got: BTreeMap<u64, u64> = snap.iter().map(|(k, v)| (*k, *v)).collect();
            assert_eq!(&got, want);
        }
    }

    #[test]
    fn cow_map_write_after_clone_copies_one_path() {
        let mut base: CowMap<u64, u64> = CowMap::default();
        for k in 0..200_000u64 {
            base.insert(k, k);
        }
        let (depth, branches) = shape(&base);
        assert!(depth >= 2, "200k keys need a deep trie, got {depth}");
        check_trie(&base);
        let before: BTreeSet<usize> = arc_ptrs(&base).into_iter().collect();

        let mut staged = base.clone();
        staged.insert(1_000_000, 0);
        *staged.get_mut(&17).unwrap() = 0;
        staged.remove(&99);
        let after = arc_ptrs(&staged);
        let copied = after.iter().filter(|p| !before.contains(p)).count();
        // Three writes, each at most one node per level plus a leaf.
        assert!(
            copied <= 3 * (depth + 1),
            "{copied} nodes copied of {branches} branches"
        );

        assert_eq!(base.len(), 200_000);
        assert_eq!(base.get(&17), Some(&17));
        assert_eq!(base.get(&99), Some(&99));
        assert!(!base.contains_key(&1_000_000));
        assert_eq!(staged.len(), 200_000);
        assert_eq!(staged.get(&17), Some(&0));
        assert!(!staged.contains_key(&99));
        assert_eq!(staged.get(&1_000_000), Some(&0));
        assert!(base != staged);
        let mut again = staged.clone();
        again.insert(99, 99);
        again.remove(&1_000_000);
        *again.get_mut(&17).unwrap() = 17;
        assert!(again == base);
    }

    #[test]
    fn cow_map_with_shape_takes_its_keys_without_splitting_or_growing() {
        // Leaf table allocations, by address, to see that none is replaced.
        fn tables(m: &CowMap<u64, Vec<u64>>) -> Vec<(usize, usize)> {
            fn walk(node: &Node<u64, Vec<u64>>, out: &mut Vec<(usize, usize)>) {
                match node {
                    Node::Empty => {}
                    Node::Leaf(map) => out.push((Arc::as_ptr(map) as usize, map.capacity())),
                    Node::Branch(b) => b.kids.iter().for_each(|k| walk(k, out)),
                }
            }
            let mut out = Vec::new();
            walk(&m.root, &mut out);
            out
        }
        for n in [0u64, 1, 300, LEAF_MAX as u64 + 1, 50_000] {
            // Every key once to three times, interleaved.
            let items: Vec<(u64, u64)> = (0..n * 3)
                .filter(|i| i % 3 != 2 || (i / 3) % 2 == 0)
                .map(|i| ((i / 3) * 7_919 % (n.max(1) * 13), i))
                .collect();
            let mut shaped: CowMap<u64, Vec<u64>> =
                CowMap::with_shape(items.iter().map(|(k, _)| route(k)).collect());
            let before = tables(&shaped);
            let mut one_by_one: CowMap<u64, Vec<u64>> = CowMap::default();
            for (k, i) in items {
                shaped.get_or_insert_with(k, Vec::new).push(i);
                one_by_one.get_or_insert_with(k, Vec::new).push(i);
            }
            assert_eq!(tables(&shaped), before, "n = {n}: a leaf split or grew");
            check_trie(&shaped);
            assert_eq!(shaped.len(), one_by_one.len(), "n = {n}");
            assert!(shaped == one_by_one, "n = {n}");
            // A shaped map is an ordinary map from there on.
            let snapshot = shaped.clone();
            shaped.insert(u64::MAX, vec![]);
            for k in (0..n * 13).step_by(5) {
                shaped.remove(&k);
            }
            check_trie(&shaped);
            assert!(snapshot == one_by_one);
        }
        let mut big: CowMap<u64, Vec<u64>> =
            CowMap::with_shape((0..200_000u64).map(|k| route(&k)).collect());
        for k in 0..200_000u64 {
            big.insert(k, vec![k]);
        }
        check_trie(&big);
        check_routing_of(&big);
        assert!(shape(&big).0 >= 2);
    }

    #[test]
    fn upsert_inserts_or_updates_and_keeps_counts() {
        let mut m: CowMap<u64, u64> = CowMap::default();
        let mut o: CowOrdMap<u64, u64> = CowOrdMap::default();
        let mut model = BTreeMap::new();
        for i in 0..30_000u64 {
            let k = (i * 7_919) % 10_000;
            m.upsert(k, || 1, |v| *v += 1);
            o.upsert(k, || 1, |v| *v += 1);
            *model.entry(k).or_insert(0) += 1;
            if i == 15_000 {
                let (ms, os, snap) = (m.clone(), o.clone(), model.clone());
                for k in 0..10_000u64 {
                    m.upsert(k, || 0, |v| *v += 100);
                    o.upsert(k, || 0, |v| *v += 100);
                    *model.entry(k).or_insert(0) += 100;
                }
                // The clones kept their contents.
                assert!(os.iter().map(|(k, v)| (*k, *v)).eq(snap.clone()));
                let got: BTreeMap<u64, u64> = ms.iter().map(|(k, v)| (*k, *v)).collect();
                assert_eq!(got, snap);
            }
        }
        check_trie(&m);
        check_ord(&o);
        let got: BTreeMap<u64, u64> = m.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(got, model);
        assert!(o.iter().map(|(k, v)| (*k, *v)).eq(model));
    }

    #[test]
    fn cow_map_folds_branches_back_as_it_empties() {
        let mut m: CowMap<u64, u64> = CowMap::default();
        for k in 0..50_000u64 {
            m.insert(k, k);
        }
        assert_eq!(shape(&m).0, 2, "50k keys: two levels of branches");
        let snapshot = m.clone();
        // Remove in a scrambled order, keeping 50 keys.
        for i in 0..50_000u64 {
            let k = (i * 7_919) % 50_000;
            if k % 1_000 != 0 {
                assert_eq!(m.remove(&k), Some(k));
            }
        }
        check_trie(&m);
        assert_eq!(m.len(), 50);
        assert_eq!(shape(&m), (0, 0), "folded back into one leaf");
        assert!((0..50_000).step_by(1_000).all(|k| m.get(&k) == Some(&k)));
        // The clone kept every key and its shape.
        check_trie(&snapshot);
        assert_eq!(snapshot.len(), 50_000);
        assert_eq!(shape(&snapshot).0, 2);
        for k in (0..50_000u64).step_by(1_000) {
            m.remove(&k);
        }
        assert!(m.is_empty() && matches!(m.root, Node::Empty));
    }

    #[test]
    fn cow_map_tolerates_identical_routing_hashes() {
        // Keys whose hashes all collide still work: they share a leaf at
        // the deepest level, which stops splitting.
        #[derive(Clone, PartialEq, Eq, Debug)]
        struct Same(u32);
        impl Hash for Same {
            fn hash<H: Hasher>(&self, state: &mut H) {
                state.write_u8(1);
            }
        }
        let mut m: CowMap<Same, u32> = CowMap::default();
        for i in 0..(LEAF_MAX as u32 * 2) {
            m.insert(Same(i), i);
        }
        assert_eq!(m.len(), LEAF_MAX * 2);
        assert_eq!(m.get(&Same(700)), Some(&700));
        for i in 0..(LEAF_MAX as u32 * 2) {
            assert_eq!(m.remove(&Same(i)), Some(i));
        }
        assert!(m.is_empty());
    }

    #[test]
    fn cow_ord_map_matches_btreemap_in_order_and_ranges() {
        use std::ops::Bound::{Excluded, Included, Unbounded};
        let mut ours: CowOrdMap<u64, u64> = CowOrdMap::default();
        let mut reference = BTreeMap::new();
        // Scrambled insert order exercises splits in the middle.
        for i in 0..5_000u64 {
            let k = (i * 7919) % 5_000;
            *ours.get_or_insert_with(k, || 0) += k;
            reference.insert(k, k);
        }
        let snapshot = ours.clone();
        for k in (0..5_000u64).step_by(3) {
            assert_eq!(ours.remove(&k), reference.remove(&k));
        }
        *ours.get_mut(&1).unwrap() = 99;
        reference.insert(1, 99);
        assert_eq!(ours.len(), reference.len());
        assert!(ours.iter().eq(reference.iter()));
        assert!(ours.iter().rev().eq(reference.iter().rev()));
        for (lo, hi) in [
            (Included(100), Excluded(2_000)),
            (Excluded(4_990), Unbounded),
            (Unbounded, Included(17)),
            (Included(2_500), Included(2_500)),
        ] {
            assert!(ours.range(lo, hi).eq(reference.range((lo, hi))));
            assert!(ours.range(lo, hi).rev().eq(reference.range((lo, hi)).rev()));
        }
        assert_eq!(snapshot.get(&3), Some(&3), "the clone is unaffected");
        assert_eq!(snapshot.len(), 5_000);
    }

    /// Partitions and groups are non-empty, within bounds, sorted, and
    /// carry their true minimum keys.
    fn check_ord<V>(m: &CowOrdMap<u64, V>) {
        let mut prev: Option<u64> = None;
        let mut n = 0;
        for (gmin, group) in m.groups.iter() {
            assert!(!group.is_empty() && group.len() <= GROUP_MAX);
            assert_eq!(*gmin, group[0].0, "group minimum");
            for (pmin, part) in group.iter() {
                assert!(!part.is_empty() && part.len() <= PART_MAX);
                assert_eq!(pmin, part.keys().next().unwrap(), "partition minimum");
                for k in part.keys() {
                    assert!(prev.is_none_or(|p| p < *k), "out of order");
                    prev = Some(*k);
                    n += 1;
                }
            }
        }
        assert_eq!(n, m.len);
    }

    #[test]
    fn cow_ord_map_matches_btreemap_through_splits_and_merges() {
        use std::ops::Bound::{Excluded, Included, Unbounded};
        let mut seed = 0x9e37_79b9_7f4a_7c15;
        let mut model: BTreeMap<u64, u64> = BTreeMap::new();
        let mut m: CowOrdMap<u64, u64> = CowOrdMap::default();
        let mut snapshots = Vec::new();
        // Up to ~60k keys (several groups), down to a handful, up again;
        // a quarter of the inserts are below every key so far.
        for step in 0..200_000u64 {
            let grow = (step / 50_000) % 2 == 0;
            let roll = rng(&mut seed) % 100;
            let k = if roll < 20 {
                model
                    .first_key_value()
                    .map_or(1 << 40, |(k, _)| k.saturating_sub(1))
            } else {
                rng(&mut seed) % (1 << 40)
            };
            if (grow && roll < 75) || (!grow && roll < 5) {
                *m.get_or_insert_with(k, || 0) += 1;
                *model.entry(k).or_insert(0) += 1;
            } else if !model.is_empty() {
                // Remove an existing key (the smallest one now and then,
                // which moves the minimums).
                let target = if roll.is_multiple_of(7) {
                    *model.keys().next().unwrap()
                } else {
                    let probe = rng(&mut seed) % (1 << 40);
                    *model
                        .range(probe..)
                        .next()
                        .unwrap_or(model.iter().next_back().unwrap())
                        .0
                };
                assert_eq!(m.remove(&target), model.remove(&target));
                assert_eq!(m.remove(&target), None);
            }
            assert_eq!(m.len(), model.len());
            if step % 9_973 == 0 {
                check_ord(&m);
                assert!(m.iter().eq(model.iter()));
                assert!(m.iter().rev().eq(model.iter().rev()));
                let a = rng(&mut seed) % (1 << 40);
                let b = a + rng(&mut seed) % (1 << 38);
                for (lo, hi) in [
                    (Included(a), Excluded(b)),
                    (Excluded(a), Included(b)),
                    (Unbounded, Included(a)),
                    (Included(b), Unbounded),
                ] {
                    assert!(m.range(lo, hi).eq(model.range((lo, hi))));
                    assert!(m.range(lo, hi).rev().eq(model.range((lo, hi)).rev()));
                }
                for k in model.keys().step_by(97) {
                    assert_eq!(m.get(k), model.get(k));
                }
                snapshots.push((m.clone(), model.clone()));
            }
        }
        for (snap, want) in &snapshots {
            check_ord(snap);
            assert!(snap.iter().eq(want.iter()));
        }
        // Drain completely: every partition and group is released.
        let keys: Vec<u64> = model.keys().copied().collect();
        for k in keys {
            assert!(m.remove(&k).is_some());
        }
        assert_eq!(m.len(), 0);
        assert!(m.groups.is_empty());
    }

    #[test]
    fn cow_ord_map_write_after_clone_copies_one_path() {
        let mut base: CowOrdMap<u64, u64> = CowOrdMap::default();
        for k in 0..400_000u64 {
            base.get_or_insert_with(k * 2, || k);
        }
        assert!(base.groups.len() > 4, "{} groups", base.groups.len());
        let parts = |m: &CowOrdMap<u64, u64>| -> Vec<*const SortedPart<u64, u64>> {
            m.groups
                .iter()
                .flat_map(|(_, g)| g.iter().map(|(_, p)| Arc::as_ptr(p)))
                .collect()
        };
        let groups = |m: &CowOrdMap<u64, u64>| -> Vec<*const Vec<(u64, Part<u64, u64>)>> {
            m.groups.iter().map(|(_, g)| Arc::as_ptr(g)).collect()
        };
        let (before_parts, before_groups) = (parts(&base), groups(&base));
        let mut staged = base.clone();
        staged.get_or_insert_with(400_001, || 7);
        staged.remove(&2);
        *staged.get_mut(&600_000).unwrap() = 0;
        let new_parts = parts(&staged)
            .into_iter()
            .filter(|p| !before_parts.contains(p))
            .count();
        let new_groups = groups(&staged)
            .into_iter()
            .filter(|g| !before_groups.contains(g))
            .count();
        // Three writes, each copying one group and one partition; the
        // append past the last key (a full last partition and group) also
        // opens a partition and a group of its own.
        assert!(
            new_parts <= 4 && new_groups <= 4,
            "{new_parts} parts, {new_groups} groups"
        );
        assert_eq!(base.get(&2), Some(&1));
        assert_eq!(base.get(&600_000), Some(&300_000));
        assert!(!base.contains_key(&400_001));
        assert_eq!(staged.get(&400_001), Some(&7));
        assert_eq!(staged.len(), 400_000);
        check_ord(&base);
        check_ord(&staged);
    }

    #[test]
    fn cow_ord_map_borrowed_lookups_and_string_keys() {
        use std::ops::Bound::{Included, Unbounded};
        let mut m: CowOrdMap<Arc<str>, u32> = CowOrdMap::default();
        for i in 0..3_000u32 {
            m.get_or_insert_with(Arc::from(format!("t{i:05}")), || i);
        }
        assert_eq!(m.get("t00042"), Some(&42));
        assert!(m.get_mut("nope").is_none());
        assert_eq!(m.remove("t00042"), Some(42));
        let prefix: Vec<u32> = m
            .range(Included(Arc::from("t0010")), Unbounded)
            .take_while(|(k, _)| k.starts_with("t0010"))
            .map(|(_, v)| *v)
            .collect();
        assert_eq!(prefix, (100..110).collect::<Vec<_>>());
    }
}
