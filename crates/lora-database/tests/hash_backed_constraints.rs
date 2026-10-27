//! `CREATE CONSTRAINT … IS UNIQUE OPTIONS { indexProvider: 'hash' }`
//! (G-64): the backing index keeps the hash index the uniqueness check and
//! key lookups read, and no sorted copy of the key. Range, prefix and
//! ordered reads on the key still answer correctly, from a scan.

mod test_helpers;

use std::path::PathBuf;

use lora_database::{Database, DatabaseOpenOptions, ExecuteOptions, PlanTreeNode, ResultFormat};
use serde_json::json;
use test_helpers::TestDb;

fn operators(node: &PlanTreeNode, out: &mut Vec<String>) {
    out.push(node.operator.clone());
    for child in &node.children {
        operators(child, out);
    }
}

fn plan(db: &Database<lora_database::InMemoryGraph>, q: &str) -> Vec<String> {
    let mut out = Vec::new();
    operators(&db.explain(q, None).unwrap().tree.root, &mut out);
    out
}

fn seeded(provider: &str) -> TestDb {
    let db = TestDb::new();
    db.run(&format!(
        "CREATE CONSTRAINT rk FOR (r:Record) REQUIRE r.key IS UNIQUE \
         OPTIONS {{ indexProvider: '{provider}' }}"
    ));
    db.run("UNWIND range(1, 500) AS i CREATE (:Record {key: 'k' + right('000' + toString(i), 3)})");
    db
}

#[test]
fn a_hash_backed_constraint_enforces_and_seeks_by_key() {
    let db = seeded("hash");
    let err = db.run_err("CREATE (:Record {key: 'k001'})");
    assert!(
        err.contains("already exists") || err.contains("22N"),
        "{err}"
    );
    assert_eq!(
        db.scalar("MATCH (r:Record {key: 'k042'}) RETURN count(r)"),
        json!(1)
    );
    assert!(plan(&db.service, "MATCH (r:Record {key: 'k042'}) RETURN r")
        .contains(&"NodeByPropertyScan".to_string()));
    // MERGE on the key stays idempotent.
    db.run("MERGE (:Record {key: 'k042'})");
    db.assert_count("MATCH (r:Record) RETURN r", 500);

    let shown = db.run("SHOW CONSTRAINTS");
    assert_eq!(shown[0]["indexProvider"], json!("hash"));
    assert_eq!(
        seeded("range").run("SHOW CONSTRAINTS")[0]["indexProvider"],
        json!("range")
    );
}

#[test]
fn range_prefix_and_ordered_reads_scan_but_answer_the_same() {
    let hash = seeded("hash");
    let range = seeded("range");
    for q in [
        "MATCH (r:Record) WHERE r.key > 'k490' RETURN r.key AS k ORDER BY k",
        "MATCH (r:Record) WHERE r.key STARTS WITH 'k49' RETURN r.key AS k ORDER BY k",
        "MATCH (r:Record) RETURN r.key AS k ORDER BY k LIMIT 3",
    ] {
        assert_eq!(hash.run(q), range.run(q), "{q}");
        let ops = plan(&hash.service, q);
        assert!(
            !ops.contains(&"NodeByPropertyRangeScan".to_string()),
            "{q}: {ops:?}"
        );
    }
    assert!(plan(
        &range.service,
        "MATCH (r:Record) WHERE r.key > 'k490' RETURN r.key AS k"
    )
    .contains(&"NodeByPropertyRangeScan".to_string()));
}

#[test]
fn bad_options_are_refused() {
    let db = TestDb::new();
    for (q, needle) in [
        (
            "CREATE CONSTRAINT a FOR (r:R) REQUIRE r.k IS UNIQUE OPTIONS { indexProvider: 'btree' }",
            "'range' or 'hash'",
        ),
        (
            "CREATE CONSTRAINT b FOR (r:R) REQUIRE r.k IS UNIQUE OPTIONS { sorted: false }",
            "unknown constraint option",
        ),
        (
            "CREATE CONSTRAINT c FOR (r:R) REQUIRE r.k IS NOT NULL OPTIONS { indexProvider: 'hash' }",
            "uniqueness and key constraints only",
        ),
    ] {
        let err = db.run_err(q);
        assert!(err.contains(needle), "{q}: {err}");
    }
    // Neo4j's default spelling is accepted.
    db.run("CREATE CONSTRAINT d FOR (r:R) REQUIRE r.k IS NODE KEY OPTIONS { indexProvider: 'range-1.0' }");
}

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lora-hash-constraint-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn rows() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

#[test]
fn the_backing_survives_wal_replay_and_a_snapshot() {
    let dir = TmpDir::new("restart");
    let options = DatabaseOpenOptions::default().with_database_dir(&dir.0);
    let provider = |db: &Database<lora_database::InMemoryGraph>| {
        let result = db.execute("SHOW CONSTRAINTS", rows()).unwrap();
        serde_json::to_value(&result).unwrap()["rows"][0]["indexProvider"].clone()
    };
    let sorted_bytes = |db: &Database<lora_database::InMemoryGraph>| {
        db.with_store(|g| g.memory_estimate().sorted_index_bytes)
    };
    {
        let db = Database::open_named("app", options.clone()).unwrap();
        db.execute(
            "CREATE CONSTRAINT rk FOR (r:Record) REQUIRE r.key IS UNIQUE \
             OPTIONS { indexProvider: 'hash' }",
            rows(),
        )
        .unwrap();
        db.execute(
            "UNWIND range(1, 1000) AS i CREATE (:Record {key: 'k' + toString(i)})",
            rows(),
        )
        .unwrap();
    }
    // Replayed from the WAL.
    {
        let db = Database::open_named("app", options.clone()).unwrap();
        assert_eq!(provider(&db), json!("hash"));
        assert_eq!(sorted_bytes(&db), 0);
        assert!(db.execute("CREATE (:Record {key: 'k7'})", rows()).is_err());
        db.checkpoint().unwrap();
    }
    // Loaded from the snapshot.
    let db = Database::open_named("app", options).unwrap();
    assert_eq!(provider(&db), json!("hash"));
    assert_eq!(sorted_bytes(&db), 0);
    assert!(db.execute("CREATE (:Record {key: 'k7'})", rows()).is_err());
}
