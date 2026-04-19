/// MERGE clause tests — idempotent creation, ON MATCH / ON CREATE,
/// property-based deduplication, relationship merge (pending).
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// Basic MERGE
// ============================================================

#[test]
fn merge_creates_node_when_absent() {
    let db = TestDb::new();
    db.run("MERGE (n:User {name: 'Alice'})");
    db.assert_count("MATCH (n:User {name: 'Alice'}) RETURN n", 1);
}

#[test]
fn merge_does_not_duplicate_when_present() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    db.run("MERGE (n:User {name: 'Alice'})");
    db.assert_count("MATCH (n:User {name: 'Alice'}) RETURN n", 1);
}

#[test]
fn merge_is_idempotent_for_nodes() {
    let db = TestDb::new();
    db.run("MERGE (n:Tag {name:'important'})");
    db.run("MERGE (n:Tag {name:'important'})");
    db.run("MERGE (n:Tag {name:'important'})");
    db.assert_count("MATCH (n:Tag {name:'important'}) RETURN n", 1);
}

#[test]
fn merge_creates_when_property_differs() {
    let db = TestDb::new();
    db.run("MERGE (n:Tag {name:'a'})");
    db.run("MERGE (n:Tag {name:'b'})");
    db.assert_count("MATCH (n:Tag) RETURN n", 2);
}

// ============================================================
// ON MATCH / ON CREATE
// ============================================================

#[test]
fn merge_on_match_set() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 25})");
    db.run("MERGE (n:User {name: 'Alice'}) ON MATCH SET n.age = 30");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["age"], 30);
}

#[test]
fn merge_on_create_set() {
    let db = TestDb::new();
    db.run("MERGE (n:User {name: 'Alice'}) ON CREATE SET n.age = 30");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["age"], 30);
}

#[test]
fn merge_on_create_not_triggered_when_exists() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 25})");
    db.run("MERGE (n:User {name: 'Alice'}) ON CREATE SET n.age = 99");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["age"], 25);
}

#[test]
fn merge_on_match_not_triggered_when_not_exists() {
    let db = TestDb::new();
    db.run("MERGE (n:User {name: 'Alice'}) ON MATCH SET n.age = 99");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) RETURN n");
    assert!(
        rows[0]["n"]["properties"].get("age").is_none()
            || rows[0]["n"]["properties"]["age"].is_null()
    );
}

#[test]
fn merge_on_match_updates_existing() {
    let db = TestDb::new();
    db.run("CREATE (:Counter {name:'hits', count: 0})");
    db.run("MERGE (c:Counter {name:'hits'}) ON MATCH SET c.count = 1");
    let rows = db.run("MATCH (c:Counter {name:'hits'}) RETURN c");
    assert_eq!(rows[0]["c"]["properties"]["count"], 1);
}

#[test]
fn merge_on_create_initializes() {
    let db = TestDb::new();
    db.run("MERGE (c:Counter {name:'views'}) ON CREATE SET c.count = 0");
    let rows = db.run("MATCH (c:Counter {name:'views'}) RETURN c");
    assert_eq!(rows[0]["c"]["properties"]["count"], 0);
}

// ============================================================
// Pending: MERGE on relationships
// ============================================================

#[test]
fn merge_relationship_creates_when_absent() {
    let db = TestDb::new();
    db.run("CREATE (:A {id:1})");
    db.run("CREATE (:B {id:2})");
    db.run("MATCH (a:A), (b:B) MERGE (a)-[:REL]->(b)");
    db.assert_count("MATCH (a)-[r:REL]->(b) RETURN r", 1);
}

#[test]
fn merge_relationship_is_idempotent() {
    let db = TestDb::new();
    db.run("CREATE (:A {id:1})");
    db.run("CREATE (:B {id:2})");
    db.run("MATCH (a:A), (b:B) MERGE (a)-[:REL]->(b)");
    db.run("MATCH (a:A), (b:B) MERGE (a)-[:REL]->(b)");
    db.assert_count("MATCH (a)-[r:REL]->(b) RETURN r", 1);
}

// ============================================================
// MERGE with multiple properties
// ============================================================

#[test]
fn merge_with_multiple_properties() {
    let db = TestDb::new();
    db.run("MERGE (n:Config {key:'timeout', value: 30, unit: 'seconds'})");
    db.assert_count("MATCH (c:Config {key:'timeout', value: 30}) RETURN c", 1);
}

#[test]
fn merge_does_not_match_partial_properties() {
    let db = TestDb::new();
    db.run("CREATE (:Config {key:'timeout', value: 30})");
    db.run("MERGE (:Config {key:'timeout', value: 60})");
    // Should create second node because value differs
    db.assert_count("MATCH (c:Config {key:'timeout'}) RETURN c", 2);
}

// ============================================================
// MERGE ON MATCH + ON CREATE together
// ============================================================

#[test]
fn merge_on_match_and_on_create_both_present() {
    let db = TestDb::new();
    db.run("CREATE (:Counter {name:'hits', count: 5})");
    db.run("MERGE (c:Counter {name:'hits'}) ON MATCH SET c.count = c.count + 1 ON CREATE SET c.count = 0");
    let rows = db.run("MATCH (c:Counter {name:'hits'}) RETURN c");
    // ON MATCH fires: count should be 6
    assert_eq!(rows[0]["c"]["properties"]["count"], 6);
}

#[test]
fn merge_on_create_fires_when_new() {
    let db = TestDb::new();
    db.run("MERGE (c:Counter {name:'views'}) ON MATCH SET c.count = c.count + 1 ON CREATE SET c.count = 0");
    let rows = db.run("MATCH (c:Counter {name:'views'}) RETURN c");
    // ON CREATE fires: count should be 0
    assert_eq!(rows[0]["c"]["properties"]["count"], 0);
}

// ============================================================
// MERGE stability
// ============================================================

#[test]
fn merge_stability_many_repetitions() {
    let db = TestDb::new();
    for _ in 0..10 {
        db.run("MERGE (n:Singleton {key:'unique'})");
    }
    db.assert_count("MATCH (n:Singleton {key:'unique'}) RETURN n", 1);
}

// ============================================================
// MERGE does not affect other nodes
// ============================================================

#[test]
fn merge_does_not_affect_other_nodes() {
    let db = TestDb::new();
    db.run("CREATE (:Tag {name:'existing', count: 5})");
    db.run("MERGE (:Tag {name:'new'})");
    let rows = db.run("MATCH (t:Tag {name:'existing'}) RETURN t");
    assert_eq!(rows[0]["t"]["properties"]["count"], 5);
    db.assert_count("MATCH (t:Tag) RETURN t", 2);
}

// ============================================================
// MERGE with label-only pattern
// ============================================================

#[test]
fn merge_with_label_only() {
    let db = TestDb::new();
    db.run("MERGE (n:Marker)");
    db.assert_count("MATCH (n:Marker) RETURN n", 1);
    db.run("MERGE (n:Marker)");
    db.assert_count("MATCH (n:Marker) RETURN n", 1);
}

// ============================================================
// MERGE creates when label differs
// ============================================================

#[test]
fn merge_creates_when_label_differs() {
    let db = TestDb::new();
    db.run("CREATE (:TypeA {name:'x'})");
    db.run("MERGE (:TypeB {name:'x'})");
    db.assert_count("MATCH (n) RETURN n", 2);
}

// ============================================================
// Pending: MERGE relationship with ON CREATE
// ============================================================

#[test]
fn merge_relationship_with_on_create() {
    let db = TestDb::new();
    db.run("CREATE (:A {id:1})");
    db.run("CREATE (:B {id:2})");
    db.run("MATCH (a:A), (b:B) MERGE (a)-[r:REL]->(b) ON CREATE SET r.created = true");
    let rows = db.run("MATCH (a)-[r:REL]->(b) RETURN r");
    assert_eq!(rows[0]["r"]["properties"]["created"], true);
}

#[test]
fn merge_relationship_with_on_match() {
    let db = TestDb::new();
    db.run("CREATE (a:A {id:1})-[:REL {count: 0}]->(b:B {id:2})");
    db.run("MATCH (a:A), (b:B) MERGE (a)-[r:REL]->(b) ON MATCH SET r.count = 1");
    let rows = db.run("MATCH (a)-[r:REL]->(b) RETURN r");
    assert_eq!(rows[0]["r"]["properties"]["count"], 1);
}

// ============================================================
// MERGE advanced patterns
// ============================================================

#[test]
fn merge_in_a_loop_many_repetitions_count_stays_one() {
    let db = TestDb::new();
    for _ in 0..25 {
        db.run("MERGE (n:Once {key:'only-one'})");
    }
    db.assert_count("MATCH (n:Once {key:'only-one'}) RETURN n", 1);
}

#[test]
fn merge_on_match_incrementing_counter_multiple_times() {
    let db = TestDb::new();
    db.run("CREATE (:Accumulator {name:'hits', count: 0})");
    for _ in 0..5 {
        db.run("MERGE (a:Accumulator {name:'hits'}) ON MATCH SET a.count = a.count + 1");
    }
    let rows = db.run("MATCH (a:Accumulator {name:'hits'}) RETURN a");
    assert_eq!(rows[0]["a"]["properties"]["count"], 5);
}

#[test]
fn merge_on_create_and_on_match_both_present_verify_correct_branch() {
    let db = TestDb::new();
    // First MERGE: node does not exist, ON CREATE fires
    db.run("MERGE (n:Track {key:'status'}) ON CREATE SET n.source = 'created' ON MATCH SET n.source = 'matched'");
    let rows = db.run("MATCH (n:Track {key:'status'}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["source"], "created");

    // Second MERGE: node exists, ON MATCH fires
    db.run("MERGE (n:Track {key:'status'}) ON CREATE SET n.source = 'created_again' ON MATCH SET n.source = 'matched'");
    let rows = db.run("MATCH (n:Track {key:'status'}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["source"], "matched");
    // Still only one node
    db.assert_count("MATCH (n:Track) RETURN n", 1);
}

#[test]
fn merge_with_multiple_properties_partial_match_creates_new() {
    let db = TestDb::new();
    db.run("MERGE (n:Entry {a: 1, b: 2, c: 3})");
    // Same a and b but different c => creates a new node
    db.run("MERGE (n:Entry {a: 1, b: 2, c: 99})");
    db.assert_count("MATCH (n:Entry) RETURN n", 2);
    db.assert_count("MATCH (n:Entry {a: 1, b: 2, c: 3}) RETURN n", 1);
    db.assert_count("MATCH (n:Entry {a: 1, b: 2, c: 99}) RETURN n", 1);
}

#[test]
fn merge_with_all_properties_matching_does_not_duplicate() {
    let db = TestDb::new();
    db.run("MERGE (n:Entry {a: 1, b: 2, c: 3})");
    db.run("MERGE (n:Entry {a: 1, b: 2, c: 3})");
    db.assert_count("MATCH (n:Entry) RETURN n", 1);
}

// ============================================================
// MERGE after MATCH
// ============================================================

#[test]
fn match_then_merge_in_same_query() {
    let db = TestDb::new();
    db.run("CREATE (:Team {name:'Alpha'})");
    db.run("MATCH (t:Team {name:'Alpha'}) MERGE (m:Member {name:'Alice', team:'Alpha'})");
    db.assert_count("MATCH (m:Member {name:'Alice'}) RETURN m", 1);
}

#[test]
fn merge_creating_nodes_used_in_subsequent_match() {
    let db = TestDb::new();
    db.run("MERGE (:Anchor {key:'root'})");
    db.assert_count("MATCH (a:Anchor {key:'root'}) RETURN a", 1);
    // Use the merged node in a subsequent query
    db.run("MATCH (a:Anchor {key:'root'}) CREATE (a)-[:HAS]->(:Leaf {val: 1})");
    db.assert_count("MATCH (a:Anchor)-[:HAS]->(l:Leaf) RETURN l", 1);
}

#[test]
fn merge_with_with_pipeline() {
    let db = TestDb::new();
    db.run("CREATE (:Source {val: 10})");
    db.run("CREATE (:Source {val: 20})");
    db.run("MATCH (s:Source) WITH s.val AS v MERGE (t:Target {val: v})");
    db.assert_count("MATCH (t:Target) RETURN t", 2);
}

#[test]
fn merge_after_unwind() {
    let db = TestDb::new();
    db.run("UNWIND ['a', 'b', 'c'] AS name MERGE (:Tag {name: name})");
    db.assert_count("MATCH (t:Tag) RETURN t", 3);
    // Running again should not create duplicates
    db.run("UNWIND ['a', 'b', 'c'] AS name MERGE (:Tag {name: name})");
    db.assert_count("MATCH (t:Tag) RETURN t", 3);
}

// ============================================================
// Ignored MERGE relationship tests
// ============================================================

// Lora: MERGE relationship creates when absent
#[test]
fn merge_relationship_creates_when_absent_advanced() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    db.run("CREATE (:Y {id:2})");
    db.run("MATCH (x:X {id:1}), (y:Y {id:2}) MERGE (x)-[:CONNECTS]->(y)");
    db.assert_count("MATCH (x:X)-[:CONNECTS]->(y:Y) RETURN x, y", 1);
}

// Lora: MERGE relationship idempotent
#[test]
fn merge_relationship_is_idempotent_advanced() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    db.run("CREATE (:Y {id:2})");
    db.run("MATCH (x:X {id:1}), (y:Y {id:2}) MERGE (x)-[:CONNECTS]->(y)");
    db.run("MATCH (x:X {id:1}), (y:Y {id:2}) MERGE (x)-[:CONNECTS]->(y)");
    db.run("MATCH (x:X {id:1}), (y:Y {id:2}) MERGE (x)-[:CONNECTS]->(y)");
    db.assert_count("MATCH (x:X)-[:CONNECTS]->(y:Y) RETURN x, y", 1);
}

// Lora: MERGE relationship ON CREATE SET
#[test]
fn merge_relationship_on_create_set_advanced() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    db.run("CREATE (:Y {id:2})");
    db.run(
        "MATCH (x:X {id:1}), (y:Y {id:2}) MERGE (x)-[r:CONNECTS]->(y) ON CREATE SET r.weight = 1.0",
    );
    let rows = db.run("MATCH (x)-[r:CONNECTS]->(y) RETURN r");
    let weight = rows[0]["r"]["properties"]["weight"].as_f64().unwrap();
    assert!((weight - 1.0).abs() < 0.001);
}

// Lora: MERGE relationship ON MATCH SET
#[test]
fn merge_relationship_on_match_set_advanced() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})-[:CONNECTS {count: 0}]->(:Y {id:2})");
    db.run("MATCH (x:X {id:1}), (y:Y {id:2}) MERGE (x)-[r:CONNECTS]->(y) ON MATCH SET r.count = r.count + 1");
    let rows = db.run("MATCH (x)-[r:CONNECTS]->(y) RETURN r");
    assert_eq!(rows[0]["r"]["properties"]["count"], 1);
}

// Lora: MERGE full path pattern (a)-[:R]->(b)-[:R]->(c)
#[test]
fn merge_full_path_pattern() {
    let db = TestDb::new();
    db.run("MERGE (a:Chain {id:1})-[:STEP]->(b:Chain {id:2})-[:STEP]->(c:Chain {id:3})");
    db.assert_count("MATCH (n:Chain) RETURN n", 3);
    db.assert_count("MATCH (a:Chain)-[:STEP]->(b:Chain) RETURN a, b", 2);
}

// ============================================================
// MERGE: realistic upsert patterns
// ============================================================

#[test]
fn merge_upsert_pattern_counter() {
    let db = TestDb::new();
    // Simulate 10 page views using MERGE + ON MATCH increment
    db.run("MERGE (c:PageCounter {page: '/home'}) ON CREATE SET c.views = 1 ON MATCH SET c.views = c.views + 1");
    for _ in 0..9 {
        db.run("MERGE (c:PageCounter {page: '/home'}) ON CREATE SET c.views = 1 ON MATCH SET c.views = c.views + 1");
    }
    let rows = db.run("MATCH (c:PageCounter {page: '/home'}) RETURN c.views AS views");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["views"], 10);
}

#[test]
fn merge_upsert_last_seen_timestamp() {
    let db = TestDb::new();
    db.run("MERGE (u:Session {user: 'alice'}) ON CREATE SET u.count = 1, u.status = 'new' ON MATCH SET u.count = u.count + 1, u.status = 'returning'");
    let rows =
        db.run("MATCH (u:Session {user: 'alice'}) RETURN u.status AS status, u.count AS cnt");
    assert_eq!(rows[0]["status"], "new");
    assert_eq!(rows[0]["cnt"], 1);

    db.run("MERGE (u:Session {user: 'alice'}) ON CREATE SET u.count = 1, u.status = 'new' ON MATCH SET u.count = u.count + 1, u.status = 'returning'");
    let rows =
        db.run("MATCH (u:Session {user: 'alice'}) RETURN u.status AS status, u.count AS cnt");
    assert_eq!(rows[0]["status"], "returning");
    assert_eq!(rows[0]["cnt"], 2);
}

// ============================================================
// MERGE with UNWIND: batch upsert
// ============================================================

#[test]
fn merge_batch_upsert_via_unwind() {
    let db = TestDb::new();
    // First batch
    db.run("UNWIND ['a', 'b', 'c'] AS name MERGE (:Item {name: name})");
    db.assert_count("MATCH (i:Item) RETURN i", 3);
    // Overlapping batch — should not create duplicates
    db.run("UNWIND ['b', 'c', 'd', 'e'] AS name MERGE (:Item {name: name})");
    db.assert_count("MATCH (i:Item) RETURN i", 5);
}

// ============================================================
// MERGE relationship: conditional edge creation
// ============================================================

#[test]
fn merge_relationship_conditional_on_properties() {
    let db = TestDb::new();
    db.run("CREATE (:Person {name:'Alice'}), (:Person {name:'Bob'})");
    // MERGE a KNOWS relationship with specific properties
    db.run("MATCH (a:Person {name:'Alice'}), (b:Person {name:'Bob'}) MERGE (a)-[r:KNOWS {context: 'work'}]->(b)");
    db.assert_count("MATCH ()-[r:KNOWS]->() RETURN r", 1);
    // MERGE with different property -> creates new edge
    db.run("MATCH (a:Person {name:'Alice'}), (b:Person {name:'Bob'}) MERGE (a)-[r:KNOWS {context: 'school'}]->(b)");
    db.assert_count("MATCH ()-[r:KNOWS]->() RETURN r", 2);
    // Same property again -> no new edge
    db.run("MATCH (a:Person {name:'Alice'}), (b:Person {name:'Bob'}) MERGE (a)-[r:KNOWS {context: 'work'}]->(b)");
    db.assert_count("MATCH ()-[r:KNOWS]->() RETURN r", 2);
}

// ============================================================
// Future MERGE tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn merge_with_call_subquery() {
    let db = TestDb::new();
    db.run("CREATE (:Source {name: 'data', val: 42})");
    let _rows = db.run(
        "CALL { MATCH (s:Source) RETURN s.val AS v } \
         MERGE (t:Target {val: v})",
    );
}
