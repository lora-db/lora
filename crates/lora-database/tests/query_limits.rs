//! Queries that used to overflow the stack (aborting the process) or
//! exhaust memory now fail with an error. See `lora_ast::limits`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use lora_database::{Database, InMemoryGraph, LoraErrorCode, LoraValue, TransactionMode};

/// Run `f` on a thread with the stack an unoptimised build needs at the
/// limits (release builds need far less).
fn on_big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

fn db() -> Database<InMemoryGraph> {
    Database::in_memory()
}

fn nested(depth: usize) -> LoraValue {
    let mut v = LoraValue::Int(1);
    for _ in 0..depth {
        v = LoraValue::List(vec![v]);
    }
    v
}

fn params(entries: Vec<(&str, LoraValue)>) -> BTreeMap<String, LoraValue> {
    entries
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

/// The error `query` fails with; panics when it succeeds.
fn error_of(
    db: &Database<InMemoryGraph>,
    query: &str,
    params: BTreeMap<String, LoraValue>,
) -> (LoraErrorCode, String) {
    match db.execute_with_params(query, None, params) {
        Ok(_) => panic!("`{query}` succeeded"),
        Err(e) => (e.code(), e.message().to_string()),
    }
}

#[test]
fn query_built_values_cannot_nest_past_the_limit() {
    on_big_stack(|| {
        let db = db();
        for query in [
            "RETURN size(reduce(acc = [], i IN range(1, 10000) | [acc])) AS x",
            "RETURN size(reduce(acc = {}, i IN range(1, 10000) | {a: acc})) AS x",
            "RETURN size(reduce(acc = [], i IN range(1, 10000) | [x IN [1] | acc])) AS x",
            "RETURN size(reduce(acc = [], i IN range(1, 10000) | [1] + {a: acc})) AS x",
            "RETURN size(reduce(acc = {}, i IN range(1, 10000) | acc {.*, a: acc})) AS x",
        ] {
            let (code, message) = error_of(&db, query, BTreeMap::new());
            assert_eq!(code, LoraErrorCode::Validation, "{query}: {message}");
            assert!(message.contains("128 levels deep"), "{query}: {message}");
        }
        // Exactly at the limit is fine.
        let rows = db
            .execute_rows("RETURN reduce(acc = 1, i IN range(1, 128) | [acc]) AS x")
            .unwrap();
        assert_eq!(rows.len(), 1);
        // collect() adds a level too.
        let (_, message) = error_of(
            &db,
            "UNWIND [$p] AS x RETURN collect(x) AS c",
            params(vec![("p", nested(128))]),
        );
        assert!(message.contains("levels deep"), "{message}");
    });
}

#[test]
fn deep_parameters_are_refused_on_every_path() {
    on_big_stack(|| {
        let db = db();
        // A million levels: walking or dropping it recursively would
        // overflow any stack.
        for depth in [129, 1_000_000] {
            let deep = || params(vec![("p", nested(depth))]);
            for query in [
                "RETURN size($p) AS x",
                "RETURN $p = $p AS x",
                "RETURN 1 AS x",
            ] {
                let (code, message) = error_of(&db, query, deep());
                assert_eq!(code, LoraErrorCode::InvalidParams, "{message}");
                assert!(message.contains("$p"), "{message}");
            }
            assert!(db.stream_with_params("RETURN $p AS x", deep()).is_err());
            assert!(db.explain("RETURN $p AS x", Some(deep())).is_err());
            assert!(db.profile("RETURN $p AS x", Some(deep())).is_err());
            let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
            assert!(tx
                .execute_with_params("RETURN $p AS x", None, deep())
                .is_err());
            assert!(tx.stream_with_params("RETURN $p AS x", deep()).is_err());
            assert!(tx.explain("RETURN $p AS x", Some(deep())).is_err());
            assert!(tx.profile("RETURN $p AS x", Some(deep())).is_err());
            tx.rollback().unwrap();
        }
        // At the limit a parameter works, and can be stored.
        let p = || params(vec![("p", nested(128))]);
        db.execute_with_params("RETURN size($p) AS x", None, p())
            .unwrap();
        db.execute_with_params("CREATE (:N {v: $p})", None, p())
            .unwrap();
        // One more level, built by the query, is refused before the write.
        let (code, message) = error_of(&db, "CREATE (:N {v: [$p]})", p());
        assert_eq!(code, LoraErrorCode::Validation, "{message}");
        let (_, message) = error_of(&db, "MATCH (n:N) SET n.w = [n.v]", BTreeMap::new());
        assert!(message.contains("levels deep"), "{message}");
        let count = db.execute_rows("MATCH (n:N) RETURN n").unwrap().len();
        assert_eq!(count, 1);
    });
}

#[test]
fn values_cannot_grow_past_the_size_limit() {
    let db = db();
    for query in [
        "RETURN size(reduce(s = 'x', i IN range(1, 40) | s + s)) AS x",
        "RETURN size(reduce(s = [1], i IN range(1, 40) | s + s)) AS x",
        "RETURN size(reduce(s = ['xxxxxxxxxxxxxxxx'], i IN range(1, 40) | s + s)) AS x",
        "RETURN size(reduce(s = 'x', i IN range(1, 40) | [s, s, s])) AS x",
    ] {
        let start = Instant::now();
        let (code, message) = error_of(&db, query, BTreeMap::new());
        assert_eq!(code, LoraErrorCode::Validation, "{query}: {message}");
        assert!(message.contains("MiB"), "{query}: {message}");
        assert!(start.elapsed() < Duration::from_secs(30), "{query}");
    }
}

#[test]
fn oversized_properties_are_refused_before_the_write() {
    let db = db();
    let big = "x".repeat(70 << 20);
    // Each value fits, but one node's properties are written as one record.
    let (code, message) = error_of(
        &db,
        "CREATE (:N {a: $s, b: $s})",
        params(vec![("s", LoraValue::String(big.clone()))]),
    );
    assert_eq!(code, LoraErrorCode::Validation, "{message}");
    assert!(message.contains("MiB"), "{message}");
    // A parameter over the limit is refused at the door.
    let (code, message) = error_of(
        &db,
        "CREATE (:N {a: $s})",
        params(vec![("s", LoraValue::String("x".repeat((128 << 20) + 1)))]),
    );
    assert_eq!(code, LoraErrorCode::InvalidParams, "{message}");
    // A query-built value over the limit fails while it is built.
    let (_, message) = error_of(
        &db,
        "CREATE (:N {a: $s + $s})",
        params(vec![("s", LoraValue::String(big))]),
    );
    assert!(message.contains("MiB"), "{message}");
    assert!(db.execute_rows("MATCH (n) RETURN n").unwrap().is_empty());
}

#[test]
fn query_shape_limits_are_parse_errors() {
    let db = db();
    for query in [
        format!(
            "RETURN {}1{} AS x",
            "[".repeat(100_000),
            "]".repeat(100_000)
        ),
        format!("RETURN {} AS x", vec!["1"; 100_000].join(" + ")),
        format!("RETURN {}true AS x", "NOT ".repeat(100_000)),
        format!(
            "WITH 1 AS a {}RETURN a",
            "WITH a WHERE a = 1 ".repeat(2_000)
        ),
        format!("RETURN {}1{} AS x", "(".repeat(100), ")".repeat(100)),
    ] {
        let start = Instant::now();
        let (code, _) = error_of(&db, &query, BTreeMap::new());
        assert_eq!(code, LoraErrorCode::Parse);
        assert!(start.elapsed() < Duration::from_secs(30));
    }
}

/// A chain `(:C {i: 0})-[:N]->(:C {i: 1})-[:N]->...` of `n` nodes.
fn chain(n: i64) -> Database<InMemoryGraph> {
    let db = db();
    db.execute("CREATE INDEX FOR (n:C) ON (n.i)", None).unwrap();
    db.execute(
        &format!("UNWIND range(0, {}) AS i CREATE (:C {{i: i}})", n - 1),
        None,
    )
    .unwrap();
    db.execute(
        &format!(
            "UNWIND range(1, {}) AS i MATCH (a:C {{i: i - 1}}) MATCH (b:C {{i: i}}) \
             CREATE (a)-[:N]->(b)",
            n - 1
        ),
        None,
    )
    .unwrap();
    db
}

fn single_int(db: &Database<InMemoryGraph>, query: &str) -> i64 {
    let rows = db
        .execute_rows(query)
        .unwrap_or_else(|e| panic!("{query}: {e}"));
    let value = rows[0].iter().next().map(|(_, v)| v.clone());
    match value {
        Some(LoraValue::Int(n)) => n,
        other => panic!("{query}: {other:?}"),
    }
}

#[test]
fn long_variable_length_paths_do_not_recurse() {
    on_big_stack(|| {
        let db = chain(1_200);
        // In an expression (was a recursive walk), buffered and pulled.
        for query in [
            "MATCH (a:C {i: 0}) RETURN size([(a)-[:N*1..1000]->(b) | 1]) AS n",
            "MATCH (a:C {i: 0})-[:N*1..1000]->(b) RETURN count(b) AS n",
            "MATCH (a:C {i: 0})-[r:N*1..1000]->(b) RETURN count(r) AS n",
            "MATCH (a:C {i: 0})-[r:N*1..1000]->(b) WITH b LIMIT 5000 RETURN count(b) AS n",
        ] {
            assert_eq!(single_int(&db, query), 1000, "{query}");
        }
        // Bounds past the limit are refused when parsed.
        let err = db
            .execute("MATCH (a)-[:N*1..1001]->(b) RETURN b", None)
            .unwrap_err();
        assert_eq!(err.code(), LoraErrorCode::Parse, "{err}");
        assert!(err.message().contains("1000 hops"), "{err}");
    });
}

#[test]
fn variable_length_expansion_stops_at_the_deadline() {
    on_big_stack(|| {
        // A complete graph: the number of paths explodes with length.
        let db = db();
        db.execute("UNWIND range(1, 12) AS i CREATE (:K {i: i})", None)
            .unwrap();
        db.execute("MATCH (a:K), (b:K) WHERE a <> b CREATE (a)-[:E]->(b)", None)
            .unwrap();
        for query in [
            "MATCH (a:K {i: 1})-[:E*1..12]->(b) RETURN count(b) AS n",
            "MATCH (a:K {i: 1})-[r:E*1..12]->(b) RETURN count(r) AS n",
            "MATCH (a:K {i: 1})-[:E*1..12]->(b {i: 99}) RETURN b LIMIT 1",
            "MATCH (a:K {i: 1}) RETURN size([(a)-[:E*1..12]->(b) | 1]) AS n",
        ] {
            let start = Instant::now();
            let err = db
                .execute_with_timeout(query, None, Duration::from_millis(200))
                .unwrap_err();
            assert!(
                matches!(
                    err.code(),
                    LoraErrorCode::Timeout | LoraErrorCode::Validation
                ),
                "{query}: {err}"
            );
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "{query}: {:?}",
                start.elapsed()
            );
        }
    });
}
