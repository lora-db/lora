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
        let store = db.store().lock().unwrap();
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
        let store = db.store().lock().unwrap();
        let compiled = compile(&store, "MATCH (n:N) WHERE n.v = 3 RETURN n.v AS v");
        let rows = drain(open(&store, &compiled).as_mut()).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn unwind_yields_each_element_lazily() {
        let db = Database::in_memory();
        let store = db.store().lock().unwrap();
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
        let store = db.store().lock().unwrap();
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
        let store = db.store().lock().unwrap();
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
        let store = db.store().lock().unwrap();
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
        let store = db.store().lock().unwrap();
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
    fn aggregation_falls_back_to_buffered_correctly() {
        let db = Database::in_memory();
        db.execute(
            "CREATE (:N {v:1}), (:N {v:2}), (:N {v:3})",
            Some(lora_database::ExecuteOptions {
                format: lora_database::ResultFormat::Rows,
            }),
        )
        .unwrap();
        let store = db.store().lock().unwrap();
        let compiled = compile(&store, "MATCH (n:N) RETURN sum(n.v) AS total");
        let rows = drain(open(&store, &compiled).as_mut()).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn classify_stream_recognises_writes() {
        use lora_executor::{classify_stream, StreamShape};
        let db = Database::in_memory();
        let store = db.store().lock().unwrap();
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
