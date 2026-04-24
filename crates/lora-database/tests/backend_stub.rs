//! Proves the storage trait surface works for backends that cannot hand out
//! long-lived borrows.
//!
//! The refactor in `lora-store` demoted `node_ref` / `relationship_ref` onto
//! an optional [`BorrowedGraphStorage`] capability. Everything the engine
//! actually needs is reachable through the required primitives plus the
//! closure-based `with_node` / `with_relationship` hooks (which default to
//! owned fetches if a backend does not override them).
//!
//! `OwnedMapStore` below is deliberately minimal: BTreeMap-backed, no label
//! / type / adjacency indexes, and critically **no impl of
//! `BorrowedGraphStorage`**. If a query can run end-to-end against it via
//! `Database`, the trait surface is genuinely backend-neutral.

use std::collections::{BTreeMap, BTreeSet};

use lora_ast::Direction;
use lora_database::{Database, ExecuteOptions, QueryResult, ResultFormat};
use lora_store::{
    GraphStorage, GraphStorageMut, NodeId, NodeRecord, Properties, PropertyValue, RelationshipId,
    RelationshipRecord,
};

#[derive(Debug, Default)]
struct OwnedMapStore {
    next_node_id: NodeId,
    next_rel_id: RelationshipId,
    nodes: BTreeMap<NodeId, NodeRecord>,
    relationships: BTreeMap<RelationshipId, RelationshipRecord>,
}

impl OwnedMapStore {
    fn new() -> Self {
        Self::default()
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
}

impl GraphStorage for OwnedMapStore {
    fn contains_node(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    fn node(&self, id: NodeId) -> Option<NodeRecord> {
        self.nodes.get(&id).cloned()
    }

    fn all_node_ids(&self) -> Vec<NodeId> {
        self.nodes.keys().copied().collect()
    }

    fn node_ids_by_label(&self, label: &str) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, n)| n.labels.iter().any(|l| l == label))
            .map(|(id, _)| *id)
            .collect()
    }

    fn contains_relationship(&self, id: RelationshipId) -> bool {
        self.relationships.contains_key(&id)
    }

    fn relationship(&self, id: RelationshipId) -> Option<RelationshipRecord> {
        self.relationships.get(&id).cloned()
    }

    fn all_rel_ids(&self) -> Vec<RelationshipId> {
        self.relationships.keys().copied().collect()
    }

    fn rel_ids_by_type(&self, rel_type: &str) -> Vec<RelationshipId> {
        self.relationships
            .iter()
            .filter(|(_, r)| r.rel_type == rel_type)
            .map(|(id, _)| *id)
            .collect()
    }

    fn relationship_endpoints(&self, id: RelationshipId) -> Option<(NodeId, NodeId)> {
        self.relationships.get(&id).map(|r| (r.src, r.dst))
    }

    fn expand_ids(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<(RelationshipId, NodeId)> {
        // No adjacency index — scan every relationship. Correct but O(E) per
        // expand call; sufficient for a compliance test.
        self.relationships
            .values()
            .filter(|r| {
                if !types.is_empty() && !types.iter().any(|t| t == &r.rel_type) {
                    return false;
                }
                match direction {
                    Direction::Right => r.src == node_id,
                    Direction::Left => r.dst == node_id,
                    Direction::Undirected => r.src == node_id || r.dst == node_id,
                }
            })
            .filter_map(|r| {
                let other = if r.src == node_id {
                    r.dst
                } else if r.dst == node_id {
                    r.src
                } else {
                    return None;
                };
                Some((r.id, other))
            })
            .collect()
    }

    fn all_labels(&self) -> Vec<String> {
        let mut labels = BTreeSet::new();
        for n in self.nodes.values() {
            for l in &n.labels {
                labels.insert(l.clone());
            }
        }
        labels.into_iter().collect()
    }

    fn all_relationship_types(&self) -> Vec<String> {
        let mut types = BTreeSet::new();
        for r in self.relationships.values() {
            types.insert(r.rel_type.clone());
        }
        types.into_iter().collect()
    }

    // Intentionally NOT overriding `with_node` / `with_relationship` — the
    // defaults (clone-through `node` / `relationship`) must be sufficient for
    // every executor/analyzer call site.
    //
    // Intentionally NOT implementing `BorrowedGraphStorage` — this proves the
    // engine no longer depends on `&NodeRecord` / `&RelationshipRecord`.
}

impl GraphStorageMut for OwnedMapStore {
    fn create_node(&mut self, labels: Vec<String>, properties: Properties) -> NodeRecord {
        let id = self.alloc_node_id();
        let labels: Vec<String> = {
            let mut seen = BTreeSet::new();
            labels
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .filter(|s| seen.insert(s.clone()))
                .collect()
        };
        let record = NodeRecord {
            id,
            labels,
            properties,
        };
        self.nodes.insert(id, record.clone());
        record
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
        let record = RelationshipRecord {
            id,
            src,
            dst,
            rel_type: trimmed.to_string(),
            properties,
        };
        self.relationships.insert(id, record.clone());
        Some(record)
    }

    fn set_node_property(&mut self, node_id: NodeId, key: String, value: PropertyValue) -> bool {
        match self.nodes.get_mut(&node_id) {
            Some(n) => {
                n.properties.insert(key, value);
                true
            }
            None => false,
        }
    }

    fn remove_node_property(&mut self, node_id: NodeId, key: &str) -> bool {
        match self.nodes.get_mut(&node_id) {
            Some(n) => n.properties.remove(key).is_some(),
            None => false,
        }
    }

    fn add_node_label(&mut self, node_id: NodeId, label: &str) -> bool {
        let label = label.trim();
        if label.is_empty() {
            return false;
        }
        match self.nodes.get_mut(&node_id) {
            Some(n) => {
                if n.labels.iter().any(|l| l == label) {
                    return false;
                }
                n.labels.push(label.to_string());
                true
            }
            None => false,
        }
    }

    fn remove_node_label(&mut self, node_id: NodeId, label: &str) -> bool {
        match self.nodes.get_mut(&node_id) {
            Some(n) => {
                let before = n.labels.len();
                n.labels.retain(|l| l != label);
                n.labels.len() != before
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
            Some(r) => {
                r.properties.insert(key, value);
                true
            }
            None => false,
        }
    }

    fn remove_relationship_property(&mut self, rel_id: RelationshipId, key: &str) -> bool {
        match self.relationships.get_mut(&rel_id) {
            Some(r) => r.properties.remove(key).is_some(),
            None => false,
        }
    }

    fn delete_relationship(&mut self, rel_id: RelationshipId) -> bool {
        self.relationships.remove(&rel_id).is_some()
    }

    fn delete_node(&mut self, node_id: NodeId) -> bool {
        if !self.nodes.contains_key(&node_id) {
            return false;
        }
        let incident = self
            .relationships
            .values()
            .any(|r| r.src == node_id || r.dst == node_id);
        if incident {
            return false;
        }
        self.nodes.remove(&node_id).is_some()
    }

    fn detach_delete_node(&mut self, node_id: NodeId) -> bool {
        if !self.nodes.contains_key(&node_id) {
            return false;
        }
        let incident: Vec<RelationshipId> = self
            .relationships
            .values()
            .filter(|r| r.src == node_id || r.dst == node_id)
            .map(|r| r.id)
            .collect();
        for id in incident {
            self.relationships.remove(&id);
        }
        self.nodes.remove(&node_id).is_some()
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

fn rows_of(result: QueryResult) -> Vec<Vec<lora_database::LoraValue>> {
    match result {
        QueryResult::RowArrays(r) => r.rows,
        other => panic!("expected RowArrays, got {:?}", other),
    }
}

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::RowArrays,
    })
}

#[test]
fn queries_run_without_borrow_access() {
    let db: Database<OwnedMapStore> = Database::from_graph(OwnedMapStore::new());

    db.execute("CREATE (:Person {name: 'Alice', age: 30})", opts())
        .unwrap();
    db.execute("CREATE (:Person {name: 'Bob', age: 25})", opts())
        .unwrap();
    db.execute(
        "MATCH (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'}) \
         CREATE (a)-[:KNOWS {since: 2020}]->(b)",
        opts(),
    )
    .unwrap();

    // Scan + property read — exercises with_node through the owned-fetch default.
    let rows = rows_of(
        db.execute(
            "MATCH (p:Person) RETURN p.name AS name ORDER BY p.name",
            opts(),
        )
        .unwrap(),
    );
    assert_eq!(rows.len(), 2);

    // Traversal + property filter on the relationship — exercises
    // with_relationship through the default path.
    let rows = rows_of(
        db.execute(
            "MATCH (a:Person)-[k:KNOWS]->(b:Person) \
             WHERE k.since = 2020 \
             RETURN a.name AS a, b.name AS b",
            opts(),
        )
        .unwrap(),
    );
    assert_eq!(rows.len(), 1);

    // Catalog call path (has_label_name / has_relationship_type_name /
    // has_property_key used by the analyzer).
    assert_eq!(db.node_count(), 2);
    assert_eq!(db.relationship_count(), 1);

    // Admin path.
    db.clear();
    assert_eq!(db.node_count(), 0);
    assert_eq!(db.relationship_count(), 0);
}

#[test]
fn variable_length_path_runs_without_borrow_access() {
    let db: Database<OwnedMapStore> = Database::from_graph(OwnedMapStore::new());

    db.execute(
        "CREATE (a:N {id: 1})-[:NEXT]->(b:N {id: 2})-[:NEXT]->(c:N {id: 3})",
        opts(),
    )
    .unwrap();

    let rows = rows_of(
        db.execute(
            "MATCH (a:N {id: 1})-[:NEXT*1..3]->(b:N) \
             RETURN b.id AS id ORDER BY b.id",
            opts(),
        )
        .unwrap(),
    );
    assert_eq!(rows.len(), 2);
}
