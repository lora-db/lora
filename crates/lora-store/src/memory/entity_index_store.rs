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
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

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

#[derive(Debug, Default)]
pub(super) struct EntityIndexStore<T> {
    node: RwLock<T>,
    relationship: RwLock<T>,
}

impl<T> EntityIndexStore<T> {
    pub(super) fn read(&self, entity: StoredIndexEntity) -> RwLockReadGuard<'_, T> {
        self.lock_for(entity)
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn write(&self, entity: StoredIndexEntity) -> RwLockWriteGuard<'_, T> {
        self.lock_for(entity)
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_for(&self, entity: StoredIndexEntity) -> &RwLock<T> {
        match entity {
            StoredIndexEntity::Node => &self.node,
            StoredIndexEntity::Relationship => &self.relationship,
        }
    }
}

impl<T: Clone> Clone for EntityIndexStore<T> {
    fn clone(&self) -> Self {
        Self {
            node: RwLock::new(self.read(StoredIndexEntity::Node).clone()),
            relationship: RwLock::new(self.read(StoredIndexEntity::Relationship).clone()),
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
/// * `catalog` — declared indexes (CREATE INDEX entries).
/// * `properties` — hash-bucket property indexes used by
///   `find_*_by_property`. Shared across both entity kinds, with
///   internal `node_properties` / `relationship_properties` splits.
/// * `text`, `sorted`, `point`, `fulltext` — catalog-backed secondary
///   indexes split per entity kind via [`EntityIndexStore`].
/// * `active_*` atomics — fast-path counters that let mutation hooks
///   skip the registry locks when nothing is installed.
///
/// **Performance**
///
/// Pure packaging change: same data, same locks, same granularity.
/// Field access from `pub(super)` graph code stays a direct
/// `self.indexes.<field>` away — no extra indirection, no `dyn` calls.
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
    pub(super) catalog: RwLock<IndexCatalog>,
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
            .map(|g| g.clone())
            .unwrap_or_default();
        let catalog = self.catalog.read().map(|g| g.clone()).unwrap_or_default();

        Self {
            catalog: RwLock::new(catalog),
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
