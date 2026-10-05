//! A path variable on a `CREATE` or `MERGE` pattern (`CREATE p = (a)-[:R]->(b)`)
//! is bound to the nodes and relationships the pattern created, reused or
//! matched, in pattern order.
//!
//! Regression: the path binding was ignored (`trace!("path
//! materialization not implemented")`), so `RETURN p` gave null, as did
//! `length(p)` and `nodes(p)`; a MERGE that matched left it null too. Every
//! case runs on each write path (`execute`, the pull pipeline, an explicit
//! transaction).

mod test_helpers;

use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat, Row, TransactionMode};
use serde_json::{json, Value as JsonValue};
use test_helpers::TestDb;

#[derive(Clone, Copy, Debug)]
enum Path {
    Execute,
    Stream,
    Tx,
}

const WRITE_PATHS: [Path; 3] = [Path::Execute, Path::Stream, Path::Tx];

fn rows_json(rows: Vec<Row>) -> Vec<JsonValue> {
    rows.into_iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .collect()
}

fn run_on(db: &Database<InMemoryGraph>, path: Path, query: &str) -> JsonValue {
    let run = || -> Result<Vec<JsonValue>, String> {
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
    };
    JsonValue::Array(run().unwrap_or_else(|e| panic!("{path:?}: `{query}` failed: {e}")))
}

/// Runs `setup` then `query` on a fresh database per write path.
fn assert_write(setup: &str, query: &str, expected: JsonValue) {
    for path in WRITE_PATHS {
        let db = TestDb::new();
        if !setup.is_empty() {
            db.run(setup);
        }
        assert_eq!(
            run_on(&db.service, path, query),
            expected,
            "{path:?}: `{query}`"
        );
    }
}

/// Labels, keys and types along `p`, so expectations don't depend on ids.
const SHAPE: &str = "length(p) AS len, [n IN nodes(p) | labels(n)[0]] AS ns, \
                     [r IN relationships(p) | type(r)] AS rs";

#[test]
fn create_binds_the_path_it_created() {
    assert_write(
        "",
        &format!("CREATE p = (:A)-[:R]->(:B) RETURN {SHAPE}"),
        json!([{"len": 1, "ns": ["A", "B"], "rs": ["R"]}]),
    );
    assert_write(
        "",
        &format!("CREATE p = (:A) RETURN {SHAPE}"),
        json!([{"len": 0, "ns": ["A"], "rs": []}]),
    );
    // Direction doesn't change the order: nodes follow the pattern.
    assert_write(
        "",
        &format!("CREATE p = (:A)-[:R]->(:B)<-[:S]-(:C) RETURN {SHAPE}"),
        json!([{"len": 2, "ns": ["A", "B", "C"], "rs": ["R", "S"]}]),
    );
}

#[test]
fn create_reuses_bound_nodes_in_its_path() {
    assert_write(
        "CREATE (:A {k: 1})",
        &format!("MATCH (a:A) CREATE p = (a)-[:R]->(:B) RETURN {SHAPE}, nodes(p)[0].k AS k"),
        json!([{"len": 1, "ns": ["A", "B"], "rs": ["R"], "k": 1}]),
    );
}

#[test]
fn each_pattern_part_binds_its_own_path() {
    assert_write(
        "",
        "CREATE p = (:A)-[:R]->(:B), q = (:C) RETURN length(p) AS lp, length(q) AS lq",
        json!([{"lp": 1, "lq": 0}]),
    );
    assert_write(
        "",
        "UNWIND [1, 2] AS i CREATE p = (:A {i: i})-[:R]->(:B) \
         RETURN nodes(p)[0].i AS i ORDER BY i",
        json!([{"i": 1}, {"i": 2}]),
    );
}

#[test]
fn the_path_is_a_real_path_value() {
    assert_write(
        "",
        "CREATE p = (a:A)-[r:R]->(b:B) \
         RETURN nodes(p)[0] = a AND nodes(p)[1] = b AS same_nodes, \
                relationships(p)[0] = r AS same_rel",
        json!([{"same_nodes": true, "same_rel": true}]),
    );
}

#[test]
fn merge_binds_the_path_whether_it_creates_or_matches() {
    let merge = format!("MERGE p = (:M {{k: 1}})-[:R]->(:N {{k: 2}}) RETURN {SHAPE}");
    let expected = json!([{"len": 1, "ns": ["M", "N"], "rs": ["R"]}]);
    // Creates.
    assert_write("", &merge, expected.clone());
    // Matches.
    assert_write("CREATE (:M {k: 1})-[:R]->(:N {k: 2})", &merge, expected);
    assert_write(
        "CREATE (:M {k: 1})",
        &format!("MERGE p = (:M {{k: 1}}) RETURN {SHAPE}"),
        json!([{"len": 0, "ns": ["M"], "rs": []}]),
    );
    // Every part already bound by an earlier clause.
    assert_write(
        "CREATE (:M)-[:R]->(:N)",
        &format!("MATCH (m:M)-[r:R]->(n:N) MERGE p = (m)-[r]->(n) RETURN {SHAPE}"),
        json!([{"len": 1, "ns": ["M", "N"], "rs": ["R"]}]),
    );
}
