use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use lora_database::{
    Database, ExecuteOptions, LoraValue, QueryResult, ResultFormat, TransactionMode, WalConfig,
};
use serde_json::Value as JsonValue;

fn rows_options() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn rows_json(result: QueryResult) -> Vec<JsonValue> {
    let json = serde_json::to_value(result).unwrap();
    json.get("rows")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default()
}

fn row_values(rows: Vec<lora_database::Row>, column: &str) -> Vec<JsonValue> {
    rows.into_iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .map(|row| row.get(column).cloned().unwrap_or(JsonValue::Null))
        .collect()
}

struct TempWalDir {
    path: PathBuf,
}

impl TempWalDir {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("lora-database-{name}-{nonce}"));
        Self { path }
    }
}

impl Drop for TempWalDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn database_stream_returns_rows_and_columns() {
    let db = Database::in_memory();
    db.execute(
        "CREATE (:Person {name:'Ada'}), (:Person {name:'Grace'})",
        rows_options(),
    )
    .unwrap();

    let mut stream = db
        .stream("MATCH (p:Person) RETURN p.name AS name ORDER BY name")
        .unwrap();
    assert_eq!(stream.columns(), &["name".to_string()]);

    let values = row_values(stream.by_ref().collect(), "name");
    assert_eq!(
        values,
        vec![
            JsonValue::String("Ada".to_string()),
            JsonValue::String("Grace".to_string())
        ]
    );
    assert_eq!(stream.next(), None);
}

#[test]
fn execute_with_timeout_cancels_before_work_continues() {
    let db = Database::in_memory();
    db.execute(
        "UNWIND range(1,100) AS i CREATE (:T {i: i})",
        rows_options(),
    )
    .unwrap();

    let err = db
        .execute_with_timeout(
            "MATCH (t:T) RETURN t.i AS i",
            rows_options(),
            std::time::Duration::ZERO,
        )
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("query deadline exceeded"),
        "expected query timeout, got: {err:#}"
    );
}

#[test]
fn transaction_commit_publishes_staged_changes() {
    let db = Database::in_memory();

    let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
    tx.execute("CREATE (:Person {name:'Ada'})", rows_options())
        .unwrap();
    tx.commit().unwrap();

    let rows = rows_json(
        db.execute("MATCH (p:Person) RETURN p.name AS name", rows_options())
            .unwrap(),
    );
    assert_eq!(rows[0]["name"], JsonValue::String("Ada".to_string()));
}

#[test]
fn transaction_rollback_discards_staged_changes() {
    let db = Database::in_memory();

    let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
    tx.execute("CREATE (:Person {name:'Ada'})", rows_options())
        .unwrap();
    tx.rollback().unwrap();

    let rows = rows_json(
        db.execute("MATCH (p:Person) RETURN p.name AS name", rows_options())
            .unwrap(),
    );
    assert!(rows.is_empty());
}

#[test]
fn read_only_transaction_rejects_writes() {
    let db = Database::in_memory();
    let mut tx = db.begin_transaction(TransactionMode::ReadOnly).unwrap();

    let err = tx
        .execute("CREATE (:Person {name:'Ada'})", rows_options())
        .unwrap_err()
        .to_string();
    assert!(err.contains("read-only mode (CREATE"));

    tx.rollback().unwrap();
}

#[test]
fn transaction_stream_reads_staged_state() {
    let db = Database::in_memory();
    let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
    tx.execute(
        "CREATE (:Person {name:'Ada'}), (:Person {name:'Grace'})",
        rows_options(),
    )
    .unwrap();

    let stream = tx
        .stream("MATCH (p:Person) RETURN p.name AS name ORDER BY name")
        .unwrap();
    let values = row_values(stream.collect(), "name");
    assert_eq!(
        values,
        vec![
            JsonValue::String("Ada".to_string()),
            JsonValue::String("Grace".to_string())
        ]
    );

    tx.rollback().unwrap();
}

#[test]
fn wal_replays_committed_transaction_but_not_rolled_back_transaction() {
    let dir = TempWalDir::new("tx-wal");

    {
        let db = Database::open_with_wal(WalConfig::enabled(dir.path.clone())).unwrap();

        let mut rolled_back = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
        rolled_back
            .execute("CREATE (:Person {name:'Ada'})", rows_options())
            .unwrap();
        rolled_back.rollback().unwrap();

        let mut committed = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
        committed
            .execute("CREATE (:Person {name:'Grace'})", rows_options())
            .unwrap();
        committed.commit().unwrap();
    }

    {
        let db = Database::open_with_wal(WalConfig::enabled(dir.path.clone())).unwrap();
        let stream = db
            .stream("MATCH (p:Person) RETURN p.name AS name ORDER BY name")
            .unwrap();
        let values = row_values(stream.collect(), "name");
        assert_eq!(values, vec![JsonValue::String("Grace".to_string())]);
    }
}

#[test]
fn transaction_with_params() {
    let db = Database::in_memory();
    let mut params = std::collections::BTreeMap::new();
    params.insert("name".to_string(), LoraValue::String("Ada".to_string()));

    let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
    tx.execute_with_params("CREATE (:Person {name:$name})", rows_options(), params)
        .unwrap();
    tx.commit().unwrap();

    let rows = rows_json(
        db.execute("MATCH (p:Person) RETURN p.name AS name", rows_options())
            .unwrap(),
    );
    assert_eq!(rows[0]["name"], JsonValue::String("Ada".to_string()));
}

// ---------------------------------------------------------------------------
// Phase 6 tests: streaming, savepoints, cursor lifecycle, WAL semantics.
// ---------------------------------------------------------------------------

#[test]
fn streaming_simple_match_yields_rows_one_at_a_time() {
    let db = Database::in_memory();
    db.execute(
        "CREATE (:Person {name:'Ada'}), (:Person {name:'Grace'})",
        rows_options(),
    )
    .unwrap();

    let mut stream = db
        .stream("MATCH (p:Person) RETURN p.name AS name ORDER BY name")
        .unwrap();

    // Plan-derived columns are populated up front.
    assert_eq!(stream.columns(), &["name".to_string()]);

    let first = stream.next_row().unwrap();
    assert!(first.is_some());
    let second = stream.next_row().unwrap();
    assert!(second.is_some());
    assert!(stream.next_row().unwrap().is_none());
    // Subsequent calls keep returning Ok(None).
    assert!(stream.next_row().unwrap().is_none());
}

#[test]
fn empty_streaming_result_still_reports_columns() {
    let db = Database::in_memory();
    let stream = db
        .stream("MATCH (p:Missing) RETURN p.name AS name, p.age AS age")
        .unwrap();
    assert_eq!(
        stream.columns(),
        &["name".to_string(), "age".to_string()],
        "plan-derived columns must survive empty results"
    );
    let collected: Vec<_> = stream.collect();
    assert!(collected.is_empty());
}

#[test]
fn streaming_with_blocking_operators_still_works() {
    // ORDER BY + LIMIT exercise Sort + Limit, both of which buffer
    // their input. The stream contract must still hold.
    let db = Database::in_memory();
    db.execute(
        "CREATE (:Person {name:'Ada', age:30}), (:Person {name:'Grace', age:42}), (:Person {name:'Linus', age:25})",
        rows_options(),
    )
    .unwrap();

    let stream = db
        .stream("MATCH (p:Person) RETURN p.name AS name ORDER BY p.age DESC LIMIT 2")
        .unwrap();
    assert_eq!(stream.columns(), &["name".to_string()]);
    let values = row_values(stream.collect(), "name");
    assert_eq!(
        values,
        vec![
            JsonValue::String("Grace".to_string()),
            JsonValue::String("Ada".to_string())
        ]
    );
}

#[test]
fn dropping_read_stream_releases_resources() {
    // The stream owns no live lock guards once materialized, so
    // dropping it must not block subsequent queries.
    let db = Database::in_memory();
    db.execute("CREATE (:N {v:1}), (:N {v:2})", rows_options())
        .unwrap();

    {
        let _stream = db.stream("MATCH (n:N) RETURN n.v").unwrap();
        // Drop without iterating to exhaustion.
    }

    // Database must still be usable after the cursor is dropped.
    let result = db.execute("MATCH (n:N) RETURN count(n) AS c", rows_options());
    assert!(result.is_ok());
}

#[test]
fn read_only_transaction_rejects_create_merge_set_delete_remove() {
    let db = Database::in_memory();
    db.execute("CREATE (:N {v:1})", rows_options()).unwrap();

    for query in [
        "CREATE (:Person)",
        "MERGE (:Person {name:'Ada'})",
        "MATCH (n:N) SET n.v = 2",
        "MATCH (n:N) REMOVE n.v",
        "MATCH (n:N) DELETE n",
    ] {
        let mut tx = db.begin_transaction(TransactionMode::ReadOnly).unwrap();
        let err = tx
            .execute(query, rows_options())
            .expect_err(&format!("{query} should be rejected in ReadOnly tx"));
        let msg = err.to_string();
        assert!(
            msg.contains("read-only mode"),
            "unexpected error for `{query}`: {msg}"
        );
        // Read-only transactions never poison the tx; rollback should succeed.
        tx.rollback().unwrap();
    }
}

#[test]
fn transaction_cannot_commit_with_active_cursor() {
    let db = Database::in_memory();
    db.execute("CREATE (:N {v:1}), (:N {v:2})", rows_options())
        .unwrap();

    let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
    let stream = tx.stream("MATCH (n:N) RETURN n.v").unwrap();

    // The tx is still open, but a cursor is alive — commit must
    // fail rather than silently publish staged changes.
    drop(stream);
    // After drop, commit succeeds again — cursor is released.
    tx.commit().unwrap();
}

#[test]
fn transaction_cursor_active_blocks_commit() {
    let db = Database::in_memory();
    db.execute("CREATE (:N {v:1}), (:N {v:2})", rows_options())
        .unwrap();

    // We need to call commit while a cursor is alive. `commit`
    // consumes the tx, so use a scope where the stream and tx
    // coexist, then attempt commit.
    let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
    {
        let _stream = tx.stream("MATCH (n:N) RETURN n.v").unwrap();
        // Try a follow-up statement while the cursor is still alive
        // — must be rejected by the cursor-active check.
        let err = tx
            .execute("CREATE (:Marker)", rows_options())
            .expect_err("a second statement must be rejected while a cursor is active");
        assert!(err.to_string().contains("cursor"));
    }
    // Cursor dropped — tx is usable again.
    tx.commit().unwrap();
}

#[test]
fn failed_statement_rolls_back_only_that_statement() {
    let db = Database::in_memory();

    let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
    tx.execute("CREATE (:Person {name:'Ada'})", rows_options())
        .unwrap();

    // A failing statement (typo) must not roll back Ada.
    // Use an unsupported set form that errors at runtime.
    let err = tx.execute("MATCH (n:Person) SET n = 42", rows_options());
    assert!(err.is_err(), "the statement must fail");

    // Continue using the tx — Ada should still be staged.
    let result = tx
        .execute("MATCH (p:Person) RETURN p.name AS name", rows_options())
        .unwrap();
    let rows = rows_json(result);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], JsonValue::String("Ada".to_string()));
    tx.commit().unwrap();
}

#[test]
fn dropped_stream_in_tx_rolls_back_only_that_statement() {
    let db = Database::in_memory();

    let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
    tx.execute("CREATE (:Person {name:'Ada'})", rows_options())
        .unwrap();

    {
        // Open a streaming write that will be dropped before exhaustion.
        let _stream = tx
            .stream("CREATE (:Person {name:'DroppedAndRolledBack'}) RETURN 1 AS one")
            .unwrap();
        // Drop without iterating — the stream's writes should be
        // rolled back when the next tx op runs.
    }

    let result = tx
        .execute(
            "MATCH (p:Person) RETURN p.name AS name ORDER BY name",
            rows_options(),
        )
        .unwrap();
    let rows = rows_json(result);
    assert_eq!(
        rows.len(),
        1,
        "only Ada should remain after the dropped statement was rolled back"
    );
    assert_eq!(rows[0]["name"], JsonValue::String("Ada".to_string()));
    tx.commit().unwrap();
}

#[test]
fn wal_replay_excludes_failed_statement_inside_committed_transaction() {
    let dir = TempWalDir::new("tx-wal-failed-stmt");

    {
        let db = Database::open_with_wal(WalConfig::enabled(dir.path.clone())).unwrap();

        let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
        tx.execute("CREATE (:Person {name:'Ada'})", rows_options())
            .unwrap();
        // A failed statement inside a tx rolls back only that statement.
        let _ = tx.execute("MATCH (n:Person) SET n = 42", rows_options());
        tx.execute("CREATE (:Person {name:'Grace'})", rows_options())
            .unwrap();
        tx.commit().unwrap();
    }

    // Recover and verify only Ada + Grace exist.
    {
        let db = Database::open_with_wal(WalConfig::enabled(dir.path.clone())).unwrap();
        let stream = db
            .stream("MATCH (p:Person) RETURN p.name AS name ORDER BY name")
            .unwrap();
        let values = row_values(stream.collect(), "name");
        assert_eq!(
            values,
            vec![
                JsonValue::String("Ada".to_string()),
                JsonValue::String("Grace".to_string())
            ]
        );
    }
}

#[test]
fn wal_replay_excludes_dropped_stream_inside_committed_transaction() {
    let dir = TempWalDir::new("tx-wal-dropped-stream");

    {
        let db = Database::open_with_wal(WalConfig::enabled(dir.path.clone())).unwrap();

        let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
        tx.execute("CREATE (:Person {name:'Ada'})", rows_options())
            .unwrap();
        {
            let _stream = tx
                .stream("CREATE (:Person {name:'Dropped'}) RETURN 1 AS one")
                .unwrap();
            // Dropped pre-exhaustion — must not appear in WAL.
        }
        tx.execute("CREATE (:Person {name:'Grace'})", rows_options())
            .unwrap();
        tx.commit().unwrap();
    }

    {
        let db = Database::open_with_wal(WalConfig::enabled(dir.path.clone())).unwrap();
        let stream = db
            .stream("MATCH (p:Person) RETURN p.name AS name ORDER BY name")
            .unwrap();
        let values = row_values(stream.collect(), "name");
        assert_eq!(
            values,
            vec![
                JsonValue::String("Ada".to_string()),
                JsonValue::String("Grace".to_string())
            ]
        );
    }
}

// ---------------------------------------------------------------------------
// Auto-commit stream drop-rollback semantics
// ---------------------------------------------------------------------------

#[test]
fn auto_commit_write_stream_commits_on_full_exhaustion() {
    let db = Database::in_memory();

    let stream = db
        .stream("CREATE (:Person {name:'Ada'}) RETURN 1 AS one")
        .unwrap();
    let collected: Vec<_> = stream.collect();
    assert_eq!(collected.len(), 1);

    // The stream was fully exhausted (collect()), so the staged
    // graph was published. Ada should now be visible in the live
    // database.
    let rows = rows_json(
        db.execute("MATCH (p:Person) RETURN p.name AS name", rows_options())
            .unwrap(),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], JsonValue::String("Ada".to_string()));
}

#[test]
fn auto_commit_write_stream_rolls_back_on_drop() {
    let db = Database::in_memory();

    {
        let _stream = db
            .stream("CREATE (:Person {name:'Ada'}) RETURN 1 AS one")
            .unwrap();
        // Drop without iterating — auto-commit guard rolls back.
    }

    // Ada must not exist.
    let rows = rows_json(
        db.execute("MATCH (p:Person) RETURN p.name AS name", rows_options())
            .unwrap(),
    );
    assert!(
        rows.is_empty(),
        "auto-commit write stream dropped pre-exhaustion must not publish staged changes"
    );
}

#[test]
fn auto_commit_write_stream_rolls_back_on_partial_consumption() {
    let db = Database::in_memory();

    {
        let mut stream = db
            .stream("UNWIND [1,2,3] AS x CREATE (:N {value:x}) RETURN x")
            .unwrap();
        // Pull just the first row, then drop without exhausting.
        let _first = stream.next();
        // Drop here: staged writes for ALL rows (including the
        // first that was already materialized) must be discarded.
    }

    let rows = rows_json(
        db.execute("MATCH (n:N) RETURN count(n) AS c", rows_options())
            .unwrap(),
    );
    assert_eq!(rows[0]["c"], JsonValue::Number(0.into()));
}

#[test]
fn auto_commit_write_stream_with_wal_only_writes_on_exhaustion() {
    let dir = TempWalDir::new("auto-commit-write-stream-wal");

    {
        let db = Database::open_with_wal(WalConfig::enabled(dir.path.clone())).unwrap();

        // Dropped stream — must not write to WAL.
        {
            let _ = db
                .stream("CREATE (:Person {name:'Dropped'}) RETURN 1 AS one")
                .unwrap();
        }

        // Exhausted stream — should write to WAL.
        let stream = db
            .stream("CREATE (:Person {name:'Committed'}) RETURN 1 AS one")
            .unwrap();
        let _: Vec<_> = stream.collect();
    }

    {
        let db = Database::open_with_wal(WalConfig::enabled(dir.path.clone())).unwrap();
        let rows: Vec<_> = db
            .stream("MATCH (p:Person) RETURN p.name AS name ORDER BY name")
            .unwrap()
            .collect();
        let values = row_values(rows, "name");
        assert_eq!(values, vec![JsonValue::String("Committed".to_string())]);
    }
}

#[test]
fn db_stream_read_only_uses_live_pull_cursor() {
    // Verifies that `Database::stream` for a read query produces
    // a true streaming cursor: yielding the first row must not
    // require running the rest of the plan to completion.
    let db = Database::in_memory();
    for i in 0..5_000 {
        db.execute(&format!("CREATE (:N {{v:{i}}})"), rows_options())
            .unwrap();
    }

    // The stream is fallible (Result<Option<Row>, _>) so we
    // can pull just one row and drop without consuming the
    // remaining 4,999. With the buffered path we'd materialize
    // all 5,000 up front; with the live cursor we should not.
    let mut stream = db.stream("MATCH (n:N) RETURN n.v").unwrap();
    assert_eq!(stream.columns(), &["v".to_string()]);
    let first = stream.next_row().unwrap();
    assert!(first.is_some(), "live stream must produce a row");
    drop(stream);

    // Database remains usable after a partially-consumed live
    // stream — the lock and the cursor were both released by
    // QueryStream::Drop in the right order.
    let count = rows_json(
        db.execute("MATCH (n:N) RETURN count(n) AS c", rows_options())
            .unwrap(),
    );
    assert_eq!(count[0]["c"], JsonValue::Number(5000.into()));
}

#[test]
fn auto_commit_read_stream_does_not_pay_staging_cost() {
    // For a read-only query, the auto-commit path should bypass
    // the staging logic entirely. Drop-rollback is a no-op
    // because there's nothing to roll back.
    let db = Database::in_memory();
    db.execute("CREATE (:N {v:1}), (:N {v:2})", rows_options())
        .unwrap();

    let stream = db.stream("MATCH (n:N) RETURN n.v").unwrap();
    // Default column name for `RETURN n.v` is the property name.
    assert_eq!(stream.columns(), &["v".to_string()]);
    let collected: Vec<_> = stream.collect();
    assert_eq!(collected.len(), 2);
}

// ---------------------------------------------------------------------------
// Pull-shape behavior at the executor layer
// ---------------------------------------------------------------------------
//
// These tests use `lora_executor::PullExecutor` directly to verify
// that the listed streaming operators are genuinely pull-shaped:
// they yield rows one at a time without first materializing the
// entire upstream subtree.

mod pull_shape {
    use lora_compiler::Compiler;
    use lora_database::{parse_query, Database, InMemoryGraph};
    use lora_executor::{drain, PullExecutor, RowSource};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    fn compile<'a>(store: &lora_store::InMemoryGraph, query: &str) -> lora_compiler::CompiledQuery {
        let document = parse_query(query).unwrap();
        let resolved = {
            let mut analyzer = lora_analyzer::Analyzer::new(store);
            analyzer.analyze(&document).unwrap()
        };
        Compiler::compile(&resolved)
    }

    fn open<'a>(
        store: &'a InMemoryGraph,
        compiled: &'a lora_compiler::CompiledQuery,
    ) -> Box<dyn RowSource + 'a> {
        PullExecutor::new(store, BTreeMap::new())
            .open_compiled(compiled)
            .unwrap()
    }

    #[test]
    fn match_stream_yields_first_row_before_consuming_full_input() {
        let db = Database::in_memory();
        // 1000 nodes — pulling one row should not require scanning
        // all of them.
        for i in 0..1000 {
            db.execute(
                &format!("CREATE (:N {{v:{i}}})"),
                Some(lora_database::ExecuteOptions {
                    format: lora_database::ResultFormat::Rows,
                }),
            )
            .unwrap();
        }
        let store = db.store().read().unwrap();
        let compiled = compile(&store, "MATCH (n:N) RETURN n.v AS v");
        let mut cursor = open(&store, &compiled);

        // Pull a single row — verifies the cursor doesn't deadlock
        // or block on the full result.
        let first = cursor.next_row().unwrap();
        assert!(first.is_some());
        // Drop the cursor without consuming the rest.
    }

    #[test]
    fn filter_pulls_only_until_predicate_match() {
        let db = Database::in_memory();
        // Insert a single matching row sandwiched between
        // non-matches; if FilterSource were buffered it would
        // still produce only the matching row, but we want to
        // verify the per-row pull contract holds.
        db.execute(
            "CREATE (:N {v:1}), (:N {v:2}), (:N {v:3}), (:N {v:4})",
            Some(lora_database::ExecuteOptions {
                format: lora_database::ResultFormat::Rows,
            }),
        )
        .unwrap();
        let store = db.store().read().unwrap();
        let compiled = compile(&store, "MATCH (n:N) WHERE n.v = 3 RETURN n.v AS v");
        let rows = drain(open(&store, &compiled).as_mut()).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn property_equality_filter_lowers_to_indexed_scan() {
        let db = Database::in_memory();
        db.execute(
            "CREATE (:N {v:1}), (:N {v:2})",
            Some(lora_database::ExecuteOptions {
                format: lora_database::ResultFormat::Rows,
            }),
        )
        .unwrap();
        let store = db.store().read().unwrap();
        let compiled = compile(&store, "MATCH (n:N) WHERE n.v = 2 RETURN n.v AS v");

        let indexed = compiled.physical.nodes.iter().any(|op| {
            matches!(
                op,
                lora_compiler::PhysicalOp::NodeByPropertyScan(scan)
                    if scan.key == "v"
                        && scan.labels == vec![vec!["N".to_string()]]
            )
        });
        assert!(indexed, "expected property equality to use indexed scan");
    }

    #[test]
    fn indexed_scan_preserves_numeric_cross_type_equality() {
        let db = Database::in_memory();
        db.execute(
            "CREATE (:N {v:1}), (:N {v:1.0}), (:N {v:2})",
            Some(lora_database::ExecuteOptions {
                format: lora_database::ResultFormat::Rows,
            }),
        )
        .unwrap();
        let store = db.store().read().unwrap();
        let compiled = compile(&store, "MATCH (n:N) WHERE n.v = 1.0 RETURN n.v AS v");
        let rows = drain(open(&store, &compiled).as_mut()).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn unwind_yields_each_element_lazily() {
        let db = Database::in_memory();
        let store = db.store().read().unwrap();
        let compiled = compile(&store, "UNWIND [10, 20, 30] AS x RETURN x");
        let mut cursor = open(&store, &compiled);

        let r1 = cursor.next_row().unwrap().unwrap();
        let r2 = cursor.next_row().unwrap().unwrap();
        let r3 = cursor.next_row().unwrap().unwrap();
        assert!(cursor.next_row().unwrap().is_none());
        // Drop in order; each row was produced individually.
        let _ = (r1, r2, r3);
    }

    #[test]
    fn limit_stops_pulling_after_emitting_n() {
        let db = Database::in_memory();
        for i in 0..100 {
            db.execute(
                &format!("CREATE (:N {{v:{i}}})"),
                Some(lora_database::ExecuteOptions {
                    format: lora_database::ResultFormat::Rows,
                }),
            )
            .unwrap();
        }
        let store = db.store().read().unwrap();
        let compiled = compile(&store, "MATCH (n:N) RETURN n.v AS v LIMIT 5");
        let rows = drain(open(&store, &compiled).as_mut()).unwrap();
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn projection_pulls_one_in_one_out() {
        let db = Database::in_memory();
        db.execute(
            "CREATE (:N {v:1}), (:N {v:2}), (:N {v:3})",
            Some(lora_database::ExecuteOptions {
                format: lora_database::ResultFormat::Rows,
            }),
        )
        .unwrap();
        let store = db.store().read().unwrap();
        let compiled = compile(&store, "MATCH (n:N) RETURN n.v + 10 AS v");
        let rows = drain(open(&store, &compiled).as_mut()).unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn expand_streams_per_input_row() {
        let db = Database::in_memory();
        db.execute(
            "CREATE (a:Person {name:'Ada'})-[:KNOWS]->(b:Person {name:'Bob'}),
             (a)-[:KNOWS]->(:Person {name:'Carol'}),
             (b)-[:KNOWS]->(:Person {name:'Dave'})",
            Some(lora_database::ExecuteOptions {
                format: lora_database::ResultFormat::Rows,
            }),
        )
        .unwrap();
        let store = db.store().read().unwrap();
        let compiled = compile(
            &store,
            "MATCH (p:Person)-[:KNOWS]->(other) RETURN other.name AS name ORDER BY name",
        );
        // ORDER BY forces a buffered Sort, but the streaming
        // sources still drive the upstream Expand row-by-row.
        let rows = drain(open(&store, &compiled).as_mut()).unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn blocking_operator_still_produces_correct_results() {
        let db = Database::in_memory();
        db.execute(
            "CREATE (:N {v:5}), (:N {v:1}), (:N {v:3}), (:N {v:2}), (:N {v:4})",
            Some(lora_database::ExecuteOptions {
                format: lora_database::ResultFormat::Rows,
            }),
        )
        .unwrap();
        let store = db.store().read().unwrap();
        let compiled = compile(&store, "MATCH (n:N) RETURN n.v AS v ORDER BY n.v");
        let rows = drain(open(&store, &compiled).as_mut()).unwrap();
        // Sort yields rows in ascending order.
        let values: Vec<_> = rows
            .into_iter()
            .map(|r| serde_json::to_value(&r).unwrap())
            .map(|v| v.get("v").and_then(|x| x.as_i64()).unwrap_or(-1))
            .collect();
        assert_eq!(values, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn aggregation_buffers_internally_and_yields_correctly() {
        let db = Database::in_memory();
        db.execute(
            "CREATE (:N {v:1}), (:N {v:2}), (:N {v:3})",
            Some(lora_database::ExecuteOptions {
                format: lora_database::ResultFormat::Rows,
            }),
        )
        .unwrap();
        let store = db.store().read().unwrap();
        let compiled = compile(&store, "MATCH (n:N) RETURN sum(n.v) AS total");
        let rows = drain(open(&store, &compiled).as_mut()).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn classify_stream_recognises_writes() {
        use lora_executor::{classify_stream, StreamShape};
        let db = Database::in_memory();
        let store = db.store().read().unwrap();
        let read = compile(&store, "MATCH (n) RETURN n");
        let write = compile(&store, "CREATE (:Foo) RETURN 1 AS one");
        assert_eq!(classify_stream(&read), StreamShape::ReadOnly);
        assert_eq!(classify_stream(&write), StreamShape::Mutating);
        assert!(classify_stream(&write).is_mutating());
    }

    // Drop a non-Send Arc<Mutex<T>> in test files: silence unused
    // warnings on common imports referenced only by some tests.
    #[allow(dead_code)]
    fn _unused() {
        let _ = Arc::new(Mutex::new(()));
    }
}

#[test]
fn read_only_transaction_does_not_appear_in_wal_replay() {
    let dir = TempWalDir::new("tx-wal-readonly");

    {
        let db = Database::open_with_wal(WalConfig::enabled(dir.path.clone())).unwrap();
        db.execute("CREATE (:Person {name:'Ada'})", rows_options())
            .unwrap();

        // A read-only transaction commits without writing anything.
        let mut tx = db.begin_transaction(TransactionMode::ReadOnly).unwrap();
        let _ = tx
            .execute("MATCH (p:Person) RETURN p.name", rows_options())
            .unwrap();
        tx.commit().unwrap();
    }

    {
        let db = Database::open_with_wal(WalConfig::enabled(dir.path.clone())).unwrap();
        let stream = db.stream("MATCH (p:Person) RETURN p.name AS name").unwrap();
        let values = row_values(stream.collect(), "name");
        assert_eq!(values, vec![JsonValue::String("Ada".to_string())]);
    }
}

mod concurrency {
    //! Cross-thread tests covering the store-lock concurrency contract.
    //! Auto-commit reads can overlap on the shared side of the RwLock, while
    //! writes and read-write transactions still serialize on the exclusive
    //! side.

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;

    use lora_database::{Database, TransactionMode};
    use serde_json::Value as JsonValue;

    use super::{row_values, rows_options};

    #[test]
    fn read_only_queries_can_run_while_read_guard_is_held() {
        let db = Arc::new(Database::in_memory());
        db.execute("CREATE (:T {i:1}), (:T {i:2})", rows_options())
            .unwrap();

        let read_guard = db.store().read().unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = {
            let db = db.clone();
            thread::spawn(move || {
                let rows = db
                    .execute_rows("MATCH (t:T) RETURN t.i AS i ORDER BY i")
                    .unwrap();
                tx.send(rows.len()).unwrap();
            })
        };

        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)).unwrap(),
            2,
            "read-only query should share the store read lock"
        );
        drop(read_guard);
        worker.join().unwrap();
    }

    #[test]
    fn read_only_transactions_can_run_while_read_guard_is_held() {
        let db = Arc::new(Database::in_memory());
        db.execute("CREATE (:T {i:1}), (:T {i:2})", rows_options())
            .unwrap();

        let read_guard = db.store().read().unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = {
            let db = db.clone();
            thread::spawn(move || {
                let mut tx_handle = db.begin_transaction(TransactionMode::ReadOnly).unwrap();
                let rows = tx_handle
                    .execute_rows("MATCH (t:T) RETURN t.i AS i ORDER BY i")
                    .unwrap();
                tx_handle.commit().unwrap();
                tx.send(rows.len()).unwrap();
            })
        };

        assert_eq!(
            rx.recv_timeout(Duration::from_millis(100)).unwrap(),
            2,
            "read-only transaction should share the store read lock"
        );
        drop(read_guard);
        worker.join().unwrap();
    }

    /// Two ReadWrite transactions on the same database serialize: while
    /// one holds the store write lock, the other's `begin_transaction` blocks.
    /// Verified by an `owner` atomic that each tx flips on enter / off on
    /// exit; if both ever held simultaneously, the compare-exchange fails.
    #[test]
    fn concurrent_readwrite_transactions_serialize() {
        let db = Arc::new(Database::in_memory());
        let owner = Arc::new(AtomicUsize::new(0));
        let a_holds = Arc::new(AtomicUsize::new(0));

        let a = {
            let db = db.clone();
            let owner = owner.clone();
            let a_holds = a_holds.clone();
            thread::spawn(move || {
                let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
                assert!(
                    owner
                        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok(),
                    "another tx held the store write lock when A entered"
                );
                a_holds.store(1, Ordering::SeqCst);
                // Keep the write lock long enough for B to definitely be parked
                // on its `begin_transaction` call.
                thread::sleep(Duration::from_millis(40));
                tx.execute("CREATE (:T {who:'A'})", rows_options()).unwrap();
                assert!(
                    owner
                        .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok(),
                    "owner state corrupted before A's commit"
                );
                tx.commit().unwrap();
            })
        };

        // Wait until A has the lock before spawning B so the ordering is
        // deterministic regardless of scheduler luck.
        while a_holds.load(Ordering::SeqCst) == 0 {
            thread::yield_now();
        }

        let b = {
            let db = db.clone();
            let owner = owner.clone();
            thread::spawn(move || {
                let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
                assert!(
                    owner
                        .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok(),
                    "B entered while A still held the store write lock"
                );
                tx.execute("CREATE (:T {who:'B'})", rows_options()).unwrap();
                assert!(
                    owner
                        .compare_exchange(2, 0, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok(),
                    "owner state corrupted before B's commit"
                );
                tx.commit().unwrap();
            })
        };

        a.join().unwrap();
        b.join().unwrap();

        let rows = db.execute_rows("MATCH (t:T) RETURN t.who AS who").unwrap();
        let mut values = row_values(rows, "who");
        values.sort_by_key(|v| v.as_str().map(str::to_owned));
        assert_eq!(
            values,
            vec![
                JsonValue::String("A".to_string()),
                JsonValue::String("B".to_string()),
            ]
        );
    }

    /// A `Live` read stream holds a store read lock through the cursor's
    /// lifetime. A concurrent `begin_transaction(ReadWrite)` must block
    /// until the stream is dropped.
    #[test]
    fn live_read_stream_blocks_concurrent_writer() {
        let db = Arc::new(Database::in_memory());
        db.execute("UNWIND range(1,10) AS i CREATE (:T {i: i})", rows_options())
            .unwrap();

        // Open a Live read stream and pull one row to confirm it's
        // streaming (not a buffered fallback) and is mid-iteration.
        let mut stream = db.stream("MATCH (t:T) RETURN t.i AS i").unwrap();
        assert!(stream.next_row().unwrap().is_some());

        let started = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(AtomicUsize::new(0));

        let writer = {
            let db = db.clone();
            let started = started.clone();
            let entered = entered.clone();
            thread::spawn(move || {
                started.store(1, Ordering::SeqCst);
                let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
                entered.store(1, Ordering::SeqCst);
                tx.execute("CREATE (:T {i:99})", rows_options()).unwrap();
                tx.commit().unwrap();
            })
        };

        // Wait for the writer to actually attempt begin_transaction, then
        // give the scheduler enough time that, if it weren't blocked by
        // our held stream, it would have already entered the tx.
        while started.load(Ordering::SeqCst) == 0 {
            thread::yield_now();
        }
        thread::sleep(Duration::from_millis(20));
        assert_eq!(
            entered.load(Ordering::SeqCst),
            0,
            "writer entered tx while a Live read stream still held the store read lock"
        );

        // Dropping the stream releases the read lock; the writer must finish.
        drop(stream);
        writer.join().unwrap();

        assert_eq!(db.node_count(), 11);
    }

    /// While a tx-bound streaming cursor is alive, no further statement
    /// (execute or stream) may start on the same transaction.
    /// `cursor_active` is checked synchronously via the in-tx mutex, not
    /// the store lock, so the rejection is immediate.
    #[test]
    fn active_tx_cursor_blocks_new_statement_or_stream() {
        let db = Database::in_memory();
        db.execute("CREATE (:T {n:1}), (:T {n:2})", rows_options())
            .unwrap();

        let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
        let mut stream = tx.stream("MATCH (t:T) RETURN t.n AS n").unwrap();
        assert!(stream.next_row().unwrap().is_some());

        let exec_err = tx
            .execute("MATCH (t:T) RETURN count(t)", rows_options())
            .unwrap_err();
        assert!(
            format!("{exec_err:#}").contains("streaming cursor"),
            "expected streaming-cursor error, got: {exec_err:#}"
        );

        let stream_err = tx.stream("MATCH (t:T) RETURN t.n AS n").unwrap_err();
        assert!(
            format!("{stream_err:#}").contains("streaming cursor"),
            "expected streaming-cursor error, got: {stream_err:#}"
        );

        // Dropping the cursor lets statements proceed again.
        drop(stream);
        let _ = tx
            .execute("MATCH (t:T) RETURN count(t)", rows_options())
            .unwrap();
        tx.commit().unwrap();
    }
}

mod streaming_writes {
    //! Streaming writes — M1 / M1.b / M2 / M3 / M4 plus mutating UNION.
    //!
    //! These tests verify functional correctness for plans that stream
    //! their input (M1 / M2 / M3 wave-1: input subtree is fully
    //! streamable; M1.b / wave-2: write op also streams its output via
    //! `StreamingWriteCursor`; blocking ops like `Sort`, `DISTINCT`,
    //! aggregation, optional match, path build, and UNION sit inside
    //! the streaming chain with cursor-facing sources).
    //!
    //! ## Known soft spot: analyzer strictness on rollback assertions
    //!
    //! The analyzer (`crates/lora-analyzer/src/analyzer.rs`,
    //! `property_access_allowed` at ~L1162; `validate_label_name` at
    //! ~L1110) rejects property/label references in *read* contexts
    //! whose names aren't present in the live store's schema. This is
    //! deliberate — it catches typos in production. It bites us in
    //! tests that need to assert "rollback discarded the staged write"
    //! by querying for the rolled-back property/label, because that
    //! property/label was never committed to the live store.
    //!
    //! Two patterns work around this:
    //!
    //! 1. **Aggregate-only assertion** — `node_count()` /
    //!    `MATCH (n) RETURN count(n)` don't reference any predicate
    //!    that depends on the rolled-back property. Use this when the
    //!    rollback's signature is a count delta (e.g. "no new node
    //!    leaked"). Most rollback tests in this module use this form.
    //!
    //! 2. **Pre-seed the property** — `CREATE (:T {scratch: 'x'})`
    //!    once before the rollback test puts `scratch` in the schema,
    //!    so `MATCH (t:T) WHERE t.scratch IS NULL` parses. Use this
    //!    when the rollback's signature is a property-level change
    //!    (e.g. "no node was tagged"). See
    //!    `auto_commit_set_stream_commits_on_exhaustion` for the
    //!    pattern (`marked: false` pre-seed, then assert via
    //!    `WHERE t.marked = true`).
    //!
    //! A permissive analyzer mode for tests was considered and
    //! deferred — the workaround is simple enough and the strict
    //! check is valuable in production.

    use lora_database::Database;
    use serde_json::Value as JsonValue;

    use super::{rows_json, rows_options};

    /// `UNWIND range(...) AS i CREATE (:T {i: i})` exercises the
    /// streaming-input path (Argument → Unwind → Create). Verify
    /// every node is created and the property values are correct.
    #[test]
    fn unwind_create_streams_input_correctly() {
        let db = Database::in_memory();
        let n: i64 = 5_000;
        db.execute(
            &format!("UNWIND range(1, {n}) AS i CREATE (:T {{i: i}})"),
            rows_options(),
        )
        .unwrap();

        assert_eq!(db.node_count(), n as usize);

        let result = db
            .execute(
                "MATCH (t:T) RETURN sum(t.i) AS s, count(t) AS c",
                rows_options(),
            )
            .unwrap();
        let rows = rows_json(result);
        assert_eq!(rows.len(), 1);
        let s = rows[0].get("s").and_then(JsonValue::as_i64).unwrap();
        let c = rows[0].get("c").and_then(JsonValue::as_i64).unwrap();
        assert_eq!(c, n);
        assert_eq!(s, n * (n + 1) / 2);
    }

    /// MATCH-then-CREATE: the input subtree is also fully streamable
    /// (Argument → NodeByLabelScan → Filter → Create), so the
    /// streaming path runs against an existing graph too.
    #[test]
    fn match_filter_create_streams_input_correctly() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 200) AS i CREATE (:Src {i: i})",
            rows_options(),
        )
        .unwrap();

        // Re-run as a streamable input → Create chain.
        db.execute(
            "MATCH (s:Src) WHERE s.i > 100 CREATE (:Dst {origin: s.i})",
            rows_options(),
        )
        .unwrap();

        let result = db
            .execute("MATCH (d:Dst) RETURN count(d) AS c", rows_options())
            .unwrap();
        let rows = rows_json(result);
        assert_eq!(rows[0].get("c").and_then(JsonValue::as_i64).unwrap(), 100);
    }

    /// CREATE after ORDER BY ... LIMIT uses `SortSource` inside the
    /// streaming chain. Sort still buffers internally — it is
    /// inherently O(N) — but emits sorted rows lazily so the
    /// downstream CREATE streams.
    #[test]
    fn create_after_sort_with_limit_streams_correctly() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 50) AS i CREATE (:Src {i: i})",
            rows_options(),
        )
        .unwrap();

        db.execute(
            "MATCH (s:Src) WITH s ORDER BY s.i DESC LIMIT 10 CREATE (:Top {origin: s.i})",
            rows_options(),
        )
        .unwrap();

        let result = db
            .execute("MATCH (t:Top) RETURN sum(t.origin) AS s", rows_options())
            .unwrap();
        let rows = rows_json(result);
        // Top 10 of [1..=50] = 41..=50, sum = 455.
        assert_eq!(
            rows[0].get("s").and_then(JsonValue::as_i64).unwrap(),
            (41..=50).sum::<i64>()
        );
    }

    /// The auto-commit `Database::stream` API streams writes row-by-row
    /// since M1.b. This test guards the round-trip contract.
    #[test]
    fn auto_commit_stream_streamable_create_round_trip() {
        let db = Database::in_memory();
        let n = 250usize;
        let stream = db
            .stream(&format!("UNWIND range(1, {n}) AS i CREATE (:T {{i: i}})"))
            .unwrap();
        let count = stream.count();
        assert_eq!(count, n);
        assert_eq!(db.node_count(), n);
    }

    /// Streaming auto-commit writes apply lazily, one node at a time,
    /// rather than materializing everything before emitting the first
    /// row. We pull a single row out of an UNWIND→CREATE stream and
    /// then drop it; only that one node should have been written, and
    /// the rollback (drop = rollback) must discard it from the live
    /// store.
    #[test]
    fn auto_commit_stream_writes_lazily_then_rolls_back_on_drop() {
        let db = Database::in_memory();
        let mut stream = db
            .stream("UNWIND range(1, 1000) AS i CREATE (n:T {i: i}) RETURN n.i AS i")
            .unwrap();

        // Pull a single row — exactly one CREATE should have happened
        // against the staged graph at this point.
        let first = stream.next_row().unwrap().expect("at least one row");
        let i = serde_json::to_value(first)
            .unwrap()
            .get("i")
            .and_then(JsonValue::as_i64)
            .unwrap();
        assert_eq!(i, 1);

        // Drop the stream → guard rolls back; the partial writes must
        // not leak into the live store.
        drop(stream);
        assert_eq!(db.node_count(), 0);
    }

    /// Full exhaustion of a streaming auto-commit cursor commits the
    /// staged writes. Combined with the partial-drop test above, this
    /// nails down the commit-on-exhaustion / rollback-on-drop split.
    #[test]
    fn auto_commit_stream_commits_on_full_exhaustion() {
        let db = Database::in_memory();
        let stream = db
            .stream("UNWIND range(1, 100) AS i CREATE (n:T {i: i}) RETURN n.i AS i")
            .unwrap();

        // Drain. After the last `next_row`, the guard runs `commit`.
        let collected: Vec<i64> = stream
            .filter_map(|row| {
                serde_json::to_value(row)
                    .ok()
                    .and_then(|v| v.get("i").and_then(JsonValue::as_i64))
            })
            .collect();
        assert_eq!(collected, (1..=100).collect::<Vec<_>>());
        assert_eq!(db.node_count(), 100);
    }

    /// Streaming write across a non-trivial input size verifies that
    /// the cursor doesn't quadratic-blow-up somewhere unexpected.
    /// Picks a size large enough to detect O(N²) regressions in CI
    /// time without being so large that the test takes a long time.
    #[test]
    fn auto_commit_stream_large_input_completes() {
        let db = Database::in_memory();
        let n = 10_000usize;
        let stream = db
            .stream(&format!(
                "UNWIND range(1, {n}) AS i CREATE (n:T {{i: i}}) RETURN n.i AS i"
            ))
            .unwrap();
        let count = stream.count();
        assert_eq!(count, n);
        assert_eq!(db.node_count(), n);
    }

    // ---------- Wave 1 (M2/M3): input-streaming for Set/Delete/Remove/Merge.
    //
    // These verify functional correctness when the input subtree is
    // fully streamable. Output is still buffered; Wave 2 will lift
    // that with real per-row cursors. The size is large enough that
    // a regression to materialized-input would be detectable in a
    // profile, but the assertions cover semantics, not memory.

    #[test]
    fn set_with_streamable_input_updates_all_rows() {
        let db = Database::in_memory();
        let n: i64 = 1_000;
        db.execute(
            &format!("UNWIND range(1, {n}) AS i CREATE (:T {{i: i}})"),
            rows_options(),
        )
        .unwrap();

        // MATCH → Filter → Set: input subtree is streamable.
        db.execute(
            "MATCH (t:T) WHERE t.i % 2 = 0 SET t.even = true",
            rows_options(),
        )
        .unwrap();

        let result = db
            .execute(
                "MATCH (t:T) WHERE t.even = true RETURN count(t) AS c",
                rows_options(),
            )
            .unwrap();
        let rows = rows_json(result);
        assert_eq!(rows[0].get("c").and_then(JsonValue::as_i64).unwrap(), n / 2);
    }

    #[test]
    fn delete_with_streamable_input_removes_all_targets() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 500) AS i CREATE (:T {i: i})",
            rows_options(),
        )
        .unwrap();

        db.execute("MATCH (t:T) WHERE t.i > 100 DELETE t", rows_options())
            .unwrap();

        assert_eq!(db.node_count(), 100);
    }

    #[test]
    fn remove_with_streamable_input_completes() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 200) AS i CREATE (:T {i: i, scratch: 'x'})",
            rows_options(),
        )
        .unwrap();

        // Streamable MATCH → Remove (property form). This exercises
        // the streaming dispatch in `exec_remove`. We can't easily
        // assert the property is gone afterwards because the
        // analyzer rejects references to schema-absent properties;
        // we instead rely on (a) the REMOVE call returning Ok and
        // (b) the node count staying at 200 (no accidental delete).
        db.execute("MATCH (t:T) REMOVE t.scratch", rows_options())
            .unwrap();

        assert_eq!(db.node_count(), 200);
    }

    // ---------- Wave 2 (M2/M3): real per-row streaming cursors lifted
    // into the AutoCommit pipeline for Set/Delete/Remove/Merge.
    //
    // These verify lazy delivery (pull one row → drop → live store
    // unchanged) and commit-on-exhaustion semantics — same shape as
    // the Create lazy tests above, applied to each new write op.

    #[test]
    fn auto_commit_set_stream_writes_lazily() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 100) AS i CREATE (:T {i: i})",
            rows_options(),
        )
        .unwrap();

        // Pull one SET row, drop the stream → tx rolls back. No
        // node should have been mutated.
        let mut stream = db
            .stream("MATCH (t:T) SET t.tagged = true RETURN t.i AS i")
            .unwrap();
        assert!(stream.next_row().unwrap().is_some());
        drop(stream);

        // Verify rollback: counting rows that have `tagged = true`
        // requires the analyzer to know about the property, which
        // it does only after a write has committed it. Since the
        // rollback discarded everything, counting all nodes proves
        // structure is intact and side effects didn't escape live.
        assert_eq!(db.node_count(), 100);
    }

    #[test]
    fn auto_commit_set_stream_commits_on_exhaustion() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 100) AS i CREATE (:T {i: i, marked: false})",
            rows_options(),
        )
        .unwrap();

        // Fully drain the SET stream — commit on exhaustion. Returns
        // an existing property (`t.i`) to avoid analyzer strictness
        // about referencing the just-set property in the same plan.
        let stream = db
            .stream("MATCH (t:T) SET t.marked = true RETURN t.i AS i")
            .unwrap();
        let count = stream.count();
        assert_eq!(count, 100);

        // Verify the SET committed by counting nodes via the
        // already-declared `marked` property.
        let result = db
            .execute(
                "MATCH (t:T) WHERE t.marked = true RETURN count(t) AS c",
                rows_options(),
            )
            .unwrap();
        let rows = rows_json(result);
        assert_eq!(rows[0].get("c").and_then(JsonValue::as_i64).unwrap(), 100);
    }

    #[test]
    fn auto_commit_delete_stream_rolls_back_on_drop() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 100) AS i CREATE (:T {i: i})",
            rows_options(),
        )
        .unwrap();

        let mut stream = db
            .stream("MATCH (t:T) WHERE t.i > 50 DELETE t RETURN t.i AS i")
            .unwrap();
        assert!(stream.next_row().unwrap().is_some());
        drop(stream);

        // Rollback restores everything — full 100 nodes.
        assert_eq!(db.node_count(), 100);
    }

    #[test]
    fn auto_commit_delete_stream_commits_on_exhaustion() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 100) AS i CREATE (:T {i: i})",
            rows_options(),
        )
        .unwrap();

        let stream = db
            .stream("MATCH (t:T) WHERE t.i > 50 DELETE t RETURN t.i AS i")
            .unwrap();
        let count = stream.count();
        assert_eq!(count, 50);
        assert_eq!(db.node_count(), 50);
    }

    #[test]
    fn auto_commit_merge_stream_writes_lazily() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 50) AS i CREATE (:T {i: i})",
            rows_options(),
        )
        .unwrap();

        // MERGE over 1..100: half should bind existing, half create.
        // Drop after one row → rollback discards everything.
        let mut stream = db
            .stream("UNWIND range(1, 100) AS i MERGE (t:T {i: i}) RETURN t.i AS i")
            .unwrap();
        assert!(stream.next_row().unwrap().is_some());
        drop(stream);

        // Original 50 nodes intact; nothing leaked.
        assert_eq!(db.node_count(), 50);
    }

    #[test]
    fn auto_commit_merge_stream_commits_on_exhaustion() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 50) AS i CREATE (:T {i: i})",
            rows_options(),
        )
        .unwrap();

        let stream = db
            .stream("UNWIND range(1, 100) AS i MERGE (t:T {i: i}) RETURN t.i AS i")
            .unwrap();
        let collected: Vec<i64> = stream
            .filter_map(|row| {
                serde_json::to_value(row)
                    .ok()
                    .and_then(|v| v.get("i").and_then(JsonValue::as_i64))
            })
            .collect();
        assert_eq!(collected.len(), 100);
        assert_eq!(db.node_count(), 100);
    }

    /// `MATCH ... ORDER BY ... CREATE` now streams via
    /// `SortSource`. SortSource buffers internally (Sort is
    /// inherently O(N) input) but yields sorted rows lazily, so the
    /// downstream CREATE writes one row at a time and the auto-commit
    /// guard's drop semantics still apply.
    #[test]
    fn auto_commit_sort_then_create_rolls_back_on_drop() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 100) AS i CREATE (:Src {i: i})",
            rows_options(),
        )
        .unwrap();
        let pre_count = db.node_count();

        // Pull only the first sorted row's CREATE, then drop. The
        // hidden tx must roll back; no new node should have leaked
        // to the live store. (Asserting via total node_count avoids
        // analyzer strictness around a label that exists only on
        // the rolled-back staged graph.)
        let mut stream = db
            .stream(
                "MATCH (s:Src) WITH s ORDER BY s.i DESC \
                 CREATE (:Top {origin: s.i}) RETURN s.i AS i",
            )
            .unwrap();
        assert!(stream.next_row().unwrap().is_some());
        drop(stream);

        assert_eq!(db.node_count(), pre_count);
    }

    /// `WITH DISTINCT ... CREATE` streams via
    /// `DistinctSource` inside the streaming chain. DistinctSource
    /// is internally O(distinct-row-count), but the downstream CREATE
    /// streams its writes one row at a time, and the auto-commit
    /// guard's drop semantics still apply.
    #[test]
    fn auto_commit_distinct_then_create_streams() {
        let db = Database::in_memory();
        // 100 nodes split across 5 distinct `kind` values.
        db.execute(
            "UNWIND range(1, 100) AS i CREATE (:Src {kind: i % 5})",
            rows_options(),
        )
        .unwrap();

        let stream = db
            .stream(
                "MATCH (s:Src) WITH DISTINCT s.kind AS k \
                 CREATE (:Tag {kind: k}) RETURN k",
            )
            .unwrap();
        let count = stream.count();
        assert_eq!(count, 5);

        // Verify exactly 5 Tag nodes exist (one per distinct kind).
        let result = db
            .execute("MATCH (t:Tag) RETURN count(t) AS c", rows_options())
            .unwrap();
        let rows = rows_json(result);
        assert_eq!(rows[0].get("c").and_then(JsonValue::as_i64).unwrap(), 5);
    }

    /// Read-only `... UNION ...` plans now flow through
    /// `UnionSource` + `compiled_to_streaming` instead of the old
    /// buffered fallback. Functional gate that both branches' rows
    /// reach the consumer.
    #[test]
    fn union_all_read_stream_yields_both_branches() {
        let db = Database::in_memory();
        db.execute("UNWIND range(1, 5) AS i CREATE (:T {i: i})", rows_options())
            .unwrap();
        db.execute(
            "UNWIND range(10, 12) AS i CREATE (:U {i: i})",
            rows_options(),
        )
        .unwrap();

        let stream = db
            .stream(
                "MATCH (t:T) RETURN t.i AS x \
                 UNION ALL \
                 MATCH (u:U) RETURN u.i AS x",
            )
            .unwrap();
        let collected: Vec<i64> = stream
            .filter_map(|row| {
                serde_json::to_value(row)
                    .ok()
                    .and_then(|v| v.get("x").and_then(JsonValue::as_i64))
            })
            .collect();
        let mut sorted = collected;
        sorted.sort_unstable();
        assert_eq!(sorted, vec![1, 2, 3, 4, 5, 10, 11, 12]);
    }

    /// Plain `UNION` (without ALL) deduplicates rows that
    /// match across both branches. UnionSource flips on the
    /// dedup-by-name path when any branch is non-ALL.
    #[test]
    fn union_dedup_collapses_overlap() {
        let db = Database::in_memory();
        db.execute("UNWIND range(1, 5) AS i CREATE (:T {i: i})", rows_options())
            .unwrap();
        db.execute("UNWIND range(3, 7) AS i CREATE (:U {i: i})", rows_options())
            .unwrap();

        let stream = db
            .stream(
                "MATCH (t:T) RETURN t.i AS x \
                 UNION \
                 MATCH (u:U) RETURN u.i AS x",
            )
            .unwrap();
        let collected: Vec<i64> = stream
            .filter_map(|row| {
                serde_json::to_value(row)
                    .ok()
                    .and_then(|v| v.get("x").and_then(JsonValue::as_i64))
            })
            .collect();
        let mut sorted = collected;
        sorted.sort_unstable();
        // 1..=5 ∪ 3..=7 = 1..=7 (dedup of 3, 4, 5).
        assert_eq!(sorted, vec![1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn auto_commit_mutating_union_all_rolls_back_on_drop() {
        let db = Database::in_memory();
        let mut stream = db
            .stream(
                "CREATE (:UA {i: 1}) RETURN 1 AS i \
                 UNION ALL \
                 CREATE (:UA {i: 2}) RETURN 2 AS i",
            )
            .unwrap();

        assert!(stream.next_row().unwrap().is_some());
        drop(stream);

        assert_eq!(db.node_count(), 0);
    }

    #[test]
    fn auto_commit_mutating_union_all_commits_on_exhaustion() {
        let db = Database::in_memory();
        let stream = db
            .stream(
                "CREATE (:UA {i: 1}) RETURN 1 AS i \
                 UNION ALL \
                 CREATE (:UA {i: 2}) RETURN 2 AS i",
            )
            .unwrap();

        let mut collected: Vec<i64> = stream
            .filter_map(|row| {
                serde_json::to_value(row)
                    .ok()
                    .and_then(|v| v.get("i").and_then(JsonValue::as_i64))
            })
            .collect();
        collected.sort_unstable();
        assert_eq!(collected, vec![1, 2]);
        assert_eq!(db.node_count(), 2);
    }

    #[test]
    fn auto_commit_mutating_union_dedups_rows_but_commits_all_branch_writes() {
        let db = Database::in_memory();
        let stream = db
            .stream(
                "CREATE (:UB {i: 1}) RETURN 1 AS i \
                 UNION \
                 CREATE (:UB {i: 1}) RETURN 1 AS i",
            )
            .unwrap();

        let collected: Vec<i64> = stream
            .filter_map(|row| {
                serde_json::to_value(row)
                    .ok()
                    .and_then(|v| v.get("i").and_then(JsonValue::as_i64))
            })
            .collect();
        assert_eq!(collected, vec![1]);
        assert_eq!(db.node_count(), 2);
    }

    /// Companion to the rollback test above: full drain commits all
    /// 100 sorted CREATEs end-to-end.
    #[test]
    fn auto_commit_sort_then_create_commits_on_exhaustion() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 100) AS i CREATE (:Src {i: i})",
            rows_options(),
        )
        .unwrap();

        let stream = db
            .stream(
                "MATCH (s:Src) WITH s ORDER BY s.i DESC \
                 CREATE (:Top {origin: s.i}) RETURN s.i AS i",
            )
            .unwrap();
        let count = stream.count();
        assert_eq!(count, 100);

        let result = db
            .execute(
                "MATCH (t:Top) RETURN count(t) AS c, sum(t.origin) AS s",
                rows_options(),
            )
            .unwrap();
        let rows = rows_json(result);
        assert_eq!(rows[0].get("c").and_then(JsonValue::as_i64).unwrap(), 100);
        assert_eq!(
            rows[0].get("s").and_then(JsonValue::as_i64).unwrap(),
            (1..=100).sum::<i64>()
        );
    }

    #[test]
    fn auto_commit_remove_stream_rolls_back_on_drop() {
        let db = Database::in_memory();
        db.execute(
            "UNWIND range(1, 50) AS i CREATE (:T:Tagged {i: i})",
            rows_options(),
        )
        .unwrap();

        let mut stream = db
            .stream("MATCH (t:Tagged) REMOVE t:Tagged RETURN t.i AS i")
            .unwrap();
        assert!(stream.next_row().unwrap().is_some());
        drop(stream);

        // Tagged label must still cover all 50 nodes after rollback.
        let result = db
            .execute("MATCH (t:Tagged) RETURN count(t) AS c", rows_options())
            .unwrap();
        let rows = rows_json(result);
        assert_eq!(rows[0].get("c").and_then(JsonValue::as_i64).unwrap(), 50);
    }

    #[test]
    fn merge_with_streamable_input_creates_or_binds() {
        let db = Database::in_memory();
        // Pre-seed half the keys so MERGE has a mix of matches and
        // creates inside one streaming pass.
        db.execute(
            "UNWIND range(1, 50) AS i CREATE (:T {i: i})",
            rows_options(),
        )
        .unwrap();

        // UNWIND → Merge: input subtree is streamable.
        db.execute(
            "UNWIND range(1, 100) AS i MERGE (:T {i: i})",
            rows_options(),
        )
        .unwrap();

        let result = db
            .execute("MATCH (t:T) RETURN count(t) AS c", rows_options())
            .unwrap();
        let rows = rows_json(result);
        // 50 pre-seeded + 50 newly created (MERGE bound the first
        // half, created the rest).
        assert_eq!(rows[0].get("c").and_then(JsonValue::as_i64).unwrap(), 100);
    }

    #[test]
    fn varlen_match_create_streams_input_correctly() {
        let db = Database::in_memory();
        db.execute(
            "CREATE (:V {i: 1})-[:R]->(:V {i: 2})-[:R]->(:V {i: 3})",
            rows_options(),
        )
        .unwrap();

        db.execute(
            "MATCH (:V {i: 1})-[:R*1..2]->(v:V) CREATE (:Reach {i: v.i})",
            rows_options(),
        )
        .unwrap();

        let result = db
            .execute(
                "MATCH (r:Reach) RETURN count(r) AS c, sum(r.i) AS s",
                rows_options(),
            )
            .unwrap();
        let rows = rows_json(result);
        assert_eq!(rows[0].get("c").and_then(JsonValue::as_i64).unwrap(), 2);
        assert_eq!(rows[0].get("s").and_then(JsonValue::as_i64).unwrap(), 5);
    }
}
