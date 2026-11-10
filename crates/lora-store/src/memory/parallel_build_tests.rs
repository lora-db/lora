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

/// Past `PARALLEL_INDEX_MIN` over all nodes and over the two thirds that
/// carry `A` (so a sorted index on `A` takes the parallel path too), and
/// over several 65,536-id ranges.
const NODES: u64 = 200_000;

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
    let parallel = graph.build_node_property_index_parallel("k", None);
    assert_eq!(serial.scoped_values.len(), 3, "A, B and unlabelled");
    assert!(serial.scoped_values == parallel.scoped_values);
}

/// A build told where its key occurs reads those blocks only, and gives
/// the index a build reading every node gives: for a key on most nodes,
/// one on a band of them and a few strays, and one on none.
#[test]
fn a_property_index_built_from_the_blocks_holding_its_key_matches_the_full_scan() {
    let mut graph = InMemoryGraph::new();
    for i in 0..NODES {
        let mut properties = Properties::new();
        if let Some(value) = value(i) {
            properties.insert(intern("k"), value);
        }
        if (70_000..70_900).contains(&i) || i % 30_011 == 0 {
            properties.insert(intern("rare"), PropertyValue::Int((i % 40) as i64));
        }
        if i == 0 {
            properties.insert(intern("lost"), PropertyValue::Int(1));
        }
        let labels = if i % 2 == 0 {
            vec!["A".to_string()]
        } else {
            Vec::new()
        };
        graph.create_node(labels, properties);
    }
    // No node holds `lost` any more, though the key is still numbered.
    graph.remove_node_property(0, "lost");

    let keys: Vec<String> = ["k", "rare", "lost", "never"].map(String::from).to_vec();
    let blocks = graph.key_blocks(StoredIndexEntity::Node, &keys);
    for key in &keys {
        let full = graph.build_node_property_index_parallel(key, None);
        let within = graph.build_node_property_index_parallel(key, Some(&blocks));
        assert!(full.scoped_values == within.scoped_values, "{key}");
        assert!(full.any_scope == within.any_scope, "{key}");
    }
    let rare = graph.build_node_property_index_parallel("rare", Some(&blocks));
    assert!(!rare.scoped_values.is_empty());
    // A key the pass was not given is read everywhere.
    let unlisted = graph.key_blocks(StoredIndexEntity::Node, &["rare".to_string()]);
    let full = graph.build_node_property_index_parallel("k", None);
    let within = graph.build_node_property_index_parallel("k", Some(&unlisted));
    assert!(full.scoped_values == within.scoped_values);
}

/// A fulltext index's content, comparable: each term's `(entity,
/// frequency)` list and each entity's terms.
type FulltextContent = (Vec<(String, Vec<(u64, u32)>)>, Vec<(u64, Vec<String>)>);

fn fulltext_content(index: &super::fulltext_index::FulltextIndex) -> FulltextContent {
    let postings = index
        .postings
        .iter()
        .map(|(term, list)| {
            (
                term.to_string(),
                list.iter().map(|(id, tf)| (*id, *tf)).collect(),
            )
        })
        .collect();
    let mut entities: Vec<(u64, Vec<String>)> = index
        .entity_terms
        .iter()
        .map(|(id, terms)| (*id, terms.iter().map(|term| term.to_string()).collect()))
        .collect();
    entities.sort();
    (postings, entities)
}

/// The index a backfill builds at once holds what indexing each entity in
/// turn holds: over two labels sharing nodes, a string and a list
/// property, repeated and accented words, and nodes without text.
#[test]
fn a_fulltext_index_built_at_once_matches_the_entity_by_entity_build() {
    use super::fulltext_index::{term_counts_for_properties, FulltextIndex};
    use super::index_catalog::{IndexRequest, StoredIndexKind};

    let words = [
        "Night",
        "night",
        "Sónar",
        "Øya",
        "love-song",
        "42",
        "the",
        "The",
        "x",
    ];
    let mut graph = InMemoryGraph::new();
    for i in 0..40_000u64 {
        let labels = match i % 4 {
            0 => vec!["A".to_string()],
            1 => vec!["B".to_string(), "A".to_string()],
            2 => vec!["B".to_string()],
            _ => vec!["C".to_string()],
        };
        let mut properties = Properties::new();
        if i % 9 != 0 {
            let w = |n: u64| words[(n % words.len() as u64) as usize];
            properties.insert(
                intern("title"),
                PropertyValue::String(format!("{} {} t{} {}", w(i), w(i / 3), i % 500, w(i))),
            );
        }
        if i % 5 == 0 {
            properties.insert(
                intern("names"),
                PropertyValue::List(vec![
                    PropertyValue::String(format!("alias{} night", i % 70)),
                    PropertyValue::Int(7),
                    PropertyValue::String("...".into()),
                ]),
            );
        }
        properties.insert(
            intern("other"),
            PropertyValue::String("unindexed words".into()),
        );
        graph.create_node(labels, properties);
    }
    let labels = vec!["A".to_string(), "B".to_string()];
    let properties = vec!["title".to_string(), "names".to_string()];

    let mut serial = FulltextIndex::from_chunks(labels.clone(), properties.clone(), Vec::new());
    for (id, node) in graph.iter_nodes() {
        if node.labels().strs().any(|l| labels.iter().any(|w| w == l)) {
            let counts = term_counts_for_properties(node.properties(), &properties);
            if !counts.is_empty() {
                serial.reindex_entity(id, counts);
            }
        }
    }

    graph
        .register_index(
            IndexRequest {
                explicit_name: Some("search".into()),
                kind: StoredIndexKind::Fulltext,
                entity: StoredIndexEntity::Node,
                label: Some("A".into()),
                additional_labels: vec!["B".into()],
                properties: properties.clone(),
                options: Default::default(),
            },
            false,
        )
        .unwrap();
    let registry = graph.fulltext_indexes_read(StoredIndexEntity::Node);
    let built = registry.get("search").expect("registered");
    let (postings, entities) = fulltext_content(built);
    assert!(
        postings.len() > 300 && entities.len() > 20_000,
        "{} terms, {} entities",
        postings.len(),
        entities.len()
    );
    assert!(postings.iter().any(|(term, _)| term == "sonar"));
    assert_eq!((postings, entities), fulltext_content(&serial));
    assert_eq!(
        built.query("night sona*", Some(5)),
        serial.query("night sona*", Some(5))
    );
}

/// A sorted index over a label too small for the parallel path reads the
/// label's nodes through the label index, and holds what a scan of every
/// node finds.
#[test]
fn a_sorted_index_on_a_small_label_matches_the_scan_of_every_node() {
    let mut graph = graph(value);
    for i in 0..300u64 {
        let mut properties = Properties::new();
        if let Some(value) = value(i) {
            properties.insert(intern("k"), value);
        }
        graph.create_node(vec!["Few".to_string(), "A".to_string()], properties);
    }
    let mut serial = SortedPropertyIndex::default();
    serial.add_scope("Few", "k");
    for (id, node) in graph.iter_nodes() {
        if !node.labels().strs().any(|l| l == "Few") {
            continue;
        }
        if let Some(value) = node.properties().get("k") {
            serial.insert("Few", "k", id, &value.to_owned());
        }
    }
    graph.activate_sorted_scope(StoredIndexEntity::Node, "Few", "k");
    let built = graph.sorted_indexes_read(StoredIndexEntity::Node);
    assert_eq!(entries(&serial, "Few"), entries(&built, "Few"));
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
        .entries(&std::ops::Bound::Unbounded, &std::ops::Bound::Unbounded)
        .map(|(key, ids)| (key.to_key(), ids.to_vec()))
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

/// A lookup that finds its key's index being built by another lookup
/// scans instead of building it a second time, and answers the same.
#[test]
fn a_lookup_scans_while_another_builds_its_index() {
    use crate::GraphStorage;
    let graph = graph(value);
    let wanted = PropertyValue::String("v4".into());
    let build = graph.indexes.property_builds.claim(true, "k").unwrap();
    assert!(graph.indexes.property_builds.claim(true, "k").is_none());
    let scanned = graph.find_node_ids_by_property(Some("A"), "k", &wanted);
    assert_eq!(scanned, vec![4]);
    assert!(!graph.indexes_read().node_properties.is_active("k"));
    drop(build);
    let indexed = graph.find_node_ids_by_property(Some("A"), "k", &wanted);
    assert_eq!(indexed, scanned);
    assert!(graph.indexes_read().node_properties.is_active("k"));
}
