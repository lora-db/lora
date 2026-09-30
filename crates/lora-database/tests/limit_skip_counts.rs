//! `SKIP` and `LIMIT` take a non-negative integer. `null` (an omitted
//! parameter), a negative number or a fraction is an error, as in Neo4j,
//! never "no limit".

mod test_helpers;
use std::collections::BTreeMap;

use lora_database::LoraValue;
use test_helpers::TestDb;

fn params(pairs: &[(&str, LoraValue)]) -> BTreeMap<String, LoraValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn limit_null_parameter_is_an_error() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 5) AS i CREATE (:N {i: i})");
    for query in [
        "MATCH (n:N) RETURN n.i AS i LIMIT $l",
        "MATCH (n:N) RETURN n.i AS i ORDER BY i LIMIT $l",
        "MATCH (n:N) RETURN n.i AS i SKIP $l",
        "MATCH (n:N) WITH n LIMIT $l RETURN count(n) AS c",
        "RETURN 1 AS x LIMIT $l",
    ] {
        let err = db
            .exec_with_params(query, params(&[("l", LoraValue::Null)]))
            .expect_err(query)
            .to_string();
        assert!(
            err.contains("non-negative integer") && err.contains("null"),
            "{query}: {err}"
        );
    }
    // An absent parameter is null too.
    let err = db
        .exec_with_params("RETURN 1 AS x LIMIT $missing", BTreeMap::new())
        .expect_err("absent")
        .to_string();
    assert!(err.contains("LIMIT"), "{err}");
}

#[test]
fn limit_rejects_negative_and_fractional_counts() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 5) AS i CREATE (:N {i: i})");
    for (query, value) in [
        ("MATCH (n:N) RETURN n LIMIT $l", LoraValue::Int(-1)),
        ("MATCH (n:N) RETURN n SKIP $l", LoraValue::Int(-2)),
        ("MATCH (n:N) RETURN n LIMIT $l", LoraValue::Float(1.5)),
        (
            "MATCH (n:N) RETURN n LIMIT $l",
            LoraValue::String("2".into()),
        ),
    ] {
        assert!(
            db.exec_with_params(query, params(&[("l", value.clone())]))
                .is_err(),
            "{query} with {value:?}"
        );
    }
}

#[test]
fn limit_accepts_integral_counts() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 5) AS i CREATE (:N {i: i})");
    let rows = db.run_with_params(
        "MATCH (n:N) RETURN n.i AS i ORDER BY i SKIP $s LIMIT $l",
        params(&[("s", LoraValue::Int(1)), ("l", LoraValue::Float(2.0))]),
    );
    let got: Vec<i64> = rows.iter().map(|r| r["i"].as_i64().unwrap()).collect();
    assert_eq!(got, vec![2, 3]);
    db.assert_count("MATCH (n:N) RETURN n LIMIT 0", 0);
}
