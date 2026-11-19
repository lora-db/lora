//! The `where` option of the vector procedures (taste N-01): nearest
//! neighbours among the entities that meet property conditions, decided
//! before ranking, through RANGE and POINT indexes where they exist.
//!
//! Every case is checked against the same search written as a plain
//! `MATCH ... WHERE ... ORDER BY vector.similarity(...)`.

mod test_helpers;
use serde_json::{json, Value};
use test_helpers::TestDb;

const N: usize = 400;

fn seeded(indexed: bool) -> TestDb {
    let db = TestDb::new();
    db.run(
        "CREATE VECTOR INDEX ev FOR (e:E) ON (e.v) \
         OPTIONS {indexConfig: {`vector.dimensions`: 4, `vector.similarity_function`: 'cosine'}}",
    );
    if indexed {
        db.run("CREATE INDEX e_at FOR (e:E) ON (e.at)");
        db.run("CREATE INDEX e_genre FOR (e:E) ON (e.genre)");
        db.run("CREATE POINT INDEX e_loc FOR (e:E) ON (e.loc)");
    }
    db.run(&format!(
        "UNWIND range(0, {}) AS i \
         CREATE (:E {{k: i, at: i % 97, genre: ['techno', 'house', 'jazz'][i % 3], \
                      loc: point({{longitude: (i % 40) * 0.5, latitude: 50.0 + (i % 17) * 0.1}}), \
                      v: vector([toFloat(i % 7), toFloat(i % 5), toFloat(i % 3) + 1.0, 1.0], 4, FLOAT32)}})",
        N - 1
    ));
    // Entities the filters must skip: no `at`, and another label.
    db.run("CREATE (:E {k: -1, genre: 'techno', v: vector([1, 1, 1, 1], 4, FLOAT32)})");
    db.run("CREATE (:Other {k: -2, at: 5, v: vector([1, 1, 1, 1], 4, FLOAT32)})");
    db
}

fn keys(rows: &[Value]) -> Vec<i64> {
    rows.iter().map(|r| r["k"].as_i64().unwrap()).collect()
}

const QUERY: &str = "vector([3, 1, 2, 1], 4, FLOAT32)";

fn check(db: &TestDb, options: &str, predicate: &str) {
    let expected = keys(&db.run(&format!(
        "MATCH (e:E) WHERE {predicate} \
         RETURN e.k AS k ORDER BY vector.similarity(e.v, {QUERY}) DESC, id(e) LIMIT 10"
    )));
    assert!(
        !expected.is_empty(),
        "predicate `{predicate}` matches nothing"
    );
    let in_query = keys(&db.run(&format!(
        "CALL db.index.vector.queryNodes('ev', 10, {QUERY}, {{where: {options}}}) \
         YIELD node, score RETURN node.k AS k"
    )));
    assert_eq!(in_query, expected, "in-query where {options}");
    let standalone: Vec<i64> = db
        .run(&format!(
            "CALL db.index.vector.queryNodes('ev', 10, [3.0, 1.0, 2.0, 1.0], {{where: {options}}})"
        ))
        .iter()
        .map(|r| r["node"]["properties"]["k"].as_i64().unwrap())
        .collect();
    assert_eq!(standalone, expected, "standalone where {options}");
}

const CASES: &[(&str, &str)] = &[
    ("{at: {gte: 10, lt: 20}}", "e.at >= 10 AND e.at < 20"),
    ("{at: {gt: 90}}", "e.at > 90"),
    ("{genre: 'jazz'}", "e.genre = 'jazz'"),
    ("{genre: ['jazz', 'house'], at: {lte: 30}}", "e.genre IN ['jazz', 'house'] AND e.at <= 30"),
    (
        "{loc: {withinBBox: [point({longitude: 2.0, latitude: 50.2}), point({longitude: 9.0, latitude: 50.9})]}}",
        "geo.within_bbox(e.loc, point({longitude: 2.0, latitude: 50.2}), point({longitude: 9.0, latitude: 50.9}))",
    ),
    (
        "{at: {in: [1, 2, 3, 50]}, loc: {withinBBox: [point({longitude: 0.0, latitude: 50.0}), point({longitude: 20.0, latitude: 52.0})]}}",
        "e.at IN [1, 2, 3, 50]",
    ),
    ("{at: {eq: 2.0}}", "e.at = 2"),
];

#[test]
fn where_matches_the_plain_query_with_indexes() {
    let db = seeded(true);
    for (options, predicate) in CASES {
        check(&db, options, predicate);
    }
}

#[test]
fn where_matches_the_plain_query_without_indexes() {
    let db = seeded(false);
    for (options, predicate) in CASES {
        check(&db, options, predicate);
    }
}

#[test]
fn where_and_restrict_to_both_apply() {
    let db = seeded(true);
    let ids: Vec<Value> = db
        .run("MATCH (e:E) WHERE e.k < 50 RETURN id(e) AS id")
        .into_iter()
        .map(|r| r["id"].clone())
        .collect();
    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "ids".to_string(),
        lora_database::LoraValue::List(
            ids.iter()
                .map(|v| lora_database::LoraValue::Int(v.as_i64().unwrap()))
                .collect(),
        ),
    );
    let rows = db.run_with_params(
        &format!(
            "CALL db.index.vector.queryNodes('ev', 100, {QUERY}, {{restrictTo: $ids, where: {{genre: 'jazz'}}}}) \
             YIELD node RETURN node.k AS k"
        ),
        params,
    );
    let mut got = keys(&rows);
    got.sort();
    let want: Vec<i64> = (0..50).filter(|k| k % 3 == 2).collect();
    assert_eq!(got, want);
}

#[test]
fn where_options_come_from_a_parameter_and_bad_ones_fail() {
    let db = seeded(true);
    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "opts".to_string(),
        lora_database::LoraValue::Map(std::collections::BTreeMap::from([(
            "where".to_string(),
            lora_database::LoraValue::Map(std::collections::BTreeMap::from([(
                "at".to_string(),
                lora_database::LoraValue::Int(3),
            )])),
        )])),
    );
    let rows = db.run_with_params(
        &format!("CALL db.index.vector.queryNodes('ev', 3, {QUERY}, $opts) YIELD node RETURN node.at AS at"),
        params,
    );
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r["at"] == json!(3)));

    for (options, needle) in [
        ("{where: {at: {between: 1}}}", "unknown operator `between`"),
        ("{where: {at: null}}", "is null"),
        ("{where: {at: {gt: 1, gte: 2}}}", "both `gt` and `gte`"),
        ("{where: 5}", "MAP"),
        ("{filter: {}}", "unknown option `filter`"),
    ] {
        let err = db.run_err(&format!(
            "CALL db.index.vector.queryNodes('ev', 3, {QUERY}, {options}) YIELD node RETURN node"
        ));
        assert!(err.contains(needle), "{options}: {err}");
    }
}
