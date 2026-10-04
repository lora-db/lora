//! A node or relationship carried through an aggregation as a grouping key
//! keeps its identity.
//!
//! `WITH u, count(*) AS c` used to emit `u` as the hydrated output map
//! `{id, kind, labels, properties}` instead of a node reference. Every
//! downstream clause then saw a map: `CREATE (u)-[:R]->(f)` silently made a
//! fresh blank node for `u`, `MATCH (u)-->(x)` / `SET` / `DELETE` errored,
//! and `id(u)`, `u:U`, `type(r)` and `u = n` went null or false — while
//! `u.k` and `RETURN u` still looked right. Nodes nested inside a list or
//! map grouping key were hit the same way.
//!
//! Each case runs through the buffered executors (a `ReadWrite`
//! transaction's `execute_rows`), the pull pipeline (`Database::stream`)
//! and the default `Database::execute`, with a fold-only aggregate
//! (`count`, the streaming-fold path) and a buffered one (`collect`).
//!
//! Separately: a variable bound to a non-node value in a node position of
//! `CREATE` / `MERGE` is an error, never a fresh node.

mod test_helpers;

use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat, Row, TransactionMode};
use serde_json::{json, Value as JsonValue};
use test_helpers::TestDb;

#[derive(Clone, Copy, Debug)]
enum Path {
    /// `Database::execute`.
    Execute,
    /// `Database::stream` — the pull pipeline.
    Stream,
    /// `Transaction::execute_rows` in a `ReadWrite` transaction — the
    /// buffered executors.
    Tx,
}

const PATHS: [Path; 3] = [Path::Execute, Path::Stream, Path::Tx];

fn rows_json(rows: Vec<Row>) -> Vec<JsonValue> {
    rows.into_iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .collect()
}

fn run_on(db: &Database<InMemoryGraph>, path: Path, query: &str) -> Result<Vec<JsonValue>, String> {
    match path {
        Path::Execute => {
            let result = db
                .execute(
                    query,
                    Some(ExecuteOptions {
                        format: ResultFormat::Rows,
                    }),
                )
                .map_err(|e| e.to_string())?;
            let json = serde_json::to_value(result).unwrap();
            Ok(json
                .get("rows")
                .and_then(JsonValue::as_array)
                .cloned()
                .unwrap_or_default())
        }
        Path::Stream => {
            let mut stream = db.stream(query).map_err(|e| e.to_string())?;
            let mut rows = Vec::new();
            while let Some(row) = stream.next_row().map_err(|e| e.to_string())? {
                rows.push(row);
            }
            stream.finish().map_err(|e| e.to_string())?;
            Ok(rows_json(rows))
        }
        Path::Tx => {
            let mut tx = db
                .begin_transaction(TransactionMode::ReadWrite)
                .map_err(|e| e.to_string())?;
            let rows = tx.execute_rows(query).map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok(rows_json(rows))
        }
    }
}

const SEED: &str = "CREATE (:U {k: 1})-[:R0 {w: 7}]->(:X {k: 2}), (:F {k: 3})";

fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run(SEED);
    db
}

/// The single value of column `v`.
fn v(db: &TestDb, query: &str) -> JsonValue {
    db.scalar(query)
}

/// Prefixes that leave `u` bound to the `:U` node after an aggregation,
/// as `(name, cypher)`.
const NODE_PREFIXES: &[(&str, &str)] = &[
    ("node key, count", "MATCH (u:U) WITH u, count(*) AS c"),
    (
        "node key, collect",
        "MATCH (u:U)-[:R0]->(x) WITH u, collect(x) AS xs",
    ),
    (
        "list key, count",
        "MATCH (a:U) WITH collect(a) AS us WITH us, count(*) AS c WITH us[0] AS u",
    ),
    (
        "list key, collect",
        "MATCH (a:U) WITH collect(a) AS us MATCH (b:F) \
         WITH us, collect(b) AS fs WITH us[0] AS u",
    ),
    (
        "map key, count",
        "MATCH (u0:U) WITH {n: u0} AS m, count(*) AS c WITH m.n AS u",
    ),
    (
        "map key, collect",
        "MATCH (u0:U)-[:R0]->(x) WITH {n: u0} AS m, collect(x) AS xs WITH m.n AS u",
    ),
];

/// Prefixes that leave `r` bound to the `:R0` relationship after an
/// aggregation.
const REL_PREFIXES: &[(&str, &str)] = &[
    (
        "rel key, count",
        "MATCH ()-[r:R0]->() WITH r, count(*) AS c",
    ),
    (
        "rel key, collect",
        "MATCH ()-[r:R0]->(b) WITH r, collect(b) AS bs",
    ),
    (
        "rel list key, count",
        "MATCH ()-[r0:R0]->() WITH collect(r0) AS rs WITH rs, count(*) AS c WITH rs[0] AS r",
    ),
    (
        "rel map key, collect",
        "MATCH ()-[r0:R0]->(b) WITH {r: r0} AS m, collect(b) AS bs WITH m.r AS r",
    ),
];

/// Run `prefix + suffix` on a fresh seeded graph through every path, then
/// check the graph with `checks` (`(query, expected v)`).
fn assert_write(prefix: &str, name: &str, suffix: &str, checks: &[(&str, JsonValue)]) {
    for path in PATHS {
        let db = seeded();
        let query = format!("{prefix} {suffix}");
        run_on(&db.service, path, &query)
            .unwrap_or_else(|e| panic!("[{name} / {path:?}] `{query}` failed: {e}"));
        for (check, expected) in checks {
            assert_eq!(
                &v(&db, check),
                expected,
                "[{name} / {path:?}] after `{query}`, `{check}`"
            );
        }
    }
}

/// Run `prefix + suffix` (a read) through every path and expect one row
/// whose `v` equals `expected`.
fn assert_read(prefix: &str, name: &str, suffix: &str, expected: JsonValue) {
    let db = seeded();
    for path in PATHS {
        let query = format!("{prefix} {suffix}");
        let rows = run_on(&db.service, path, &query)
            .unwrap_or_else(|e| panic!("[{name} / {path:?}] `{query}` failed: {e}"));
        assert_eq!(
            rows.len(),
            1,
            "[{name} / {path:?}] `{query}` rows: {rows:?}"
        );
        assert_eq!(
            rows[0].get("v"),
            Some(&expected),
            "[{name} / {path:?}] `{query}`"
        );
    }
}

const ATTACHED: &str = "MATCH (:U {k: 1})-[:R]->(:F {k: 3}) RETURN count(*) AS v";
const NODES: &str = "MATCH (n) RETURN count(n) AS v";

#[test]
fn create_after_grouping_attaches_to_the_grouped_node() {
    for (name, prefix) in NODE_PREFIXES {
        assert_write(
            prefix,
            name,
            "MATCH (f:F) CREATE (u)-[:R]->(f)",
            &[(NODES, json!(3)), (ATTACHED, json!(1))],
        );
    }
}

#[test]
fn merge_after_grouping_attaches_to_the_grouped_node() {
    for (name, prefix) in NODE_PREFIXES {
        assert_write(
            prefix,
            name,
            "MATCH (f:F) MERGE (u)-[:R]->(f)",
            &[(NODES, json!(3)), (ATTACHED, json!(1))],
        );
    }
}

#[test]
fn set_after_grouping_updates_the_grouped_node() {
    for (name, prefix) in NODE_PREFIXES {
        assert_write(
            prefix,
            name,
            "SET u.x = 1, u:Seen",
            &[
                ("MATCH (n:U) RETURN n.x AS v", json!(1)),
                ("MATCH (n:Seen) RETURN count(n) AS v", json!(1)),
            ],
        );
    }
}

#[test]
fn detach_delete_after_grouping_deletes_the_grouped_node() {
    for (name, prefix) in NODE_PREFIXES {
        assert_write(
            prefix,
            name,
            "DETACH DELETE u",
            &[
                ("MATCH (n:U) RETURN count(n) AS v", json!(0)),
                (NODES, json!(2)),
            ],
        );
    }
}

#[test]
fn expand_after_grouping_starts_from_the_grouped_node() {
    for (name, prefix) in NODE_PREFIXES {
        assert_read(
            prefix,
            name,
            "MATCH (u)-[:R0]->(y) RETURN y.k AS v",
            json!(2),
        );
    }
}

#[test]
fn identity_functions_after_grouping_see_the_node() {
    for (name, prefix) in NODE_PREFIXES {
        assert_read(
            prefix,
            name,
            "MATCH (n:U) RETURN id(u) = id(n) AND u:U AND labels(u) = ['U'] AND u = n \
             AND u.k = 1 AS v",
            json!(true),
        );
    }
}

#[test]
fn returning_the_grouped_node_still_hydrates() {
    for (name, prefix) in NODE_PREFIXES {
        assert_read(prefix, name, "RETURN u.k AS v", json!(1));
        let db = seeded();
        for path in PATHS {
            let rows = run_on(&db.service, path, &format!("{prefix} RETURN u AS v")).unwrap();
            let node = rows[0].get("v").unwrap();
            assert_eq!(node["labels"], json!(["U"]), "[{name} / {path:?}] {node}");
            assert_eq!(
                node["properties"]["k"],
                json!(1),
                "[{name} / {path:?}] {node}"
            );
        }
    }
}

#[test]
fn grouping_by_node_still_groups_by_entity() {
    let db = TestDb::new();
    db.run("CREATE (a:U {k: 1}), (b:U {k: 1}), (a)-[:T]->(:X), (a)-[:T]->(:X), (b)-[:T]->(:X)");
    for path in PATHS {
        let mut counts: Vec<i64> = run_on(
            &db.service,
            path,
            "MATCH (u:U)-[:T]->(x) WITH u, count(x) AS c RETURN c",
        )
        .unwrap()
        .iter()
        .map(|row| row["c"].as_i64().unwrap())
        .collect();
        counts.sort();
        assert_eq!(counts, vec![1, 2], "{path:?}");

        // Same entity in a nested list key groups together too.
        let rows = run_on(
            &db.service,
            path,
            "MATCH (u:U)-[:T]->(x) WITH [u] AS key, count(*) AS c RETURN count(key) AS groups",
        )
        .unwrap();
        assert_eq!(rows[0]["groups"], json!(2), "{path:?}");
    }
}

#[test]
fn relationship_identity_survives_grouping() {
    for (name, prefix) in REL_PREFIXES {
        assert_read(
            prefix,
            name,
            "RETURN type(r) = 'R0' AND id(r) IS NOT NULL AND startNode(r).k = 1 \
             AND endNode(r).k = 2 AS v",
            json!(true),
        );
        assert_read(prefix, name, "MATCH (a)-[r]->(b) RETURN b.k AS v", json!(2));
        assert_read(
            prefix,
            name,
            "MATCH ()-[x:R0]->() RETURN x = r AS v",
            json!(true),
        );
        assert_write(
            prefix,
            name,
            "SET r.w = 8",
            &[("MATCH ()-[x:R0]->() RETURN x.w AS v", json!(8))],
        );
        assert_write(
            prefix,
            name,
            "DELETE r",
            &[("MATCH ()-[x]->() RETURN count(x) AS v", json!(0))],
        );
    }
}

#[test]
fn reported_repros() {
    let db = TestDb::new();
    db.run("CREATE (:U {k: 1}), (:F {k: 1})");
    db.run("MATCH (u:U) WITH u, count(*) AS c MATCH (f:F) CREATE (u)-[:R]->(f)");
    assert_eq!(v(&db, NODES), json!(2));
    assert_eq!(
        v(&db, "MATCH (:U)-[:R]->(:F) RETURN count(*) AS v"),
        json!(1)
    );

    let db = TestDb::new();
    db.run("CREATE (:U {k: 1}), (:F {k: 1})");
    db.run(
        "MATCH (a:U) WITH collect(a) AS us MATCH (b:F) WITH us, collect(b) AS fs \
         WITH us[0] AS u, fs[0] AS f CREATE (u)-[:R]->(f)",
    );
    assert_eq!(v(&db, NODES), json!(2));
    assert_eq!(
        v(&db, "MATCH (:U)-[:R]->(:F) RETURN count(*) AS v"),
        json!(1)
    );
}

#[test]
fn create_on_a_non_node_binding_is_an_error() {
    let bindings = [
        ("map", "WITH {k: 1} AS u"),
        ("null", "WITH null AS u"),
        ("scalar", "WITH 1 AS u"),
        ("string", "WITH 'u' AS u"),
    ];
    let writes = [
        "CREATE (u)-[:R]->(:F)",
        "CREATE (:F)-[:R]->(u)",
        "MERGE (u)-[:R]->(:F)",
    ];
    for (kind, binding) in bindings {
        for write in writes {
            for path in PATHS {
                let db = TestDb::new();
                let query = format!("{binding} {write}");
                let err = run_on(&db.service, path, &query)
                    .expect_err(&format!("[{kind} / {path:?}] `{query}` should fail"));
                assert!(
                    err.contains("to be bound to a node"),
                    "[{kind} / {path:?}] `{query}`: {err}"
                );
                assert_eq!(
                    v(&db, NODES),
                    json!(0),
                    "[{kind} / {path:?}] `{query}` must not create nodes"
                );
            }
        }
    }
}

#[test]
fn create_on_a_non_entity_binding_is_an_error_for_lone_nodes_and_relationships() {
    let cases = [
        ("WITH {a: 1} AS u CREATE (u)", "to be bound to a node"),
        ("WITH {a: 1} AS u MERGE (u)", "to be bound to a node"),
        (
            "WITH 1 AS r CREATE ()-[r:R]->()",
            "to be bound to a relationship",
        ),
        (
            "WITH {a: 1} AS r MERGE ()-[r:R]->()",
            "to be bound to a relationship",
        ),
    ];
    for (query, message) in cases {
        for path in PATHS {
            let db = TestDb::new();
            let err = run_on(&db.service, path, query)
                .expect_err(&format!("[{path:?}] `{query}` should fail"));
            assert!(err.contains(message), "[{path:?}] `{query}`: {err}");
            assert_eq!(v(&db, NODES), json!(0), "[{path:?}] `{query}`");
        }
    }
}
