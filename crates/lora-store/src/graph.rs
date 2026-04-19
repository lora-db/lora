use crate::spatial::LoraPoint;
use crate::temporal::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraTime,
};
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

pub trait GraphStorage {
    // ---------- Node scans / lookup ----------

    fn all_nodes(&self) -> Vec<NodeRecord>;
    fn nodes_by_label(&self, label: &str) -> Vec<NodeRecord>;

    /// Borrow-based lookup. Required primitive; `node()` defaults to
    /// `node_ref(id).cloned()` so every implementation only has to supply the
    /// borrow variant.
    fn node_ref(&self, id: NodeId) -> Option<&NodeRecord>;

    fn node(&self, id: NodeId) -> Option<NodeRecord> {
        self.node_ref(id).cloned()
    }

    fn has_node(&self, id: NodeId) -> bool {
        self.node_ref(id).is_some()
    }

    fn node_count(&self) -> usize {
        self.all_node_ids().len()
    }

    /// ID-only scan. Default falls back to cloning; implementations should
    /// override for O(nodes) without cloning records/properties.
    fn all_node_ids(&self) -> Vec<NodeId> {
        self.all_nodes().into_iter().map(|n| n.id).collect()
    }

    /// Index lookup returning only IDs. Default falls back to cloning.
    fn node_ids_by_label(&self, label: &str) -> Vec<NodeId> {
        self.nodes_by_label(label).into_iter().map(|n| n.id).collect()
    }

    // ---------- Relationship scans / lookup ----------

    fn all_relationships(&self) -> Vec<RelationshipRecord>;
    fn relationships_by_type(&self, rel_type: &str) -> Vec<RelationshipRecord>;

    fn relationship_ref(&self, id: RelationshipId) -> Option<&RelationshipRecord>;

    fn relationship(&self, id: RelationshipId) -> Option<RelationshipRecord> {
        self.relationship_ref(id).cloned()
    }

    fn has_relationship(&self, id: RelationshipId) -> bool {
        self.relationship_ref(id).is_some()
    }

    fn relationship_count(&self) -> usize {
        self.all_rel_ids().len()
    }

    fn all_rel_ids(&self) -> Vec<RelationshipId> {
        self.all_relationships().into_iter().map(|r| r.id).collect()
    }

    fn rel_ids_by_type(&self, rel_type: &str) -> Vec<RelationshipId> {
        self.relationships_by_type(rel_type)
            .into_iter()
            .map(|r| r.id)
            .collect()
    }

    // ---------- Schema / introspection ----------

    fn all_labels(&self) -> Vec<String> {
        let mut labels = BTreeSet::new();
        for node in self.all_nodes() {
            for label in node.labels {
                labels.insert(label);
            }
        }
        labels.into_iter().collect()
    }

    fn all_relationship_types(&self) -> Vec<String> {
        let mut types = BTreeSet::new();
        for rel in self.all_relationships() {
            types.insert(rel.rel_type);
        }
        types.into_iter().collect()
    }

    fn all_node_property_keys(&self) -> Vec<String> {
        let mut keys = BTreeSet::new();
        for node in self.all_nodes() {
            for key in node.properties.keys() {
                keys.insert(key.clone());
            }
        }
        keys.into_iter().collect()
    }

    fn all_relationship_property_keys(&self) -> Vec<String> {
        let mut keys = BTreeSet::new();
        for rel in self.all_relationships() {
            for key in rel.properties.keys() {
                keys.insert(key.clone());
            }
        }
        keys.into_iter().collect()
    }

    fn all_property_keys(&self) -> Vec<String> {
        let mut keys = BTreeSet::new();

        for key in self.all_node_property_keys() {
            keys.insert(key);
        }

        for key in self.all_relationship_property_keys() {
            keys.insert(key);
        }

        keys.into_iter().collect()
    }

    fn label_property_keys(&self, label: &str) -> Vec<String> {
        let mut keys = BTreeSet::new();
        for node in self.nodes_by_label(label) {
            for key in node.properties.keys() {
                keys.insert(key.clone());
            }
        }
        keys.into_iter().collect()
    }

    fn rel_type_property_keys(&self, rel_type: &str) -> Vec<String> {
        let mut keys = BTreeSet::new();
        for rel in self.relationships_by_type(rel_type) {
            for key in rel.properties.keys() {
                keys.insert(key.clone());
            }
        }
        keys.into_iter().collect()
    }

    fn has_label_name(&self, label: &str) -> bool {
        self.all_labels().iter().any(|l| l == label)
    }

    fn has_relationship_type_name(&self, rel_type: &str) -> bool {
        self.all_relationship_types().iter().any(|t| t == rel_type)
    }

    fn has_property_key(&self, key: &str) -> bool {
        self.all_property_keys().iter().any(|k| k == key)
    }

    fn label_has_property_key(&self, label: &str, key: &str) -> bool {
        self.nodes_by_label(label)
            .into_iter()
            .any(|n| n.properties.contains_key(key))
    }

    fn rel_type_has_property_key(&self, rel_type: &str, key: &str) -> bool {
        self.relationships_by_type(rel_type)
            .into_iter()
            .any(|r| r.properties.contains_key(key))
    }

    // ---------- Property helpers ----------

    fn node_has_label(&self, node_id: NodeId, label: &str) -> bool {
        self.node_ref(node_id)
            .map(|n| n.labels.iter().any(|l| l == label))
            .unwrap_or(false)
    }

    fn node_labels(&self, node_id: NodeId) -> Option<Vec<String>> {
        self.node_ref(node_id).map(|n| n.labels.clone())
    }

    fn node_properties(&self, node_id: NodeId) -> Option<Properties> {
        self.node_ref(node_id).map(|n| n.properties.clone())
    }

    fn node_property(&self, node_id: NodeId, key: &str) -> Option<PropertyValue> {
        self.node_ref(node_id)
            .and_then(|n| n.properties.get(key).cloned())
    }

    fn relationship_type(&self, rel_id: RelationshipId) -> Option<String> {
        self.relationship_ref(rel_id).map(|r| r.rel_type.clone())
    }

    fn relationship_properties(&self, rel_id: RelationshipId) -> Option<Properties> {
        self.relationship_ref(rel_id).map(|r| r.properties.clone())
    }

    fn relationship_property(&self, rel_id: RelationshipId, key: &str) -> Option<PropertyValue> {
        self.relationship_ref(rel_id)
            .and_then(|r| r.properties.get(key).cloned())
    }

    // ---------- Relationship endpoint helpers ----------

    fn relationship_endpoints(&self, rel_id: RelationshipId) -> Option<(NodeId, NodeId)> {
        self.relationship_ref(rel_id).map(|r| (r.src, r.dst))
    }

    fn relationship_source(&self, rel_id: RelationshipId) -> Option<NodeId> {
        self.relationship_ref(rel_id).map(|r| r.src)
    }

    fn relationship_target(&self, rel_id: RelationshipId) -> Option<NodeId> {
        self.relationship_ref(rel_id).map(|r| r.dst)
    }

    fn other_node(&self, rel_id: RelationshipId, node_id: NodeId) -> Option<NodeId> {
        self.relationship_ref(rel_id)
            .and_then(|r| r.other_node(node_id))
    }

    // ---------- Traversal ----------

    fn outgoing_relationships(&self, node_id: NodeId) -> Vec<RelationshipRecord>;
    fn incoming_relationships(&self, node_id: NodeId) -> Vec<RelationshipRecord>;

    fn relationships_of(&self, node_id: NodeId, direction: Direction) -> Vec<RelationshipRecord> {
        match direction {
            Direction::Right => self.outgoing_relationships(node_id),
            Direction::Left => self.incoming_relationships(node_id),
            Direction::Undirected => {
                let mut rels = self.outgoing_relationships(node_id);
                rels.extend(self.incoming_relationships(node_id));
                rels
            }
        }
    }

    /// ID-only variant of `relationships_of`. Default uses `expand_ids` so
    /// implementations without adjacency overrides still avoid record clones.
    fn relationship_ids_of(&self, node_id: NodeId, direction: Direction) -> Vec<RelationshipId> {
        self.expand_ids(node_id, direction, &[])
            .into_iter()
            .map(|(rel_id, _)| rel_id)
            .collect()
    }

    fn expand(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<(RelationshipRecord, NodeRecord)> {
        let rels = self.relationships_of(node_id, direction);

        rels.into_iter()
            .filter(|r| types.is_empty() || types.iter().any(|t| t == &r.rel_type))
            .filter_map(|r| {
                let other_id = r.other_node(node_id)?;
                let other = self.node(other_id)?;
                Some((r, other))
            })
            .collect()
    }

    /// Lightweight traversal used on hot paths: only returns `(RelationshipId, NodeId)`
    /// pairs, avoiding the record + property-map clones of `expand()`.
    ///
    /// Callers that need rel/node records can look them up with
    /// `relationship_ref` / `node_ref`.
    fn expand_ids(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<(RelationshipId, NodeId)> {
        self.expand(node_id, direction, types)
            .into_iter()
            .map(|(r, n)| (r.id, n.id))
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
        self.expand(node_id, direction, types)
            .into_iter()
            .map(|(_, node)| node)
            .collect()
    }

    fn degree(&self, node_id: NodeId, direction: Direction) -> usize {
        match direction {
            Direction::Left => self.incoming_relationships(node_id).len(),
            Direction::Right => self.outgoing_relationships(node_id).len(),
            Direction::Undirected => {
                self.outgoing_relationships(node_id).len()
                    + self.incoming_relationships(node_id).len()
            }
        }
    }

    fn is_isolated(&self, node_id: NodeId) -> bool {
        self.degree(node_id, Direction::Undirected) == 0
    }

    // ---------- Optional optimization hooks ----------

    fn find_nodes_by_property(
        &self,
        label: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<NodeRecord> {
        let ids = match label {
            Some(label) => self.node_ids_by_label(label),
            None => self.all_node_ids(),
        };

        ids.into_iter()
            .filter_map(|id| {
                let n = self.node_ref(id)?;
                if n.properties.get(key) == Some(value) {
                    Some(n.clone())
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
    ) -> Vec<RelationshipRecord> {
        let ids = match rel_type {
            Some(rel_type) => self.rel_ids_by_type(rel_type),
            None => self.all_rel_ids(),
        };

        ids.into_iter()
            .filter_map(|id| {
                let r = self.relationship_ref(id)?;
                if r.properties.get(key) == Some(value) {
                    Some(r.clone())
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
    ) -> bool {
        !self
            .find_nodes_by_property(Some(label), key, value)
            .is_empty()
    }

    fn relationship_exists_with_type_and_property(
        &self,
        rel_type: &str,
        key: &str,
        value: &PropertyValue,
    ) -> bool {
        !self
            .find_relationships_by_property(Some(rel_type), key, value)
            .is_empty()
    }
}

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

    fn replace_node_properties(&mut self, node_id: NodeId, properties: Properties) -> bool {
        if !self.has_node(node_id) {
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
        if !self.has_node(node_id) {
            return false;
        }

        for (k, v) in properties {
            self.set_node_property(node_id, k, v);
        }

        true
    }

    fn add_node_label(&mut self, node_id: NodeId, label: &str) -> bool;
    fn remove_node_label(&mut self, node_id: NodeId, label: &str) -> bool;

    fn set_node_labels(&mut self, node_id: NodeId, labels: Vec<String>) -> bool {
        if !self.has_node(node_id) {
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

    // ---------- Relationship mutation ----------

    fn set_relationship_property(
        &mut self,
        rel_id: RelationshipId,
        key: String,
        value: PropertyValue,
    ) -> bool;

    fn remove_relationship_property(&mut self, rel_id: RelationshipId, key: &str) -> bool;

    fn replace_relationship_properties(&mut self, rel_id: RelationshipId, properties: Properties) -> bool {
        if !self.has_relationship(rel_id) {
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

    fn merge_relationship_properties(&mut self, rel_id: RelationshipId, properties: Properties) -> bool {
        if !self.has_relationship(rel_id) {
            return false;
        }

        for (k, v) in properties {
            self.set_relationship_property(rel_id, k, v);
        }

        true
    }

    // ---------- Deletion ----------

    fn delete_relationship(&mut self, rel_id: RelationshipId) -> bool;

    /// Returns false if the node still has attached relationships.
    fn delete_node(&mut self, node_id: NodeId) -> bool;

    /// Deletes the node and all attached relationships.
    fn detach_delete_node(&mut self, node_id: NodeId) -> bool;

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

    // ---------- Convenience helpers ----------

    fn get_or_create_node(
        &mut self,
        labels: Vec<String>,
        match_key: &str,
        match_value: &PropertyValue,
        init_properties: Properties,
    ) -> NodeRecord {
        for label in &labels {
            let matches = self.find_nodes_by_property(Some(label), match_key, match_value);
            if let Some(node) = matches.into_iter().next() {
                return node;
            }
        }

        self.create_node(labels, init_properties)
    }
}
