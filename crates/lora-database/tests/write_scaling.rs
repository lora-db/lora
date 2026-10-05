//! A write's cost must not grow with the size of the graph.
//!
//! Writes that cannot run in place work on a copy of the graph so a
//! failure can be discarded. That copy is structurally shared (radix-tree
//! slabs, copy-on-write indexes and catalogs), so cloning the graph is
//! O(1) and a single-row write copies only what it touches. Before that, a
//! relationship CREATE or a MERGE copied every node, adjacency list and
//! index: tens of milliseconds at 50k nodes. After a first fix the copy
//! was still one refcount bump per 512 entries, O(N) again: 218 µs per
//! staged write at 2M nodes / 8M relationships.

mod test_helpers;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use lora_database::{Database, ExecuteOptions, InMemoryGraph, LoraValue, ResultFormat};
use lora_store::{intern, GraphStorageMut, Properties, PropertyValue};
use test_helpers::TestDb;

fn festivals(n: usize) -> TestDb {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT fk FOR (f:Festival) REQUIRE f.key IS UNIQUE");
    db.run("CREATE FULLTEXT INDEX ft FOR (f:Festival) ON EACH [f.name]");
    db.run("CREATE POINT INDEX loc FOR (f:Festival) ON (f.location)");
    db.run("CREATE TEXT INDEX tx FOR (f:Festival) ON (f.name)");
    db.run(&format!(
        "UNWIND range(0, {}) AS i CREATE (:Festival {{key: 'f' + toString(i), \
         name: 'Festival number ' + toString(i), \
         location: point({{latitude: 50.0 + (i % 100) / 100.0, longitude: 4.0 + (i % 50) / 50.0}})}})",
        n - 1
    ));
    db
}

fn per_write(db: &TestDb) -> f64 {
    let writes = [
        "MERGE (f:Festival {key: 'f7'}) SET f.name = 'Renamed ' + randomUUID()",
        "MATCH (a:Festival {key: 'f1'}), (b:Festival {key: 'f2'}) CREATE (a)-[:NEAR]->(b)",
        "MERGE (f:Festival {key: randomUUID()}) \
         SET f.name = 'New', f.location = point({latitude: 50.5, longitude: 4.5})",
    ];
    for q in writes {
        db.run(q);
    }
    let start = Instant::now();
    for _ in 0..10 {
        for q in writes {
            db.run(q);
        }
    }
    start.elapsed().as_secs_f64() / 30.0
}

#[test]
fn staged_write_cost_is_independent_of_graph_size() {
    let small = per_write(&festivals(2_000));
    let large = per_write(&festivals(40_000));
    // A full copy would make the large case ~20x slower.
    assert!(
        large < small * 4.0 + 0.001,
        "small {small:.5}s, large {large:.5}s"
    );
}

#[test]
fn readers_keep_their_snapshot_while_writes_share_structure() {
    let db = festivals(3_000);
    let before = db.service.snapshot();
    db.run("MERGE (f:Festival {key: 'f7'}) SET f.name = 'Changed'");
    db.run("MATCH (a:Festival {key: 'f1'}), (b:Festival {key: 'f2'}) CREATE (a)-[:NEAR]->(b)");
    // The earlier snapshot is untouched; the live graph has both writes.
    assert_eq!(lora_store::GraphStorage::relationship_count(&*before), 0);
    assert_eq!(db.scalar("MATCH ()-[r:NEAR]->() RETURN count(r) AS c"), 1);
    assert_eq!(
        db.scalar("MATCH (f:Festival {key: 'f7'}) RETURN f.name AS n"),
        "Changed"
    );
    let hits = db
        .run("CALL db.index.fulltext.queryNodes('ft', 'Changed') YIELD node RETURN node.key AS k");
    assert_eq!(hits.len(), 1);
}

/// `n` `:Person {id, name}` nodes with three outgoing `:KNOWS` each, built
/// through the store API (no Cypher) so a large graph is quick to make.
/// A uniqueness constraint on another label sends every SET down the
/// staged (copying) path, and `person_id` gives the writes an index seek.
fn people(n: u64) -> Database<InMemoryGraph> {
    let mut graph = InMemoryGraph::new();
    let (id, name) = (intern("id"), intern("name"));
    for i in 0..n {
        let mut props = Properties::new();
        props.insert(id.clone(), PropertyValue::Int(i as i64));
        props.insert(name.clone(), PropertyValue::String(format!("p{i}")));
        graph.create_node(vec!["Person".to_string()], props);
    }
    for i in 0..n {
        for k in 1..=3u64 {
            let dst = (i * 7_919 + k * 104_729) % n;
            graph
                .create_relationship(i, dst, "KNOWS", Properties::new())
                .expect("endpoints exist");
        }
    }
    let db = Database::from_graph(graph);
    for q in [
        "CREATE CONSTRAINT acct FOR (a:Account) REQUIRE a.key IS UNIQUE",
        "CREATE INDEX person_id FOR (p:Person) ON (p.id)",
    ] {
        db.execute(q, None).unwrap_or_else(|e| panic!("{q}: {e}"));
    }
    db
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

/// Median cost of one round of staged writes: a SET (staged because a
/// constraint exists), a relationship CREATE, a CREATE on the constrained
/// label and a `graph_*` API write.
fn staged_round(db: &Database<InMemoryGraph>, n: u64, rounds: u64) -> Duration {
    let rows = Some(ExecuteOptions {
        format: ResultFormat::Rows,
    });
    let mut samples = Vec::new();
    for r in 0..rounds + 5 {
        let a = LoraValue::Int(((r * 2_654_435_761) % n) as i64);
        let b = LoraValue::Int(((r * 40_503 + 1) % n) as i64);
        let start = Instant::now();
        db.execute_with_params(
            "MATCH (p:Person {id: $a}) SET p.score = $a",
            rows,
            BTreeMap::from([("a".to_string(), a.clone())]),
        )
        .expect("set");
        db.execute_with_params(
            "MATCH (a:Person {id: $a}), (b:Person {id: $b}) CREATE (a)-[:KNOWS]->(b)",
            rows,
            BTreeMap::from([("a".to_string(), a), ("b".to_string(), b)]),
        )
        .expect("create rel");
        db.execute_with_params(
            "CREATE (:Account {key: $k})",
            rows,
            BTreeMap::from([("k".to_string(), LoraValue::String(format!("{n}-{r}")))]),
        )
        .expect("create account");
        db.graph_create_node(vec!["Tag".to_string()], BTreeMap::new())
            .expect("api create");
        indexed_creates(db, n, r);
        if r >= 5 {
            samples.push(start.elapsed());
        }
    }
    median(samples)
}

/// Two creates on the indexed `:Person` label, new ids above `n`: one
/// through the `graph_*` API and one through Cypher. Each adds an entry to
/// the hash buckets (flat and per label) and the sorted index of
/// `person_id`, on the staged copy for the API write.
fn indexed_creates(db: &Database<InMemoryGraph>, n: u64, r: u64) {
    let id = (n + 2 * r) as i64;
    db.graph_create_node(
        vec!["Person".to_string()],
        BTreeMap::from([("id".to_string(), LoraValue::Int(id))]),
    )
    .expect("api create person");
    db.execute_with_params(
        "CREATE (:Person {id: $id, name: 'new'})",
        Some(ExecuteOptions {
            format: ResultFormat::Rows,
        }),
        BTreeMap::from([("id".to_string(), LoraValue::Int(id + 1))]),
    )
    .expect("create person");
}

/// Median cost of [`indexed_creates`].
fn indexed_create_round(db: &Database<InMemoryGraph>, n: u64, rounds: u64) -> Duration {
    let mut samples = Vec::new();
    for r in 0..rounds + 5 {
        let start = Instant::now();
        indexed_creates(db, n, r);
        if r >= 5 {
            samples.push(start.elapsed());
        }
    }
    median(samples)
}

/// Median time to clone the whole graph, the copy every staged write and
/// every reader snapshot starts from.
fn clone_cost(db: &Database<InMemoryGraph>) -> Duration {
    db.with_store(|graph| {
        let samples = (0..51)
            .map(|_| {
                let start = Instant::now();
                std::hint::black_box(graph.clone());
                start.elapsed()
            })
            .collect();
        median(samples)
    })
}

fn assert_flat(what: &str, small: Duration, large: Duration, slack: Duration) {
    eprintln!("{what}: small {small:?}, large {large:?}");
    assert!(
        large < small * 3 + slack,
        "{what} grows with the graph: small {small:?}, large {large:?}"
    );
}

/// A cheap check of the clone itself, 2k vs 200k nodes. The per-chunk
/// clone this replaced took 11.5 µs vs 61 µs here unoptimised; the shared
/// clone takes ~1.2 µs at both sizes.
#[test]
fn graph_clone_cost_is_independent_of_graph_size() {
    let small = clone_cost(&people(2_000));
    let large = clone_cost(&people(200_000));
    assert_flat("graph clone", small, large, Duration::from_micros(10));
}

/// Index maintenance on creates of indexed nodes, 2k vs 200k nodes. Index
/// containers with a fixed top-level fan-out made each such staged write
/// copy O(N) entries (a hash shard of N/256 buckets, a partition table of
/// N/1024 pointers): 33 µs at 200k and 197 µs at 2M nodes per API create
/// in a release build. They now copy a bounded path, ~15 µs at both.
/// Unoptimised, the fixed per-write cost hides the difference at these
/// sizes, so this is a guard; the 1M-node test below tells them apart.
#[test]
fn indexed_create_cost_is_independent_of_graph_size() {
    let (small_n, large_n) = (2_000, 200_000);
    let small = indexed_create_round(&people(small_n), small_n, 300);
    let large = indexed_create_round(&people(large_n), large_n, 300);
    assert_flat("indexed create", small, large, Duration::from_micros(20));
}

/// Staged writes on a ~1M-node / 3M-relationship graph against a 10k-node
/// one. With the per-chunk clone a round of four writes took 100 µs vs
/// 483 µs in a release build; now ~85 µs at both sizes. The round also
/// creates two indexed `:Person` nodes, and those creates are checked on
/// their own too: with the fixed fan-out index containers they took
/// 25 µs vs 117 µs, now ~17 µs vs ~19 µs. Ignored by default because it
/// takes ~20 s unoptimised (~3 s in release); run with `cargo test
/// --release -p lora-database --test write_scaling -- --ignored`.
#[test]
#[ignore = "builds a 1M-node graph; run explicitly"]
fn staged_write_latency_is_independent_of_graph_size_at_1m_nodes() {
    let (small_n, large_n) = (10_000, 1_000_000);
    let small_db = people(small_n);
    let large_db = people(large_n);
    let small = staged_round(&small_db, small_n, 200);
    let large = staged_round(&large_db, large_n, 200);
    assert_flat(
        "staged write round",
        small,
        large,
        Duration::from_micros(100),
    );
    let (small, large) = (clone_cost(&small_db), clone_cost(&large_db));
    assert_flat("graph clone", small, large, Duration::from_micros(10));
    let small = indexed_create_round(&small_db, small_n + 1_000, 500);
    let large = indexed_create_round(&large_db, large_n + 1_000, 500);
    assert_flat("indexed create", small, large, Duration::from_micros(20));
}
