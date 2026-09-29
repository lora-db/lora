//! ORDER BY / SKIP / LIMIT run after projection, aggregation and DISTINCT.
//!
//! Regression: the planner placed Sort and Limit *before* the projection.
//! Sorting on an alias then sorted on an unbound variable (a no-op),
//! sorting on an aggregate sorted pre-aggregation rows, and LIMIT cut the
//! input before counting (`WITH v, count(*) AS c ORDER BY c DESC LIMIT 1`
//! reported c = 1) or before DISTINCT, all silently.

mod test_helpers;
use serde_json::json;
use test_helpers::TestDb;

fn db() -> TestDb {
    let db = TestDb::new();
    db.run("UNWIND [3, 1, 2, 1, 3, 3] AS v CREATE (:N {v: v})");
    db
}

fn col(db: &TestDb, q: &str, c: &str) -> Vec<serde_json::Value> {
    db.column(q, c)
}

#[test]
fn order_by_alias_sorts() {
    let db = db();
    assert_eq!(
        col(&db, "MATCH (n:N) RETURN n.v AS v ORDER BY v", "v"),
        [1, 1, 2, 3, 3, 3].map(|v| json!(v))
    );
    assert_eq!(
        col(&db, "MATCH (n:N) RETURN n.v * 10 AS w ORDER BY w DESC", "w"),
        [30, 30, 30, 20, 10, 10].map(|v| json!(v))
    );
}

#[test]
fn order_by_original_variable_still_works() {
    let db = db();
    assert_eq!(
        col(&db, "MATCH (n:N) RETURN n.v * 10 AS w ORDER BY n.v", "w"),
        [10, 10, 20, 30, 30, 30].map(|v| json!(v))
    );
    // Only the projected column comes back.
    let rows = db.run("MATCH (n:N) RETURN n.v AS v ORDER BY n.v LIMIT 1");
    assert_eq!(rows, vec![json!({"v": 1})]);
}

#[test]
fn order_by_aggregate_alias_and_expression() {
    let db = db();
    assert_eq!(
        col(
            &db,
            "MATCH (n:N) RETURN n.v AS v, count(*) AS c ORDER BY c DESC",
            "v"
        ),
        [3, 1, 2].map(|v| json!(v))
    );
    assert_eq!(
        col(
            &db,
            "MATCH (n:N) RETURN n.v AS v, count(*) AS c ORDER BY count(*)",
            "v"
        ),
        [2, 1, 3].map(|v| json!(v))
    );
}

#[test]
fn limit_applies_after_aggregation() {
    let db = db();
    let rows =
        db.run("MATCH (n:N) WITH n.v AS v, count(*) AS c ORDER BY c DESC LIMIT 1 RETURN v, c");
    assert_eq!(rows, vec![json!({"v": 3, "c": 3})]);
    let rows = db.run("MATCH (n:N) RETURN count(*) AS c LIMIT 1");
    assert_eq!(rows, vec![json!({"c": 6})]);
}

#[test]
fn limit_applies_after_distinct() {
    let db = db();
    assert_eq!(
        col(
            &db,
            "MATCH (n:N) RETURN DISTINCT n.v AS v ORDER BY v LIMIT 2",
            "v"
        ),
        [1, 2].map(|v| json!(v))
    );
    assert_eq!(
        db.run("MATCH (n:N) RETURN DISTINCT n.v AS v LIMIT 3").len(),
        3
    );
    assert_eq!(
        col(
            &db,
            "MATCH (n:N) RETURN DISTINCT n.v AS v ORDER BY v SKIP 1",
            "v"
        ),
        [2, 3].map(|v| json!(v))
    );
}

#[test]
fn with_order_limit_then_where() {
    let db = db();
    assert_eq!(
        col(
            &db,
            "MATCH (n:N) WITH n.v AS v ORDER BY v DESC LIMIT 4 WHERE v > 2 RETURN v",
            "v"
        ),
        [3, 3, 3].map(|v| json!(v))
    );
}
