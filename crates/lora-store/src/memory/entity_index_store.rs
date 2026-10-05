//! Typed storage for index registries split by graph entity kind, plus
//! the [`IndexBundle`] that groups every index-related structure into a
//! single owned unit.
//!
//! The in-memory graph keeps separate physical registries for nodes and
//! relationships, but most callers only know the catalog entity they are
//! operating on. [`EntityIndexStore`] centralises the entity-to-lock
//! routing so the graph implementation does not repeat the same `match`
//! for every index kind.
//!
//! [`IndexBundle`] then bundles every secondary-index registry (text,
//! sorted-range, point, fulltext) together with the index catalog and
//! the hash-bucket property index registry, so `InMemoryGraph` carries
//! a single `indexes: IndexBundle` field instead of a constellation of
//! ten separate ones. The constraint catalog stays on the graph itself
//! because constraints are about data invariants, not indexed access.
//!
//! The bundle is intentionally a concrete, owned struct. No trait
//! object or dyn dispatch is introduced — the existing data structures
//! and their lock granularity are preserved verbatim, so hot paths
//! see zero performance change.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::fulltext_index::FulltextRegistry;
use super::index_catalog::{IndexCatalog, StoredIndexEntity};
use super::point_index::PointRegistry;
use super::property_index::PropertyIndexRegistry;
use super::sorted_property_index::SortedPropertyIndex;
use super::text_index::TrigramRegistry;
use super::vector_index::VectorIndexRegistry;

#[derive(Debug, Clone, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub(super) struct ScopedPropertyKey {
    pub label: String,
    pub property: String,
}

impl ScopedPropertyKey {
    pub(super) fn new(label: &str, property: &str) -> Self {
        Self {
            label: label.to_string(),
            property: property.to_string(),
        }
    }
}

/// A registry per entity kind, each behind `Arc` so cloning the graph
/// (the staged copy a write works on) shares the registries. The first
/// mutable access through [`IndexWrite`] copies a registry only if a
/// clone still shares it. Maintenance paths check coverage through a read
/// guard first, so writes that do not touch an index never copy it.
#[derive(Debug, Default)]
pub(super) struct EntityIndexStore<T> {
    node: RwLock<Arc<T>>,
    relationship: RwLock<Arc<T>>,
}

/// Read guard for one registry of an [`EntityIndexStore`].
pub(super) struct IndexRead<'a, T>(RwLockReadGuard<'a, Arc<T>>);

impl<T> std::ops::Deref for IndexRead<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

/// Write guard for one registry of an [`EntityIndexStore`]; mutable
/// access copies the registry first if a graph clone still shares it.
pub(super) struct IndexWrite<'a, T>(RwLockWriteGuard<'a, Arc<T>>);

impl<T> std::ops::Deref for IndexWrite<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone> std::ops::DerefMut for IndexWrite<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

/// Read guard on a lone copy-on-write structure (a catalog).
pub(super) fn read_shared<T>(lock: &RwLock<Arc<T>>) -> IndexRead<'_, T> {
    IndexRead(lock.read().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// Write guard on a lone copy-on-write structure (a catalog); mutable
/// access copies it first if a graph clone still shares it.
pub(super) fn write_shared<T>(lock: &RwLock<Arc<T>>) -> IndexWrite<'_, T> {
    IndexWrite(
        lock.write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// A new lock sharing the structure behind `lock`; O(1).
pub(super) fn share<T>(lock: &RwLock<Arc<T>>) -> RwLock<Arc<T>> {
    RwLock::new(Arc::clone(
        &lock.read().unwrap_or_else(|poisoned| poisoned.into_inner()),
    ))
}

impl<T> EntityIndexStore<T> {
    pub(super) fn read(&self, entity: StoredIndexEntity) -> IndexRead<'_, T> {
        read_shared(self.lock_for(entity))
    }

    pub(super) fn write(&self, entity: StoredIndexEntity) -> IndexWrite<'_, T> {
        write_shared(self.lock_for(entity))
    }

    fn lock_for(&self, entity: StoredIndexEntity) -> &RwLock<Arc<T>> {
        match entity {
            StoredIndexEntity::Node => &self.node,
            StoredIndexEntity::Relationship => &self.relationship,
        }
    }
}

impl<T> Clone for EntityIndexStore<T> {
    /// Shares both registries; O(1).
    fn clone(&self) -> Self {
        Self {
            node: share(&self.node),
            relationship: share(&self.relationship),
        }
    }
}

/// Bundled storage for every index-related structure backing an
/// [`super::InMemoryGraph`]. The graph holds exactly one
/// `indexes: IndexBundle` field instead of a constellation of separate
/// fields; methods on the graph that need a specific registry reach
/// into the bundle directly.
///
/// **Layout**
///
/// * `catalog` — declared indexes (CREATE INDEX entries), behind `Arc`
///   and copied on write like the registries below.
/// * `properties` — hash-bucket property indexes used by
///   `find_*_by_property`. Shared across both entity kinds, with
///   internal `node_properties` / `relationship_properties` splits.
///   Not behind an extra `Arc`: every level inside it already is (see
///   [`super::property_index::PropertyIndexState`]), so cloning it is a
///   handful of refcount bumps and a write copies only what it touches.
/// * `text`, `sorted`, `point`, `fulltext` — catalog-backed secondary
///   indexes split per entity kind via [`EntityIndexStore`].
/// * `active_*` atomics — fast-path counters that let mutation hooks
///   skip the registry locks when nothing is installed.
///
/// **Performance**
///
/// Cloning the bundle (part of every staged write's graph clone) is
/// O(1): every structure in it is shared and copied on its first write.
/// Field access from `pub(super)` graph code stays a direct
/// `self.indexes.<field>` away — no `dyn` calls.
///
/// **Constraint catalog**
///
/// Deliberately *not* part of the bundle. Constraints describe data
/// invariants (uniqueness, existence, type) rather than indexed
/// access; the constraint catalog therefore stays on the graph itself.
/// The fact that uniqueness/key constraints register a backing range
/// index lives in the constraint code path, not in the bundle's shape.
#[derive(Debug, Default)]
pub(super) struct IndexBundle {
    pub(super) catalog: RwLock<Arc<IndexCatalog>>,
    pub(super) properties: RwLock<PropertyIndexRegistry>,
    pub(super) text: EntityIndexStore<TrigramRegistry>,
    pub(super) sorted: EntityIndexStore<SortedPropertyIndex>,
    pub(super) point: EntityIndexStore<PointRegistry>,
    pub(super) fulltext: EntityIndexStore<FulltextRegistry>,
    pub(super) vector: EntityIndexStore<VectorIndexRegistry>,
    pub(super) active_node_property_indexes: AtomicUsize,
    pub(super) active_relationship_property_indexes: AtomicUsize,
    pub(super) active_fulltext_indexes: AtomicUsize,
}

impl Clone for IndexBundle {
    fn clone(&self) -> Self {
        use std::sync::atomic::Ordering;

        let properties = self
            .properties
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        Self {
            catalog: share(&self.catalog),
            properties: RwLock::new(properties),
            text: self.text.clone(),
            sorted: self.sorted.clone(),
            point: self.point.clone(),
            fulltext: self.fulltext.clone(),
            vector: self.vector.clone(),
            active_node_property_indexes: AtomicUsize::new(
                self.active_node_property_indexes.load(Ordering::Relaxed),
            ),
            active_relationship_property_indexes: AtomicUsize::new(
                self.active_relationship_property_indexes
                    .load(Ordering::Relaxed),
            ),
            active_fulltext_indexes: AtomicUsize::new(
                self.active_fulltext_indexes.load(Ordering::Relaxed),
            ),
        }
    }
}
