//! Builtin calls on constant arguments are folded when the plan is built:
//! `n.at > datetime('2024-01-01T00:00:00Z')` parsed the string again for
//! every row, a quarter of the query's time over 10k rows. The clock,
//! random values and calls that fail are left to run per row.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::{LoraValue, PlanTreeNode};
use test_helpers::TestDb;

fn details(node: &PlanTreeNode, op: &str, key: &str, out: &mut Vec<String>) {
    if node.operator == op {
        if let Some(value) = node.details.get(key) {
            out.push(value.clone());
        }
    }
    for child in &node.children {
        details(child, op, key, out);
    }
}

/// The `key` details of every `op` operator in the plan of `query`.
fn plan_details(db: &TestDb, query: &str, op: &str, key: &str) -> String {
    let plan = db.service.explain(query, None).unwrap().tree.root;
    let mut out = Vec::new();
    details(&plan, op, key, &mut out);
    assert!(!out.is_empty(), "no {op} in the plan of {query}");
    out.join("\n")
}

fn seed_events(db: &TestDb) {
    db.run(
        "UNWIND range(0, 9) AS i CREATE (:Event {id: i, \
         at: datetime('2024-01-01T00:00:00Z') + duration({hours: i}), \
         loc: point({x: i, y: 0})})",
    );
}

#[test]
fn a_temporal_constructor_in_a_filter_is_folded() {
    let db = TestDb::new();
    seed_events(&db);
    let query = "MATCH (e:Event) WHERE e.at > datetime('2024-01-01T05:00:00Z') RETURN e.id AS id";

    let predicate = plan_details(&db, query, "Filter", "predicate");
    assert!(!predicate.contains("Function"), "{predicate}");
    assert!(predicate.contains("DateTime"), "{predicate}");

    assert_eq!(db.sorted_ints(query, "id"), vec![6, 7, 8, 9]);
}

#[test]
fn folded_values_answer_as_the_calls_did() {
    let db = TestDb::new();
    seed_events(&db);
    // Each query with what its folded constant shows as in the plan.
    let cases = [
        (
            "MATCH (e:Event) WHERE date(e.at) = date('2024-01-01') RETURN e.id AS id",
            "Literal(Value(Date(",
            10,
        ),
        (
            "MATCH (e:Event) WHERE e.at < datetime('2024-01-01T00:00:00Z') + duration('PT3H') RETURN e.id AS id",
            "Literal(Value(Duration(",
            3,
        ),
        (
            "MATCH (e:Event) WHERE localdatetime(e.at) >= localdatetime('2024-01-01T08:00:00') RETURN e.id AS id",
            "Literal(Value(LocalDateTime(",
            2,
        ),
        (
            "MATCH (e:Event) WHERE localtime(e.at) < localtime('02:30') RETURN e.id AS id",
            "Literal(Value(LocalTime(",
            3,
        ),
        (
            "MATCH (e:Event) WHERE point.distance(e.loc, point({x: 0, y: 0})) <= 4.0 RETURN e.id AS id",
            "Literal(Value(Point(",
            5,
        ),
        (
            "MATCH (e:Event) WHERE e.at > datetime({year: 2024, month: 1, day: 1, hour: 7}) RETURN e.id AS id",
            "Literal(Value(DateTime(",
            2,
        ),
        (
            "MATCH (e:Event) WHERE e.id < toInteger('4') + 1 RETURN e.id AS id",
            "Literal(Integer(4))",
            5,
        ),
        (
            "MATCH (e:Event) WHERE date(e.at) = date(datetime('2024-01-01T05:00:00Z')) RETURN e.id AS id",
            "Literal(Value(Date(",
            10,
        ),
    ];
    for (query, folded, expected) in cases {
        let predicate = plan_details(&db, query, "Filter", "predicate");
        assert!(predicate.contains(folded), "{query}: {predicate}");
        assert_eq!(db.sorted_ints(query, "id").len(), expected, "{query}");
    }

    let row =
        &db.run("RETURN date(datetime('2024-03-04T05:06:07Z')) AS d, duration('P1D') AS p")[0];
    assert_eq!(row["d"], "2024-03-04");
    assert_eq!(row["p"], "P1D");
}

#[test]
fn the_clock_and_random_values_are_not_folded() {
    let db = TestDb::new();
    for call in [
        "datetime()",
        "date()",
        "time()",
        "localtime()",
        "localdatetime()",
        "datetime.transaction()",
        "timestamp()",
        "rand()",
        "randomUUID()",
        // A TIME in a named zone takes the zone's offset now.
        "time('12:00[Europe/Amsterdam]')",
    ] {
        let query = format!("UNWIND [1] AS x WITH x WHERE {call} IS NOT NULL RETURN x");
        let predicate = plan_details(&db, &query, "Filter", "predicate");
        assert!(
            predicate.contains("Function"),
            "{call} was folded: {predicate}"
        );
    }

    let values = db.run("UNWIND range(1, 5) AS i RETURN rand() AS r, randomUUID() AS u");
    let rs: std::collections::BTreeSet<String> =
        values.iter().map(|row| row["r"].to_string()).collect();
    let us: std::collections::BTreeSet<String> =
        values.iter().map(|row| row["u"].to_string()).collect();
    assert_eq!((rs.len(), us.len()), (5, 5));
}

#[test]
fn a_call_that_fails_still_fails_only_when_a_row_reaches_it() {
    let db = TestDb::new();
    let bad = "MATCH (e:Event) WHERE e.at > datetime('not a datetime') RETURN e.id AS id";
    let predicate = plan_details(&db, bad, "Filter", "predicate");
    assert!(predicate.contains("cast.to"), "{predicate}");

    // No rows: nothing evaluates the call.
    assert!(db.run(bad).is_empty());
    assert!(db
        .run("RETURN CASE WHEN false THEN datetime('not a datetime') END AS v")
        .iter()
        .all(|row| row["v"].is_null()));

    // A row reaches it: the same error as before folding existed.
    seed_events(&db);
    let message = db.run_err(bad);
    assert!(message.contains("not a datetime"), "{message}");
    assert_eq!(
        db.run_err("RETURN datetime('not a datetime') AS v"),
        message
    );
}

#[test]
fn parameters_are_not_folded() {
    let db = TestDb::new();
    seed_events(&db);
    let query = "MATCH (e:Event) WHERE e.at > datetime($since) RETURN e.id AS id";
    let count = |since: &str| {
        let params = BTreeMap::from([("since".to_string(), LoraValue::String(since.into()))]);
        db.run_with_params(query, params).len()
    };
    // The same cached plan answers for each value.
    assert_eq!(count("2024-01-01T05:00:00Z"), 4);
    assert_eq!(count("2024-01-01T08:00:00Z"), 1);
    assert_eq!(count("2024-01-01T05:00:00Z"), 4);
}

#[test]
fn a_folded_constructor_still_lets_an_index_answer() {
    let db = TestDb::new();
    db.run("CREATE INDEX event_at FOR (e:Event) ON (e.at)");
    db.run("CREATE POINT INDEX event_loc FOR (e:Event) ON (e.loc)");
    seed_events(&db);

    let query = "MATCH (e:Event) WHERE e.at >= datetime('2024-01-01T07:00:00Z') RETURN e.id AS id";
    let plan = db.service.explain(query, None).unwrap().tree.root;
    let mut seeks = Vec::new();
    details(&plan, "NodeByPropertyRangeScan", "lo", &mut seeks);
    assert_eq!(seeks.len(), 1, "{plan:?}");
    assert_eq!(db.sorted_ints(query, "id"), vec![7, 8, 9]);

    let query = "MATCH (e:Event) WHERE point.distance(e.loc, point({x: 0, y: 0})) < 2.5 \
                 RETURN e.id AS id";
    let plan = db.service.explain(query, None).unwrap().tree.root;
    let mut scans = Vec::new();
    details(&plan, "NodeByPointScan", "predicate", &mut scans);
    assert_eq!(scans.len(), 1, "{plan:?}");
    assert_eq!(db.sorted_ints(query, "id"), vec![0, 1, 2]);
}
