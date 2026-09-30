//! `<`, `<=`, `>`, `>=` follow Cypher ordering: lists compare element by
//! element, null or incomparable operands give null (never false), and
//! integers compare exactly. Regression for the Festimap brief's P0-5:
//! `[f.name, f.key] > $after` silently matched nothing.

mod test_helpers;
use std::collections::BTreeMap;

use lora_database::LoraValue;
use serde_json::{json, Value};
use test_helpers::TestDb;

fn scalar(q: &str) -> Value {
    TestDb::new().scalar(q)
}

#[test]
fn keyset_list_comparison_matches_rows_after_the_cursor() {
    let db = TestDb::new();
    db.run(
        "CREATE (:F {key: 'a', name: 'Tomorrowland'}), (:F {key: 'b', name: 'Awakenings'}), \
         (:F {key: 'c', name: 'Lowlands'}), (:F {key: 'd', name: 'Awakenings'})",
    );
    let mut params = BTreeMap::new();
    params.insert(
        "after".to_string(),
        LoraValue::List(vec![
            LoraValue::String("Awakenings".into()),
            LoraValue::String("b".into()),
        ]),
    );
    let rows = db.run_with_params(
        "MATCH (f:F) WHERE [f.name, f.key] > $after RETURN f.key AS k ORDER BY f.name, f.key",
        params,
    );
    let keys: Vec<_> = rows.iter().map(|r| r["k"].clone()).collect();
    assert_eq!(keys, vec![json!("d"), json!("c"), json!("a")]);
}

#[test]
fn lists_of_equal_length_compare_lexicographically() {
    assert_eq!(scalar("RETURN [1, 2] < [1, 3]"), json!(true));
    assert_eq!(scalar("RETURN [1, 2] <= [1, 2]"), json!(true));
    assert_eq!(scalar("RETURN [1, 2] > [1, 2]"), json!(false));
    assert_eq!(scalar("RETURN [1, 2] >= [1, 2]"), json!(true));
    assert_eq!(scalar("RETURN [2, 0] > [1, 9]"), json!(true));
    assert_eq!(scalar("RETURN ['b', 'a'] < ['a', 'z']"), json!(false));
}

#[test]
fn a_prefix_sorts_before_the_longer_list() {
    assert_eq!(scalar("RETURN [1] < [1, 0]"), json!(true));
    assert_eq!(scalar("RETURN [1, 0] > [1]"), json!(true));
    assert_eq!(scalar("RETURN [] < [1]"), json!(true));
    assert_eq!(scalar("RETURN [] >= []"), json!(true));
    assert_eq!(scalar("RETURN [2] > [1, 5]"), json!(true));
}

#[test]
fn nulls_in_lists_decide_only_when_reached() {
    // The first differing pair decides before the null is reached.
    assert_eq!(scalar("RETURN [1, null] < [2, 0]"), json!(true));
    // A null pair before any difference makes the comparison null.
    assert_eq!(scalar("RETURN [null, 1] < [2, 0]"), Value::Null);
    assert_eq!(scalar("RETURN [1, null] <= [1, 2]"), Value::Null);
    assert_eq!(scalar("RETURN [1, 2] > null"), Value::Null);
}

#[test]
fn nested_lists_compare_recursively() {
    assert_eq!(scalar("RETURN [[1, 2], 3] < [[1, 3], 0]"), json!(true));
}

#[test]
fn incomparable_operands_are_null_not_false() {
    assert_eq!(scalar("RETURN 1 < 'a'"), Value::Null);
    assert_eq!(scalar("RETURN [1] < 1"), Value::Null);
    assert_eq!(scalar("RETURN [1, 'a'] < [1, 2]"), Value::Null);
    assert_eq!(
        scalar("RETURN date('2026-01-01') < datetime('2026-01-02T00:00:00Z')"),
        Value::Null
    );
    // So negating one cannot make it true.
    assert_eq!(scalar("RETURN NOT (1 < 'a')"), Value::Null);
    let db = TestDb::new();
    db.run("CREATE (:N {v: 1}), (:N {v: 'x'})");
    // 1 < 'm' is null, so NOT keeps it out; 'x' < 'm' is false, so NOT keeps it.
    assert_eq!(
        db.column("MATCH (n:N) WHERE NOT (n.v < 'm') RETURN n.v AS v", "v"),
        vec![json!("x")]
    );
}

#[test]
fn integers_compare_exactly_above_2_pow_53() {
    assert_eq!(
        scalar("RETURN 9007199254740993 > 9007199254740992"),
        json!(true)
    );
    assert_eq!(
        scalar("RETURN [9007199254740993] > [9007199254740992]"),
        json!(true)
    );
    assert_eq!(
        scalar("RETURN 9007199254740993 > 9007199254740992.0"),
        json!(true)
    );
    assert_eq!(scalar("RETURN 2 > 1.5"), json!(true));
    assert_eq!(scalar("RETURN 1.5 < 2"), json!(true));
    assert_eq!(scalar("RETURN 2 >= 2.0"), json!(true));
    assert_eq!(scalar("RETURN -3 < -2.5"), json!(true));
    assert_eq!(scalar("RETURN 9223372036854775807 < 1e19"), json!(true));
    assert_eq!(scalar("RETURN 1 < 0.0 / 0.0"), Value::Null);
}

#[test]
fn booleans_order_false_before_true() {
    assert_eq!(scalar("RETURN false < true"), json!(true));
    assert_eq!(scalar("RETURN [true, false] > [true, true]"), json!(false));
}

#[test]
fn scalar_comparisons_are_unchanged() {
    assert_eq!(scalar("RETURN 'a' < 'b'"), json!(true));
    assert_eq!(
        scalar("RETURN date('2026-01-01') < date('2026-01-02')"),
        json!(true)
    );
    assert_eq!(
        scalar("RETURN duration('PT1H') < duration('PT2H')"),
        json!(true)
    );
    assert_eq!(scalar("RETURN 1 < null"), Value::Null);
}
