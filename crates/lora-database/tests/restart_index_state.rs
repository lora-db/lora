//! A restarted process holds the same hash property indexes as the
//! process that wrote the data: the declared ones (RANGE indexes and the
//! backing indexes of uniqueness / key constraints). Indexes that an
//! equality lookup activated implicitly are not rebuilt on snapshot load
//! or WAL replay; the next lookup that needs one builds it again.
//!
//! Each test compares the writer with the restarted database on: the
//! active hash-index keys (from `MemoryReport`), `list_indexes`, the
//! planner statistics (`graph_stats`, which decide plans; their distinct
//! counts come from sketches kept for every key, so they never depend on
//! which hash indexes are active), EXPLAIN output,
//! query results, and constraint enforcement.

use std::path::{Path, PathBuf};

use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat};
use lora_store::{GraphStorage, MemoryReport, StoredIndexEntity};
use lora_wal::{SyncMode, WalConfig};

struct TmpDir {
    path: PathBuf,
}

impl TmpDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lora-db-restart-idx-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn wal(dir: &Path) -> WalConfig {
    WalConfig::Enabled {
        dir: dir.to_path_buf(),
        sync_mode: SyncMode::GroupSync {
            interval_ms: 60_000,
        },
        segment_target_bytes: 8 * 1024 * 1024,
    }
}

fn run(db: &Database<InMemoryGraph>, query: &str) {
    db.execute(
        query,
        Some(ExecuteOptions {
            format: ResultFormat::Rows,
        }),
    )
    .unwrap_or_else(|e| panic!("{query}: {e}"));
}

fn rows(db: &Database<InMemoryGraph>, query: &str) -> String {
    let rows = db
        .execute_rows(query)
        .unwrap_or_else(|e| panic!("{query}: {e}"));
    format!("{rows:?}")
}

fn report(db: &Database<InMemoryGraph>) -> MemoryReport {
    db.with_store(|g| g.memory_estimate())
}

/// `(entity, key, declared)` for every active hash property index.
fn active_keys(db: &Database<InMemoryGraph>) -> Vec<(StoredIndexEntity, String, bool)> {
    report(db)
        .property_index_keys
        .into_iter()
        .map(|k| (k.entity, k.key, k.declared))
        .collect()
}

/// Queries whose plan depends on the planner statistics: equality seeks
/// on declared and undeclared keys, and a two-node pattern whose start
/// side the planner picks from distinct-value counts.
const PLAN_QUERIES: &[&str] = &[
    "MATCH (p:Person {email: 'p3@x'}) RETURN p.name AS name",
    "MATCH (p:Person) WHERE p.age = 33 RETURN p.name AS name",
    "MATCH (p:Person) WHERE p.age > 40 RETURN p.name AS name ORDER BY name",
    "MATCH (p:Person {city: 'Oslo'})-[:KNOWS]->(q:Person {email: 'p1@x'}) RETURN p.name, q.name",
    "MATCH (c:Company {code: 'C1'}) RETURN c.name AS name",
];

/// Result checks. The ones on `city` / `name` / `rating` use keys that are
/// never declared, so they run against lazily-built indexes after restart.
const RESULT_QUERIES: &[&str] = &[
    "MATCH (p:Person {email: 'p3@x'}) RETURN p.name AS name",
    "MATCH (p:Person) WHERE p.age = 33 RETURN p.name AS name ORDER BY name",
    "MATCH (p:Person) WHERE p.age >= 40 RETURN p.name AS name ORDER BY name",
    "MATCH (p:Person {city: 'Oslo'}) RETURN p.name AS name ORDER BY name",
    "MATCH (p:Person {name: 'p7'}) RETURN p.email AS email",
    "MATCH (c:Company {code: 'C1'}) RETURN c.name AS name",
    "MATCH ()-[k:KNOWS {since: 2021}]->() RETURN count(k) AS n",
    "MATCH ()-[k:KNOWS]->() WHERE k.rating = 4 RETURN count(k) AS n",
];

fn plans(db: &Database<InMemoryGraph>) -> Vec<String> {
    PLAN_QUERIES
        .iter()
        .map(|q| format!("{:?}", db.explain(q, None).unwrap().tree))
        .collect()
}

/// Everything a later caller can observe about index state, captured in
/// an order that does not itself activate implicit indexes (EXPLAIN and
/// stats first, then the result queries, which may).
struct Observed {
    keys_before_queries: Vec<(StoredIndexEntity, String, bool)>,
    indexes: Vec<lora_store::IndexDefinition>,
    stats: lora_store::GraphStats,
    plans: Vec<String>,
    results: Vec<String>,
    keys_after_queries: Vec<(StoredIndexEntity, String, bool)>,
}

fn observe(db: &Database<InMemoryGraph>) -> Observed {
    let keys_before_queries = active_keys(db);
    let indexes = db.with_store(|g| g.list_indexes());
    let stats = db.with_store(|g| g.graph_stats());
    let plans = plans(db);
    let results = RESULT_QUERIES.iter().map(|q| rows(db, q)).collect();
    let keys_after_queries = active_keys(db);
    Observed {
        keys_before_queries,
        indexes,
        stats,
        plans,
        results,
        keys_after_queries,
    }
}

fn assert_same(writer: &Observed, restarted: &Observed) {
    assert_eq!(
        writer.keys_before_queries, restarted.keys_before_queries,
        "active hash-index keys"
    );
    assert_eq!(writer.indexes, restarted.indexes, "list_indexes");
    assert_eq!(writer.stats, restarted.stats, "graph_stats");
    assert_eq!(writer.plans, restarted.plans, "EXPLAIN plans");
    assert_eq!(writer.results, restarted.results, "query results");
    assert_eq!(
        writer.keys_after_queries, restarted.keys_after_queries,
        "active keys after the same lookups"
    );
}

fn seed_people(db: &Database<InMemoryGraph>, from: u32, to: u32) {
    for i in from..to {
        let city = ["Oslo", "Lima", "Kyiv"][(i % 3) as usize];
        run(
            db,
            &format!(
                "CREATE (:Person {{name: 'p{i}', email: 'p{i}@x', age: {}, city: '{city}', \
                 score: {}.5}})",
                30 + i % 15,
                i
            ),
        );
    }
}

fn seed_knows(db: &Database<InMemoryGraph>, n: u32) {
    // Relationships created by id() to avoid an equality lookup in the
    // writer (which would activate an implicit index there).
    for i in 0..n {
        run(
            db,
            &format!(
                "MATCH (a:Person), (b:Person) WHERE id(a) = {i} AND id(b) = {} \
                 CREATE (a)-[:KNOWS {{since: {}, rating: {}}}]->(b)",
                (i + 1) % n,
                2020 + i % 3,
                i % 5
            ),
        );
    }
}

fn expect_violation(db: &Database<InMemoryGraph>, query: &str) {
    let err = db
        .execute(
            query,
            Some(ExecuteOptions {
                format: ResultFormat::Rows,
            }),
        )
        .expect_err("constraint must reject the write");
    let msg = err.to_string();
    assert!(
        msg.contains("22N79") || msg.contains("22N77"),
        "unexpected error for {query}: {err}"
    );
}

// (a) + (c) + (d), snapshot.
#[test]
fn snapshot_restart_reproduces_declared_index_state() {
    let dir = TmpDir::new("snapshot");
    let path = dir.path().join("snap.bin");

    let writer = Database::in_memory();
    seed_people(&writer, 0, 20);
    seed_knows(&writer, 20);
    run(&writer, "CREATE (:Company {code: 'C1', name: 'Acme'})");
    run(&writer, "CREATE INDEX person_age FOR (p:Person) ON (p.age)");
    run(
        &writer,
        "CREATE CONSTRAINT person_email FOR (p:Person) REQUIRE p.email IS UNIQUE",
    );

    // Only the declared keys are active in the writer.
    assert_eq!(
        active_keys(&writer),
        vec![
            (StoredIndexEntity::Node, "age".to_string(), true),
            (StoredIndexEntity::Node, "email".to_string(), true),
        ]
    );
    writer.save_snapshot_to(&path).unwrap();
    let restarted = Database::in_memory_from_snapshot(&path).unwrap();

    // Memory of the declared indexes is the same; nothing implicit exists.
    let (w, r) = (report(&writer), report(&restarted));
    assert_eq!(r.implicit_property_index_bytes(), 0);
    assert_eq!(w.property_index_keys, r.property_index_keys);
    assert_eq!(w.property_index_bytes, r.property_index_bytes);

    let (wo, ro) = (observe(&writer), observe(&restarted));
    assert_same(&wo, &ro);

    // (c) the undeclared keys queried above came up lazily, as implicit.
    let implicit: Vec<String> = report(&restarted)
        .implicit_property_index_keys()
        .map(|k| k.key.clone())
        .collect();
    assert!(implicit.contains(&"city".to_string()), "{implicit:?}");
    assert!(implicit.contains(&"name".to_string()), "{implicit:?}");
    assert!(report(&restarted).implicit_property_index_bytes() > 0);

    // (d) the uniqueness constraint still rejects a duplicate, and still
    // accepts a fresh value.
    expect_violation(&restarted, "CREATE (:Person {email: 'p3@x'})");
    run(&restarted, "CREATE (:Person {email: 'new@x'})");
    expect_violation(&restarted, "CREATE (:Person {email: 'new@x'})");
    expect_violation(
        &restarted,
        "MATCH (p:Person {name: 'p4'}) SET p.email = 'p5@x'",
    );
}

// (b) + (c) + (d), WAL replay with DDL interleaved with data.
#[test]
fn wal_replay_reproduces_declared_index_state() {
    let dir = TmpDir::new("wal");

    let writer_observed;
    {
        let writer = Database::open_with_wal(wal(dir.path())).unwrap();
        // Data before any DDL.
        seed_people(&writer, 0, 10);
        // RANGE index mid-log: backfills the 10 nodes so far.
        run(&writer, "CREATE INDEX person_age FOR (p:Person) ON (p.age)");
        // Data after it: maintained incrementally.
        seed_people(&writer, 10, 20);
        seed_knows(&writer, 20);
        // Updates and deletes on the declared key after declaration.
        run(&writer, "MATCH (p:Person) WHERE id(p) = 2 SET p.age = 99");
        run(&writer, "MATCH (p:Person) WHERE id(p) = 5 REMOVE p.age");
        run(&writer, "MATCH (p:Person) WHERE id(p) = 19 DETACH DELETE p");
        // A constraint created mid-log, then more data.
        run(
            &writer,
            "CREATE CONSTRAINT person_email FOR (p:Person) REQUIRE p.email IS UNIQUE",
        );
        seed_people(&writer, 20, 25);
        // A label added after the fact moves the node into the scope.
        run(&writer, "CREATE (:Company {code: 'C1', name: 'Acme'})");
        run(
            &writer,
            "CREATE INDEX company_code FOR (c:Company) ON (c.code)",
        );
        run(
            &writer,
            "MATCH (p:Person) WHERE id(p) = 3 SET p:Company, p.code = 'C3'",
        );
        // A dropped index: in the writer its hash buckets stay active
        // (implicit from then on); replaying the DROP must do the same.
        run(
            &writer,
            "CREATE INDEX person_score FOR (p:Person) ON (p.score)",
        );
        run(&writer, "DROP INDEX person_score");
        // Existence constraint (no backing index).
        run(
            &writer,
            "CREATE CONSTRAINT company_name FOR (c:Company) REQUIRE c.name IS NOT NULL",
        );
        run(
            &writer,
            "MATCH (p:Person) WHERE id(p) = 3 SET p.name = 'p3'",
        );

        assert_eq!(
            active_keys(&writer),
            vec![
                (StoredIndexEntity::Node, "age".to_string(), true),
                (StoredIndexEntity::Node, "code".to_string(), true),
                (StoredIndexEntity::Node, "email".to_string(), true),
                (StoredIndexEntity::Node, "score".to_string(), false),
            ]
        );
        writer.sync().unwrap();
        writer_observed = observe(&writer);
    }

    let restarted = Database::open_with_wal(wal(dir.path())).unwrap();
    let restarted_observed = observe(&restarted);
    assert_same(&writer_observed, &restarted_observed);

    // (c) undeclared keys came up lazily.
    let implicit: Vec<String> = report(&restarted)
        .implicit_property_index_keys()
        .map(|k| k.key.clone())
        .collect();
    assert!(implicit.contains(&"city".to_string()), "{implicit:?}");

    // (d) constraints still enforced after replay.
    expect_violation(&restarted, "CREATE (:Person {email: 'p21@x'})");
    expect_violation(&restarted, "CREATE (:Person {email: 'p1@x'})");
    expect_violation(&restarted, "CREATE (:Company {code: 'C9'})");
    run(&restarted, "CREATE (:Person {email: 'fresh@x'})");
}

// A snapshot drops what the writer only had implicitly, including the
// buckets a dropped RANGE index left behind; query results are unchanged.
#[test]
fn snapshot_restart_drops_implicit_indexes() {
    let dir = TmpDir::new("implicit");
    let path = dir.path().join("snap.bin");

    let writer = Database::in_memory();
    seed_people(&writer, 0, 12);
    run(
        &writer,
        "CREATE INDEX person_score FOR (p:Person) ON (p.score)",
    );
    run(&writer, "DROP INDEX person_score");
    let before = rows(
        &writer,
        "MATCH (p:Person {city: 'Lima'}) RETURN p.name AS n ORDER BY n",
    );
    assert_eq!(
        active_keys(&writer),
        vec![
            (StoredIndexEntity::Node, "city".to_string(), false),
            (StoredIndexEntity::Node, "score".to_string(), false),
        ]
    );
    assert!(report(&writer).implicit_property_index_bytes() > 0);

    writer.save_snapshot_to(&path).unwrap();
    let restarted = Database::in_memory_from_snapshot(&path).unwrap();
    assert!(active_keys(&restarted).is_empty());
    assert_eq!(report(&restarted).property_index_bytes, 0);
    // The planner's distinct counts don't come from the hash indexes: the
    // restarted process has the writer's, implicit index or not.
    let stats = restarted.with_store(|g| g.graph_stats());
    assert_eq!(stats, writer.with_store(|g| g.graph_stats()));
    assert_eq!(
        stats.node_distinct_values[&("Person".to_string(), "city".to_string())],
        3
    );

    let after = rows(
        &restarted,
        "MATCH (p:Person {city: 'Lima'}) RETURN p.name AS n ORDER BY n",
    );
    assert_eq!(before, after);
    assert_eq!(
        active_keys(&restarted),
        vec![(StoredIndexEntity::Node, "city".to_string(), false)]
    );
}
