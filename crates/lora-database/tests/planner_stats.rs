//! Planner statistics come from distinct-value sketches kept for every
//! (label / type, property key), not from whichever hash indexes happen
//! to be active. So:
//!
//! - plans are the same for the writer, a process restarted from a
//!   snapshot or the WAL, and the writer after unrelated lookups
//!   activated implicit indexes (no history-dependent planning);
//! - an equality on an undeclared key has a real estimate, so the
//!   planner anchors a pattern on it and seeks on the most selective
//!   conjunct;
//! - after updates and deletes the writer's stats equal those of a
//!   process that loaded only the surviving data.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lora_database::{
    Database, ExecuteOptions, InMemoryGraph, LoraValue, PlanTreeNode, ResultFormat,
};
use lora_wal::{SyncMode, WalConfig};

struct TmpDir {
    path: PathBuf,
}

impl TmpDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "lora-db-planner-stats-{}-{}-{}",
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

fn rows_with(db: &Database<InMemoryGraph>, query: &str, params: &[(&str, LoraValue)]) -> usize {
    let params: BTreeMap<String, LoraValue> = params
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    db.execute_rows_with_params(query, params)
        .unwrap_or_else(|e| panic!("{query}: {e}"))
        .len()
}

const PEOPLE: i64 = 600;
const TEAMS: i64 = 150;

/// `PEOPLE` `:Person` nodes: `name` unique, `city` 3 values, `team`
/// `TEAMS` values; a `:KNOWS` chain. Nothing is declared and nothing
/// is looked up by property, so no hash index is active.
fn seed(db: &Database<InMemoryGraph>) {
    run(
        db,
        &format!(
            "UNWIND range(0, {}) AS i CREATE (:Person {{name: 'p' + toString(i), \
             city: ['Oslo', 'Lima', 'Kyiv'][i % 3], team: i % {TEAMS}}})",
            PEOPLE - 1
        ),
    );
    run(
        db,
        &format!(
            "UNWIND range(0, {}) AS i MATCH (a) WHERE id(a) = i MATCH (b) WHERE id(b) = i + 1 \
             CREATE (a)-[:KNOWS {{since: 2000 + i % 20}}]->(b)",
            PEOPLE - 2
        ),
    );
}

/// The examples from the storage design doc's behaviour-change note.
const ANCHORED: &str = "MATCH (p:Person)-[:KNOWS]->(q:Person {name: $n}) RETURN p.name AS name";
const SELECTIVE: &str = "MATCH (p:Person) WHERE p.city = $a AND p.team = $b RETURN p.name AS name";
const SELECTIVE_SWAPPED: &str =
    "MATCH (p:Person) WHERE p.team = $b AND p.city = $a RETURN p.name AS name";
const ESTIMATE: &str = "MATCH (p:Person {team: $b}) RETURN p.name AS name";
const REL_ESTIMATE: &str = "MATCH ()-[k:KNOWS {since: $s}]->() RETURN count(k) AS n";

const PLAN_QUERIES: &[&str] = &[
    ANCHORED,
    SELECTIVE,
    SELECTIVE_SWAPPED,
    ESTIMATE,
    REL_ESTIMATE,
];

fn plans(db: &Database<InMemoryGraph>) -> Vec<String> {
    PLAN_QUERIES
        .iter()
        .map(|q| format!("{:?}", db.explain(q, None).unwrap().tree))
        .collect()
}

fn walk<'a>(node: &'a PlanTreeNode, out: &mut Vec<&'a PlanTreeNode>) {
    out.push(node);
    for child in &node.children {
        walk(child, out);
    }
}

/// Every operator of `query`'s plan, root first.
fn operators(db: &Database<InMemoryGraph>, query: &str) -> Vec<PlanTreeNode> {
    let plan = db.explain(query, None).unwrap();
    let mut nodes = Vec::new();
    walk(&plan.tree.root, &mut nodes);
    nodes.into_iter().cloned().collect()
}

/// The property seeks of `query`'s plan, as their details.
fn seeks(db: &Database<InMemoryGraph>, query: &str) -> Vec<PlanTreeNode> {
    operators(db, query)
        .into_iter()
        .filter(|n| n.operator.contains("ByProperty"))
        .collect()
}

fn details(node: &PlanTreeNode) -> String {
    format!("{} {:?}", node.operator, node.details)
}

fn assert_selective_plans(db: &Database<InMemoryGraph>) {
    // The pattern starts at the side with the unique-key equality.
    let anchored = seeks(db, ANCHORED);
    assert_eq!(anchored.len(), 1, "{:#?}", operators(db, ANCHORED));
    let anchor = details(&anchored[0]);
    assert!(
        anchor.contains("name")
            && !operators(db, ANCHORED)
                .iter()
                .any(|n| n.operator == "NodeByLabelScan"),
        "pattern should start with a seek on q.name: {:#?}",
        operators(db, ANCHORED)
    );

    // The seek is on the selective conjunct whatever the WHERE order.
    for query in [SELECTIVE, SELECTIVE_SWAPPED] {
        let found = seeks(db, query);
        assert_eq!(found.len(), 1, "{:#?}", operators(db, query));
        let seek = details(&found[0]);
        assert!(
            seek.contains("team") && !seek.contains("city"),
            "{query}: should seek on p.team: {seek}"
        );
    }

    // EXPLAIN estimates an undeclared key from its distinct count, not
    // the label count.
    let est = seeks(db, ESTIMATE)[0]
        .estimated_rows
        .expect("estimated rows");
    let exact = (PEOPLE / TEAMS) as u64;
    assert!(
        (exact..=exact * 2).contains(&est),
        "estimate {est}, expected about {exact}"
    );
}

#[test]
fn undeclared_keys_get_selective_plans_without_any_lookup() {
    let db = Database::in_memory();
    seed(&db);
    assert!(
        db.with_store(|g| g.memory_estimate())
            .property_index_keys
            .is_empty(),
        "no hash index may be active before the checks"
    );
    let stats = db.with_store(|g| g.graph_stats());
    let distinct = |key: &str| stats.node_distinct_values[&("Person".into(), key.into())];
    assert_eq!(distinct("city"), 3);
    // Sketch estimates: within the documented error of the true 600 / 150.
    assert!(
        (420..=780).contains(&distinct("name")),
        "{}",
        distinct("name")
    );
    assert!(
        (105..=195).contains(&distinct("team")),
        "{}",
        distinct("team")
    );
    assert_eq!(
        stats.relationship_distinct_values[&("KNOWS".into(), "since".into())],
        20
    );
    assert_selective_plans(&db);
}

#[test]
fn plans_do_not_depend_on_lookup_history() {
    let db = Database::in_memory();
    seed(&db);
    let fresh_stats = db.with_store(|g| g.graph_stats());
    let fresh = plans(&db);

    // Lookups that activate implicit hash indexes on two keys, in the
    // order that used to make `city` look indexed (and `team` not).
    rows_with(
        &db,
        "MATCH (p:Person {city: $a}) RETURN p",
        &[("a", LoraValue::String("Oslo".into()))],
    );
    rows_with(
        &db,
        "MATCH ()-[k:KNOWS]->() WHERE k.since = $s RETURN k",
        &[("s", LoraValue::Int(2003))],
    );
    assert!(!db
        .with_store(|g| g.memory_estimate())
        .property_index_keys
        .is_empty());

    assert_eq!(fresh_stats, db.with_store(|g| g.graph_stats()));
    assert_eq!(fresh, plans(&db));
    assert_selective_plans(&db);

    // The queries return the right rows.
    assert_eq!(
        rows_with(&db, ANCHORED, &[("n", LoraValue::String("p7".into()))]),
        1
    );
    assert_eq!(
        rows_with(
            &db,
            SELECTIVE,
            &[
                ("a", LoraValue::String("Oslo".into())),
                ("b", LoraValue::Int(3)),
            ]
        ),
        // i ≡ 3 (mod 150) and i ≡ 0 (mod 3): every i ≡ 3 (mod 150).
        (PEOPLE / TEAMS) as usize
    );
}

#[test]
fn restarted_processes_plan_like_the_writer() {
    let dir = TmpDir::new("restart");
    let snap = dir.path().join("snap.bin");
    let wal_dir = dir.path().join("wal");

    let (writer_stats, writer_plans) = {
        let writer = Database::open_with_wal(wal(&wal_dir)).unwrap();
        seed(&writer);
        // History in the writer: a lookup activates an implicit index.
        rows_with(
            &writer,
            "MATCH (p:Person {name: $n}) RETURN p",
            &[("n", LoraValue::String("p1".into()))],
        );
        writer.sync().unwrap();
        writer.save_snapshot_to(&snap).unwrap();
        (writer.with_store(|g| g.graph_stats()), plans(&writer))
    };

    let from_snapshot = Database::in_memory_from_snapshot(&snap).unwrap();
    let from_wal = Database::open_with_wal(wal(&wal_dir)).unwrap();
    for (name, db) in [("snapshot", &from_snapshot), ("wal", &from_wal)] {
        assert!(
            db.with_store(|g| g.memory_estimate())
                .property_index_keys
                .is_empty(),
            "{name}: restart activates no implicit index"
        );
        assert_eq!(writer_stats, db.with_store(|g| g.graph_stats()), "{name}");
        assert_eq!(writer_plans, plans(db), "{name}");
        assert_selective_plans(db);
    }
}

/// Updates, removals, label changes and deletes are subtracted exactly:
/// the writer's stats equal those of a process that loaded only what
/// survived.
#[test]
fn stats_after_updates_and_deletes_match_a_rebuild() {
    let dir = TmpDir::new("churn");
    let snap = dir.path().join("snap.bin");

    let writer = Database::in_memory();
    seed(&writer);
    run(
        &writer,
        "MATCH (p:Person) WHERE id(p) % 2 = 0 SET p.team = id(p) % 7, p.city = 'Rome'",
    );
    run(
        &writer,
        "MATCH (p:Person) WHERE id(p) % 5 = 0 REMOVE p.city",
    );
    run(
        &writer,
        "MATCH (p:Person) WHERE id(p) % 11 = 0 SET p:Staff REMOVE p:Person",
    );
    run(
        &writer,
        "MATCH (p:Person) WHERE id(p) % 13 = 0 DETACH DELETE p",
    );
    run(
        &writer,
        "MATCH ()-[k:KNOWS]->() WHERE k.since < 2010 SET k.since = 1999",
    );
    run(
        &writer,
        "MATCH ()-[k:KNOWS]->() WHERE k.since = 1999 DELETE k",
    );
    // A key every entity of a label loses disappears from the stats.
    run(&writer, "CREATE (:Temp {x: 1})");
    run(&writer, "MATCH (t:Temp) REMOVE t.x");

    let writer_stats = writer.with_store(|g| g.graph_stats());
    assert!(!writer_stats
        .node_distinct_values
        .contains_key(&("Temp".into(), "x".into())));
    assert_eq!(
        writer_stats.node_distinct_values[&("Person".into(), "city".into())],
        4
    );
    assert_eq!(
        writer_stats.relationship_distinct_values[&("KNOWS".into(), "since".into())],
        10
    );

    writer.save_snapshot_to(&snap).unwrap();
    let restarted = Database::in_memory_from_snapshot(&snap).unwrap();
    assert_eq!(writer_stats, restarted.with_store(|g| g.graph_stats()));
    assert_eq!(plans(&writer), plans(&restarted));
    let (w, r) = (
        writer.with_store(|g| g.memory_estimate()),
        restarted.with_store(|g| g.memory_estimate()),
    );
    assert_eq!(w.distinct_stats_sketches, r.distinct_stats_sketches);
    assert_eq!(w.distinct_stats_bytes, r.distinct_stats_bytes);
}

/// A staged write (explicit transaction) that rolls back leaves the
/// stats untouched; one that commits updates them.
#[test]
fn rolled_back_writes_leave_stats_untouched() {
    let db = Database::in_memory();
    seed(&db);
    let before = db.with_store(|g| g.graph_stats());
    {
        let mut tx = db
            .begin_transaction(lora_database::TransactionMode::ReadWrite)
            .unwrap();
        tx.execute_rows("UNWIND range(1, 50) AS i CREATE (:Person {city: 'Rome' + toString(i)})")
            .unwrap();
        tx.rollback().unwrap();
    }
    assert_eq!(before, db.with_store(|g| g.graph_stats()));
    {
        let mut tx = db
            .begin_transaction(lora_database::TransactionMode::ReadWrite)
            .unwrap();
        tx.execute_rows("CREATE (:Person {city: 'Rome'})").unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(
        db.with_store(|g| g.graph_stats()).node_distinct_values[&("Person".into(), "city".into())],
        4
    );
}
