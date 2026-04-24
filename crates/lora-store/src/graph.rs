use crate::spatial::LoraPoint;
use crate::temporal::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraTime,
};
use crate::vector::LoraVector;
use lora_ast::Direction;
use std::collections::{BTreeMap, BTreeSet};

pub type NodeId = u64;
pub type RelationshipId = u64;

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<PropertyValue>),
    Map(BTreeMap<String, PropertyValue>),
    Date(LoraDate),
    Time(LoraTime),
    LocalTime(LoraLocalTime),
    DateTime(LoraDateTime),
    LocalDateTime(LoraLocalDateTime),
    Duration(LoraDuration),
    Point(LoraPoint),
    Vector(LoraVector),
}

pub type Properties = BTreeMap<String, PropertyValue>;

#[derive(Debug, Clone, PartialEq)]
pub struct NodeRecord {
    pub id: NodeId,
    pub labels: Vec<String>,
    pub properties: Properties,
}

impl NodeRecord {
    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    pub fn property(&self, key: &str) -> Option<&PropertyValue> {
        self.properties.get(key)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelationshipRecord {
    pub id: RelationshipId,
    pub src: NodeId,
    pub dst: NodeId,
    pub rel_type: String,
    pub properties: Properties,
}

impl RelationshipRecord {
    pub fn property(&self, key: &str) -> Option<&PropertyValue> {
        self.properties.get(key)
    }

    pub fn other_node(&self, node_id: NodeId) -> Option<NodeId> {
        if self.src == node_id {
            Some(self.dst)
        } else if self.dst == node_id {
            Some(self.src)
        } else {
            None
        }
    }

    pub fn matches_direction_from(&self, node_id: NodeId, direction: Direction) -> bool {
        match direction {
            Direction::Right => self.src == node_id,
            Direction::Left => self.dst == node_id,
            Direction::Undirected => self.src == node_id || self.dst == node_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedRelationship {
    pub relationship: RelationshipRecord,
    pub other_node: NodeRecord,
}

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
            || self.all_relationship_property_keys().iter().any(|k| k == key)
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
