use std::sync::Arc;

use grafeo::GrafeoDB;
use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat};
use surrealdb::{engine::local::Db, Surreal};
use tempfile::TempDir;
use tokio::runtime::Runtime;

#[cfg(feature = "helixdb")]
use crate::engines::helixdb as helix_eng;
#[cfg(feature = "helixdb")]
use crate::engines::helixdb::HelixHandle;
#[cfg(feature = "kuzu")]
use crate::engines::kuzu as kuzu_eng;
#[cfg(feature = "kuzu")]
use crate::engines::kuzu::KuzuHandle;
#[cfg(feature = "memgraph")]
use crate::engines::memgraph as memgraph_eng;
#[cfg(feature = "memgraph")]
use crate::engines::memgraph::MemgraphHandle;
#[cfg(feature = "neo4j")]
use crate::engines::neo4j as neo4j_eng;
#[cfg(feature = "neo4j")]
use crate::engines::neo4j::Neo4jHandle;
use crate::engines::{grafeo as grafeo_eng, lora as lora_eng, surrealdb as surreal_eng};
use crate::fixtures::Fixture;

/// Type-erased database handle. New engines add a variant here and a
/// matching dispatch arm in [`registry`] (and their own seed/execute fns).
///
/// The `Option<TempDir>` slot keeps any on-disk persistent storage alive
/// for the lifetime of the handle and cleans it up at drop. In-memory
/// variants pass `None`.
//
// `clippy::large_enum_variant`: GrafeoDB is ~half a kB on the stack, but
// handles are constructed at most once per Criterion sample (or once per
// `iter_batched` setup, never on the timed hot path) so the size delta
// is below the bench's noise floor. Boxing would just add an indirection
// without changing what we measure.
#[allow(clippy::large_enum_variant)]
pub enum DbHandle {
    Lora(Database<InMemoryGraph>, Option<TempDir>),
    Grafeo(GrafeoDB, Option<TempDir>),
    Surreal {
        rt: Arc<Runtime>,
        db: Surreal<Db>,
        dir: Option<TempDir>,
    },
    #[cfg(feature = "kuzu")]
    Kuzu(KuzuHandle, Option<TempDir>),
    #[cfg(feature = "memgraph")]
    Memgraph(MemgraphHandle, Option<TempDir>),
    #[cfg(feature = "neo4j")]
    Neo4j(Neo4jHandle, Option<TempDir>),
    #[cfg(feature = "helixdb")]
    Helix(HelixHandle, Option<TempDir>),
}

pub fn lora_opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

/// Static description of an engine in the registry.
pub struct EngineSpec {
    /// Name used in the criterion bench id and looked up in
    /// `workloads.yml`'s `queries` map.
    pub name: &'static str,
    /// Fallback engine name to look up in `queries` when `name` itself
    /// has no entry. Lets persistent variants reuse the in-memory
    /// engine's query strings without duplicating YAML.
    pub query_alias: Option<&'static str>,
    pub fresh: fn() -> DbHandle,
    pub seed: fn(&mut DbHandle, Fixture),
    /// Create a property index after seeding. Engines that auto-index
    /// (lora) implement this as a no-op.
    pub create_index: fn(&DbHandle, &str),
    pub execute: fn(&DbHandle, &str),
}

/// Engines that the bench runner will try every workload against.
///
/// The bench output groups results by workload, so adding an engine here
/// extends every applicable group with a new bench function. Engines that
/// don't have a query string for a given workload are skipped silently.
///
/// Storage modes are selected via env vars:
///
/// * Default — only in-memory engines (3 specs).
/// * `LORA_VS_PERSISTENT=1` — in-memory + persistent engines (6 specs).
///   Persistent specs fall back to the in-memory engine's query when
///   `workloads.yml` has no entry under the persistent name.
/// * `LORA_VS_PERSISTENT_ONLY=1` — implies `LORA_VS_PERSISTENT=1` and
///   drops the in-memory specs (3 persistent specs only).
pub fn registry() -> Vec<EngineSpec> {
    let mode = StorageMode::from_env();
    let mut specs = Vec::new();
    if mode.includes_mem() {
        specs.extend([
            EngineSpec {
                name: "lora",
                query_alias: None,
                fresh: lora_eng::fresh,
                seed: lora_eng::seed,
                create_index: lora_eng::create_index,
                execute: lora_eng::execute,
            },
            EngineSpec {
                name: "grafeo",
                query_alias: None,
                fresh: grafeo_eng::fresh,
                seed: grafeo_eng::seed,
                create_index: grafeo_eng::create_index,
                execute: grafeo_eng::execute,
            },
            EngineSpec {
                name: "surrealdb",
                query_alias: None,
                fresh: surreal_eng::fresh,
                seed: surreal_eng::seed,
                create_index: surreal_eng::create_index,
                execute: surreal_eng::execute,
            },
        ]);
        // Kuzu carries its own explicit `kuzu:` query strings for every
        // workload it supports (no grafeo alias). Its Cypher dialect
        // diverges from Grafeo's on a handful of functions (`ceiling`,
        // `round(x, 0)`, …) and it is strict-schema; workloads it can't
        // express carry a `notes.kuzu` entry instead.
        #[cfg(feature = "kuzu")]
        specs.push(EngineSpec {
            name: "kuzu",
            query_alias: None,
            fresh: kuzu_eng::fresh,
            seed: kuzu_eng::seed,
            create_index: kuzu_eng::create_index,
            execute: kuzu_eng::execute,
        });
        // Memgraph is server-only, so there is no separate persistent
        // variant. It carries explicit `memgraph:` query strings in the
        // YAML (no grafeo alias). The bench expects a Memgraph instance
        // reachable at `bolt://127.0.0.1:7687`; see `engines/memgraph.rs`.
        #[cfg(feature = "memgraph")]
        specs.push(EngineSpec {
            name: "memgraph",
            query_alias: None,
            fresh: memgraph_eng::fresh,
            seed: memgraph_eng::seed,
            create_index: memgraph_eng::create_index,
            execute: memgraph_eng::execute,
        });
        // Neo4j is server-only like Memgraph, also Bolt + Cypher. It
        // carries explicit `neo4j:` query strings in the YAML (no grafeo
        // alias). The bench expects a Neo4j instance reachable at
        // `bolt://127.0.0.1:7688` (override via `NEO4J_URL`) — the
        // non-standard port avoids colliding with Memgraph's 7687 when
        // both are run side-by-side; see `engines/neo4j.rs`.
        #[cfg(feature = "neo4j")]
        specs.push(EngineSpec {
            name: "neo4j",
            query_alias: None,
            fresh: neo4j_eng::fresh,
            seed: neo4j_eng::seed,
            create_index: neo4j_eng::create_index,
            execute: neo4j_eng::execute,
        });
        // HelixDB is server-only (the `helix-db` crate is a client SDK
        // that POSTs typed `DynamicQueryRequest`s over HTTP). The DSL is
        // purely programmatic, so the adapter interprets the workload's
        // `helixdb:` value as a sentinel keyword and dispatches to a
        // hand-written DSL traversal. The
        // bench expects a HelixDB instance reachable at
        // `http://127.0.0.1:6969` (override via `HELIXDB_URL`); see
        // `engines/helixdb.rs`.
        #[cfg(feature = "helixdb")]
        specs.push(EngineSpec {
            name: "helixdb",
            query_alias: None,
            fresh: helix_eng::fresh,
            seed: helix_eng::seed,
            create_index: helix_eng::create_index,
            execute: helix_eng::execute,
        });
    }
    if mode.includes_persistent() {
        specs.extend([
            EngineSpec {
                name: "lora_wal",
                query_alias: Some("lora"),
                fresh: lora_eng::fresh_persistent,
                seed: lora_eng::seed,
                create_index: lora_eng::create_index,
                execute: lora_eng::execute,
            },
            EngineSpec {
                name: "grafeo_file",
                query_alias: Some("grafeo"),
                fresh: grafeo_eng::fresh_persistent,
                seed: grafeo_eng::seed,
                create_index: grafeo_eng::create_index,
                execute: grafeo_eng::execute,
            },
            EngineSpec {
                name: "surrealdb_kv",
                query_alias: Some("surrealdb"),
                fresh: surreal_eng::fresh_persistent,
                seed: surreal_eng::seed,
                create_index: surreal_eng::create_index,
                execute: surreal_eng::execute,
            },
        ]);
        #[cfg(feature = "kuzu")]
        specs.push(EngineSpec {
            // Persistent Kuzu reuses the in-memory variant's explicit
            // `kuzu:` query strings via a one-level alias (there are no
            // separate `kuzu_file:` keys). Since the in-memory `kuzu` spec
            // no longer aliases grafeo, this resolves to the kuzu entries
            // directly rather than chaining.
            name: "kuzu_file",
            query_alias: Some("kuzu"),
            fresh: kuzu_eng::fresh_persistent,
            seed: kuzu_eng::seed,
            create_index: kuzu_eng::create_index,
            execute: kuzu_eng::execute,
        });
    }
    specs
}

#[derive(Debug, Clone, Copy)]
enum StorageMode {
    MemOnly,
    Both,
    PersistentOnly,
}

impl StorageMode {
    fn from_env() -> Self {
        if env_truthy("LORA_VS_PERSISTENT_ONLY") {
            Self::PersistentOnly
        } else if env_truthy("LORA_VS_PERSISTENT") {
            Self::Both
        } else {
            Self::MemOnly
        }
    }

    fn includes_mem(self) -> bool {
        matches!(self, Self::MemOnly | Self::Both)
    }

    fn includes_persistent(self) -> bool {
        matches!(self, Self::Both | Self::PersistentOnly)
    }
}

fn env_truthy(key: &str) -> bool {
    matches!(
        std::env::var(key).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}
