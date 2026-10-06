//! Randomized comparison of [`InMemoryGraph`] with a plain model.
//!
//! The store keeps records as encoded bytes, adjacency as packed lists and
//! several indexes beside them, all copy-on-write across graph clones. The
//! model is two `BTreeMap`s. A seeded run applies the same random
//! operations to both and compares every way of reading the store with
//! the model: owned records, views, scans, expansion, equality lookups
//! with and without a label, a snapshot round trip, and clones taken
//! along the way that must keep reading what they read when taken.

use std::collections::{BTreeMap, BTreeSet};

use lora_ast::Direction;

use super::InMemoryGraph;
use crate::{
    intern, GraphStorage, GraphStorageMut, IndexRequest, LoraDate, Properties, PropertyValue,
    StoredIndexEntity, StoredIndexKind,
};

const LABELS: [&str; 5] = ["A", "B", "C", "Dd", "Long label"];
const TYPES: [&str; 4] = ["R", "S", "T_LONG", "U"];
const KEYS: [&str; 8] = ["a", "b", "c", "k", "name", "z", "é", "m00"];

#[derive(Clone, Debug, PartialEq)]
struct ModelNode {
    labels: Vec<String>,
    props: BTreeMap<String, PropertyValue>,
}

#[derive(Clone, Debug, PartialEq)]
struct ModelRel {
    src: u64,
    dst: u64,
    rel_type: String,
    props: BTreeMap<String, PropertyValue>,
}

#[derive(Clone, Default)]
struct Model {
    nodes: BTreeMap<u64, ModelNode>,
    rels: BTreeMap<u64, ModelRel>,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    fn pick_key<V>(&mut self, map: &BTreeMap<u64, V>) -> Option<u64> {
        if map.is_empty() {
            return None;
        }
        map.keys().nth(self.below(map.len())).copied()
    }

    /// A value from a small pool, so equality lookups find several
    /// entities and indexes hold buckets of more than one id.
    fn value(&mut self) -> PropertyValue {
        match self.below(12) {
            0 => PropertyValue::Null,
            1 => PropertyValue::Bool(self.below(2) == 0),
            2 | 3 => PropertyValue::Int(self.below(4) as i64 - 1),
            4 => PropertyValue::Int(i64::MAX - self.below(2) as i64),
            5 => PropertyValue::Float(self.below(3) as f64 * 0.5),
            6 | 7 => {
                PropertyValue::String(["", "x", "longer string value", "é"][self.below(4)].into())
            }
            8 => PropertyValue::List(vec![
                PropertyValue::Int(self.below(2) as i64),
                PropertyValue::String("in list".into()),
            ]),
            9 => PropertyValue::Map(
                [(
                    "inner".to_string(),
                    PropertyValue::Int(self.below(2) as i64),
                )]
                .into_iter()
                .collect(),
            ),
            10 => PropertyValue::Date(LoraDate {
                year: 2026,
                month: 1 + self.below(2) as u32,
                day: 6,
            }),
            _ => PropertyValue::String("x".repeat(40 + self.below(200))),
        }
    }

    fn props(&mut self) -> BTreeMap<String, PropertyValue> {
        let mut out = BTreeMap::new();
        for _ in 0..self.below(5) {
            out.insert(self.pick(&KEYS).to_string(), self.value());
        }
        out
    }

    fn labels(&mut self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for _ in 0..self.below(4) {
            let label = self.pick(&LABELS).to_string();
            if !out.contains(&label) {
                out.push(label);
            }
        }
        out
    }
}

fn to_properties(props: &BTreeMap<String, PropertyValue>) -> Properties {
    props.iter().map(|(k, v)| (intern(k), v.clone())).collect()
}

fn sorted(mut ids: Vec<u64>) -> Vec<u64> {
    ids.sort_unstable();
    ids
}

impl Model {
    fn incident(&self, node: u64) -> Vec<u64> {
        self.rels
            .iter()
            .filter(|(_, r)| r.src == node || r.dst == node)
            .map(|(id, _)| *id)
            .collect()
    }

    /// `(relationship, neighbour)` pairs an expansion must report. An
    /// undirected expansion reports a self-loop once.
    fn expand(&self, node: u64, direction: Direction, types: &[String]) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        for (id, rel) in &self.rels {
            if !types.is_empty() && !types.contains(&rel.rel_type) {
                continue;
            }
            let outgoing = rel.src == node;
            let incoming = rel.dst == node;
            match direction {
                Direction::Right if outgoing => out.push((*id, rel.dst)),
                Direction::Left if incoming => out.push((*id, rel.src)),
                Direction::Undirected if outgoing => out.push((*id, rel.dst)),
                Direction::Undirected if incoming => out.push((*id, rel.src)),
                _ => {}
            }
        }
        out.sort_unstable();
        out
    }
}

/// Compare everything readable from `g` with `model`.
fn check(g: &InMemoryGraph, model: &Model, context: &str) {
    // Nodes: ids, owned records, views.
    assert_eq!(
        g.all_node_ids(),
        model.nodes.keys().copied().collect::<Vec<_>>(),
        "{context}: node ids"
    );
    assert_eq!(g.node_count(), model.nodes.len(), "{context}");
    for (id, expected) in &model.nodes {
        let record = g
            .node(*id)
            .unwrap_or_else(|| panic!("{context}: node {id}"));
        assert_eq!(record.id, *id);
        assert_eq!(
            record.labels.to_strings(),
            expected.labels,
            "{context}: node {id}"
        );
        let props: BTreeMap<String, PropertyValue> = record
            .properties
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        assert_eq!(props, expected.props, "{context}: node {id}");

        g.with_node(*id, |view| {
            assert_eq!(view.id(), *id);
            assert_eq!(view.labels().to_strings(), expected.labels);
            assert_eq!(view.labels().len(), expected.labels.len());
            for label in LABELS {
                assert_eq!(
                    view.has_label(label),
                    expected.labels.iter().any(|l| l == label)
                );
            }
            assert_eq!(view.properties().len(), expected.props.len());
            let seen: Vec<(String, PropertyValue)> = view
                .properties()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_owned()))
                .collect();
            let wanted: Vec<(String, PropertyValue)> = expected
                .props
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            assert_eq!(seen, wanted, "{context}: node {id} view, in key order");
            for key in KEYS {
                let got = view.property(key);
                assert_eq!(got.map(|v| v.to_owned()).as_ref(), expected.props.get(key));
                if let (Some(got), Some(want)) = (got, expected.props.get(key)) {
                    assert!(got == *want);
                }
            }
            assert_eq!(view.to_record(), record);
        })
        .unwrap();
    }
    assert!(g.node(u64::MAX).is_none());

    // Label scans, as sets (the index does not keep id order) and paged.
    for label in LABELS {
        let expected: Vec<u64> = model
            .nodes
            .iter()
            .filter(|(_, n)| n.labels.iter().any(|l| l == label))
            .map(|(id, _)| *id)
            .collect();
        let listed = g.node_ids_by_label(label);
        assert_eq!(sorted(listed.clone()), expected, "{context}: label {label}");
        assert_eq!(g.node_count_by_label(label), expected.len());
        let mut cursor = 0;
        let mut paged = Vec::new();
        while g.scan_node_ids(Some(label), &mut cursor, 7, &mut paged) {}
        assert_eq!(paged, listed, "{context}: paged label {label}");
    }
    let mut cursor = 0;
    let mut paged = Vec::new();
    while g.scan_node_ids(None, &mut cursor, 5, &mut paged) {}
    assert_eq!(paged, g.all_node_ids(), "{context}: paged all");

    // Relationships.
    assert_eq!(
        g.all_rel_ids(),
        model.rels.keys().copied().collect::<Vec<_>>(),
        "{context}: rel ids"
    );
    for (id, expected) in &model.rels {
        let record = g
            .relationship(*id)
            .unwrap_or_else(|| panic!("{context}: rel {id}"));
        assert_eq!((record.src, record.dst), (expected.src, expected.dst));
        assert_eq!(record.rel_type, expected.rel_type);
        let props: BTreeMap<String, PropertyValue> = record
            .properties
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        assert_eq!(props, expected.props, "{context}: rel {id}");
        assert_eq!(
            g.relationship_endpoints(*id),
            Some((expected.src, expected.dst))
        );
        g.with_relationship(*id, |view| {
            assert_eq!(
                (view.id(), view.src(), view.dst()),
                (*id, expected.src, expected.dst)
            );
            assert_eq!(view.rel_type(), expected.rel_type);
            assert_eq!(view.properties().to_owned(), record.properties);
            assert_eq!(view.to_record(), record);
        })
        .unwrap();
    }
    for rel_type in TYPES {
        let expected: Vec<u64> = model
            .rels
            .iter()
            .filter(|(_, r)| r.rel_type == rel_type)
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(
            sorted(g.rel_ids_by_type(rel_type)),
            expected,
            "{context}: type {rel_type}"
        );
    }

    // Expansion, for every node, direction and a few type filters.
    let filters: Vec<Vec<String>> = vec![
        vec![],
        vec!["R".into()],
        vec!["T_LONG".into(), "U".into()],
        vec!["NEVER".into()],
        vec!["NEVER".into(), "S".into()],
    ];
    for id in model.nodes.keys() {
        for direction in [Direction::Right, Direction::Left, Direction::Undirected] {
            for types in &filters {
                let mut got = g.expand_ids(*id, direction, types);
                got.sort_unstable();
                assert_eq!(
                    got,
                    model.expand(*id, direction, types),
                    "{context}: expand {id} {direction:?} {types:?}"
                );
            }
            assert_eq!(
                g.degree(*id, direction),
                model.expand(*id, direction, &[]).len(),
                "{context}: degree {id} {direction:?}"
            );
        }
    }

    // Equality lookups. The first one for a key builds its index; later
    // rounds check that the index followed every write since.
    for key in ["k", "name", "é"] {
        let mut values: BTreeSet<String> = BTreeSet::new();
        let mut probes: Vec<PropertyValue> = vec![PropertyValue::Int(12345)];
        for node in model.nodes.values() {
            if let Some(value) = node.props.get(key) {
                if values.insert(format!("{value:?}")) {
                    probes.push(value.clone());
                }
            }
        }
        for value in &probes {
            for label in [None, Some("A"), Some("Long label"), Some("NoSuchLabel")] {
                let expected: Vec<u64> = model
                    .nodes
                    .iter()
                    .filter(|(_, n)| label.is_none_or(|l| n.labels.iter().any(|x| x == l)))
                    .filter(|(_, n)| n.props.get(key) == Some(value))
                    .map(|(id, _)| *id)
                    .collect();
                assert_eq!(
                    sorted(g.find_node_ids_by_property(label, key, value)),
                    expected,
                    "{context}: find nodes {label:?} {key} = {value:?}"
                );
            }
        }
        for value in &probes {
            for rel_type in [None, Some("R"), Some("NoSuchType")] {
                let expected: Vec<u64> = model
                    .rels
                    .iter()
                    .filter(|(_, r)| rel_type.is_none_or(|t| r.rel_type == t))
                    .filter(|(_, r)| r.props.get(key) == Some(value))
                    .map(|(id, _)| *id)
                    .collect();
                assert_eq!(
                    sorted(g.find_relationship_ids_by_property(rel_type, key, value)),
                    expected,
                    "{context}: find rels {rel_type:?} {key} = {value:?}"
                );
            }
        }
    }
    g.assert_property_indexes_match_scan();
}

fn run(seed: u64, steps: usize) {
    let mut rng = Rng(seed | 1);
    let mut g = InMemoryGraph::new();
    let mut model = Model::default();
    // Clones taken along the way, each with the model of that moment.
    let mut snapshots: Vec<(InMemoryGraph, Model, usize)> = Vec::new();

    for step in 0..steps {
        match rng.below(100) {
            0..=17 => {
                let labels = rng.labels();
                let props = rng.props();
                let record = g.create_node(labels.clone(), to_properties(&props));
                assert!(
                    model
                        .nodes
                        .insert(record.id, ModelNode { labels, props })
                        .is_none(),
                    "node id reused"
                );
            }
            18..=33 => {
                let (Some(src), Some(dst)) =
                    (rng.pick_key(&model.nodes), rng.pick_key(&model.nodes))
                else {
                    continue;
                };
                // Self-loops now and then.
                let dst = if rng.below(8) == 0 { src } else { dst };
                let rel_type = rng.pick(&TYPES).to_string();
                let props = rng.props();
                let record = g
                    .create_relationship(src, dst, &rel_type, to_properties(&props))
                    .expect("endpoints exist");
                model.rels.insert(
                    record.id,
                    ModelRel {
                        src,
                        dst,
                        rel_type,
                        props,
                    },
                );
            }
            34..=49 => {
                let Some(id) = rng.pick_key(&model.nodes) else {
                    continue;
                };
                let (key, value) = (rng.pick(&KEYS).to_string(), rng.value());
                assert!(g.set_node_property(id, key.clone(), value.clone()));
                model.nodes.get_mut(&id).unwrap().props.insert(key, value);
            }
            50..=57 => {
                let Some(id) = rng.pick_key(&model.nodes) else {
                    continue;
                };
                let key = *rng.pick(&KEYS);
                let had = model
                    .nodes
                    .get_mut(&id)
                    .unwrap()
                    .props
                    .remove(key)
                    .is_some();
                assert_eq!(g.remove_node_property(id, key), had);
            }
            58..=65 => {
                let Some(id) = rng.pick_key(&model.nodes) else {
                    continue;
                };
                let label = *rng.pick(&LABELS);
                let node = model.nodes.get_mut(&id).unwrap();
                let has = node.labels.iter().any(|l| l == label);
                assert_eq!(g.add_node_label(id, label), !has);
                if !has {
                    node.labels.push(label.to_string());
                }
            }
            66..=73 => {
                let Some(id) = rng.pick_key(&model.nodes) else {
                    continue;
                };
                let label = *rng.pick(&LABELS);
                let node = model.nodes.get_mut(&id).unwrap();
                let has = node.labels.iter().any(|l| l == label);
                assert_eq!(g.remove_node_label(id, label), has);
                node.labels.retain(|l| l != label);
            }
            74..=80 => {
                let Some(id) = rng.pick_key(&model.rels) else {
                    continue;
                };
                let (key, value) = (rng.pick(&KEYS).to_string(), rng.value());
                assert!(g.set_relationship_property(id, key.clone(), value.clone()));
                model.rels.get_mut(&id).unwrap().props.insert(key, value);
            }
            81..=83 => {
                let Some(id) = rng.pick_key(&model.rels) else {
                    continue;
                };
                let key = *rng.pick(&KEYS);
                let had = model.rels.get_mut(&id).unwrap().props.remove(key).is_some();
                assert_eq!(g.remove_relationship_property(id, key), had);
            }
            84..=88 => {
                let Some(id) = rng.pick_key(&model.rels) else {
                    continue;
                };
                assert!(g.delete_relationship(id));
                assert!(!g.delete_relationship(id));
                model.rels.remove(&id);
            }
            89..=92 => {
                // A plain delete refuses a node that still has relationships.
                let Some(id) = rng.pick_key(&model.nodes) else {
                    continue;
                };
                let free = model.incident(id).is_empty();
                assert_eq!(g.delete_node(id), free, "delete node {id}");
                if free {
                    model.nodes.remove(&id);
                }
            }
            93..=94 => {
                let Some(id) = rng.pick_key(&model.nodes) else {
                    continue;
                };
                assert!(g.detach_delete_node(id));
                for rel in model.incident(id) {
                    model.rels.remove(&rel);
                }
                model.nodes.remove(&id);
            }
            95..=96 => {
                // What a staged write does: carry on with a clone while a
                // reader keeps the graph it was cloned from.
                let old = std::mem::replace(&mut g, InMemoryGraph::new());
                g = old.clone();
                if snapshots.len() < 6 {
                    snapshots.push((old, model.clone(), step));
                }
            }
            97 => {
                // A declared RANGE index, once: its sorted half has to
                // follow every write from here on.
                let _ = g.create_index(
                    IndexRequest {
                        explicit_name: Some("a_k".into()),
                        kind: StoredIndexKind::Range,
                        entity: StoredIndexEntity::Node,
                        label: Some("A".into()),
                        additional_labels: Vec::new(),
                        properties: vec!["k".into()],
                        options: BTreeMap::new(),
                    },
                    true,
                );
            }
            _ => {
                // A snapshot round trip gives the same graph.
                let mut loaded = InMemoryGraph::new();
                loaded
                    .load_snapshot_payload(g.snapshot_payload())
                    .expect("snapshot loads");
                check(
                    &loaded,
                    &model,
                    &format!("seed {seed} step {step}: reloaded"),
                );
            }
        }
        if step % 25 == 24 {
            check(&g, &model, &format!("seed {seed} step {step}"));
        }
    }

    check(&g, &model, &format!("seed {seed}: end"));
    // Graphs the run was cloned away from still read as they did then,
    // whatever names and records their successors added since.
    for (old, old_model, step) in &snapshots {
        check(
            old,
            old_model,
            &format!("seed {seed}: clone source of step {step}"),
        );
    }
}

#[test]
fn random_operations_match_the_model() {
    for seed in 1..=12 {
        run(seed * 0x9E37_79B9, 400);
    }
}

/// Longer runs over more seeds. `cargo test -p lora-store --release
/// model -- --ignored`
#[test]
#[ignore]
fn random_operations_match_the_model_at_length() {
    for seed in 100..=400 {
        run(seed * 0x9E37_79B9, 2_500);
    }
}
