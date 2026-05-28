use lora_database::{Database, InMemoryGraph, WalConfig};
use tempfile::TempDir;

use crate::engine::{lora_opts, DbHandle};
use crate::fixtures::{Fixture, FixtureKind};

pub fn fresh() -> DbHandle {
    DbHandle::Lora(Database::in_memory(), None)
}

pub fn fresh_persistent() -> DbHandle {
    let dir = TempDir::new().expect("lora_wal: tempdir create failed");
    let cfg = WalConfig::enabled(dir.path());
    let db = Database::open_with_wal(cfg).expect("lora_wal: open_with_wal failed");
    DbHandle::Lora(db, Some(dir))
}

pub fn seed(handle: &mut DbHandle, fixture: Fixture) {
    let DbHandle::Lora(db, _) = handle else {
        panic!("lora seed: wrong handle variant");
    };
    match fixture.kind {
        FixtureKind::Empty => {}
        FixtureKind::Nodes => seed_nodes(db, fixture.size),
        FixtureKind::Chain => seed_chain(db, fixture.size),
        FixtureKind::Star => seed_star(db, fixture.size),
        FixtureKind::Social => seed_social(db, fixture.size),
    }
}

/// Lora auto-indexes property-equality lookups, so explicit index
/// requests are a no-op.
pub fn create_index(_handle: &DbHandle, _prop: &str) {}

pub fn execute(handle: &DbHandle, q: &str) {
    let DbHandle::Lora(db, _) = handle else {
        panic!("lora execute: wrong handle variant");
    };
    db.execute(q, lora_opts())
        .unwrap_or_else(|e| panic!("lora query failed: {q}\nerror: {e}"));
}

const CHUNK: usize = 2_000;

fn seed_nodes(db: &Database<InMemoryGraph>, n: usize) {
    let mut i = 0;
    while i < n {
        let end = (i + CHUNK).min(n);
        let q = format!(
            "UNWIND range({i}, {}) AS i \
             CREATE (:Node {{id: i, name: 'node_' + toString(i), value: i % 100}})",
            end - 1
        );
        db.execute(&q, lora_opts()).expect("lora seed_nodes failed");
        i = end;
    }
}

fn seed_chain(db: &Database<InMemoryGraph>, len: usize) {
    let mut i = 0;
    while i < len {
        let end = (i + CHUNK).min(len);
        db.execute(
            &format!(
                "UNWIND range({i}, {}) AS i CREATE (:Chain {{idx: i}})",
                end - 1
            ),
            lora_opts(),
        )
        .expect("lora seed_chain (nodes) failed");
        i = end;
    }
    if len > 1 {
        let mut i = 0;
        while i < len - 1 {
            let end = (i + CHUNK).min(len - 1);
            db.execute(
                &format!(
                    "UNWIND range({i}, {}) AS i \
                     MATCH (a:Chain {{idx: i}}), (b:Chain {{idx: i + 1}}) \
                     CREATE (a)-[:NEXT {{step: i}}]->(b)",
                    end - 1
                ),
                lora_opts(),
            )
            .expect("lora seed_chain (edges) failed");
            i = end;
        }
    }
}

fn seed_star(db: &Database<InMemoryGraph>, spokes: usize) {
    db.execute("CREATE (:Hub {name: 'center'})", lora_opts())
        .expect("lora seed_star (hub) failed");
    let mut i = 0;
    while i < spokes {
        let end = (i + CHUNK).min(spokes);
        db.execute(
            &format!(
                "UNWIND range({i}, {}) AS i \
                 MATCH (h:Hub) CREATE (h)-[:ARM]->(:Leaf {{id: i}})",
                end - 1
            ),
            lora_opts(),
        )
        .expect("lora seed_star (leaves) failed");
        i = end;
    }
}

fn seed_social(db: &Database<InMemoryGraph>, n: usize) {
    let mut i = 0;
    while i < n {
        let end = (i + CHUNK).min(n);
        db.execute(
            &format!(
                "UNWIND range({i}, {}) AS i \
                 CREATE (:Person {{idx: i, name: 'person_' + toString(i)}})",
                end - 1
            ),
            lora_opts(),
        )
        .expect("lora seed_social (people) failed");
        i = end;
    }
    for offset in 1..=2usize {
        let mut i = 0;
        while i < n {
            let end = (i + CHUNK).min(n);
            db.execute(
                &format!(
                    "UNWIND range({i}, {}) AS i \
                     MATCH (a:Person {{idx: i}}), (b:Person {{idx: (i + {offset}) % {n}}}) \
                     CREATE (a)-[:KNOWS {{strength: (i + {offset}) % 5}}]->(b)",
                    end - 1
                ),
                lora_opts(),
            )
            .expect("lora seed_social (edges) failed");
            i = end;
        }
    }
}
