//! The index builds and constraint scans a restore runs on every core
//! give what their one-thread forms give, on graphs large enough to take
//! the parallel paths.

use super::constraint_catalog::{ConstraintDefinition, StoredConstraintKind};
use super::entity_index_store::ScopedPropertyKey;
use super::index_catalog::StoredIndexEntity;
use super::property_index::{PropertyIndexKey, PropertyIndexState};
use super::sorted_property_index::SortedPropertyIndex;
use super::InMemoryGraph;
use crate::{intern, GraphStorageMut, Properties, PropertyValue};

/// Past `PARALLEL_INDEX_MIN`, and over several 65,536-id ranges.
const NODES: u64 = 150_000;

/// A value for node `i`: unique strings mostly, with repeated strings,
/// repeated integers, lists, and nodes without the key mixed in.
fn value(i: u64) -> Option<PropertyValue> {
    if i.is_multiple_of(17) {
        None
    } else if i.is_multiple_of(7) {
        Some(PropertyValue::Int((i % 50) as i64))
    } else if i.is_multiple_of(11) {
        Some(PropertyValue::String(format!("dup{}", i % 13)))
    } else if i.is_multiple_of(23) {
        Some(PropertyValue::List(vec![
            PropertyValue::Int((i % 5) as i64),
            PropertyValue::String("x".into()),
        ]))
    } else {
        Some(PropertyValue::String(format!("v{i}")))
    }
}

fn graph(value: impl Fn(u64) -> Option<PropertyValue>) -> InMemoryGraph {
    let mut graph = InMemoryGraph::new();
    for i in 0..NODES {
        let labels = match i % 3 {
            0 => vec!["A".to_string()],
            1 => vec!["A".to_string(), "B".to_string()],
            _ => Vec::new(),
        };
        let mut properties = Properties::new();
        if let Some(value) = value(i) {
            properties.insert(intern("k"), value);
        }
        properties.insert(intern("n"), PropertyValue::Int(i as i64));
        graph.create_node(labels, properties);
    }
    graph
}

#[test]
fn a_property_index_built_on_every_core_matches_the_one_by_one_build() {
    let graph = graph(value);
    let mut serial = PropertyIndexState::default();
    serial.insert_bulk(
        "k",
        || {
            graph.iter_nodes().filter_map(|(id, node)| {
                let value = node.properties().get("k")?;
                Some((id, node.labels().strs(), value))
            })
        },
        &graph.node_values(),
    );
    let parallel = graph.build_node_property_index_parallel("k");
    assert_eq!(serial.scoped_values.len(), 3, "A, B and unlabelled");
    assert!(serial.scoped_values == parallel.scoped_values);
}

/// A sorted scope's entries, comparable.
type Entries = (
    Vec<(PropertyIndexKey, Vec<u64>)>,
    Vec<(PropertyIndexKey, Vec<u64>)>,
);

fn entries(index: &SortedPropertyIndex, label: &str) -> Entries {
    let scope = &index.by_scope[&ScopedPropertyKey::new(label, "k")];
    let mut elements: Vec<_> = scope
        .by_element
        .iter()
        .map(|(key, ids)| (key.clone(), ids.to_vec()))
        .collect();
    elements.sort();
    let values = scope
        .by_value
        .iter()
        .map(|(key, ids)| (key.clone(), ids.to_vec()))
        .collect();
    (values, elements)
}

#[test]
fn a_sorted_index_built_on_every_core_matches_the_one_by_one_build() {
    let graph = graph(value);
    let mut serial = SortedPropertyIndex::default();
    serial.add_scope("A", "k");
    for (id, node) in graph.iter_nodes() {
        if !node.labels().strs().any(|l| l == "A") {
            continue;
        }
        if let Some(value) = node.properties().get("k") {
            serial.insert("A", "k", id, &value.to_owned());
        }
    }
    graph.activate_sorted_scope(StoredIndexEntity::Node, "A", "k");
    let parallel = graph.sorted_indexes_read(StoredIndexEntity::Node);
    assert_eq!(entries(&serial, "A"), entries(&parallel, "A"));
}

fn constraint(kind: StoredConstraintKind) -> ConstraintDefinition {
    ConstraintDefinition {
        backing: Default::default(),
        name: "c".into(),
        kind,
        entity: StoredIndexEntity::Node,
        label: "A".into(),
        properties: vec!["k".into()],
        owned_index: None,
    }
}

/// Unique values on every `A` node but `repeat` (which takes node 3's)
/// and `missing` (which has none).
fn keyed(repeat: u64, missing: u64) -> InMemoryGraph {
    graph(move |i| {
        (i != missing)
            .then(|| PropertyValue::String(format!("key{}", if i == repeat { 3 } else { i })))
    })
}

#[test]
fn the_constraint_scan_reports_what_a_scan_in_id_order_meets_first() {
    let unique = constraint(StoredConstraintKind::Unique);
    let key = constraint(StoredConstraintKind::NodeKey);

    assert!(keyed(u64::MAX, u64::MAX)
        .validate_existing_data_for_constraint(&key)
        .is_ok());

    // Node 3's value again at 100,002, a missing key at 120,000: the
    // repeat comes first.
    let graph = keyed(100_002, 120_000);
    let err = graph
        .validate_existing_data_for_constraint(&key)
        .unwrap_err();
    assert_eq!(err.gql_status(), "22N79");
    // Plain uniqueness skips the node without the key.
    let err = graph
        .validate_existing_data_for_constraint(&unique)
        .unwrap_err();
    assert_eq!(err.gql_status(), "22N79");

    // The missing key first.
    let graph = keyed(120_000, 99_999);
    let err = graph
        .validate_existing_data_for_constraint(&key)
        .unwrap_err();
    assert_eq!(err.gql_status(), "22N77");
    let err = graph
        .validate_existing_data_for_constraint(&unique)
        .unwrap_err();
    assert_eq!(err.gql_status(), "22N79");
}

#[test]
fn relationships_restored_in_batches_keep_every_adjacency_list_in_order() {
    use super::SnapshotGraphBuilder;
    use crate::GraphStorage;
    use lora_ast::Direction;

    // Hubs with many relationships, self-loops, and both directions, so
    // one adjacency leaf takes ends from many batches.
    let mut graph = InMemoryGraph::new();
    for i in 0..20_000u64 {
        graph.create_node(vec![format!("L{}", i % 3)], Properties::new());
    }
    for i in 0..60_000u64 {
        let (src, dst) = match i % 4 {
            0 => (i % 20_000, (i * 7) % 20_000),
            1 => (i % 50, i % 20_000),
            2 => (i % 20_000, i % 20_000),
            _ => ((i * 13) % 20_000, i % 7),
        };
        let mut properties = Properties::new();
        properties.insert(intern("w"), PropertyValue::Int(i as i64));
        graph.create_relationship(src, dst, if i % 2 == 0 { "A" } else { "B" }, properties);
    }

    let names = graph.snapshot_names();
    let mut builder = SnapshotGraphBuilder::new(graph.next_node_id, graph.next_rel_id).unwrap();
    let encoder = builder.prime(&names).unwrap();
    let mut nodes = encoder.node_batch(0);
    for node in graph.snapshot_nodes_in(0..u64::MAX) {
        let labels: Vec<u32> = node.label_ids().collect();
        let mut properties = node.properties().map(|(k, v)| (k, v.to_owned())).collect();
        encoder
            .push_node(&mut nodes, node.id(), &labels, &mut properties)
            .unwrap();
    }
    builder.push_nodes(nodes).unwrap();
    // Uneven batches, filed a few at a time as rounds are.
    let mut batches = Vec::new();
    for start in (0..60_000u64).step_by(7_000) {
        let mut batch = encoder.relationship_batch(0);
        for rel in graph.snapshot_relationships_in(start..start + 7_000) {
            let mut properties = rel.properties().map(|(k, v)| (k, v.to_owned())).collect();
            encoder
                .push_relationship(
                    &mut batch,
                    rel.id(),
                    rel.src(),
                    rel.dst(),
                    rel.type_id(),
                    &mut properties,
                )
                .unwrap();
        }
        batches.push(batch);
        if batches.len() == 3 {
            builder
                .push_relationship_batches(std::mem::take(&mut batches))
                .unwrap();
        }
    }
    builder.push_relationship_batches(batches).unwrap();
    let restored = builder.finish(Vec::new(), Vec::new(), Vec::new()).unwrap();

    for node in 0..20_000u64 {
        for direction in [Direction::Right, Direction::Left, Direction::Undirected] {
            assert_eq!(
                restored.expand_ids(node, direction, &[]),
                graph.expand_ids(node, direction, &[]),
                "node {node} {direction:?}"
            );
        }
    }
    assert_eq!(restored.snapshot_payload(), graph.snapshot_payload());
}
