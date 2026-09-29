//! Schema DDL participates in transactions: a migration can apply its
//! DDL and record itself atomically.

mod test_helpers;
use lora_database::{ExecuteOptions, LoraErrorCode, ResultFormat, TransactionMode};
use test_helpers::TestDb;

fn opts() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

const CONSTRAINT: &str =
    "CREATE CONSTRAINT festival_key_unique IF NOT EXISTS FOR (n:Festival) REQUIRE n.key IS UNIQUE";

#[test]
fn ddl_and_data_commit_together() {
    let db = TestDb::new();
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadWrite)
        .unwrap();
    tx.execute(CONSTRAINT, opts()).unwrap();
    tx.execute("CREATE (:__Migration {version: 1})", opts())
        .unwrap();
    // Visible inside the transaction before commit.
    let shown = tx.execute("SHOW CONSTRAINTS", opts()).unwrap();
    assert!(serde_json::to_string(&shown)
        .unwrap()
        .contains("festival_key_unique"));
    // Not visible outside yet.
    assert!(db.run("SHOW CONSTRAINTS").is_empty());
    tx.commit().unwrap();

    assert_eq!(db.run("SHOW CONSTRAINTS").len(), 1);
    db.assert_count("MATCH (m:__Migration) RETURN m", 1);
    // And the constraint is enforced.
    db.run("CREATE (:Festival {key: 'a'})");
    let err = db
        .service
        .execute("CREATE (:Festival {key: 'a'})", opts())
        .unwrap_err();
    assert_eq!(err.code(), LoraErrorCode::UniqueConstraint);
}

#[test]
fn failed_batch_leaves_neither_ddl_nor_data() {
    let db = TestDb::new();
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadWrite)
        .unwrap();
    tx.execute(CONSTRAINT, opts()).unwrap();
    tx.execute("CREATE (:__Migration {version: 1})", opts())
        .unwrap();
    assert!(tx.execute("RETURN 1 +", opts()).is_err());
    drop(tx); // rolls back

    assert!(db.run("SHOW CONSTRAINTS").is_empty());
    db.assert_count("MATCH (m:__Migration) RETURN m", 0);
}

#[test]
fn explicit_rollback_discards_index() {
    let db = TestDb::new();
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadWrite)
        .unwrap();
    tx.execute("CREATE INDEX k FOR (n:X) ON (n.k)", opts())
        .unwrap();
    tx.rollback().unwrap();
    assert!(db.run("SHOW INDEXES").is_empty());
}

#[test]
fn ddl_is_refused_in_a_read_only_transaction() {
    let db = TestDb::new();
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadOnly)
        .unwrap();
    assert!(tx.execute("SHOW INDEXES", opts()).is_ok());
    let err = tx.execute(CONSTRAINT, opts()).unwrap_err();
    assert_eq!(err.code(), LoraErrorCode::ReadOnlyViolation, "{err}");
}

#[test]
fn committed_ddl_survives_reopen() {
    use lora_database::{Database, DatabaseOpenOptions};
    let dir = std::env::temp_dir().join(format!(
        "lora-ddl-tx-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    {
        let db = Database::open_named(
            "app",
            DatabaseOpenOptions::default().with_database_dir(&dir),
        )
        .unwrap();
        let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
        tx.execute(CONSTRAINT, opts()).unwrap();
        tx.execute("CREATE (:__Migration {version: 1})", opts())
            .unwrap();
        tx.commit().unwrap();
    }
    let db = Database::open_named(
        "app",
        DatabaseOpenOptions::default().with_database_dir(&dir),
    )
    .unwrap();
    let shown = serde_json::to_string(&db.execute("SHOW CONSTRAINTS", opts()).unwrap()).unwrap();
    assert!(shown.contains("festival_key_unique"), "{shown}");
    let err = {
        db.execute("CREATE (:Festival {key: 'a'})", opts()).unwrap();
        db.execute("CREATE (:Festival {key: 'a'})", opts())
            .unwrap_err()
    };
    assert_eq!(err.code(), LoraErrorCode::UniqueConstraint);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
