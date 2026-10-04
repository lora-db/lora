//! Query deadlines and cancellation reach every execution path.
//!
//! Regression coverage for two gaps: writes whose input runs on the pull
//! pipeline never checked their deadline, and a bounded write that timed
//! out on the in-place fast path could leave a partial write behind.

mod test_helpers;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use lora_database::{CancellableDeadline, LoraErrorCode};
use test_helpers::TestDb;

const N: usize = 20_000;

fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run(&format!("UNWIND range(1, {N}) AS i CREATE (:A {{i: i}})"));
    db.run(&format!("UNWIND range(1, {N}) AS i CREATE (:B {{i: i}})"));
    db
}

fn timed_out(db: &TestDb, query: &str, timeout: Duration) -> Duration {
    let start = Instant::now();
    let err = db
        .service
        .execute_with_params_timeout(query, None, BTreeMap::new(), timeout)
        .expect_err("query should time out");
    assert_eq!(err.code(), LoraErrorCode::Timeout, "{err}");
    start.elapsed()
}

#[test]
fn streaming_write_honours_its_deadline_and_leaves_nothing() {
    let db = seeded();
    // CREATE over a 4e8-row cartesian product runs on the pull pipeline.
    let took = timed_out(
        &db,
        "MATCH (a:A), (b:B) CREATE (:Pair)",
        Duration::from_millis(50),
    );
    assert!(took < Duration::from_secs(2), "took {took:?}");
    db.assert_count("MATCH (p:Pair) RETURN p", 0);
    // The writer lock was released.
    db.run("CREATE (:After)");
    db.assert_count("MATCH (x:After) RETURN x", 1);
}

#[test]
fn filter_that_never_yields_still_times_out() {
    let db = seeded();
    // The condition reads both variables, so it cannot be pushed down to
    // either scan (a lone `a.i = -1` now becomes an empty index seek) and
    // the filter really does reject all 4e8 rows of the product.
    let took = timed_out(
        &db,
        "MATCH (a:A), (b:B) WHERE a.i + b.i = -1 SET a.hit = true",
        Duration::from_millis(50),
    );
    assert!(took < Duration::from_secs(2), "took {took:?}");
}

#[test]
fn cancellation_stops_a_running_query() {
    let db = std::sync::Arc::new(seeded());
    let mut handle = CancellableDeadline::new(None);
    let deadline = handle.deadline();
    let shared = db.clone();
    let worker = std::thread::spawn(move || {
        let service = &shared.service;
        service.execute_with_params_deadline(
            "MATCH (a:A), (b:B) RETURN count(*) AS c",
            None,
            BTreeMap::new(),
            deadline,
        )
    });
    std::thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    handle.cancel();
    let err = worker
        .join()
        .unwrap()
        .expect_err("cancelled query must fail");
    assert_eq!(err.code(), LoraErrorCode::Timeout);
    assert!(start.elapsed() < Duration::from_secs(2));
}

// ---------------------------------------------------------------------------
// Deadlines inside expression evaluation
// ---------------------------------------------------------------------------
//
// `@loradb/lora-graphql` compiles relationship-filter quantifiers to
// `size([(a)-[:R]->(b) WHERE <pred> | 1]) > 0`, nested for nested filters.
// Those comprehensions run inside one expression evaluation, below the
// operator boundaries the pipeline checks, so a three-level nested filter
// once ran 15 s past a 1 s deadline and returned rows.

/// ~5k festivals, ~2k users, ~23k user->festival and ~4k user->user
/// FOLLOWS edges, spread deterministically.
fn follows_graph() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE INDEX FOR (n:Festival) ON (n.i)");
    db.run("CREATE INDEX FOR (n:User) ON (n.i)");
    db.run("UNWIND range(0, 4999) AS i CREATE (:Festival {i: i, key: 'f' + toString(i)})");
    db.run(
        "UNWIND range(0, 1999) AS i \
         CREATE (:User {i: i, key: 'u' + toString(i), name: 'user ' + toString(i)})",
    );
    db.run(
        "UNWIND range(0, 22999) AS i \
         MATCH (u:User {i: (i * 7919) % 2000}), (f:Festival {i: (i * 104729) % 5000}) \
         CREATE (u)-[:FOLLOWS]->(f)",
    );
    db.run(
        "UNWIND range(0, 3999) AS i \
         MATCH (a:User {i: (i * 7919) % 2000}), (b:User {i: (i * 6007 + 1) % 2000}) \
         CREATE (a)-[:FOLLOWS]->(b)",
    );
    db
}

/// No user name contains 'zz', so every level of the nested filter runs
/// to completion: about a second of work in a release build.
const NESTED_FILTER: &str = "MATCH (f:Festival) \
     WHERE size([(f)<-[:FOLLOWS]-(u:User) \
       WHERE size([(u)-[:FOLLOWS]->(g:Festival) \
         WHERE size([(g)<-[:FOLLOWS]-(w:User) WHERE w.name CONTAINS 'zz' | 1]) > 0 \
       | 1]) > 0 \
     | 1]) > 0 \
     RETURN f.key";

#[test]
fn nested_pattern_comprehension_honours_its_deadline() {
    let db = follows_graph();
    let timeout = Duration::from_millis(150);
    // Without a LIMIT the read runs on the buffered executor, whose filter
    // loop never checked the deadline; with one it streams. Both must stop.
    for query in [
        NESTED_FILTER.to_string(),
        format!("{NESTED_FILTER} ORDER BY f.key LIMIT 10"),
        format!("{NESTED_FILTER} LIMIT 1"),
    ] {
        let took = timed_out(&db, &query, timeout);
        assert!(
            took < timeout + Duration::from_millis(150),
            "{query}: took {took:?} for a {timeout:?} deadline"
        );
    }
}

#[test]
fn nested_pattern_comprehension_stops_on_cancel() {
    let db = std::sync::Arc::new(follows_graph());
    let mut handle = CancellableDeadline::new(None);
    let deadline = handle.deadline();
    let shared = db.clone();
    let worker = std::thread::spawn(move || {
        shared
            .service
            .execute_with_params_deadline(NESTED_FILTER, None, BTreeMap::new(), deadline)
    });
    std::thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    handle.cancel();
    let err = worker
        .join()
        .unwrap()
        .expect_err("cancelled query must fail");
    assert_eq!(err.code(), LoraErrorCode::Timeout);
    assert!(
        start.elapsed() < Duration::from_millis(150),
        "stopped {:?} after cancel",
        start.elapsed()
    );
}

#[test]
fn list_comprehension_reduce_and_quantifiers_honour_their_deadline() {
    let db = TestDb::new();
    let timeout = Duration::from_millis(200);
    for query in [
        // One row, one expression: no operator boundary inside the work.
        "RETURN size([x IN range(1, 3000) WHERE size([y IN range(1, 3000) WHERE x * y = -1]) > 0]) AS n",
        "RETURN reduce(acc = 0, x IN range(1, 3000) | acc + size([y IN range(1, 3000) WHERE x = y])) AS n",
        "RETURN any(x IN range(1, 3000) WHERE any(y IN range(1, 3000) WHERE x * y = -1)) AS n",
        "RETURN all(x IN range(1, 3000) WHERE none(y IN range(1, 3000) WHERE x * y = -1)) AS n",
    ] {
        let took = timed_out(&db, query, timeout);
        assert!(
            took < timeout + Duration::from_millis(150),
            "{query}: took {took:?} for a {timeout:?} deadline"
        );
    }
}

#[test]
fn cartesian_match_stops_close_to_its_deadline() {
    let db = seeded();
    let timeout = Duration::from_millis(300);
    let took = timed_out(
        &db,
        "MATCH (a:A), (b:B) WHERE a.i + b.i = -1 RETURN a.i",
        timeout,
    );
    assert!(
        took < timeout + Duration::from_millis(150),
        "took {took:?} for a {timeout:?} deadline"
    );
}
