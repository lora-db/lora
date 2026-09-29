//! A write's cost must not grow with the size of the graph.
//!
//! Writes that cannot run in place work on a copy of the graph so a
//! failure can be discarded. That copy is structurally shared (chunked
//! slabs, copy-on-write index shards), so a single-row write copies only
//! what it touches. Before that, a relationship CREATE or a MERGE copied
//! every node, adjacency list and index: tens of milliseconds at 50k nodes.

mod test_helpers;
use std::time::Instant;

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
