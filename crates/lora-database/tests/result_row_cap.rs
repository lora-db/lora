//! `execute_with_params_deadline_max_rows`: a result over the row cap
//! fails with `LORA_RESULT_TOO_LARGE`, stops near the cap instead of being
//! built in full, and never misfires on large intermediate results that
//! shrink before they are returned.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use lora_database::{
    Database, ExecuteOptions, InMemoryGraph, LoraErrorCode, QueryResult, ResultFormat,
};

fn rows_format() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

/// `n` `:N` nodes joined in a ring of `:R` relationships.
fn seeded(n: usize) -> Database<InMemoryGraph> {
    let db = Database::in_memory();
    db.execute("CREATE INDEX n_i FOR (n:N) ON (n.i)", None)
        .unwrap();
    db.execute(
        &format!("UNWIND range(1, {n}) AS i CREATE (:N {{i: i}})"),
        None,
    )
    .unwrap();
    db.execute(
        &format!("MATCH (a:N) MATCH (b:N {{i: a.i % {n} + 1}}) CREATE (a)-[:R]->(b)"),
        None,
    )
    .unwrap();
    db
}

fn capped(
    db: &Database<InMemoryGraph>,
    query: &str,
    max_rows: usize,
) -> Result<QueryResult, lora_database::LoraError> {
    let deadline = Instant::now() + Duration::from_secs(300);
    db.execute_with_params_deadline_max_rows(
        query,
        rows_format(),
        BTreeMap::new(),
        deadline,
        max_rows,
    )
}

fn rows(result: QueryResult) -> usize {
    match result {
        QueryResult::Rows(r) => r.rows.len(),
        _ => panic!("expected the rows format"),
    }
}

fn assert_too_large(db: &Database<InMemoryGraph>, query: &str, max_rows: usize) {
    match capped(db, query, max_rows) {
        Ok(result) => panic!(
            "`{query}` returned {} rows under a cap of {max_rows}",
            rows(result)
        ),
        Err(err) => {
            assert_eq!(err.code(), LoraErrorCode::ResultTooLarge, "{query}: {err}");
            assert!(
                err.message()
                    .contains(&format!("more than {max_rows} rows")),
                "{query}: {err}"
            );
        }
    }
}

fn count_of(db: &Database<InMemoryGraph>, query: &str) -> i64 {
    match db.execute(query, rows_format()).unwrap() {
        QueryResult::Rows(r) => match r.rows[0].iter_named().next().unwrap().2 {
            lora_database::LoraValue::Int(n) => *n,
            other => panic!("expected an int, got {other:?}"),
        },
        _ => unreachable!(),
    }
}

#[test]
fn results_at_the_cap_pass_and_one_more_row_fails() {
    let db = seeded(2000);
    assert_eq!(
        rows(capped(&db, "MATCH (n:N) RETURN n.i", 2000).unwrap()),
        2000
    );
    assert_too_large(&db, "MATCH (n:N) RETURN n.i", 1999);
    assert_eq!(
        rows(capped(&db, "UNWIND range(1, 10) AS i RETURN i", 10).unwrap()),
        10
    );
    assert_too_large(&db, "UNWIND range(1, 11) AS i RETURN i", 10);
}

/// Each of these would build billions of rows (and run into the deadline)
/// if the cap were only checked on the finished result.
#[test]
fn huge_results_stop_near_the_cap() {
    let db = seeded(2000);
    for query in [
        "MATCH (a:N), (b:N), (c:N) RETURN a.i, b.i, c.i",
        "MATCH (a:N), (b:N), (c:N) RETURN a, b, c",
        "UNWIND range(1, 10000000) AS i RETURN i",
        "UNWIND range(1, 1000) AS x MATCH (a:N) RETURN a.i, x",
        "MATCH (a:N)-[:R*1..3]->(b), (c:N) RETURN a.i, b.i, c.i",
        "MATCH (a:N), (b:N), (c:N) OPTIONAL MATCH (a)-[:R]->(d) RETURN a.i, d.i",
        "MATCH (a:N), (b:N), (c:N) RETURN a.i AS x UNION ALL RETURN 1 AS x",
        "MATCH (a:N), (b:N), (c:N) RETURN a.i ORDER BY a.i",
    ] {
        let started = Instant::now();
        assert_too_large(&db, query, 1000);
        // Generous for slow debug CI; building the whole result takes
        // minutes at least.
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "`{query}` took {:?}",
            started.elapsed()
        );
    }
}

#[test]
fn large_intermediates_that_shrink_are_not_capped() {
    // 300 nodes: every cartesian product below has 90,000 rows, far over
    // the cap of 300 that each result fits in.
    let db = seeded(300);
    for (query, expected) in [
        ("MATCH (a:N), (b:N) RETURN count(*) AS c", 1),
        ("MATCH (a:N), (b:N) WHERE a.i = b.i RETURN a.i", 300),
        ("MATCH (a:N), (b:N) RETURN DISTINCT a.i % 3 AS m", 3),
        ("MATCH (a:N), (b:N) RETURN a.i ORDER BY a.i, b.i LIMIT 5", 5),
        ("MATCH (a:N), (b:N) WITH a, b LIMIT 7 RETURN a.i", 7),
        (
            "UNWIND range(1, 100000) AS i WITH i WHERE i % 50000 = 0 RETURN i",
            2,
        ),
        (
            "MATCH (a:N)-[:R]->(b)-[:R]->(c) WHERE a.i < 3 RETURN c.i",
            2,
        ),
        (
            "MATCH (a:N) OPTIONAL MATCH (a)-[:R]->(b) WHERE b.i < 3 RETURN b.i",
            300,
        ),
        (
            "MATCH (a:N), (b:N) RETURN a.i AS x UNION MATCH (b:N) RETURN b.i AS x",
            300,
        ),
    ] {
        assert_eq!(rows(capped(&db, query, 300).unwrap()), expected, "{query}");
    }
    // The inner pattern of an OPTIONAL MATCH is not part of the result.
    assert_eq!(
        rows(
            capped(
                &db,
                "MATCH (a:N) WHERE a.i <= 2 OPTIONAL MATCH (b:N) WHERE b.i % 150 = a.i RETURN a.i, b.i",
                10,
            )
            .unwrap()
        ),
        4
    );
}

#[test]
fn bounded_reads_on_the_pull_path_are_capped() {
    let db = seeded(100);
    assert_eq!(
        rows(capped(&db, "MATCH (n:N) RETURN n.i LIMIT 10", 10).unwrap()),
        10
    );
    assert_too_large(&db, "MATCH (n:N) RETURN n.i LIMIT 11", 10);
}

#[test]
fn a_write_over_the_cap_rolls_back() {
    let db = seeded(300);
    assert_too_large(
        &db,
        "UNWIND range(1, 5000) AS i CREATE (w:W {i: i}) RETURN w",
        100,
    );
    assert_too_large(&db, "MATCH (n:N) SET n.touched = true RETURN n.i", 100);
    assert_too_large(
        &db,
        "CREATE (:W {i: 0}) WITH 1 AS one MATCH (a:N), (b:N) RETURN a.i, b.i",
        100,
    );
    assert_eq!(count_of(&db, "MATCH (w:W) RETURN count(w) AS c"), 0);
    assert_eq!(
        count_of(
            &db,
            "MATCH (n:N) WHERE n.touched IS NOT NULL RETURN count(n) AS c"
        ),
        0
    );

    // A write that returns nothing is not capped, however much it writes.
    assert_eq!(
        rows(capped(&db, "UNWIND range(1, 5000) AS i CREATE (:W {i: i})", 100).unwrap()),
        0
    );
    assert_eq!(count_of(&db, "MATCH (w:W) RETURN count(w) AS c"), 5000);
}
