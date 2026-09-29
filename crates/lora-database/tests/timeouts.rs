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
