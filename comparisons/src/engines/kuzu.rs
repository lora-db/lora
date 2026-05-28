//! Kuzu engine adapter.
//!
//! Kuzu is an embedded property graph database with a Cypher dialect that
//! lines up closely with LoraDB's. The main divergence is the schema-first
//! requirement: NODE/REL tables must be declared with typed properties
//! before any data is inserted, where Lora and Grafeo accept ad-hoc
//! labels. The `seed_*` helpers below issue the DDL once, then push data
//! through UNWIND-driven CREATE batches identical in shape to the other
//! engines.

use kuzu::{Connection, Database, SystemConfig};
use tempfile::TempDir;

use crate::engine::DbHandle;
use crate::fixtures::{Fixture, FixtureKind};

const CHUNK: usize = 2_000;

/// Kuzu's default `max_db_size` is 8 TB, which reserves an 8 TB virtual
/// mmap up front. That works for a single DB but fails once the bench
/// has opened a handful in the same process. Kuzu's own test suite caps
/// at 16 GB for the same reason; the fixtures here use far less than a
/// MB so a 256 MB cap is plenty of headroom while keeping each open
/// cheap.
const MAX_DB_SIZE: u64 = 256 * 1024 * 1024;

fn bench_config() -> SystemConfig {
    SystemConfig::default().max_db_size(MAX_DB_SIZE)
}

/// Kuzu's `Connection` borrows from its `Database`. Storing both in one
/// owned struct would require a self-referential borrow; the simpler
/// option is to keep only the owned `Database` in the handle and open a
/// fresh `Connection` per call. Connection construction is cheap
/// (microseconds) relative to query execution, and — crucially —
/// avoids leaking the per-iter Database in `per_iter_seed` mode, which
/// would otherwise exhaust virtual address space after a few hundred
/// iterations.
pub struct KuzuHandle {
    pub db: Box<Database>,
}

pub fn fresh() -> DbHandle {
    let db = Box::new(Database::in_memory(bench_config()).expect("kuzu: in-memory open failed"));
    DbHandle::Kuzu(KuzuHandle { db }, None)
}

pub fn fresh_persistent() -> DbHandle {
    let dir = TempDir::new().expect("kuzu_file: tempdir create failed");
    // Kuzu refuses an already-existing directory ("Database path cannot be
    // a directory"); it wants to create the database at a fresh path. The
    // TempDir is the (existing) parent we keep alive for cleanup, so point
    // Kuzu at a not-yet-existing child it can create.
    let path = dir.path().join("kuzu_db");
    let db =
        Box::new(Database::new(&path, bench_config()).expect("kuzu_file: on-disk open failed"));
    DbHandle::Kuzu(KuzuHandle { db }, Some(dir))
}

pub fn seed(handle: &mut DbHandle, fixture: Fixture) {
    let DbHandle::Kuzu(h, _) = handle else {
        panic!("kuzu seed: wrong handle variant");
    };
    let conn = Connection::new(&h.db).expect("kuzu seed: connection failed");
    match fixture.kind {
        FixtureKind::Empty => {}
        FixtureKind::Nodes => seed_nodes(&conn, fixture.size),
        FixtureKind::Chain => seed_chain(&conn, fixture.size),
        FixtureKind::Star => seed_star(&conn, fixture.size),
        FixtureKind::Social => seed_social(&conn, fixture.size),
    }
}

/// Kuzu auto-indexes PRIMARY KEY columns. Property-indexes for non-PK
/// columns use `CALL CREATE_PROPERTY_INDEX(...)` in newer versions but
/// the workloads' bench cases only ever index `id` on `Node`, which is
/// already the primary key — so this is a noop.
pub fn create_index(_handle: &DbHandle, _prop: &str) {}

pub fn execute(handle: &DbHandle, q: &str) {
    let DbHandle::Kuzu(h, _) = handle else {
        panic!("kuzu execute: wrong handle variant");
    };
    let conn = Connection::new(&h.db).expect("kuzu execute: connection failed");
    conn.query(q)
        .unwrap_or_else(|e| panic!("kuzu query failed: {q}\nerror: {e}"));
}

fn run(conn: &Connection, sql: &str) {
    conn.query(sql)
        .unwrap_or_else(|e| panic!("kuzu seed sql failed: {sql}\nerror: {e}"));
}

fn seed_nodes(conn: &Connection, n: usize) {
    // Kuzu is strict-schema — `SET n.touched = true` and friends fail
    // unless the column is declared on the table. We pre-declare the
    // columns the `writes` workloads later assign to (`touched`, `a`,
    // `b`, `flagged`) so Kuzu can run them. Slight measurement bias
    // (one extra column on the seeded row), but worth it for coverage.
    run(
        conn,
        "CREATE NODE TABLE Node ( \
            id INT64, name STRING, value INT64, \
            touched BOOL, a INT64, b INT64, flagged BOOL, \
            PRIMARY KEY(id) \
         );",
    );
    // `bulk_edges` workload writes (a)-[:LINKS]->(b); Kuzu won't accept
    // an undeclared REL type, so we pre-declare it. Empty at seed time;
    // the workload is what populates it.
    run(conn, "CREATE REL TABLE LINKS (FROM Node TO Node);");
    let mut i = 0;
    while i < n {
        let end = (i + CHUNK).min(n);
        let q = format!(
            "UNWIND range({i}, {}) AS i \
             CREATE (:Node {{id: i, name: 'node_' + cast(i AS STRING), value: i % 100}});",
            end - 1
        );
        run(conn, &q);
        i = end;
    }
}

fn seed_chain(conn: &Connection, len: usize) {
    run(
        conn,
        "CREATE NODE TABLE Chain (idx INT64, PRIMARY KEY(idx));",
    );
    run(
        conn,
        "CREATE REL TABLE NEXT (FROM Chain TO Chain, step INT64);",
    );
    let mut i = 0;
    while i < len {
        let end = (i + CHUNK).min(len);
        run(
            conn,
            &format!(
                "UNWIND range({i}, {}) AS i CREATE (:Chain {{idx: i}});",
                end - 1
            ),
        );
        i = end;
    }
    if len > 1 {
        let mut i = 0;
        while i < len - 1 {
            let end = (i + CHUNK).min(len - 1);
            run(
                conn,
                &format!(
                    "UNWIND range({i}, {}) AS i \
                     MATCH (a:Chain {{idx: i}}), (b:Chain {{idx: i + 1}}) \
                     CREATE (a)-[:NEXT {{step: i}}]->(b);",
                    end - 1
                ),
            );
            i = end;
        }
    }
}

fn seed_star(conn: &Connection, spokes: usize) {
    run(
        conn,
        "CREATE NODE TABLE Hub (name STRING, PRIMARY KEY(name));",
    );
    run(conn, "CREATE NODE TABLE Leaf (id INT64, PRIMARY KEY(id));");
    run(conn, "CREATE REL TABLE ARM (FROM Hub TO Leaf);");
    run(conn, "CREATE (:Hub {name: 'center'});");
    let mut i = 0;
    while i < spokes {
        let end = (i + CHUNK).min(spokes);
        run(
            conn,
            &format!(
                "UNWIND range({i}, {}) AS i \
                 MATCH (h:Hub {{name: 'center'}}) \
                 CREATE (h)-[:ARM]->(:Leaf {{id: i}});",
                end - 1
            ),
        );
        i = end;
    }
}

fn seed_social(conn: &Connection, n: usize) {
    run(
        conn,
        "CREATE NODE TABLE Person (idx INT64, name STRING, PRIMARY KEY(idx));",
    );
    run(
        conn,
        "CREATE REL TABLE KNOWS (FROM Person TO Person, strength INT64);",
    );
    let mut i = 0;
    while i < n {
        let end = (i + CHUNK).min(n);
        run(
            conn,
            &format!(
                "UNWIND range({i}, {}) AS i \
                 CREATE (:Person {{idx: i, name: 'person_' + cast(i AS STRING)}});",
                end - 1
            ),
        );
        i = end;
    }
    for offset in 1..=2usize {
        let mut i = 0;
        while i < n {
            let end = (i + CHUNK).min(n);
            run(
                conn,
                &format!(
                    "UNWIND range({i}, {}) AS i \
                     MATCH (a:Person {{idx: i}}), (b:Person {{idx: (i + {offset}) % {n}}}) \
                     CREATE (a)-[:KNOWS {{strength: (i + {offset}) % 5}}]->(b);",
                    end - 1
                ),
            );
            i = end;
        }
    }
}
