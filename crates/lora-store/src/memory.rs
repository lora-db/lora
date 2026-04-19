use std::collections::{BTreeMap, BTreeSet};

use lora_ast::Direction;

use crate::{
    GraphStorage, GraphStorageMut, NodeId, NodeRecord, Properties, PropertyValue, RelationshipId,
    RelationshipRecord,
};

#[derive(Debug, Clone, Default)]
pub struct InMemoryGraph {
    next_node_id: NodeId,
    next_rel_id: RelationshipId,

    nodes: BTreeMap<NodeId, NodeRecord>,
    relationships: BTreeMap<RelationshipId, RelationshipRecord>,

    // adjacency
    outgoing: BTreeMap<NodeId, BTreeSet<RelationshipId>>,
    incoming: BTreeMap<NodeId, BTreeSet<RelationshipId>>,

    // secondary indexes
    nodes_by_label: BTreeMap<String, BTreeSet<NodeId>>,
    relationships_by_type: BTreeMap<String, BTreeSet<RelationshipId>>,
}

impl InMemoryGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity_hint(_nodes: usize, _relationships: usize) -> Self {
        // BTreeMap/BTreeSet do not support capacity reservation.
        Self::default()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn contains_node(&self, node_id: NodeId) -> bool {
        self.nodes.contains_key(&node_id)
    }

    pub fn contains_relationship(&self, rel_id: RelationshipId) -> bool {
        self.relationships.contains_key(&rel_id)
    }

    fn alloc_node_id(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }

    fn alloc_rel_id(&mut self) -> RelationshipId {
        let id = self.next_rel_id;
        self.next_rel_id += 1;
        id
    }

    fn normalize_labels(labels: Vec<String>) -> Vec<String> {
        let mut seen = BTreeSet::new();

        labels
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .filter(|s| seen.insert(s.clone()))
            .collect()
    }

    fn insert_node_label_index(&mut self, node_id: NodeId, label: &str) {
        self.nodes_by_label
            .entry(label.to_string())
            .or_default()
            .insert(node_id);
    }

    fn remove_node_label_index(&mut self, node_id: NodeId, label: &str) {
        if let Some(ids) = self.nodes_by_label.get_mut(label) {
            ids.remove(&node_id);
            if ids.is_empty() {
                self.nodes_by_label.remove(label);
            }
        }
    }

    fn insert_relationship_type_index(&mut self, rel_id: RelationshipId, rel_type: &str) {
        self.relationships_by_type
            .entry(rel_type.to_string())
            .or_default()
            .insert(rel_id);
    }

    fn remove_relationship_type_index(&mut self, rel_id: RelationshipId, rel_type: &str) {
        if let Some(ids) = self.relationships_by_type.get_mut(rel_type) {
            ids.remove(&rel_id);
            if ids.is_empty() {
                self.relationships_by_type.remove(rel_type);
            }
        }
    }

    fn attach_relationship(&mut self, rel: &RelationshipRecord) {
        self.outgoing.entry(rel.src).or_default().insert(rel.id);
        self.incoming.entry(rel.dst).or_default().insert(rel.id);
        self.insert_relationship_type_index(rel.id, &rel.rel_type);
    }

    fn detach_relationship_indexes(&mut self, rel: &RelationshipRecord) {
        if let Some(ids) = self.outgoing.get_mut(&rel.src) {
            ids.remove(&rel.id);
            if ids.is_empty() {
                self.outgoing.remove(&rel.src);
            }
        }

        if let Some(ids) = self.incoming.get_mut(&rel.dst) {
            ids.remove(&rel.id);
            if ids.is_empty() {
                self.incoming.remove(&rel.dst);
            }
        }

        self.remove_relationship_type_index(rel.id, &rel.rel_type);
    }

    fn relationship_ids_for_direction(
        &self,
        node_id: NodeId,
        direction: Direction,
    ) -> Vec<RelationshipId> {
        match direction {
            Direction::Left => self
                .incoming
                .get(&node_id)
                .map(|ids| ids.iter().copied().collect())
                .unwrap_or_default(),

            Direction::Right => self
                .outgoing
                .get(&node_id)
                .map(|ids| ids.iter().copied().collect())
                .unwrap_or_default(),

            Direction::Undirected => {
                let mut ids = BTreeSet::new();

                if let Some(out) = self.outgoing.get(&node_id) {
                    ids.extend(out.iter().copied());
                }
                if let Some(inc) = self.incoming.get(&node_id) {
                    ids.extend(inc.iter().copied());
                }

                ids.into_iter().collect()
            }
        }
    }

    fn other_endpoint(rel: &RelationshipRecord, node_id: NodeId) -> Option<NodeId> {
        if rel.src == node_id {
            Some(rel.dst)
        } else if rel.dst == node_id {
            Some(rel.src)
        } else {
            None
        }
    }

    fn has_incident_relationships(&self, node_id: NodeId) -> bool {
        self.outgoing
            .get(&node_id)
            .map(|ids| !ids.is_empty())
            .unwrap_or(false)
            || self
                .incoming
                .get(&node_id)
                .map(|ids| !ids.is_empty())
                .unwrap_or(false)
    }

    fn incident_relationship_ids(&self, node_id: NodeId) -> BTreeSet<RelationshipId> {
        let mut rel_ids = BTreeSet::new();

        if let Some(ids) = self.outgoing.get(&node_id) {
            rel_ids.extend(ids.iter().copied());
        }
        if let Some(ids) = self.incoming.get(&node_id) {
            rel_ids.extend(ids.iter().copied());
        }

        rel_ids
    }
}

impl GraphStorage for InMemoryGraph {
    fn all_nodes(&self) -> Vec<NodeRecord> {
        self.nodes.values().cloned().collect()
    }

    fn nodes_by_label(&self, label: &str) -> Vec<NodeRecord> {
        self.nodes_by_label
            .get(label)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| self.nodes.get(id).cloned())
            .collect()
    }

    fn node_ref(&self, id: NodeId) -> Option<&NodeRecord> {
        self.nodes.get(&id)
    }

    fn all_node_ids(&self) -> Vec<NodeId> {
        self.nodes.keys().copied().collect()
    }

    fn node_ids_by_label(&self, label: &str) -> Vec<NodeId> {
        match self.nodes_by_label.get(label) {
            Some(ids) => ids.iter().copied().collect(),
            None => Vec::new(),
        }
    }

    fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn has_node(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    fn all_relationships(&self) -> Vec<RelationshipRecord> {
        self.relationships.values().cloned().collect()
    }

    fn relationships_by_type(&self, rel_type: &str) -> Vec<RelationshipRecord> {
        self.relationships_by_type
            .get(rel_type)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| self.relationships.get(id).cloned())
            .collect()
    }

    fn relationship_ref(&self, id: RelationshipId) -> Option<&RelationshipRecord> {
        self.relationships.get(&id)
    }

    fn all_rel_ids(&self) -> Vec<RelationshipId> {
        self.relationships.keys().copied().collect()
    }

    fn rel_ids_by_type(&self, rel_type: &str) -> Vec<RelationshipId> {
        match self.relationships_by_type.get(rel_type) {
            Some(ids) => ids.iter().copied().collect(),
            None => Vec::new(),
        }
    }

    fn relationship_count(&self) -> usize {
        self.relationships.len()
    }

    fn has_relationship(&self, id: RelationshipId) -> bool {
        self.relationships.contains_key(&id)
    }

    fn all_labels(&self) -> Vec<String> {
        self.nodes_by_label.keys().cloned().collect()
    }

    fn all_relationship_types(&self) -> Vec<String> {
        self.relationships_by_type.keys().cloned().collect()
    }

    fn all_node_property_keys(&self) -> Vec<String> {
        let mut keys = BTreeSet::new();
        for node in self.nodes.values() {
            for key in node.properties.keys() {
                keys.insert(key.clone());
            }
        }
        keys.into_iter().collect()
    }

    fn all_relationship_property_keys(&self) -> Vec<String> {
        let mut keys = BTreeSet::new();
        for rel in self.relationships.values() {
            for key in rel.properties.keys() {
                keys.insert(key.clone());
            }
        }
        keys.into_iter().collect()
    }

    fn label_property_keys(&self, label: &str) -> Vec<String> {
        let mut keys = BTreeSet::new();

        if let Some(ids) = self.nodes_by_label.get(label) {
            for id in ids {
                if let Some(node) = self.nodes.get(id) {
                    for key in node.properties.keys() {
                        keys.insert(key.clone());
                    }
                }
            }
        }

        keys.into_iter().collect()
    }

    fn rel_type_property_keys(&self, rel_type: &str) -> Vec<String> {
        let mut keys = BTreeSet::new();

        if let Some(ids) = self.relationships_by_type.get(rel_type) {
            for id in ids {
                if let Some(rel) = self.relationships.get(id) {
                    for key in rel.properties.keys() {
                        keys.insert(key.clone());
                    }
                }
            }
        }

        keys.into_iter().collect()
    }

    fn node_has_label(&self, node_id: NodeId, label: &str) -> bool {
        self.nodes
            .get(&node_id)
            .map(|n| n.labels.iter().any(|l| l == label))
            .unwrap_or(false)
    }

    fn node_property(&self, node_id: NodeId, key: &str) -> Option<PropertyValue> {
        self.nodes
            .get(&node_id)
            .and_then(|n| n.properties.get(key).cloned())
    }

    fn relationship_property(&self, rel_id: RelationshipId, key: &str) -> Option<PropertyValue> {
        self.relationships
            .get(&rel_id)
            .and_then(|r| r.properties.get(key).cloned())
    }

    fn expand(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<(RelationshipRecord, NodeRecord)> {
        if !self.nodes.contains_key(&node_id) {
            return Vec::new();
        }

        let type_filter: Option<BTreeSet<&str>> = if types.is_empty() {
            None
        } else {
            Some(types.iter().map(String::as_str).collect())
        };

        self.relationship_ids_for_direction(node_id, direction)
            .into_iter()
            .filter_map(|rel_id| self.relationships.get(&rel_id))
            .filter(|rel| {
                type_filter
                    .as_ref()
                    .map(|allowed| allowed.contains(rel.rel_type.as_str()))
                    .unwrap_or(true)
            })
            .filter_map(|rel| {
                let other_id = Self::other_endpoint(rel, node_id)?;
                let other = self.nodes.get(&other_id)?;
                Some((rel.clone(), other.clone()))
            })
            .collect()
    }

    fn expand_ids(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<(RelationshipId, NodeId)> {
        if !self.nodes.contains_key(&node_id) {
            return Vec::new();
        }

        // Fast path: no type filter — just join adjacency + rel endpoints.
        if types.is_empty() {
            return self
                .relationship_ids_for_direction(node_id, direction)
                .into_iter()
                .filter_map(|rel_id| {
                    let rel = self.relationships.get(&rel_id)?;
                    let other_id = Self::other_endpoint(rel, node_id)?;
                    Some((rel_id, other_id))
                })
                .collect();
        }

        // Type-filtered: borrow rel_type straight from the stored record.
        // For small type lists (the common case) the linear scan beats a
        // BTreeSet; we keep it allocation-free.
        self.relationship_ids_for_direction(node_id, direction)
            .into_iter()
            .filter_map(|rel_id| {
                let rel = self.relationships.get(&rel_id)?;
                if !types.iter().any(|t| t == &rel.rel_type) {
                    return None;
                }
                let other_id = Self::other_endpoint(rel, node_id)?;
                Some((rel_id, other_id))
            })
            .collect()
    }

    fn outgoing_relationships(&self, node_id: NodeId) -> Vec<RelationshipRecord> {
        self.outgoing
            .get(&node_id)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| self.relationships.get(id).cloned())
            .collect()
    }

    fn incoming_relationships(&self, node_id: NodeId) -> Vec<RelationshipRecord> {
        self.incoming
            .get(&node_id)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| self.relationships.get(id).cloned())
            .collect()
    }

    fn relationship_ids_of(&self, node_id: NodeId, direction: Direction) -> Vec<RelationshipId> {
        self.relationship_ids_for_direction(node_id, direction)
    }

    fn degree(&self, node_id: NodeId, direction: Direction) -> usize {
        match direction {
            Direction::Right => self.outgoing.get(&node_id).map(|s| s.len()).unwrap_or(0),
            Direction::Left => self.incoming.get(&node_id).map(|s| s.len()).unwrap_or(0),
            Direction::Undirected => {
                self.outgoing.get(&node_id).map(|s| s.len()).unwrap_or(0)
                    + self.incoming.get(&node_id).map(|s| s.len()).unwrap_or(0)
            }
        }
    }
}

impl GraphStorageMut for InMemoryGraph {
    fn create_node(&mut self, labels: Vec<String>, properties: Properties) -> NodeRecord {
        let id = self.alloc_node_id();
        let labels = Self::normalize_labels(labels);

        let node = NodeRecord {
            id,
            labels: labels.clone(),
            properties,
        };

        self.nodes.insert(id, node.clone());

        for label in &labels {
            self.insert_node_label_index(id, label);
        }

        self.outgoing.entry(id).or_default();
        self.incoming.entry(id).or_default();

        node
    }

    fn create_relationship(
        &mut self,
        src: NodeId,
        dst: NodeId,
        rel_type: &str,
        properties: Properties,
    ) -> Option<RelationshipRecord> {
        if !self.nodes.contains_key(&src) || !self.nodes.contains_key(&dst) {
            return None;
        }

        let trimmed = rel_type.trim();
        if trimmed.is_empty() {
            return None;
        }

        let id = self.alloc_rel_id();
        let rel = RelationshipRecord {
            id,
            src,
            dst,
            rel_type: trimmed.to_string(),
            properties,
        };

        self.attach_relationship(&rel);
        self.relationships.insert(id, rel.clone());

        Some(rel)
    }

    fn set_node_property(&mut self, node_id: NodeId, key: String, value: PropertyValue) -> bool {
        match self.nodes.get_mut(&node_id) {
            Some(node) => {
                node.properties.insert(key, value);
                true
            }
            None => false,
        }
    }

    fn remove_node_property(&mut self, node_id: NodeId, key: &str) -> bool {
        match self.nodes.get_mut(&node_id) {
            Some(node) => node.properties.remove(key).is_some(),
            None => false,
        }
    }

    fn add_node_label(&mut self, node_id: NodeId, label: &str) -> bool {
        let label = label.trim();
        if label.is_empty() {
            return false;
        }

        match self.nodes.get_mut(&node_id) {
            Some(node) => {
                if node.labels.iter().any(|l| l == label) {
                    return false;
                }

                node.labels.push(label.to_string());
                self.insert_node_label_index(node_id, label);
                true
            }
            None => false,
        }
    }

    fn remove_node_label(&mut self, node_id: NodeId, label: &str) -> bool {
        match self.nodes.get_mut(&node_id) {
            Some(node) => {
                let original_len = node.labels.len();
                node.labels.retain(|l| l != label);

                if node.labels.len() != original_len {
                    self.remove_node_label_index(node_id, label);
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }

    fn set_relationship_property(
        &mut self,
        rel_id: RelationshipId,
        key: String,
        value: PropertyValue,
    ) -> bool {
        match self.relationships.get_mut(&rel_id) {
            Some(rel) => {
                rel.properties.insert(key, value);
                true
            }
            None => false,
        }
    }

    fn remove_relationship_property(&mut self, rel_id: RelationshipId, key: &str) -> bool {
        match self.relationships.get_mut(&rel_id) {
            Some(rel) => rel.properties.remove(key).is_some(),
            None => false,
        }
    }

    fn delete_relationship(&mut self, rel_id: RelationshipId) -> bool {
        match self.relationships.remove(&rel_id) {
            Some(rel) => {
                self.detach_relationship_indexes(&rel);
                true
            }
            None => false,
        }
    }

    fn delete_node(&mut self, node_id: NodeId) -> bool {
        if !self.nodes.contains_key(&node_id) {
            return false;
        }

        if self.has_incident_relationships(node_id) {
            return false;
        }

        let node = match self.nodes.remove(&node_id) {
            Some(node) => node,
            None => return false,
        };

        for label in &node.labels {
            self.remove_node_label_index(node_id, label);
        }

        self.outgoing.remove(&node_id);
        self.incoming.remove(&node_id);

        true
    }

    fn detach_delete_node(&mut self, node_id: NodeId) -> bool {
        if !self.nodes.contains_key(&node_id) {
            return false;
        }

        let rel_ids: Vec<_> = self
            .incident_relationship_ids(node_id)
            .into_iter()
            .collect();

        for rel_id in rel_ids {
            let _ = self.delete_relationship(rel_id);
        }

        self.delete_node(node_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, PropertyValue)]) -> Properties {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn create_and_lookup_nodes() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(
            vec!["Person".into(), "Employee".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        );
        let b = g.create_node(
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Bob".into()))]),
        );

        assert_eq!(a.id, 0);
        assert_eq!(b.id, 1);

        assert_eq!(g.all_nodes().len(), 2);
        assert_eq!(g.nodes_by_label("Person").len(), 2);
        assert_eq!(g.nodes_by_label("Employee").len(), 1);
        assert!(g.node_has_label(a.id, "Person"));
        assert_eq!(
            g.node_property(a.id, "name"),
            Some(PropertyValue::String("Alice".into()))
        );
    }

    #[test]
    fn create_and_expand_relationships() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(vec!["Person".into()], Properties::new());
        let b = g.create_node(vec!["Person".into()], Properties::new());
        let c = g.create_node(vec!["Company".into()], Properties::new());

        let r1 = g
            .create_relationship(a.id, b.id, "KNOWS", Properties::new())
            .unwrap();
        let r2 = g
            .create_relationship(a.id, c.id, "WORKS_AT", Properties::new())
            .unwrap();

        assert_eq!(g.all_relationships().len(), 2);
        assert_eq!(g.relationships_by_type("KNOWS").len(), 1);
        assert_eq!(g.outgoing_relationships(a.id).len(), 2);
        assert_eq!(g.incoming_relationships(b.id).len(), 1);

        let knows = g.expand(a.id, Direction::Right, &[String::from("KNOWS")]);
        assert_eq!(knows.len(), 1);
        assert_eq!(knows[0].0.id, r1.id);
        assert_eq!(knows[0].1.id, b.id);

        let undirected = g.expand(a.id, Direction::Undirected, &[]);
        assert_eq!(undirected.len(), 2);

        assert_eq!(g.relationship(r2.id).unwrap().dst, c.id);
    }

    #[test]
    fn incoming_and_outgoing_are_distinct() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(vec!["Person".into()], Properties::new());
        let b = g.create_node(vec!["Person".into()], Properties::new());
        let c = g.create_node(vec!["Person".into()], Properties::new());

        g.create_relationship(a.id, b.id, "KNOWS", Properties::new())
            .unwrap();
        g.create_relationship(c.id, a.id, "LIKES", Properties::new())
            .unwrap();

        let outgoing = g.expand(a.id, Direction::Right, &[]);
        let incoming = g.expand(a.id, Direction::Left, &[]);

        assert_eq!(outgoing.len(), 1);
        assert_eq!(incoming.len(), 1);
        assert_eq!(outgoing[0].1.id, b.id);
        assert_eq!(incoming[0].1.id, c.id);
    }

    #[test]
    fn set_and_remove_properties() {
        let mut g = InMemoryGraph::new();

        let n = g.create_node(vec!["Person".into()], Properties::new());
        assert!(g.set_node_property(n.id, "age".into(), PropertyValue::Int(42)));
        assert_eq!(g.node_property(n.id, "age"), Some(PropertyValue::Int(42)));
        assert!(g.remove_node_property(n.id, "age"));
        assert_eq!(g.node_property(n.id, "age"), None);

        let m = g.create_node(vec!["Person".into()], Properties::new());
        let r = g
            .create_relationship(n.id, m.id, "KNOWS", Properties::new())
            .unwrap();

        assert!(g.set_relationship_property(r.id, "since".into(), PropertyValue::Int(2020)));
        assert_eq!(
            g.relationship_property(r.id, "since"),
            Some(PropertyValue::Int(2020))
        );
        assert!(g.remove_relationship_property(r.id, "since"));
        assert_eq!(g.relationship_property(r.id, "since"), None);
    }

    #[test]
    fn delete_requires_detach() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(vec!["Person".into()], Properties::new());
        let b = g.create_node(vec!["Person".into()], Properties::new());
        let r = g
            .create_relationship(a.id, b.id, "KNOWS", Properties::new())
            .unwrap();

        assert!(!g.delete_node(a.id));
        assert!(g.delete_relationship(r.id));
        assert!(g.delete_node(a.id));
        assert!(g.node(a.id).is_none());
    }

    #[test]
    fn detach_delete_removes_incident_relationships() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(vec!["Person".into()], Properties::new());
        let b = g.create_node(vec!["Person".into()], Properties::new());
        let c = g.create_node(vec!["Person".into()], Properties::new());

        let r1 = g
            .create_relationship(a.id, b.id, "KNOWS", Properties::new())
            .unwrap();
        let r2 = g
            .create_relationship(c.id, a.id, "LIKES", Properties::new())
            .unwrap();

        assert!(g.detach_delete_node(a.id));
        assert!(g.node(a.id).is_none());
        assert!(g.relationship(r1.id).is_none());
        assert!(g.relationship(r2.id).is_none());
        assert_eq!(g.all_relationships().len(), 0);
    }

    #[test]
    fn duplicate_labels_are_normalized_on_create() {
        let mut g = InMemoryGraph::new();

        let n = g.create_node(
            vec!["Person".into(), "Person".into(), "Admin".into()],
            Properties::new(),
        );

        assert_eq!(n.labels, vec!["Person".to_string(), "Admin".to_string()]);
        assert_eq!(g.nodes_by_label("Person").len(), 1);
        assert_eq!(g.nodes_by_label("Admin").len(), 1);
    }

    #[test]
    fn empty_labels_are_ignored() {
        let mut g = InMemoryGraph::new();

        let n = g.create_node(
            vec!["Person".into(), "".into(), "   ".into()],
            Properties::new(),
        );

        assert_eq!(n.labels, vec!["Person".to_string()]);
    }

    #[test]
    fn empty_relationship_type_is_rejected() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(vec!["A".into()], Properties::new());
        let b = g.create_node(vec!["B".into()], Properties::new());

        assert!(g
            .create_relationship(a.id, b.id, "", Properties::new())
            .is_none());
    }

    #[test]
    fn storage_schema_helpers_work() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        );
        let b = g.create_node(
            vec!["Company".into()],
            props(&[("title", PropertyValue::String("Acme".into()))]),
        );

        g.create_relationship(
            a.id,
            b.id,
            "WORKS_AT",
            props(&[("since", PropertyValue::Int(2020))]),
        )
        .unwrap();

        assert!(g.has_label_name("Person"));
        assert!(g.has_relationship_type_name("WORKS_AT"));
        assert!(g.has_property_key("name"));
        assert!(g.has_property_key("since"));
        assert!(g.label_has_property_key("Person", "name"));
        assert!(g.rel_type_has_property_key("WORKS_AT", "since"));
    }
}
