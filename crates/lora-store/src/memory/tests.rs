use super::*;
use crate::{BorrowedGraphStorage, GraphStorage, GraphStorageMut};

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
    assert_eq!(BorrowedGraphStorage::node_refs(&g).count(), 2);
    assert_eq!(
        BorrowedGraphStorage::node_refs_by_label(&g, "Person").count(),
        2
    );
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
    assert_eq!(BorrowedGraphStorage::relationship_refs(&g).count(), 2);
    assert_eq!(
        BorrowedGraphStorage::relationship_refs_by_type(&g, "KNOWS").count(),
        1
    );
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
fn node_property_index_tracks_create_set_remove_and_delete() {
    let mut g = InMemoryGraph::new();
    let alice = g.create_node(
        vec!["Person".into()],
        props(&[("name", PropertyValue::String("Alice".into()))]),
    );
    let other_alice = g.create_node(
        vec!["Robot".into()],
        props(&[("name", PropertyValue::String("Alice".into()))]),
    );
    let bob = g.create_node(
        vec!["Person".into()],
        props(&[("name", PropertyValue::String("Bob".into()))]),
    );

    let alice_value = PropertyValue::String("Alice".into());
    assert_eq!(
        g.find_nodes_by_property(Some("Person"), "name", &alice_value)
            .into_iter()
            .map(|n| n.id)
            .collect::<Vec<_>>(),
        vec![alice.id]
    );
    assert!(g.node_exists_with_label_and_property("Robot", "name", &alice_value));

    assert!(g.set_node_property(
        other_alice.id,
        "name".into(),
        PropertyValue::String("Alicia".into())
    ));
    assert_eq!(
        g.find_nodes_by_property(None, "name", &alice_value)
            .into_iter()
            .map(|n| n.id)
            .collect::<Vec<_>>(),
        vec![alice.id]
    );

    assert!(g.remove_node_property(alice.id, "name"));
    assert!(!g.node_exists_with_label_and_property("Person", "name", &alice_value));

    assert!(g.delete_node(bob.id));
    assert!(!g.node_exists_with_label_and_property(
        "Person",
        "name",
        &PropertyValue::String("Bob".into())
    ));
}

#[test]
fn node_property_index_activates_on_lookup_and_tracks_later_create() {
    let mut g = InMemoryGraph::new();
    let first = g.create_node(
        vec!["Person".into()],
        props(&[("name", PropertyValue::String("Alice".into()))]),
    );

    assert!(!g.indexes_read().node_properties.is_active("name"));

    let alice = PropertyValue::String("Alice".into());
    assert_eq!(
        g.find_nodes_by_property(Some("Person"), "name", &alice)
            .into_iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        vec![first.id]
    );
    assert!(g.indexes_read().node_properties.is_active("name"));

    let second = g.create_node(
        vec!["Person".into()],
        props(&[("name", PropertyValue::String("Alice".into()))]),
    );
    assert_eq!(
        g.find_nodes_by_property(Some("Person"), "name", &alice)
            .into_iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
}

#[test]
fn property_indexes_activate_on_lookup_after_set_for_new_keys() {
    let mut g = InMemoryGraph::new();
    let node = g.create_node(vec!["Person".into()], Properties::new());

    assert!(!g.indexes_read().node_properties.is_active("name"));
    assert!(g.set_node_property(
        node.id,
        "name".into(),
        PropertyValue::String("Alice".into())
    ));
    assert!(!g.indexes_read().node_properties.is_active("name"));
    assert_eq!(
        g.find_node_ids_by_property(
            Some("Person"),
            "name",
            &PropertyValue::String("Alice".into())
        ),
        vec![node.id]
    );
    assert!(g.indexes_read().node_properties.is_active("name"));

    let other = g.create_node(vec!["Person".into()], Properties::new());
    let rel = g
        .create_relationship(node.id, other.id, "KNOWS", Properties::new())
        .unwrap();
    assert!(!g.indexes_read().relationship_properties.is_active("since"));
    assert!(g.set_relationship_property(rel.id, "since".into(), PropertyValue::Int(2020)));
    assert!(!g.indexes_read().relationship_properties.is_active("since"));
    assert_eq!(
        g.find_relationship_ids_by_property(Some("KNOWS"), "since", &PropertyValue::Int(2020)),
        vec![rel.id]
    );
    assert!(g.indexes_read().relationship_properties.is_active("since"));
}

#[test]
fn replay_create_eagerly_activates_property_indexes() {
    let mut g = InMemoryGraph::new();
    let alice = g
        .replay_create_node(
            0,
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        )
        .unwrap();
    let bob = g
        .replay_create_node(
            1,
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Bob".into()))]),
        )
        .unwrap();

    assert!(g.indexes_read().node_properties.is_active("name"));
    assert_eq!(
        g.find_node_ids_by_property(
            Some("Person"),
            "name",
            &PropertyValue::String("Alice".into())
        ),
        vec![alice.id]
    );

    let rel = g
        .replay_create_relationship(
            0,
            alice.id,
            bob.id,
            "KNOWS",
            props(&[("since", PropertyValue::Int(2020))]),
        )
        .unwrap();

    assert!(g.indexes_read().relationship_properties.is_active("since"));
    assert_eq!(
        g.find_relationship_ids_by_property(Some("KNOWS"), "since", &PropertyValue::Int(2020)),
        vec![rel.id]
    );
}

#[test]
fn node_property_index_tracks_scoped_label_buckets() {
    let mut g = InMemoryGraph::new();
    let alice = g.create_node(
        vec!["Person".into()],
        props(&[("name", PropertyValue::String("Alice".into()))]),
    );
    let robot = g.create_node(
        vec!["Robot".into()],
        props(&[("name", PropertyValue::String("Alice".into()))]),
    );
    let alice_value = PropertyValue::String("Alice".into());

    assert_eq!(
        g.find_node_ids_by_property(Some("Person"), "name", &alice_value),
        vec![alice.id]
    );
    assert_eq!(
        g.indexes_read()
            .node_properties
            .scoped_ids_for("Person", "name", &alice_value)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![alice.id]
    );

    assert!(g.add_node_label(robot.id, "Employee"));
    assert_eq!(
        g.find_node_ids_by_property(Some("Employee"), "name", &alice_value),
        vec![robot.id]
    );

    assert!(g.remove_node_label(alice.id, "Person"));
    assert!(g
        .find_node_ids_by_property(Some("Person"), "name", &alice_value)
        .is_empty());
    assert_eq!(
        g.find_node_ids_by_property(None, "name", &alice_value),
        vec![alice.id, robot.id]
    );
}

#[test]
fn relationship_property_index_tracks_create_set_remove_and_delete() {
    let mut g = InMemoryGraph::new();
    let a = g.create_node(vec!["Person".into()], Properties::new());
    let b = g.create_node(vec!["Person".into()], Properties::new());
    let c = g.create_node(vec!["Person".into()], Properties::new());
    let first = g
        .create_relationship(
            a.id,
            b.id,
            "KNOWS",
            props(&[("since", PropertyValue::Int(2020))]),
        )
        .unwrap();
    let second = g
        .create_relationship(
            b.id,
            c.id,
            "LIKES",
            props(&[("since", PropertyValue::Int(2020))]),
        )
        .unwrap();

    let year = PropertyValue::Int(2020);
    assert_eq!(
        g.find_relationships_by_property(Some("KNOWS"), "since", &year)
            .into_iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![first.id]
    );
    assert!(g.relationship_exists_with_type_and_property("LIKES", "since", &year));

    assert!(g.set_relationship_property(second.id, "since".into(), PropertyValue::Int(2021)));
    assert_eq!(
        g.find_relationships_by_property(None, "since", &year)
            .into_iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![first.id]
    );

    assert!(g.remove_relationship_property(first.id, "since"));
    assert!(!g.relationship_exists_with_type_and_property("KNOWS", "since", &year));

    assert!(g.delete_relationship(second.id));
    assert!(!g.relationship_exists_with_type_and_property(
        "LIKES",
        "since",
        &PropertyValue::Int(2021)
    ));
}

#[test]
fn property_index_falls_back_for_unhashed_values() {
    let mut g = InMemoryGraph::new();
    let date = PropertyValue::Date(crate::types::temporal::LoraDate::new(2026, 4, 26).unwrap());
    let n = g.create_node(vec!["Event".into()], props(&[("day", date.clone())]));

    // Dates are intentionally not hash-indexed yet, so this exercises the
    // scan fallback path rather than the secondary index.
    assert_eq!(
        g.find_nodes_by_property(Some("Event"), "day", &date)
            .into_iter()
            .map(|node| node.id)
            .collect::<Vec<_>>(),
        vec![n.id]
    );
}

#[test]
fn property_index_invariants_survive_mixed_mutations() {
    let mut g = InMemoryGraph::new();
    let alice = g.create_node(
        vec!["Person".into()],
        props(&[
            ("name", PropertyValue::String("Alice".into())),
            ("status", PropertyValue::String("active".into())),
        ]),
    );
    let bob = g.create_node(
        vec!["Person".into()],
        props(&[
            ("name", PropertyValue::String("Bob".into())),
            ("status", PropertyValue::String("active".into())),
        ]),
    );
    let acme = g.create_node(
        vec!["Company".into()],
        props(&[
            ("name", PropertyValue::String("Acme".into())),
            ("status", PropertyValue::String("active".into())),
        ]),
    );
    let eve = g.create_node(
        vec!["Person".into()],
        props(&[
            ("name", PropertyValue::String("Eve".into())),
            ("status", PropertyValue::String("inactive".into())),
        ]),
    );
    let knows = g
        .create_relationship(
            alice.id,
            bob.id,
            "KNOWS",
            props(&[
                ("since", PropertyValue::Int(2020)),
                ("strength", PropertyValue::Int(5)),
            ]),
        )
        .unwrap();
    let works_at = g
        .create_relationship(
            bob.id,
            acme.id,
            "WORKS_AT",
            props(&[
                ("since", PropertyValue::Int(2021)),
                ("strength", PropertyValue::Int(8)),
            ]),
        )
        .unwrap();

    assert_eq!(
        g.find_node_ids_by_property(
            Some("Person"),
            "name",
            &PropertyValue::String("Alice".into())
        ),
        vec![alice.id]
    );
    assert_eq!(
        g.find_node_ids_by_property(None, "status", &PropertyValue::String("active".into())),
        vec![alice.id, bob.id, acme.id]
    );
    assert_eq!(
        g.find_relationship_ids_by_property(Some("KNOWS"), "since", &PropertyValue::Int(2020)),
        vec![knows.id]
    );
    assert_eq!(
        g.find_relationship_ids_by_property(None, "strength", &PropertyValue::Int(8)),
        vec![works_at.id]
    );
    g.assert_property_indexes_match_scan();

    assert!(g.set_node_property(
        alice.id,
        "name".into(),
        PropertyValue::String("Alicia".into())
    ));
    g.assert_property_indexes_match_scan();

    assert!(g.remove_node_property(bob.id, "status"));
    g.assert_property_indexes_match_scan();

    assert!(g.add_node_label(acme.id, "Employer"));
    assert_eq!(
        g.find_node_ids_by_property(
            Some("Employer"),
            "status",
            &PropertyValue::String("active".into())
        ),
        vec![acme.id]
    );
    g.assert_property_indexes_match_scan();

    assert!(g.remove_node_label(alice.id, "Person"));
    assert!(g
        .find_node_ids_by_property(
            Some("Person"),
            "name",
            &PropertyValue::String("Alicia".into())
        )
        .is_empty());
    g.assert_property_indexes_match_scan();

    assert!(g.set_relationship_property(knows.id, "since".into(), PropertyValue::Int(2022)));
    assert!(g.remove_relationship_property(works_at.id, "since"));
    g.assert_property_indexes_match_scan();

    assert!(g.delete_relationship(works_at.id));
    assert!(g.delete_node(eve.id));
    g.assert_property_indexes_match_scan();

    assert!(g.detach_delete_node(bob.id));
    g.assert_property_indexes_match_scan();

    g.clear();
    g.assert_property_indexes_match_scan();
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

#[test]
fn clear_resets_the_graph() {
    let mut g = InMemoryGraph::new();
    let a = g.create_node(vec!["Person".into()], Properties::new());
    let b = g.create_node(vec!["Person".into()], Properties::new());
    g.create_relationship(a.id, b.id, "KNOWS", Properties::new())
        .unwrap();

    assert_eq!(g.node_count(), 2);
    assert_eq!(g.relationship_count(), 1);

    g.clear();

    assert_eq!(g.node_count(), 0);
    assert_eq!(g.relationship_count(), 0);
    assert_eq!(g.all_labels().len(), 0);
}

#[test]
fn snapshot_roundtrip_preserves_graph_state() {
    let mut original = InMemoryGraph::new();
    let a = original.create_node(
        vec!["Person".into()],
        props(&[("name", PropertyValue::String("Alice".into()))]),
    );
    let b = original.create_node(
        vec!["Person".into()],
        props(&[("name", PropertyValue::String("Bob".into()))]),
    );
    let r = original
        .create_relationship(
            a.id,
            b.id,
            "KNOWS",
            props(&[("since", PropertyValue::Int(2020))]),
        )
        .unwrap();

    let payload = original.snapshot_payload();
    assert_eq!(payload.nodes.len(), 2);
    assert_eq!(payload.relationships.len(), 1);

    let mut restored = InMemoryGraph::new();
    let load_meta = restored.load_snapshot_payload(payload).unwrap();
    assert_eq!(load_meta.node_count, 2);
    assert_eq!(load_meta.relationship_count, 1);
    assert_eq!(load_meta.wal_lsn, None);
    assert!(restored.indexes_read().node_properties.is_active("name"));
    assert!(restored
        .indexes_read()
        .relationship_properties
        .is_active("since"));

    assert_eq!(restored.node_count(), 2);
    assert_eq!(restored.relationship_count(), 1);
    assert_eq!(
        restored.node_property(a.id, "name"),
        Some(PropertyValue::String("Alice".into()))
    );
    assert_eq!(
        restored.relationship_property(r.id, "since"),
        Some(PropertyValue::Int(2020))
    );

    // Adjacency + label index were rebuilt on load.
    assert_eq!(restored.outgoing_relationships(a.id).len(), 1);
    assert_eq!(restored.nodes_by_label("Person").len(), 2);
    assert!(restored.node_exists_with_label_and_property(
        "Person",
        "name",
        &PropertyValue::String("Alice".into())
    ));
    assert!(restored.relationship_exists_with_type_and_property(
        "KNOWS",
        "since",
        &PropertyValue::Int(2020)
    ));

    // Counters carry over so new IDs don't collide with pre-snapshot IDs.
    let c = restored.create_node(vec!["Person".into()], Properties::new());
    assert_eq!(c.id, b.id + 1);
}

#[test]
fn mutation_recorder_observes_every_committed_mutation() {
    use std::sync::Mutex;

    #[derive(Default)]
    struct CapturingRecorder {
        events: Mutex<Vec<MutationEvent>>,
    }

    impl MutationRecorder for CapturingRecorder {
        fn record(&self, event: MutationEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    let recorder = Arc::new(CapturingRecorder::default());
    let mut g = InMemoryGraph::new();
    g.set_mutation_recorder(Some(recorder.clone() as Arc<dyn MutationRecorder>));

    let a = g.create_node(vec!["Person".into()], Properties::new());
    let b = g.create_node(vec!["Person".into()], Properties::new());
    let r = g
        .create_relationship(a.id, b.id, "KNOWS", Properties::new())
        .unwrap();
    g.set_node_property(a.id, "name".into(), PropertyValue::String("Alice".into()));
    g.remove_node_property(a.id, "name");
    g.add_node_label(a.id, "Admin");
    g.remove_node_label(a.id, "Admin");
    g.set_relationship_property(r.id, "since".into(), PropertyValue::Int(2020));
    g.remove_relationship_property(r.id, "since");
    g.detach_delete_node(a.id);
    g.clear();

    let events = recorder.events.lock().unwrap().clone();
    assert!(matches!(events[0], MutationEvent::CreateNode { .. }));
    assert!(matches!(events[1], MutationEvent::CreateNode { .. }));
    assert!(matches!(
        events[2],
        MutationEvent::CreateRelationship { .. }
    ));
    assert!(matches!(events[3], MutationEvent::SetNodeProperty { .. }));
    assert!(matches!(
        events[4],
        MutationEvent::RemoveNodeProperty { .. }
    ));
    assert!(matches!(events[5], MutationEvent::AddNodeLabel { .. }));
    assert!(matches!(events[6], MutationEvent::RemoveNodeLabel { .. }));
    assert!(matches!(
        events[7],
        MutationEvent::SetRelationshipProperty { .. }
    ));
    assert!(matches!(
        events[8],
        MutationEvent::RemoveRelationshipProperty { .. }
    ));
    // detach_delete_node composes three kinds of events: one
    // DeleteRelationship per incident edge, one DeleteNode for the node
    // itself, and a final DetachDeleteNode marker. A WAL replayer can
    // either apply every step or recognise the marker and skip forward.
    assert!(matches!(
        events[9],
        MutationEvent::DeleteRelationship { .. }
    ));
    assert!(matches!(events[10], MutationEvent::DeleteNode { .. }));
    assert!(matches!(events[11], MutationEvent::DetachDeleteNode { .. }));
    assert!(matches!(events.last(), Some(MutationEvent::Clear)));

    // Failed mutations (invalid id) do not emit events.
    let before = recorder.events.lock().unwrap().len();
    assert!(!g.set_node_property(9999, "x".into(), PropertyValue::Int(0)));
    assert_eq!(recorder.events.lock().unwrap().len(), before);
}

#[test]
fn snapshot_load_resets_but_keeps_recorder() {
    use std::sync::Mutex;

    struct CountingRecorder(Mutex<usize>);
    impl MutationRecorder for CountingRecorder {
        fn record(&self, _: MutationEvent) {
            *self.0.lock().unwrap() += 1;
        }
    }

    let counter: Arc<dyn MutationRecorder> = Arc::new(CountingRecorder(Mutex::new(0)));
    let mut g = InMemoryGraph::new();
    g.set_mutation_recorder(Some(counter));
    g.create_node(vec!["A".into()], Properties::new());

    let payload = g.snapshot_payload();

    // Load into the same graph — recorder should survive, store state
    // should be replaced by the payload contents.
    g.load_snapshot_payload(payload).unwrap();
    assert!(g.mutation_recorder().is_some());
    assert_eq!(g.node_count(), 1);

    // Subsequent mutations still feed the recorder.
    g.create_node(vec!["B".into()], Properties::new());
    // 1 for the initial A + 1 for the post-load B. The restore path
    // itself does not emit events (that's a snapshot, not a mutation).
}
