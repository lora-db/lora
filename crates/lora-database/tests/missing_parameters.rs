//! Every `$name` a query reads must be in its params map: a missing one
//! fails the query before it runs (`expected parameter: $name`,
//! `LORA_INVALID_PARAMS`), on every path that runs a query — `execute`,
//! the pull pipeline, explicit transactions, `profile` — and wherever the
//! parameter appears: a filter, a write, `SKIP` / `LIMIT`, a UNION branch,
//! a subquery. A parameter bound to `null` is a value
//! and runs. `explain` never runs the query, so it needs no parameters.
//!
//! Regression: a missing parameter read as null, so `MATCH (u {id: $id})`
//! silently matched nothing and `SET u.key = $missing` silently wrote a
//! null.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::{LoraValue, TransactionMode};
use test_helpers::TestDb;

fn params(pairs: &[(&str, LoraValue)]) -> BTreeMap<String, LoraValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE (:U {key: 'a', n: 1})");
    db
}

/// Run `query` with `given` on every path, expecting the missing-parameter
/// error naming `missing`, and the graph unchanged.
fn assert_missing(query: &str, given: &[(&str, LoraValue)], missing: &str) {
    let expected = format!("expected parameter{missing}");
    let check = |path: &str, err: String| {
        assert!(err.contains(&expected), "{path}: `{query}`: {err}");
    };
    let db = seeded();
    check(
        "execute",
        db.service
            .execute_rows_with_params(query, params(given))
            .expect_err(query)
            .to_string(),
    );
    match db.service.stream_with_params(query, params(given)) {
        Ok(_) => panic!("stream: `{query}` ran"),
        Err(e) => check("stream", e.to_string()),
    }
    for mode in [TransactionMode::ReadWrite, TransactionMode::ReadOnly] {
        let mut tx = db.service.begin_transaction(mode).unwrap();
        check(
            "tx",
            tx.execute_rows_with_params(query, params(given))
                .expect_err(query)
                .to_string(),
        );
        tx.rollback().unwrap();
    }
    check(
        "profile",
        db.service
            .profile(query, Some(params(given)))
            .expect_err(query)
            .to_string(),
    );
    let state = db.run("MATCH (u) RETURN count(u) AS c, collect(properties(u)) AS ps");
    assert_eq!(
        state,
        vec![serde_json::json!({"c": 1, "ps": [{"key": "a", "n": 1}]})],
        "`{query}` changed the graph"
    );
}

#[test]
fn a_missing_parameter_is_an_error_wherever_it_appears() {
    for query in [
        "MATCH (u:U) WHERE u.key = $id RETURN u",
        "MATCH (u:U {key: $id}) RETURN u",
        "MATCH (u:U) RETURN u.n + $id AS v",
        "RETURN 1 AS x LIMIT $id",
        "MATCH (u:U) RETURN u SKIP $id",
        "RETURN 1 AS x UNION RETURN $id AS x",
        "CALL { RETURN $id AS v } RETURN v",
        "MATCH (u:U) WHERE EXISTS { MATCH (u) WHERE u.n = $id } RETURN u",
        "UNWIND $id AS x RETURN x",
    ] {
        assert_missing(query, &[], ": $id");
    }
}

#[test]
fn a_write_with_a_missing_parameter_writes_nothing() {
    assert_missing("MATCH (u:U) SET u.key = $missing", &[], ": $missing");
    assert_missing("CREATE (:U {key: $missing})", &[], ": $missing");
    assert_missing(
        "MERGE (u:U {key: 'a'}) SET u.n = $missing",
        &[],
        ": $missing",
    );
}

#[test]
fn every_missing_parameter_is_named() {
    assert_missing(
        "MATCH (u:U) WHERE u.key = $a AND u.n = $b RETURN u",
        &[("other", LoraValue::Int(1))],
        "s: $a, $b",
    );
    assert_missing(
        "MATCH (u:U) WHERE u.key = $a AND u.n = $b RETURN u",
        &[("a", LoraValue::String("a".into()))],
        ": $b",
    );
}

#[test]
fn an_explicit_null_is_a_value() {
    let db = seeded();
    let rows = db.run_with_params(
        "MATCH (u:U) WHERE u.key = $id RETURN count(u) AS c",
        params(&[("id", LoraValue::Null)]),
    );
    assert_eq!(rows, vec![serde_json::json!({"c": 0})]);
    db.run_with_params(
        "MATCH (u:U) SET u.extra = $v",
        params(&[("v", LoraValue::Null)]),
    );
}

#[test]
fn unused_parameters_are_fine() {
    let db = seeded();
    let rows = db.run_with_params(
        "MATCH (u:U) RETURN u.key AS k",
        params(&[("unused", LoraValue::Int(1))]),
    );
    assert_eq!(rows, vec![serde_json::json!({"k": "a"})]);
}

#[test]
fn explain_needs_no_parameters() {
    let db = seeded();
    db.service
        .explain("MATCH (u:U) WHERE u.key = $id RETURN u", None)
        .expect("explain doesn't run the query");
}

#[test]
fn the_error_code_is_invalid_params() {
    let db = seeded();
    let err = db
        .service
        .execute_with_params("RETURN $id AS v", None, BTreeMap::new())
        .expect_err("missing $id");
    assert_eq!(err.code().as_str(), "LORA_INVALID_PARAMS", "{err}");
}
