//! Neo4j engine adapter.
//!
//! Neo4j is a Cypher-compatible graph database. Like Memgraph it is
//! **server-only** — there is no embedded mode — so this adapter talks
//! to a running Neo4j instance over the Bolt protocol via the `neo4rs`
//! crate.
//!
//! The harness assumes a Neo4j server is reachable at
//! `bolt://127.0.0.1:7688` (note the non-default port: Memgraph already
//! occupies the standard 7687 in this codebase, so Neo4j is mapped to
//! 7688 to let both run side-by-side). Override via the `NEO4J_URL`
//! environment variable. Credentials default to `neo4j` / `bench` and
//! can be overridden with `NEO4J_USER` / `NEO4J_PASSWORD`. Note that
//! Neo4j 5 rejects passwords shorter than 8 characters, so the default
//! `bench` only works when the server is started with
//! `NEO4J_dbms_security_auth__minimum__password__length=4` (see
//! `comparisons/README.md`). No Docker or process management is performed
//! by the bench itself; the user is expected to launch Neo4j out-of-band
//! before running `cargo bench --features neo4j`.
//!
//! "Fresh" in the [`fresh`] / [`fresh_persistent`] sense means *wiping
//! the live database* with `MATCH (n) DETACH DELETE n` and re-opening a
//! connection so transaction state is clean — the server itself
//! persists between iterations.
//!
//! ## Async / sync impedance
//!
//! `neo4rs` is async-first: `Graph::new`, `Graph::run`, etc. all return
//! futures. The bench harness in `benches/comparison.rs` calls
//! `execute` / `create_index` synchronously, so the handle owns a
//! dedicated multi-thread tokio runtime and every adapter entry point
//! `block_on`s on it — mirroring the surrealdb and helixdb adapters.

use std::sync::Arc;

use neo4rs::{query, Graph};
use tokio::runtime::{Builder, Runtime};

use crate::engine::DbHandle;
use crate::fixtures::{Fixture, FixtureKind};

const DEFAULT_URL: &str = "bolt://127.0.0.1:7688";
const DEFAULT_USER: &str = "neo4j";
const DEFAULT_PASSWORD: &str = "bench";
const CHUNK: usize = 2_000;

/// Neo4j connection handle. Unlike Memgraph's `rsmgclient::Connection`
/// (which requires `&mut self` for `execute*`), `neo4rs::Graph` exposes
/// its API on `&self` and is internally pooled, so no `RefCell` is
/// needed. The tokio runtime is owned by the handle so the synchronous
/// bench harness can `block_on` async calls without leaking a runtime
/// per iteration.
pub struct Neo4jHandle {
    pub rt: Arc<Runtime>,
    pub graph: Graph,
}

fn build_runtime() -> Runtime {
    Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("neo4j: tokio runtime build failed")
}

fn neo4j_url() -> String {
    std::env::var("NEO4J_URL").unwrap_or_else(|_| DEFAULT_URL.to_string())
}

fn neo4j_user() -> String {
    std::env::var("NEO4J_USER").unwrap_or_else(|_| DEFAULT_USER.to_string())
}

fn neo4j_password() -> String {
    std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| DEFAULT_PASSWORD.to_string())
}

fn open(rt: &Runtime) -> Graph {
    let uri = neo4j_url();
    let user = neo4j_user();
    let pass = neo4j_password();
    rt.block_on(async { Graph::new(uri.clone(), user, pass).await })
        .unwrap_or_else(|e| panic!("neo4j: connect to `{uri}` failed: {e}"))
}

fn wipe(rt: &Runtime, graph: &Graph) {
    rt.block_on(async { graph.run(query("MATCH (n) DETACH DELETE n;")).await })
        .expect("neo4j: wipe (DETACH DELETE) failed");
}

pub fn fresh() -> DbHandle {
    let rt = Arc::new(build_runtime());
    let graph = open(&rt);
    wipe(&rt, &graph);
    DbHandle::Neo4j(Neo4jHandle { rt, graph }, None)
}

/// Neo4j has no separate "persistent" mode — the server is always
/// backed by its on-disk durability subsystem. The persistent column
/// exists only so the comparison matrix has a slot; functionally this
/// is identical to [`fresh`].
pub fn fresh_persistent() -> DbHandle {
    fresh()
}

pub fn seed(handle: &mut DbHandle, fixture: Fixture) {
    let DbHandle::Neo4j(h, _) = handle else {
        panic!("neo4j seed: wrong handle variant");
    };
    match fixture.kind {
        FixtureKind::Empty => {}
        FixtureKind::Nodes => seed_nodes(&h.rt, &h.graph, fixture.size),
        FixtureKind::Chain => seed_chain(&h.rt, &h.graph, fixture.size),
        FixtureKind::Star => seed_star(&h.rt, &h.graph, fixture.size),
        FixtureKind::Social => seed_social(&h.rt, &h.graph, fixture.size),
    }
}

/// Neo4j 5.x uses the modern `CREATE INDEX FOR (n:Label) ON (n.prop)`
/// syntax — the older `CREATE INDEX ON :Label(prop)` form Memgraph
/// accepts is deprecated and rejected here. The bench only requests
/// indexes by property name, so we map the prop to its likely label
/// using the same heuristic as the memgraph adapter: `id`/`name`/`value`
/// live on `:Node`; `idx` straddles `:Chain`, `:Hub`, `:Leaf`, and
/// `:Person`, so we create the index on each candidate. Errors are
/// ignored so re-running the bench against an already-indexed database
/// is harmless (Neo4j raises on duplicate index creation).
pub fn create_index(handle: &DbHandle, prop: &str) {
    let DbHandle::Neo4j(h, _) = handle else {
        panic!("neo4j create_index: wrong handle variant");
    };
    let labels: &[&str] = match prop {
        "id" | "name" | "value" => &["Node"],
        "idx" => &["Chain", "Hub", "Leaf", "Person"],
        _ => &["Node"],
    };
    for label in labels {
        let sql = format!("CREATE INDEX FOR (n:{label}) ON (n.{prop})");
        // Best-effort: an "EquivalentSchemaRuleAlreadyExists" error on
        // re-run is expected and harmless.
        let _ = h.rt.block_on(async { h.graph.run(query(&sql)).await });
    }
}

pub fn execute(handle: &DbHandle, q: &str) {
    let DbHandle::Neo4j(h, _) = handle else {
        panic!("neo4j execute: wrong handle variant");
    };
    h.rt.block_on(async { h.graph.run(query(q)).await })
        .unwrap_or_else(|e| panic!("neo4j query failed: {q}\nerror: {e}"));
}

fn run(rt: &Runtime, graph: &Graph, sql: &str) {
    rt.block_on(async { graph.run(query(sql)).await })
        .unwrap_or_else(|e| panic!("neo4j seed sql failed: {sql}\nerror: {e}"));
}

fn seed_nodes(rt: &Runtime, graph: &Graph, n: usize) {
    let mut i = 0;
    while i < n {
        let end = (i + CHUNK).min(n);
        let q = format!(
            "UNWIND range({i}, {}) AS i \
             CREATE (:Node {{id: i, name: 'node_' + toString(i), value: i % 100}})",
            end - 1
        );
        run(rt, graph, &q);
        i = end;
    }
}

fn seed_chain(rt: &Runtime, graph: &Graph, len: usize) {
    let mut i = 0;
    while i < len {
        let end = (i + CHUNK).min(len);
        run(
            rt,
            graph,
            &format!(
                "UNWIND range({i}, {}) AS i CREATE (:Chain {{idx: i}})",
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
                rt,
                graph,
                &format!(
                    "UNWIND range({i}, {}) AS i \
                     MATCH (a:Chain {{idx: i}}), (b:Chain {{idx: i + 1}}) \
                     CREATE (a)-[:NEXT {{step: i}}]->(b)",
                    end - 1
                ),
            );
            i = end;
        }
    }
}

fn seed_star(rt: &Runtime, graph: &Graph, spokes: usize) {
    run(rt, graph, "CREATE (:Hub {name: 'center'})");
    let mut i = 0;
    while i < spokes {
        let end = (i + CHUNK).min(spokes);
        run(
            rt,
            graph,
            &format!(
                "UNWIND range({i}, {}) AS i \
                 MATCH (h:Hub {{name: 'center'}}) \
                 CREATE (h)-[:ARM]->(:Leaf {{id: i}})",
                end - 1
            ),
        );
        i = end;
    }
}

fn seed_social(rt: &Runtime, graph: &Graph, n: usize) {
    let mut i = 0;
    while i < n {
        let end = (i + CHUNK).min(n);
        run(
            rt,
            graph,
            &format!(
                "UNWIND range({i}, {}) AS i \
                 CREATE (:Person {{idx: i, name: 'person_' + toString(i)}})",
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
                rt,
                graph,
                &format!(
                    "UNWIND range({i}, {}) AS i \
                     MATCH (a:Person {{idx: i}}), (b:Person {{idx: (i + {offset}) % {n}}}) \
                     CREATE (a)-[:KNOWS {{strength: (i + {offset}) % 5}}]->(b)",
                    end - 1
                ),
            );
            i = end;
        }
    }
}
