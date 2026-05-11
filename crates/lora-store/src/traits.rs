//! Storage trait surface: read, borrow, and mutate contracts.
//!
//! Backends speak the value types defined in [`crate::types`] and surface
//! them through the traits here. The split keeps the hot loop of "what
//! shape does a record have" (types) separate from "what can a backend
//! do with one" (traits).

use std::collections::BTreeSet;

use lora_ast::Direction;

use crate::memory::{
    ConstraintDefinition, ConstraintRequest, CreateConstraintError, CreateConstraintOutcome,
    CreateIndexError, CreateIndexOutcome, DropConstraintError, DropConstraintOutcome,
    DropIndexError, DropIndexOutcome, GraphStats, IndexDefinition, IndexRequest,
};
use crate::types::{
    ExpandedRelationship, NodeId, NodeRecord, Properties, PropertyValue, RelationshipId,
    RelationshipRecord,
};

// ============================================================================
// GraphStorage — the read-side storage contract
//
// The trait is intentionally layered into three groups: a small set of
// backend-neutral required primitives, a pair of optional optimization hooks
// (`with_node` / `with_relationship`), and a large cloud of defaulted helpers
// that derive from the primitives.
//
// Adding a new backend means implementing the required primitives (roughly a
// dozen methods) plus — optionally — overriding the hooks for zero-copy or the
// record-scan helpers for bulk perf. Implementors SHOULD NOT need to rewrite
// the catalog / traversal helper surface unless they can beat the default
// composition.
// ============================================================================

pub trait GraphStorage {
    // ---------- Required node primitives ----------

    /// Cheap existence check. Should not clone or materialize the record.
    fn contains_node(&self, id: NodeId) -> bool;

    /// Point lookup returning an owned record. Backends that can hand out
    /// borrows should also implement [`BorrowedGraphStorage::node_ref`] and
    /// override [`with_node`] to avoid clones on the hot path.
    fn node(&self, id: NodeId) -> Option<NodeRecord>;

    /// Enumerate every node id. Should be O(nodes) without cloning records.
    fn all_node_ids(&self) -> Vec<NodeId>;

    /// Enumerate node ids carrying the given label. Implementations that keep
    /// a label index should override this.
    fn node_ids_by_label(&self, label: &str) -> Vec<NodeId>;

    // ---------- Required relationship primitives ----------

    fn contains_relationship(&self, id: RelationshipId) -> bool;

    fn relationship(&self, id: RelationshipId) -> Option<RelationshipRecord>;

    fn all_rel_ids(&self) -> Vec<RelationshipId>;

    fn rel_ids_by_type(&self, rel_type: &str) -> Vec<RelationshipId>;

    /// Endpoint pair `(src, dst)` for a relationship. Required because
    /// traversal uses it on hot paths; a backend that stores endpoints
    /// alongside the id index can answer this without fetching properties.
    fn relationship_endpoints(&self, id: RelationshipId) -> Option<(NodeId, NodeId)>;

    // ---------- Required traversal primitive ----------

    /// Expand a node's incident relationships filtered by direction and
    /// (optional) types. This is the single traversal primitive; variable-
    /// length paths, degree, and adjacency helpers are all derived from it.
    fn expand_ids(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<(RelationshipId, NodeId)>;

    /// Visit expanded `(relationship_id, other_node_id)` pairs without
    /// forcing backends to allocate an intermediate Vec. The default keeps the
    /// trait easy to implement; hot backends can override it.
    fn try_for_each_expand_id<F, E>(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
        mut visit: F,
    ) -> Result<(), E>
    where
        F: FnMut(RelationshipId, NodeId) -> Result<(), E>,
        Self: Sized,
    {
        for (rel_id, other_id) in self.expand_ids(node_id, direction, types) {
            visit(rel_id, other_id)?;
        }
        Ok(())
    }

    // ---------- Required catalog primitives ----------

    fn all_labels(&self) -> Vec<String>;
    fn all_relationship_types(&self) -> Vec<String>;

    // ---------- Optional optimization hooks ----------
    //
    // Generic methods gated on `Self: Sized` so they don't affect object
    // safety. Backends override these to supply borrow-based access on hot
    // paths; defaults clone through `node` / `relationship`.

    fn with_node<F, R>(&self, id: NodeId, f: F) -> Option<R>
    where
        F: FnOnce(&NodeRecord) -> R,
        Self: Sized,
    {
        self.node(id).as_ref().map(f)
    }

    fn with_relationship<F, R>(&self, id: RelationshipId, f: F) -> Option<R>
    where
        F: FnOnce(&RelationshipRecord) -> R,
        Self: Sized,
    {
        self.relationship(id).as_ref().map(f)
    }

    // ---------- Defaulted: counts / existence aliases ----------

    fn has_node(&self, id: NodeId) -> bool {
        self.contains_node(id)
    }

    fn has_relationship(&self, id: RelationshipId) -> bool {
        self.contains_relationship(id)
    }

    fn node_count(&self) -> usize {
        self.all_node_ids().len()
    }

    fn relationship_count(&self) -> usize {
        self.all_rel_ids().len()
    }

    // ---------- Defaulted: record-returning scans ----------
    //
    // These synthesize full-record scans from id scans + point lookups. That
    // is correct for any backend and fast enough for small graphs, but a
    // backend that can scan records in one pass (in-memory via a BTreeMap
    // `.values()`, a column store via a streaming read) should override.

    fn all_nodes(&self) -> Vec<NodeRecord> {
        self.all_node_ids()
            .into_iter()
            .filter_map(|id| self.node(id))
            .collect()
    }

    fn nodes_by_label(&self, label: &str) -> Vec<NodeRecord> {
        self.node_ids_by_label(label)
            .into_iter()
            .filter_map(|id| self.node(id))
            .collect()
    }

    fn all_relationships(&self) -> Vec<RelationshipRecord> {
        self.all_rel_ids()
            .into_iter()
            .filter_map(|id| self.relationship(id))
            .collect()
    }

    fn relationships_by_type(&self, rel_type: &str) -> Vec<RelationshipRecord> {
        self.rel_ids_by_type(rel_type)
            .into_iter()
            .filter_map(|id| self.relationship(id))
            .collect()
    }

    // ---------- Defaulted: traversal helpers ----------

    fn relationship_ids_of(&self, node_id: NodeId, direction: Direction) -> Vec<RelationshipId> {
        self.expand_ids(node_id, direction, &[])
            .into_iter()
            .map(|(rel_id, _)| rel_id)
            .collect()
    }

    fn outgoing_relationships(&self, node_id: NodeId) -> Vec<RelationshipRecord> {
        self.relationship_ids_of(node_id, Direction::Right)
            .into_iter()
            .filter_map(|id| self.relationship(id))
            .collect()
    }

    fn incoming_relationships(&self, node_id: NodeId) -> Vec<RelationshipRecord> {
        self.relationship_ids_of(node_id, Direction::Left)
            .into_iter()
            .filter_map(|id| self.relationship(id))
            .collect()
    }

    fn relationships_of(&self, node_id: NodeId, direction: Direction) -> Vec<RelationshipRecord> {
        self.relationship_ids_of(node_id, direction)
            .into_iter()
            .filter_map(|id| self.relationship(id))
            .collect()
    }

    fn degree(&self, node_id: NodeId, direction: Direction) -> usize {
        self.expand_ids(node_id, direction, &[]).len()
    }

    fn is_isolated(&self, node_id: NodeId) -> bool {
        self.degree(node_id, Direction::Undirected) == 0
    }

    fn expand(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<(RelationshipRecord, NodeRecord)> {
        self.expand_ids(node_id, direction, types)
            .into_iter()
            .filter_map(|(rid, nid)| {
                let rel = self.relationship(rid)?;
                let node = self.node(nid)?;
                Some((rel, node))
            })
            .collect()
    }

    fn expand_detailed(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<ExpandedRelationship> {
        self.expand(node_id, direction, types)
            .into_iter()
            .map(|(relationship, other_node)| ExpandedRelationship {
                relationship,
                other_node,
            })
            .collect()
    }

    fn neighbors(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<NodeRecord> {
        self.expand_ids(node_id, direction, types)
            .into_iter()
            .filter_map(|(_, nid)| self.node(nid))
            .collect()
    }

    // ---------- Defaulted: narrow node accessors ----------

    fn node_has_label(&self, node_id: NodeId, label: &str) -> bool
    where
        Self: Sized,
    {
        self.with_node(node_id, |n| n.labels.iter().any(|l| l == label))
            .unwrap_or(false)
    }

    fn node_labels(&self, node_id: NodeId) -> Option<Vec<String>>
    where
        Self: Sized,
    {
        self.with_node(node_id, |n| n.labels.clone())
    }

    fn node_properties(&self, node_id: NodeId) -> Option<Properties>
    where
        Self: Sized,
    {
        self.with_node(node_id, |n| n.properties.clone())
    }

    fn node_property(&self, node_id: NodeId, key: &str) -> Option<PropertyValue>
    where
        Self: Sized,
    {
        self.with_node(node_id, |n| n.properties.get(key).cloned())
            .flatten()
    }

    // ---------- Defaulted: narrow relationship accessors ----------

    fn relationship_type(&self, rel_id: RelationshipId) -> Option<String>
    where
        Self: Sized,
    {
        self.with_relationship(rel_id, |r| r.rel_type.clone())
    }

    fn relationship_properties(&self, rel_id: RelationshipId) -> Option<Properties>
    where
        Self: Sized,
    {
        self.with_relationship(rel_id, |r| r.properties.clone())
    }

    fn relationship_property(&self, rel_id: RelationshipId, key: &str) -> Option<PropertyValue>
    where
        Self: Sized,
    {
        self.with_relationship(rel_id, |r| r.properties.get(key).cloned())
            .flatten()
    }

    fn relationship_source(&self, rel_id: RelationshipId) -> Option<NodeId> {
        self.relationship_endpoints(rel_id).map(|(s, _)| s)
    }

    fn relationship_target(&self, rel_id: RelationshipId) -> Option<NodeId> {
        self.relationship_endpoints(rel_id).map(|(_, d)| d)
    }

    fn other_node(&self, rel_id: RelationshipId, node_id: NodeId) -> Option<NodeId> {
        let (src, dst) = self.relationship_endpoints(rel_id)?;
        if src == node_id {
            Some(dst)
        } else if dst == node_id {
            Some(src)
        } else {
            None
        }
    }

    // ---------- Defaulted: catalog helpers ----------

    fn has_label_name(&self, label: &str) -> bool {
        self.all_labels().iter().any(|l| l == label)
    }

    fn has_relationship_type_name(&self, rel_type: &str) -> bool {
        self.all_relationship_types().iter().any(|t| t == rel_type)
    }

    fn all_node_property_keys(&self) -> Vec<String>
    where
        Self: Sized,
    {
        let mut keys = BTreeSet::new();
        for id in self.all_node_ids() {
            self.with_node(id, |n| {
                for key in n.properties.keys() {
                    keys.insert(key.clone());
                }
            });
        }
        keys.into_iter().collect()
    }

    fn all_relationship_property_keys(&self) -> Vec<String>
    where
        Self: Sized,
    {
        let mut keys = BTreeSet::new();
        for id in self.all_rel_ids() {
            self.with_relationship(id, |r| {
                for key in r.properties.keys() {
                    keys.insert(key.clone());
                }
            });
        }
        keys.into_iter().collect()
    }

    fn all_property_keys(&self) -> Vec<String>
    where
        Self: Sized,
    {
        let mut keys = BTreeSet::new();
        for key in self.all_node_property_keys() {
            keys.insert(key);
        }
        for key in self.all_relationship_property_keys() {
            keys.insert(key);
        }
        keys.into_iter().collect()
    }

    fn has_property_key(&self, key: &str) -> bool
    where
        Self: Sized,
    {
        self.all_node_property_keys().iter().any(|k| k == key)
            || self
                .all_relationship_property_keys()
                .iter()
                .any(|k| k == key)
    }

    fn label_property_keys(&self, label: &str) -> Vec<String>
    where
        Self: Sized,
    {
        let mut keys = BTreeSet::new();
        for id in self.node_ids_by_label(label) {
            self.with_node(id, |n| {
                for key in n.properties.keys() {
                    keys.insert(key.clone());
                }
            });
        }
        keys.into_iter().collect()
    }

    fn rel_type_property_keys(&self, rel_type: &str) -> Vec<String>
    where
        Self: Sized,
    {
        let mut keys = BTreeSet::new();
        for id in self.rel_ids_by_type(rel_type) {
            self.with_relationship(id, |r| {
                for key in r.properties.keys() {
                    keys.insert(key.clone());
                }
            });
        }
        keys.into_iter().collect()
    }

    fn label_has_property_key(&self, label: &str, key: &str) -> bool
    where
        Self: Sized,
    {
        self.node_ids_by_label(label).into_iter().any(|id| {
            self.with_node(id, |n| n.properties.contains_key(key))
                .unwrap_or(false)
        })
    }

    fn rel_type_has_property_key(&self, rel_type: &str, key: &str) -> bool
    where
        Self: Sized,
    {
        self.rel_ids_by_type(rel_type).into_iter().any(|id| {
            self.with_relationship(id, |r| r.properties.contains_key(key))
                .unwrap_or(false)
        })
    }

    // ---------- Defaulted: property-filter lookups ----------

    fn find_nodes_by_property(
        &self,
        label: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<NodeRecord>
    where
        Self: Sized,
    {
        let ids = match label {
            Some(label) => self.node_ids_by_label(label),
            None => self.all_node_ids(),
        };

        ids.into_iter()
            .filter_map(|id| {
                let matches = self
                    .with_node(id, |n| n.properties.get(key) == Some(value))
                    .unwrap_or(false);
                if matches {
                    self.node(id)
                } else {
                    None
                }
            })
            .collect()
    }

    fn find_node_ids_by_property(
        &self,
        label: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<NodeId>
    where
        Self: Sized,
    {
        self.find_nodes_by_property(label, key, value)
            .into_iter()
            .map(|n| n.id)
            .collect()
    }

    fn find_relationships_by_property(
        &self,
        rel_type: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<RelationshipRecord>
    where
        Self: Sized,
    {
        let ids = match rel_type {
            Some(rel_type) => self.rel_ids_by_type(rel_type),
            None => self.all_rel_ids(),
        };

        ids.into_iter()
            .filter_map(|id| {
                let matches = self
                    .with_relationship(id, |r| r.properties.get(key) == Some(value))
                    .unwrap_or(false);
                if matches {
                    self.relationship(id)
                } else {
                    None
                }
            })
            .collect()
    }

    fn find_relationship_ids_by_property(
        &self,
        rel_type: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<RelationshipId>
    where
        Self: Sized,
    {
        self.find_relationships_by_property(rel_type, key, value)
            .into_iter()
            .map(|r| r.id)
            .collect()
    }

    fn node_exists_with_label_and_property(
        &self,
        label: &str,
        key: &str,
        value: &PropertyValue,
    ) -> bool
    where
        Self: Sized,
    {
        self.node_ids_by_label(label).into_iter().any(|id| {
            self.with_node(id, |n| n.properties.get(key) == Some(value))
                .unwrap_or(false)
        })
    }

    fn relationship_exists_with_type_and_property(
        &self,
        rel_type: &str,
        key: &str,
        value: &PropertyValue,
    ) -> bool
    where
        Self: Sized,
    {
        self.rel_ids_by_type(rel_type).into_iter().any(|id| {
            self.with_relationship(id, |r| r.properties.get(key) == Some(value))
                .unwrap_or(false)
        })
    }

    // ---------- Defaulted: index catalog ----------
    //
    // Backends that maintain an index catalog (currently the in-memory
    // backend) override these. Backends without catalog support keep
    // the no-op defaults so callers can list / look up safely.

    fn list_indexes(&self) -> Vec<IndexDefinition> {
        Vec::new()
    }

    fn get_index(&self, _name: &str) -> Option<IndexDefinition> {
        None
    }

    /// Run a FULLTEXT index query against the named index. Returns
    /// `(entity_id, score)` pairs sorted descending by score. Backends
    /// without fulltext support return an empty vector; the caller is
    /// expected to have validated that the index exists via the
    /// catalog first.
    fn fulltext_search(&self, _name: &str, _query: &str) -> Vec<(u64, f64)> {
        Vec::new()
    }

    /// List explicitly-declared constraints. Backends without a
    /// constraint catalog return the empty vector.
    fn list_constraints(&self) -> Vec<ConstraintDefinition> {
        Vec::new()
    }

    fn get_constraint(&self, _name: &str) -> Option<ConstraintDefinition> {
        None
    }

    /// Mutation-time pre-check: would creating a node with these
    /// `labels` and `properties` violate any registered constraint?
    /// Default returns `Ok(())` so backends without a constraint
    /// catalog pay nothing. The in-memory backend overrides this and
    /// the call is virtually free when the catalog is empty.
    fn check_node_create_against_constraints(
        &self,
        _labels: &[String],
        _properties: &Properties,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Mutation-time pre-check for `CREATE ()-[r:TYPE { ... }]->()`.
    fn check_relationship_create_against_constraints(
        &self,
        _rel_type: &str,
        _properties: &Properties,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Mutation-time pre-check: would setting `key = value` on this
    /// node violate any registered constraint? Default `Ok(())`.
    fn check_node_set_property_against_constraints(
        &self,
        _node_id: NodeId,
        _key: &str,
        _value: &PropertyValue,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Mutation-time pre-check: would removing `key` on this node
    /// violate an existence / key constraint? Default `Ok(())`.
    fn check_node_remove_property_against_constraints(
        &self,
        _node_id: NodeId,
        _key: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Mutation-time pre-check: would replacing all properties on this
    /// node leave it in violation of any registered constraint? Default
    /// `Ok(())`.
    fn check_node_replace_properties_against_constraints(
        &self,
        _node_id: NodeId,
        _properties: &Properties,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Mutation-time pre-check: equivalent for relationship
    /// property writes.
    fn check_relationship_set_property_against_constraints(
        &self,
        _rel_id: RelationshipId,
        _key: &str,
        _value: &PropertyValue,
    ) -> Result<(), String> {
        Ok(())
    }

    fn check_relationship_remove_property_against_constraints(
        &self,
        _rel_id: RelationshipId,
        _key: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Mutation-time pre-check: would replacing all properties on this
    /// relationship leave it in violation of any registered constraint?
    /// Default `Ok(())`.
    fn check_relationship_replace_properties_against_constraints(
        &self,
        _rel_id: RelationshipId,
        _properties: &Properties,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Mutation-time pre-check: would adding `label` to this node
    /// activate a constraint the node currently violates?
    fn check_node_add_label_against_constraints(
        &self,
        _node_id: NodeId,
        _label: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Cardinality snapshot used by the cost model. Backends without
    /// per-label / per-type indexes return [`GraphStats::default()`],
    /// which the planner treats as "no information available".
    fn graph_stats(&self) -> GraphStats {
        GraphStats::default()
    }

    /// Trigram-index candidates for `query` on `label.property`.
    ///
    /// Semantics:
    /// * `Some(ids)` → these node ids *might* match (refilter required).
    /// * `None` → no trigram scope for `(label, property)`; caller must
    ///   fall back to a full scan.
    ///
    /// Backends without text-index support always return `None`.
    fn node_text_candidates(
        &self,
        _label: &str,
        _property: &str,
        _query: &str,
    ) -> Option<Vec<NodeId>> {
        None
    }

    /// Sorted-index candidates for a `[lo, hi]` range on `label.property`.
    /// Both bounds are inclusive at this layer; the caller refilters with
    /// the precise predicate inclusivity (`>` vs `>=`, `<` vs `<=`).
    ///
    /// Returns `None` when no scope exists — caller falls back to scan.
    fn node_range_candidates(
        &self,
        _label: &str,
        _property: &str,
        _lo: Option<&PropertyValue>,
        _hi: Option<&PropertyValue>,
    ) -> Option<Vec<NodeId>> {
        None
    }

    /// Spatial-index candidates inside the closed `[ll, ur]` 2D
    /// bounding box. The executor refilters every id with the precise
    /// predicate, including the z-coordinate when the indexed point
    /// is 3D.
    fn node_point_within_bbox(
        &self,
        _label: &str,
        _property: &str,
        _ll: (f64, f64),
        _ur: (f64, f64),
    ) -> Option<Vec<NodeId>> {
        None
    }

    /// Spatial-index candidates within `max_distance` of `(x, y)`. The
    /// candidate set is conservative — the actual great-circle /
    /// cartesian distance check is the executor's responsibility.
    fn node_point_within_distance(
        &self,
        _label: &str,
        _property: &str,
        _center: (f64, f64),
        _max_distance: f64,
    ) -> Option<Vec<NodeId>> {
        None
    }

    /// Trigram-index candidates for relationships of `rel_type` whose
    /// `property` value matches `query` (substring/prefix/suffix). Mirror
    /// of [`Self::node_text_candidates`] for relationship-target indexes.
    fn relationship_text_candidates(
        &self,
        _rel_type: &str,
        _property: &str,
        _query: &str,
    ) -> Option<Vec<RelationshipId>> {
        None
    }

    /// Sorted-index candidates for relationships of `rel_type` on the
    /// closed `[lo, hi]` range. Mirror of [`Self::node_range_candidates`].
    fn relationship_range_candidates(
        &self,
        _rel_type: &str,
        _property: &str,
        _lo: Option<&PropertyValue>,
        _hi: Option<&PropertyValue>,
    ) -> Option<Vec<RelationshipId>> {
        None
    }

    /// Spatial-index candidates inside the closed `[ll, ur]` 2D bounding
    /// box, scoped to relationships of `rel_type`. Mirror of
    /// [`Self::node_point_within_bbox`].
    fn relationship_point_within_bbox(
        &self,
        _rel_type: &str,
        _property: &str,
        _ll: (f64, f64),
        _ur: (f64, f64),
    ) -> Option<Vec<RelationshipId>> {
        None
    }

    /// Spatial-index candidates within `max_distance` of `(x, y)`,
    /// scoped to relationships of `rel_type`. Mirror of
    /// [`Self::node_point_within_distance`].
    fn relationship_point_within_distance(
        &self,
        _rel_type: &str,
        _property: &str,
        _center: (f64, f64),
        _max_distance: f64,
    ) -> Option<Vec<RelationshipId>> {
        None
    }
}

// ============================================================================
// GraphCatalog — narrow schema-query slice used by the analyzer.
//
// Blanket-implemented for every `GraphStorage`, so the analyzer can bound on
// `GraphCatalog` without every backend having to implement a second trait.
// ============================================================================

pub trait GraphCatalog {
    fn node_count(&self) -> usize;
    fn relationship_count(&self) -> usize;
    fn has_label_name(&self, label: &str) -> bool;
    fn has_relationship_type_name(&self, rel_type: &str) -> bool;
    fn has_property_key(&self, key: &str) -> bool;
}

impl<T: GraphStorage> GraphCatalog for T {
    fn node_count(&self) -> usize {
        GraphStorage::node_count(self)
    }
    fn relationship_count(&self) -> usize {
        GraphStorage::relationship_count(self)
    }
    fn has_label_name(&self, label: &str) -> bool {
        GraphStorage::has_label_name(self, label)
    }
    fn has_relationship_type_name(&self, rel_type: &str) -> bool {
        GraphStorage::has_relationship_type_name(self, rel_type)
    }
    fn has_property_key(&self, key: &str) -> bool {
        GraphStorage::has_property_key(self, key)
    }
}

// ============================================================================
// BorrowedGraphStorage — optional capability for backends that can hand out
// long-lived borrows into internal records.
//
// The executor prefers `with_node` / `with_relationship` on hot paths because
// they work for both borrow-capable and owned-only backends. This trait is
// available for callers that really do want a `&NodeRecord` outliving the
// closure — mostly internal optimization paths and tests.
// ============================================================================

pub trait BorrowedGraphStorage: GraphStorage {
    fn node_ref(&self, id: NodeId) -> Option<&NodeRecord>;
    fn relationship_ref(&self, id: RelationshipId) -> Option<&RelationshipRecord>;

    fn node_refs(&self) -> Box<dyn Iterator<Item = &NodeRecord> + '_> {
        Box::new(
            self.all_node_ids()
                .into_iter()
                .filter_map(|id| self.node_ref(id)),
        )
    }

    fn node_refs_by_label(&self, label: &str) -> Box<dyn Iterator<Item = &NodeRecord> + '_> {
        Box::new(
            self.node_ids_by_label(label)
                .into_iter()
                .filter_map(|id| self.node_ref(id)),
        )
    }

    fn relationship_refs(&self) -> Box<dyn Iterator<Item = &RelationshipRecord> + '_> {
        Box::new(
            self.all_rel_ids()
                .into_iter()
                .filter_map(|id| self.relationship_ref(id)),
        )
    }

    fn relationship_refs_by_type(
        &self,
        rel_type: &str,
    ) -> Box<dyn Iterator<Item = &RelationshipRecord> + '_> {
        Box::new(
            self.rel_ids_by_type(rel_type)
                .into_iter()
                .filter_map(|id| self.relationship_ref(id)),
        )
    }
}

// ============================================================================
// GraphStorageMut — write-side storage contract.
//
// A backend that implements `GraphStorage` can additionally implement
// `GraphStorageMut` to support create / mutate / delete / admin operations.
// Everything above the `Defaulted convenience helpers` block is a required
// primitive; everything below is defaulted and can be overridden for perf.
// ============================================================================

pub trait GraphStorageMut: GraphStorage {
    // ---------- Creation ----------

    fn create_node(&mut self, labels: Vec<String>, properties: Properties) -> NodeRecord;

    fn create_relationship(
        &mut self,
        src: NodeId,
        dst: NodeId,
        rel_type: &str,
        properties: Properties,
    ) -> Option<RelationshipRecord>;

    // ---------- Node mutation ----------

    fn set_node_property(&mut self, node_id: NodeId, key: String, value: PropertyValue) -> bool;

    fn remove_node_property(&mut self, node_id: NodeId, key: &str) -> bool;

    fn add_node_label(&mut self, node_id: NodeId, label: &str) -> bool;
    fn remove_node_label(&mut self, node_id: NodeId, label: &str) -> bool;

    // ---------- Relationship mutation ----------

    fn set_relationship_property(
        &mut self,
        rel_id: RelationshipId,
        key: String,
        value: PropertyValue,
    ) -> bool;

    fn remove_relationship_property(&mut self, rel_id: RelationshipId, key: &str) -> bool;

    // ---------- Deletion ----------

    fn delete_relationship(&mut self, rel_id: RelationshipId) -> bool;

    /// Returns false if the node still has attached relationships.
    fn delete_node(&mut self, node_id: NodeId) -> bool;

    /// Deletes the node and all attached relationships.
    fn detach_delete_node(&mut self, node_id: NodeId) -> bool;

    // ---------- Admin / lifecycle ----------

    /// Drop every node and every relationship, returning the store to an
    /// empty state. Provided as a trait method so callers (bindings, admin
    /// tools) can reset a graph without knowing the concrete backend.
    ///
    /// Future snapshot / WAL / restore entry points will also hang off the
    /// `GraphStorageMut` surface — `clear` is the first of them.
    fn clear(&mut self);

    /// Register an explicitly-declared index in the catalog. Backends that
    /// don't maintain a catalog return [`CreateIndexError::Unsupported`].
    ///
    /// `if_not_exists` collapses both name and schema-equivalence
    /// conflicts into [`CreateIndexOutcome::NoOpExists`] instead of
    /// surfacing them as errors.
    #[allow(clippy::result_large_err)]
    fn create_index(
        &mut self,
        _request: IndexRequest,
        _if_not_exists: bool,
    ) -> Result<CreateIndexOutcome, CreateIndexError> {
        Err(CreateIndexError::Unsupported(
            "this backend does not maintain an index catalog",
        ))
    }

    /// Remove an explicitly-declared index from the catalog. Backends
    /// without catalog support return [`DropIndexError::Unsupported`].
    /// `if_exists` collapses missing-index errors into
    /// [`DropIndexOutcome::NoOpMissing`].
    fn drop_index(
        &mut self,
        _name: &str,
        _if_exists: bool,
    ) -> Result<DropIndexOutcome, DropIndexError> {
        Err(DropIndexError::Unsupported(
            "this backend does not maintain an index catalog",
        ))
    }

    /// Register an explicitly-declared constraint. Backends without
    /// catalog support return [`CreateConstraintError::Unsupported`].
    /// Uniqueness/key kinds may transparently register a backing range
    /// index of the same name.
    fn create_constraint(
        &mut self,
        _request: ConstraintRequest,
        _if_not_exists: bool,
    ) -> Result<CreateConstraintOutcome, CreateConstraintError> {
        Err(CreateConstraintError::Unsupported(
            "this backend does not maintain a constraint catalog",
        ))
    }

    /// Drop a named constraint. Cascades to the backing index if the
    /// constraint owned one.
    fn drop_constraint(
        &mut self,
        _name: &str,
        _if_exists: bool,
    ) -> Result<DropConstraintOutcome, DropConstraintError> {
        Err(DropConstraintError::Unsupported(
            "this backend does not maintain a constraint catalog",
        ))
    }

    // ---------- Defaulted convenience helpers ----------

    fn replace_node_properties(&mut self, node_id: NodeId, properties: Properties) -> bool
    where
        Self: Sized,
    {
        if !self.contains_node(node_id) {
            return false;
        }

        let existing_keys = match self.node_properties(node_id) {
            Some(props) => props.into_keys().collect::<Vec<_>>(),
            None => return false,
        };

        for key in existing_keys {
            self.remove_node_property(node_id, &key);
        }

        for (k, v) in properties {
            self.set_node_property(node_id, k, v);
        }

        true
    }

    fn merge_node_properties(&mut self, node_id: NodeId, properties: Properties) -> bool {
        if !self.contains_node(node_id) {
            return false;
        }

        for (k, v) in properties {
            self.set_node_property(node_id, k, v);
        }

        true
    }

    fn set_node_labels(&mut self, node_id: NodeId, labels: Vec<String>) -> bool
    where
        Self: Sized,
    {
        if !self.contains_node(node_id) {
            return false;
        }

        let current = match self.node_labels(node_id) {
            Some(labels) => labels,
            None => return false,
        };

        for label in &current {
            self.remove_node_label(node_id, label);
        }

        for label in &labels {
            self.add_node_label(node_id, label);
        }

        true
    }

    fn replace_relationship_properties(
        &mut self,
        rel_id: RelationshipId,
        properties: Properties,
    ) -> bool
    where
        Self: Sized,
    {
        if !self.contains_relationship(rel_id) {
            return false;
        }

        let existing_keys = match self.relationship_properties(rel_id) {
            Some(props) => props.into_keys().collect::<Vec<_>>(),
            None => return false,
        };

        for key in existing_keys {
            self.remove_relationship_property(rel_id, &key);
        }

        for (k, v) in properties {
            self.set_relationship_property(rel_id, k, v);
        }

        true
    }

    fn merge_relationship_properties(
        &mut self,
        rel_id: RelationshipId,
        properties: Properties,
    ) -> bool {
        if !self.contains_relationship(rel_id) {
            return false;
        }

        for (k, v) in properties {
            self.set_relationship_property(rel_id, k, v);
        }

        true
    }

    fn delete_relationships_of(&mut self, node_id: NodeId, direction: Direction) -> usize {
        let rel_ids = self.relationship_ids_of(node_id, direction);

        let mut deleted = 0;
        for rel_id in rel_ids {
            if self.delete_relationship(rel_id) {
                deleted += 1;
            }
        }
        deleted
    }

    fn get_or_create_node(
        &mut self,
        labels: Vec<String>,
        match_key: &str,
        match_value: &PropertyValue,
        init_properties: Properties,
    ) -> NodeRecord
    where
        Self: Sized,
    {
        for label in &labels {
            let matches = self.find_nodes_by_property(Some(label), match_key, match_value);
            if let Some(node) = matches.into_iter().next() {
                return node;
            }
        }

        self.create_node(labels, init_properties)
    }
}
