//! A read with a `LIMIT` stops scanning once the limit is met, also when it
//! runs under a deadline or inside an explicit transaction (E13).
//!
//! Before the fix only auto-commit reads without a deadline used the pull
//! pipeline; a bounded read or any read in `begin_transaction()` ran the
//! full executor, which materializes every row before `LIMIT` trims them.

mod test_helpers;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use lora_database::{ExecuteOptions, LoraErrorCode, ResultFormat, TransactionMode};
use test_helpers::TestDb;

const N: usize = 100_000;
const READ: &str = "MATCH (n:Item) RETURN n.i AS i LIMIT 10";

fn rows() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run(&format!(
        "UNWIND range(1, {N}) AS i CREATE (:Item {{i: i}})"
    ));
    db
}

/// Average time of `f` over a few runs, after one warm-up call.
fn avg(mut f: impl FnMut() -> usize) -> Duration {
    assert_eq!(f(), 10);
    let runs = 20;
    let start = Instant::now();
    for _ in 0..runs {
        assert_eq!(f(), 10);
    }
    start.elapsed() / runs
}

/// The time an unbounded read over the whole label takes, as a yardstick:
/// an early-stopping read must be far below it.
fn full_scan(db: &TestDb) -> Duration {
    let start = Instant::now();
    let rows = db
        .service
        .execute_rows("MATCH (n:Item) RETURN n.i AS i")
        .unwrap();
    assert_eq!(rows.len(), N);
    start.elapsed()
}

fn assert_early(label: &str, took: Duration, full: Duration) {
    eprintln!("{label}: {took:?} per read (full scan {full:?})");
    assert!(
        took * 20 < full,
        "{label} did not stop at LIMIT: {took:?} per read, full scan {full:?}"
    );
}

#[test]
fn bounded_autocommit_read_stops_at_limit() {
    let db = seeded();
    let full = full_scan(&db);
    let took = avg(|| {
        db.service
            .execute_with_params_timeout(READ, rows(), BTreeMap::new(), Duration::from_secs(30))
            .map(|r| {
                serde_json::to_value(r).unwrap()["rows"]
                    .as_array()
                    .unwrap()
                    .len()
            })
            .unwrap()
    });
    assert_early("auto-commit with timeout", took, full);
}

#[test]
fn read_in_read_write_transaction_stops_at_limit() {
    let db = seeded();
    let full = full_scan(&db);
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadWrite)
        .unwrap();
    let took = avg(|| tx.execute_rows(READ).unwrap().len());
    assert_early("read-write transaction", took, full);

    // After a write the read runs on the staged graph and sees the write.
    tx.execute_rows("CREATE (:Item {i: 0})").unwrap();
    let took = avg(|| tx.execute_rows(READ).unwrap().len());
    assert_early("read-write transaction after a write", took, full);
    let rows = tx
        .execute_rows("MATCH (n:Item) WHERE n.i = 0 RETURN n LIMIT 1")
        .unwrap();
    assert_eq!(rows.len(), 1);
    tx.rollback().unwrap();
}

#[test]
fn read_in_read_only_transaction_stops_at_limit() {
    let db = seeded();
    let full = full_scan(&db);
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadOnly)
        .unwrap();
    let took = avg(|| tx.execute_rows(READ).unwrap().len());
    assert_early("read-only transaction", took, full);
    let took = avg(|| {
        let r = tx
            .execute_with_timeout(READ, rows(), Duration::from_secs(30))
            .unwrap();
        serde_json::to_value(r).unwrap()["rows"]
            .as_array()
            .unwrap()
            .len()
    });
    assert_early("read-only transaction with timeout", took, full);
}

#[test]
fn bounded_limit_read_still_times_out() {
    let db = seeded();
    // A cartesian filter that never yields: LIMIT cannot help, the
    // deadline has to stop it from inside the pull pipeline.
    let q = "MATCH (a:Item), (b:Item) WHERE a.i = -1 RETURN a LIMIT 1";
    let start = Instant::now();
    let err = db
        .service
        .execute_with_params_timeout(q, None, BTreeMap::new(), Duration::from_millis(50))
        .expect_err("query should time out");
    assert_eq!(err.code(), LoraErrorCode::Timeout, "{err}");
    assert!(start.elapsed() < Duration::from_secs(2));

    // Zero timeout fails before any work.
    let err = db
        .service
        .execute_with_params_timeout(READ, None, BTreeMap::new(), Duration::ZERO)
        .expect_err("zero timeout");
    assert_eq!(err.code(), LoraErrorCode::Timeout, "{err}");
}

#[test]
fn bounded_limit_read_in_transaction_times_out_and_keeps_the_transaction() {
    let db = seeded();
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadWrite)
        .unwrap();
    tx.execute_rows("CREATE (:Marker)").unwrap();
    let q = "MATCH (a:Item), (b:Item) WHERE a.i = -1 RETURN a LIMIT 1";
    let start = Instant::now();
    let err = tx
        .execute_with_timeout(q, None, Duration::from_millis(50))
        .expect_err("query should time out");
    assert_eq!(err.code(), LoraErrorCode::Timeout, "{err}");
    assert!(start.elapsed() < Duration::from_secs(2));
    // A failed read leaves the earlier write in place.
    assert_eq!(
        tx.execute_rows("MATCH (m:Marker) RETURN m LIMIT 5")
            .unwrap()
            .len(),
        1
    );
    tx.commit().unwrap();
    db.assert_count("MATCH (m:Marker) RETURN m", 1);
}
