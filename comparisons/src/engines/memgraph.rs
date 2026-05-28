//! Memgraph engine adapter.
//!
//! Memgraph is a Cypher-compatible graph database written in C++. Unlike
//! the other engines in this crate it is **server-only** — there is no
//! embedded mode — so this adapter talks to a running Memgraph instance
//! over the Bolt protocol via the `rsmgclient` crate.
//!
//! The harness assumes a Memgraph server is reachable at
//! `bolt://127.0.0.1:7687`. No Docker or process management is performed
//! by the bench itself; the user is expected to launch Memgraph
//! out-of-band before running `cargo bench --features memgraph`.
//!
//! "Fresh" in the [`fresh`] / [`fresh_persistent`] sense means *wiping
//! the live database* with `MATCH (n) DETACH DELETE n` and re-opening a
//! connection so transaction state is clean — the server itself
//! persists between iterations.

use std::cell::RefCell;

use rsmgclient::{ConnectParams, Connection, SSLMode};

use crate::engine::DbHandle;
use crate::fixtures::{Fixture, FixtureKind};

const CHUNK: usize = 2_000;

/// Memgraph connection handle. The `Connection` lives behind a
/// `RefCell` because the engine adapter trait exposes `execute` /
/// `create_index` as `&DbHandle` consumers while `rsmgclient`'s
/// `execute*` methods require `&mut self`. The bench harness only ever
/// calls these serially on a single thread, so the runtime borrow check
/// is effectively free and never panics in practice.
pub struct MemgraphHandle {
    pub conn: RefCell<Connection>,
}

fn connect_params() -> ConnectParams {
    ConnectParams {
        address: Some(String::from("127.0.0.1")),
        port: 7687,
        // Local Memgraph defaults to a plaintext Bolt listener; the
        // crate's default of `SSLMode::Require` would refuse to connect.
        sslmode: SSLMode::Disable,
        // Auto-commit each statement so `execute_without_results` runs
        // are durable without an explicit `commit()`. The bench is
        // single-statement-per-call so there's no transactional
        // grouping to preserve.
        autocommit: true,
        ..Default::default()
    }
}

fn open() -> Connection {
    Connection::connect(&connect_params())
        .expect("memgraph: connect to bolt://127.0.0.1:7687 failed")
}

fn wipe(conn: &mut Connection) {
    conn.execute_without_results("MATCH (n) DETACH DELETE n;")
        .expect("memgraph: wipe (DETACH DELETE) failed");
}

pub fn fresh() -> DbHandle {
    // Wipe through one connection then drop it so the returned handle
    // starts on a clean transaction state, mirroring how the embedded
    // engines hand back a brand-new database object.
    {
        let mut wiper = open();
        wipe(&mut wiper);
    }
    let conn = open();
    DbHandle::Memgraph(
        MemgraphHandle {
            conn: RefCell::new(conn),
        },
        None,
    )
}

/// Memgraph has no separate "persistent" mode — the server is always
/// backed by its on-disk durability subsystem. The persistent column
/// exists only so the comparison matrix has a slot; functionally this
/// is identical to [`fresh`].
pub fn fresh_persistent() -> DbHandle {
    fresh()
}

pub fn seed(handle: &mut DbHandle, fixture: Fixture) {
    let DbHandle::Memgraph(h, _) = handle else {
        panic!("memgraph seed: wrong handle variant");
    };
    let mut conn = h.conn.borrow_mut();
    match fixture.kind {
        FixtureKind::Empty => {}
        FixtureKind::Nodes => seed_nodes(&mut conn, fixture.size),
        FixtureKind::Chain => seed_chain(&mut conn, fixture.size),
        FixtureKind::Star => seed_star(&mut conn, fixture.size),
        FixtureKind::Social => seed_social(&mut conn, fixture.size),
    }
}

/// Memgraph honours `CREATE INDEX ON :Label(prop);`. The bench only
/// requests indexes by property name, so we map the prop to its likely
/// label using the same heuristic as `surrealdb.rs`: `id`/`name`/`value`
/// live on `:Node`; `idx` straddles `:Chain`, `:Hub`, `:Leaf`, and
/// `:Person`, so we create the index on each candidate. Creating an
/// index on a label that has no nodes is cheap and harmless.
pub fn create_index(handle: &DbHandle, prop: &str) {
    let DbHandle::Memgraph(h, _) = handle else {
        panic!("memgraph create_index: wrong handle variant");
    };
    let labels: &[&str] = match prop {
        "id" | "name" | "value" => &["Node"],
        "idx" => &["Chain", "Hub", "Leaf", "Person"],
        _ => &["Node"],
    };
    let mut conn = h.conn.borrow_mut();
    for label in labels {
        let sql = format!("CREATE INDEX ON :{label}({prop});");
        conn.execute_without_results(&sql)
            .unwrap_or_else(|e| panic!("memgraph create_index failed: {sql}\nerror: {e}"));
    }
}

pub fn execute(handle: &DbHandle, q: &str) {
    let DbHandle::Memgraph(h, _) = handle else {
        panic!("memgraph execute: wrong handle variant");
    };
    let mut conn = h.conn.borrow_mut();
    conn.execute_without_results(q)
        .unwrap_or_else(|e| panic!("memgraph query failed: {q}\nerror: {e}"));
}

fn run(conn: &mut Connection, sql: &str) {
    conn.execute_without_results(sql)
        .unwrap_or_else(|e| panic!("memgraph seed sql failed: {sql}\nerror: {e}"));
}

fn seed_nodes(conn: &mut Connection, n: usize) {
    let mut i = 0;
    while i < n {
        let end = (i + CHUNK).min(n);
        let q = format!(
            "UNWIND range({i}, {}) AS i \
             CREATE (:Node {{id: i, name: 'node_' + toString(i), value: i % 100}});",
            end - 1
        );
        run(conn, &q);
        i = end;
    }
}

fn seed_chain(conn: &mut Connection, len: usize) {
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

fn seed_star(conn: &mut Connection, spokes: usize) {
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

fn seed_social(conn: &mut Connection, n: usize) {
    let mut i = 0;
    while i < n {
        let end = (i + CHUNK).min(n);
        run(
            conn,
            &format!(
                "UNWIND range({i}, {}) AS i \
                 CREATE (:Person {{idx: i, name: 'person_' + toString(i)}});",
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
