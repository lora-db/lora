use grafeo::{GrafeoDB, Value as GValue};
use tempfile::TempDir;

use crate::engine::DbHandle;
use crate::fixtures::{Fixture, FixtureKind};

pub fn fresh() -> DbHandle {
    DbHandle::Grafeo(GrafeoDB::new_in_memory(), None)
}

pub fn fresh_persistent() -> DbHandle {
    let dir = TempDir::new().expect("grafeo_file: tempdir create failed");
    let db = GrafeoDB::open(dir.path()).expect("grafeo_file: open failed");
    DbHandle::Grafeo(db, Some(dir))
}

pub fn seed(handle: &mut DbHandle, fixture: Fixture) {
    let DbHandle::Grafeo(db, _) = handle else {
        panic!("grafeo seed: wrong handle variant");
    };
    match fixture.kind {
        FixtureKind::Empty => {}
        FixtureKind::Nodes => seed_nodes(db, fixture.size),
        FixtureKind::Chain => seed_chain(db, fixture.size),
        FixtureKind::Star => seed_star(db, fixture.size),
        FixtureKind::Social => seed_social(db, fixture.size),
    }
}

pub fn create_index(handle: &DbHandle, prop: &str) {
    let DbHandle::Grafeo(db, _) = handle else {
        panic!("grafeo create_index: wrong handle variant");
    };
    db.create_property_index(prop);
}

pub fn execute(handle: &DbHandle, q: &str) {
    let DbHandle::Grafeo(db, _) = handle else {
        panic!("grafeo execute: wrong handle variant");
    };
    db.execute(q)
        .unwrap_or_else(|e| panic!("grafeo query failed: {q}\nerror: {e}"));
}

fn seed_nodes(db: &GrafeoDB, n: usize) {
    for i in 0..n {
        let props: [(&str, GValue); 3] = [
            ("id", GValue::from(i as i64)),
            ("name", GValue::from(format!("node_{i}"))),
            ("value", GValue::from((i % 100) as i64)),
        ];
        db.create_node_with_props(&["Node"], props);
    }
}

fn seed_chain(db: &GrafeoDB, len: usize) {
    let mut ids = Vec::with_capacity(len);
    for i in 0..len {
        let id = db.create_node_with_props(&["Chain"], [("idx", GValue::from(i as i64))]);
        ids.push(id);
    }
    for w in ids.windows(2) {
        db.create_edge(w[0], w[1], "NEXT");
    }
}

fn seed_star(db: &GrafeoDB, spokes: usize) {
    let hub = db.create_node_with_props(&["Hub"], [("name", GValue::from("center"))]);
    for i in 0..spokes {
        let leaf = db.create_node_with_props(&["Leaf"], [("id", GValue::from(i as i64))]);
        db.create_edge(hub, leaf, "ARM");
    }
}

fn seed_social(db: &GrafeoDB, n: usize) {
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let id = db.create_node_with_props(
            &["Person"],
            [
                ("idx", GValue::from(i as i64)),
                ("name", GValue::from(format!("person_{i}"))),
            ],
        );
        ids.push(id);
    }
    for i in 0..n {
        for offset in 1..=2usize {
            let other = (i + offset) % n;
            // grafeo's `create_edge` doesn't take edge props directly via
            // the high-level facade; the existing bench file accepts that
            // the social workloads on grafeo carry no `strength`. The
            // queries that *filter on* strength are surrealdb-only.
            db.create_edge(ids[i], ids[other], "KNOWS");
        }
    }
}
