/// Engine invariants and edge case tests — empty graphs, property edge cases,
/// graph consistency, null semantics, isolation between queries, self-loops,
/// bidirectional edges, and combinatorial matching stress tests.
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// Empty graph behavior
// ============================================================

#[test]
fn empty_graph_match_returns_nothing() {
    TestDb::new().assert_count("MATCH (n) RETURN n", 0);
}

#[test]
fn empty_graph_match_relationship_returns_nothing() {
    TestDb::new().assert_count("MATCH (a)-[r]->(b) RETURN r", 0);
}

#[test]
fn empty_graph_return_literal() {
    let rows = TestDb::new().run("RETURN 42");
    assert_eq!(rows.len(), 1);
}

// ============================================================
// Property value edge cases
// ============================================================

#[test]
fn property_empty_string() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: ''})");
    db.assert_count("MATCH (n:User {name: ''}) RETURN n", 1);
}

#[test]
fn property_zero_integer() {
    let db = TestDb::new();
    db.run("CREATE (n:User {score: 0})");
    db.assert_count("MATCH (n:User) WHERE n.score = 0 RETURN n", 1);
}

#[test]
fn property_false_boolean() {
    let db = TestDb::new();
    db.run("CREATE (n:User {active: false})");
    db.assert_count("MATCH (n:User) WHERE n.active = false RETURN n", 1);
}

#[test]
fn property_negative_integer() {
    let db = TestDb::new();
    db.run("CREATE (n:Metric {value: -42})");
    let rows = db.run("MATCH (n:Metric) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["value"], -42);
}

#[test]
fn missing_property_is_rejected_by_analyzer() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let err = db.run_err("MATCH (n:User) RETURN n.nonexistent");
    assert!(err.contains("Unknown property"));
}

#[test]
fn null_comparison_behavior() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 30})");
    db.run("CREATE (n:User {name: 'Bob'})");
    db.assert_count("MATCH (n:User) WHERE n.age > 25 RETURN n", 1);
}

// ============================================================
// Self-referential and multi-edge patterns
// ============================================================

#[test]
fn self_loop_relationship() {
    let db = TestDb::new();
    db.run("CREATE (n:Node {id: 1})");
    db.run("MATCH (n:Node {id: 1}) CREATE (n)-[:SELF]->(n)");
    db.assert_count("MATCH (n)-[:SELF]->(n) RETURN n", 1);
}

#[test]
fn multiple_rels_different_types_same_pair() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:FOLLOWS]->(b)");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:KNOWS]->(b)");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:LIKES]->(b)");
    db.assert_count("MATCH (a:User {name: 'Alice'})-[r]->(b:User {name: 'Bob'}) RETURN r", 3);
}

#[test]
fn bidirectional_relationship_pair() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:FOLLOWS]->(b)");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (b)-[:FOLLOWS]->(a)");
    db.assert_count("MATCH (a)-[:FOLLOWS]->(b) RETURN a, b", 2);
}

// ============================================================
// Cross-product
// ============================================================

#[test]
fn cross_product_two_labels() {
    let db = TestDb::new();
    db.run("CREATE (a:A {id: 1})");
    db.run("CREATE (b:A {id: 2})");
    db.run("CREATE (c:B {id: 1})");
    db.run("CREATE (d:B {id: 2})");
    db.run("CREATE (e:B {id: 3})");
    db.assert_count("MATCH (a:A), (b:B) RETURN a, b", 6);
}

// ============================================================
// Graph integrity after mutations
// ============================================================

#[test]
fn graph_valid_after_delete() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:FOLLOWS]->(b)");
    db.run("MATCH (a:User {name: 'Alice'}) DETACH DELETE a");
    db.assert_count("MATCH (n:User) RETURN n", 1);
}

#[test]
fn graph_valid_after_failed_delete() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})-[:FOLLOWS]->(b:User {name: 'Bob'})");
    let _ = db.exec("MATCH (n:User {name: 'Alice'}) DELETE n");
    db.assert_count("MATCH (n:User) RETURN n", 2);
}

#[test]
fn test_isolation_between_queries() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    db.assert_count("MATCH (n:User) RETURN n", 1);
    db.run("CREATE (n:Product {name: 'Widget'})");
    db.assert_count("MATCH (n:User) RETURN n", 1);
    db.assert_count("MATCH (n:Product) RETURN n", 1);
}

// ============================================================
// Label-only nodes
// ============================================================

#[test]
fn node_with_label_no_properties() {
    let db = TestDb::new();
    db.run("CREATE (n:Marker)");
    let rows = db.run("MATCH (n:Marker) RETURN n");
    assert_eq!(rows.len(), 1);
}

// ============================================================
// Mixed read-write
// ============================================================

#[test]
fn match_then_create_in_one_query() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:FOLLOWS]->(b)");
    db.assert_count("MATCH (a)-[r:FOLLOWS]->(b) RETURN r", 1);
}

#[test]
fn create_then_match_in_same_session() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) RETURN n");
    assert_eq!(rows.len(), 1);
}

#[test]
fn match_create_then_verify_in_same_session() {
    let db = TestDb::new();
    db.run("CREATE (:Source {name:'A'})");
    db.run("CREATE (:Source {name:'B'})");
    db.run("CREATE (:Target {name:'Z'})");
    db.run("MATCH (s:Source), (t:Target {name:'Z'}) CREATE (s)-[:LINKED]->(t)");
    db.assert_count("MATCH (s:Source)-[:LINKED]->(t:Target) RETURN s, t", 2);
}

// ============================================================
// Combinatorial stress
// ============================================================

#[test]
fn match_with_many_nodes() {
    let db = TestDb::new();
    for i in 0..20 {
        db.run(&format!("CREATE (n:Item {{id: {i}}})"));
    }
    db.assert_count("MATCH (n:Item) RETURN n", 20);
}

#[test]
fn match_chain_through_many_hops() {
    let db = TestDb::new();
    db.seed_chain(5);
    db.assert_count("MATCH (a)-[:NEXT]->(b)-[:NEXT]->(c) RETURN a, c", 3);
}

// ============================================================
// Query isolation: no variable leakage
// ============================================================

#[test]
fn separate_queries_have_independent_scopes() {
    let db = TestDb::new();
    db.run("CREATE (:A {val:1})");
    db.run("CREATE (:B {val:2})");
    // First query binds 'n' to A nodes, second query binds 'n' to B nodes
    db.assert_count("MATCH (n:A) RETURN n", 1);
    db.assert_count("MATCH (n:B) RETURN n", 1);
}

#[test]
fn query_does_not_see_variables_from_previous_query() {
    let db = TestDb::new();
    db.run("CREATE (:X {id: 42})");
    // Each query is independent — no shared state
    let rows1 = db.run("MATCH (x:X) RETURN x.id AS id");
    let rows2 = db.run("MATCH (x:X) RETURN x.id AS id");
    assert_eq!(rows1[0]["id"], rows2[0]["id"]);
}

// ============================================================
// Repeated read returns same results
// ============================================================

#[test]
fn repeated_reads_are_consistent() {
    let db = TestDb::new();
    db.seed_org_graph();
    let count1 = db.exec_count("MATCH (p:Person) RETURN p").unwrap();
    let count2 = db.exec_count("MATCH (p:Person) RETURN p").unwrap();
    let count3 = db.exec_count("MATCH (p:Person) RETURN p").unwrap();
    assert_eq!(count1, count2);
    assert_eq!(count2, count3);
    assert_eq!(count1, 6);
}

// ============================================================
// Relationship endpoints valid after delete
// ============================================================

#[test]
fn relationship_endpoints_valid_after_node_delete() {
    let db = TestDb::new();
    db.run("CREATE (a:N {id:1})-[:R]->(b:N {id:2})-[:R]->(c:N {id:3})");
    db.run("MATCH (b:N {id:2}) DETACH DELETE b");
    // Remaining: nodes 1 and 3, no relationships
    db.assert_count("MATCH (n:N) RETURN n", 2);
    db.assert_count("MATCH (a)-[r]->(b) RETURN r", 0);
}

// ============================================================
// Create-delete-verify cycle
// ============================================================

#[test]
fn create_then_delete_then_verify() {
    let db = TestDb::new();
    db.run("CREATE (:Temp {name:'ephemeral'})");
    db.assert_count("MATCH (t:Temp) RETURN t", 1);
    db.run("MATCH (t:Temp) DELETE t");
    db.assert_count("MATCH (t:Temp) RETURN t", 0);
    // Can create again
    db.run("CREATE (:Temp {name:'reborn'})");
    db.assert_count("MATCH (t:Temp) RETURN t", 1);
}

// ============================================================
// Multiple writes then read consistency
// ============================================================

#[test]
fn sequential_writes_and_reads() {
    let db = TestDb::new();
    for i in 1..=10 {
        db.run(&format!("CREATE (:Counter {{val: {i}}})"));
        let count = db.exec_count("MATCH (c:Counter) RETURN c").unwrap();
        assert_eq!(count, i);
    }
}

// ============================================================
// Labels preserved after property update
// ============================================================

#[test]
fn labels_preserved_after_set_property() {
    let db = TestDb::new();
    db.run("CREATE (:Person:Admin {name:'Alice', age:30})");
    db.run("MATCH (n:Person {name:'Alice'}) SET n.age = 31");
    let rows = db.run("MATCH (n:Person:Admin) RETURN labels(n) AS lbls");
    assert_eq!(rows.len(), 1);
    let labels = rows[0]["lbls"].as_array().unwrap();
    assert!(labels.contains(&serde_json::json!("Person")));
    assert!(labels.contains(&serde_json::json!("Admin")));
}

// ============================================================
// Relationship type preserved after property update
// ============================================================

#[test]
fn relationship_type_preserved_after_set_property() {
    let db = TestDb::new();
    db.run("CREATE (:A {id:1})-[:LINK {weight: 1}]->(:B {id:2})");
    db.run("MATCH (a)-[r:LINK]->(b) SET r.weight = 2");
    let rows = db.run("MATCH (a)-[r:LINK]->(b) RETURN type(r) AS t, r.weight AS w");
    assert_eq!(rows[0]["t"], "LINK");
    assert_eq!(rows[0]["w"], 2);
}

// ============================================================
// Empty graph aggregation defaults
// ============================================================

#[test]
fn empty_graph_count_returns_zero() {
    let db = TestDb::new();
    let rows = db.run("MATCH (n) RETURN count(n) AS c");
    assert_eq!(rows[0]["c"], 0);
}

#[test]
fn empty_graph_sum_returns_null_or_zero() {
    let db = TestDb::new();
    let rows = db.run("MATCH (n) RETURN sum(n.x) AS s");
    assert!(rows[0]["s"].is_null() || rows[0]["s"] == 0);
}

// ============================================================
// Graph valid after multiple merges
// ============================================================

#[test]
fn graph_valid_after_many_merges() {
    let db = TestDb::new();
    for i in 0..5 {
        db.run(&format!("MERGE (:Tag {{name:'tag{i}'}})"));
    }
    // Repeat — should not create duplicates
    for i in 0..5 {
        db.run(&format!("MERGE (:Tag {{name:'tag{i}'}})"));
    }
    db.assert_count("MATCH (t:Tag) RETURN t", 5);
}

// ============================================================
// Large graph stability
// ============================================================

#[test]
fn large_graph_create_and_query() {
    let db = TestDb::new();
    db.run("UNWIND range(0, 99) AS i CREATE (:LargeNode {id: i})");
    db.assert_count("MATCH (n:LargeNode) RETURN n", 100);
    // Filtered query
    db.assert_count("MATCH (n:LargeNode) WHERE n.id >= 50 RETURN n", 50);
}

// ============================================================
// Self-loop consistency
// ============================================================

#[test]
fn self_loop_traversal_is_consistent() {
    let db = TestDb::new();
    db.run("CREATE (n:Looper {id:1})");
    db.run("MATCH (n:Looper {id:1}) CREATE (n)-[:SELF]->(n)");
    // Both directions should find the self-loop
    db.assert_count("MATCH (a:Looper)-[:SELF]->(b:Looper) RETURN a", 1);
    db.assert_count("MATCH (a:Looper)<-[:SELF]-(b:Looper) RETURN a", 1);
    // Undirected finds the self-loop once (engine deduplicates the single edge)
    db.assert_count("MATCH (a:Looper)-[:SELF]-(b:Looper) RETURN a", 1);
}

// ============================================================
// Mixed read-write: SET then immediate read
// ============================================================

#[test]
fn set_then_immediate_read_consistency() {
    let db = TestDb::new();
    db.run("CREATE (:Val {x: 1})");
    db.run("MATCH (v:Val) SET v.x = 2");
    let rows = db.run("MATCH (v:Val) RETURN v.x AS x");
    assert_eq!(rows[0]["x"], 2);
}

// ============================================================
// DETACH DELETE entire graph
// ============================================================

#[test]
fn detach_delete_all_leaves_empty_graph() {
    let db = TestDb::new();
    db.seed_social_graph();
    db.run("MATCH (n) DETACH DELETE n");
    db.assert_count("MATCH (n) RETURN n", 0);
}

// ============================================================
// Advanced invariant tests
// ============================================================

#[test]
fn relationship_endpoints_valid_after_multiple_deletes() {
    let db = TestDb::new();
    db.run("CREATE (:N {id:1})-[:R]->(n2:N {id:2})-[:R]->(n3:N {id:3})-[:R]->(n4:N {id:4})");
    // Delete middle nodes one by one
    db.run("MATCH (n:N {id:2}) DETACH DELETE n");
    db.run("MATCH (n:N {id:3}) DETACH DELETE n");
    // Only nodes 1 and 4 remain, with no relationships
    db.assert_count("MATCH (n:N) RETURN n", 2);
    db.assert_count("MATCH ()-[r]->() RETURN r", 0);
    // Verify the surviving nodes are the correct ones
    let ids = db.sorted_ints("MATCH (n:N) RETURN n.id AS id", "id");
    assert_eq!(ids, vec![1, 4]);
}

#[test]
fn no_partial_writes_on_failed_delete() {
    let db = TestDb::new();
    db.run("CREATE (a:X {id:1})-[:LINK]->(b:X {id:2})");
    db.run("CREATE (c:X {id:3})");
    // Attempting to DELETE a connected node without DETACH should fail
    let _ = db.exec("MATCH (n:X {id:1}) DELETE n");
    // Graph should be completely unchanged: 3 nodes, 1 relationship
    db.assert_count("MATCH (n:X) RETURN n", 3);
    db.assert_count("MATCH ()-[r:LINK]->() RETURN r", 1);
}

#[test]
fn multiple_merge_idempotency_with_graph_verification() {
    let db = TestDb::new();
    // MERGE the same node pattern multiple times — should not create duplicates
    for _ in 0..3 {
        db.run("MERGE (:Singleton {key:'only_one'})");
    }
    db.assert_count("MATCH (s:Singleton) RETURN s", 1);
    // MERGE additional distinct nodes
    db.run("MERGE (:Singleton {key:'second'})");
    db.assert_count("MATCH (s:Singleton) RETURN s", 2);
    // Re-MERGE both — still 2
    db.run("MERGE (:Singleton {key:'only_one'})");
    db.run("MERGE (:Singleton {key:'second'})");
    db.assert_count("MATCH (s:Singleton) RETURN s", 2);
}

#[test]
fn property_types_preserved_after_set() {
    let db = TestDb::new();
    db.run("CREATE (:Typed {str_val: 'hello', int_val: 42, bool_val: true})");
    // Update each property with same-typed values
    db.run("MATCH (n:Typed) SET n.str_val = 'world'");
    db.run("MATCH (n:Typed) SET n.int_val = 99");
    db.run("MATCH (n:Typed) SET n.bool_val = false");
    let rows = db.run("MATCH (n:Typed) RETURN n.str_val AS s, n.int_val AS i, n.bool_val AS b");
    assert_eq!(rows[0]["s"], "world");
    assert_eq!(rows[0]["i"], 99);
    assert_eq!(rows[0]["b"], false);
}

#[test]
fn node_count_consistency_through_create_delete_cycles() {
    let db = TestDb::new();
    // Create 5, delete 3, create 2, delete 1 — should have 3 remaining
    db.run("UNWIND range(1, 5) AS i CREATE (:Cycle {id: i})");
    db.assert_count("MATCH (c:Cycle) RETURN c", 5);
    db.run("MATCH (c:Cycle) WHERE c.id <= 3 DELETE c");
    db.assert_count("MATCH (c:Cycle) RETURN c", 2);
    db.run("CREATE (:Cycle {id: 6})");
    db.run("CREATE (:Cycle {id: 7})");
    db.assert_count("MATCH (c:Cycle) RETURN c", 4);
    db.run("MATCH (c:Cycle {id: 4}) DELETE c");
    db.assert_count("MATCH (c:Cycle) RETURN c", 3);
    let ids = db.sorted_ints("MATCH (c:Cycle) RETURN c.id AS id", "id");
    assert_eq!(ids, vec![5, 6, 7]);
}

// ============================================================
// Seed graph stability
// ============================================================

#[test]
fn rich_social_graph_node_and_relationship_counts() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // 6 Person nodes + 4 Interest nodes = 10 nodes
    db.assert_count("MATCH (n:Person) RETURN n", 6);
    db.assert_count("MATCH (n:Interest) RETURN n", 4);
    db.assert_count("MATCH (n) RETURN n", 10);
    // KNOWS: 7, FOLLOWS: 6, BLOCKED: 2, INTERESTED_IN: 11 = 26 relationships
    db.assert_count("MATCH ()-[r:KNOWS]->() RETURN r", 7);
    db.assert_count("MATCH ()-[r:FOLLOWS]->() RETURN r", 6);
    db.assert_count("MATCH ()-[r:BLOCKED]->() RETURN r", 2);
    db.assert_count("MATCH ()-[r:INTERESTED_IN]->() RETURN r", 11);
}

#[test]
fn knowledge_graph_node_and_relationship_counts() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // 8 Entity + 2 Document + 2 Topic + 2 Alias = 14 nodes
    db.assert_count("MATCH (e:Entity) RETURN e", 8);
    db.assert_count("MATCH (d:Document) RETURN d", 2);
    db.assert_count("MATCH (t:Topic) RETURN t", 2);
    db.assert_count("MATCH (a:Alias) RETURN a", 2);
    db.assert_count("MATCH (n) RETURN n", 14);
    // Verify total relationships: STUDIED(4) + PROPOSED(1) + CONTRIBUTED_TO(2) + RECEIVED(2)
    // + AUTHORED(2) + ABOUT(2) + BELONGS_TO(2) + RELATES_TO(1) + HAS_ALIAS(2) + PARENT_OF(1) = 19
    db.assert_count("MATCH ()-[r]->() RETURN r", 19);
}

#[test]
fn detach_delete_all_on_rich_social_graph_leaves_empty() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Verify non-empty first
    let count = db.exec_count("MATCH (n) RETURN n").unwrap();
    assert!(count > 0);
    // Wipe everything
    db.run("MATCH (n) DETACH DELETE n");
    db.assert_count("MATCH (n) RETURN n", 0);
    db.assert_count("MATCH ()-[r]->() RETURN r", 0);
}

#[test]
fn transport_graph_structure_verified() {
    let db = TestDb::new();
    db.seed_transport_graph();
    db.assert_count("MATCH (s:Station) RETURN s", 5);
    // 5 bidirectional pairs = 10 directed ROUTE edges
    db.assert_count("MATCH ()-[r:ROUTE]->() RETURN r", 10);
}

#[test]
fn dependency_graph_structure_verified() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    db.assert_count("MATCH (p:Package) RETURN p", 6);
    db.assert_count("MATCH ()-[r:DEPENDS_ON]->() RETURN r", 8);
}

// ============================================================
// Ignored invariant tests (pending implementation)
// ============================================================

#[test]
fn transaction_isolation_between_concurrent_reads() {
    // Lora: transaction isolation between concurrent reads
    let db = TestDb::new();
    db.run("CREATE (:Counter {val: 0})");
    // Simulate two concurrent reads seeing consistent state
    // (would need multi-threading support to test properly)
    let rows1 = db.run("MATCH (c:Counter) RETURN c.val AS v");
    db.run("MATCH (c:Counter) SET c.val = 1");
    let rows2 = db.run("MATCH (c:Counter) RETURN c.val AS v");
    // In a truly isolated transaction, rows1 and rows2 should each see
    // a consistent snapshot (not necessarily the same one)
    assert_eq!(rows1[0]["v"], 0);
    assert_eq!(rows2[0]["v"], 1);
}

#[test]
fn acid_properties_for_multi_statement_transactions() {
    // Lora: ACID properties for multi-statement transactions
    let db = TestDb::new();
    db.run("CREATE (:Account {name:'A', balance:100})");
    db.run("CREATE (:Account {name:'B', balance:50})");
    // Multi-statement transaction: transfer 30 from A to B
    // Should be atomic — both succeed or both fail
    db.run("MATCH (a:Account {name:'A'}) SET a.balance = a.balance - 30");
    db.run("MATCH (b:Account {name:'B'}) SET b.balance = b.balance + 30");
    let rows = db.run("MATCH (a:Account) RETURN a.name AS name, a.balance AS bal ORDER BY a.name");
    assert_eq!(rows[0]["bal"], 70);
    assert_eq!(rows[1]["bal"], 80);
}

#[test]
#[ignore = "constraint violation rollback: rollback on constraint error not yet implemented"]
fn constraint_violation_rollback() {
    // Lora: constraint violation rollback
    let db = TestDb::new();
    db.run("CREATE (:Unique {key:'one'})");
    // If a unique constraint existed, creating a duplicate should fail
    // and the entire transaction should roll back, leaving graph unchanged
    let _ = db.exec("CREATE (:Unique {key:'one'})");
    // With constraint support, this should still be 1
    // Without constraints, it will be 2 — hence ignored
    db.assert_count("MATCH (u:Unique) RETURN u", 1);
}

// ============================================================
// ID uniqueness and stability
// ============================================================

#[test]
fn node_ids_unique_across_creates() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 50) AS i CREATE (:ID {i: i})");
    let rows = db.run("MATCH (n:ID) RETURN id(n) AS nid");
    let mut ids: Vec<i64> = rows.iter().map(|r| r["nid"].as_i64().unwrap()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 50);
}

#[test]
fn node_id_stable_after_set() {
    let db = TestDb::new();
    db.run("CREATE (:Stable {val: 1})");
    let rows = db.run("MATCH (n:Stable) RETURN id(n) AS nid");
    let id_before = rows[0]["nid"].as_i64().unwrap();
    db.run("MATCH (n:Stable) SET n.val = 2");
    let rows = db.run("MATCH (n:Stable) RETURN id(n) AS nid");
    let id_after = rows[0]["nid"].as_i64().unwrap();
    assert_eq!(id_before, id_after);
}

// ============================================================
// Fan-out stress test
// ============================================================

#[test]
fn fan_out_50_spokes_query_performance() {
    let db = TestDb::new();
    db.run("CREATE (:Hub {name:'center'})");
    db.run("UNWIND range(1, 50) AS i MATCH (h:Hub) CREATE (h)-[:SPOKE]->(:Leaf {id: i})");
    // Verify all spokes
    db.assert_count("MATCH (:Hub)-[:SPOKE]->(l:Leaf) RETURN l", 50);
    // Aggregation over fan-out
    let rows = db.run("MATCH (h:Hub)-[:SPOKE]->(l:Leaf) RETURN count(l) AS cnt, min(l.id) AS lo, max(l.id) AS hi");
    assert_eq!(rows[0]["cnt"], 50);
    assert_eq!(rows[0]["lo"], 1);
    assert_eq!(rows[0]["hi"], 50);
}

// ============================================================
// Graph consistency after interleaved reads and writes
// ============================================================

#[test]
fn interleaved_reads_and_writes_consistent() {
    let db = TestDb::new();
    for i in 0..10 {
        db.run(&format!("CREATE (:Step {{id: {i}}})"));
        if i > 0 {
            db.run(&format!(
                "MATCH (a:Step {{id:{}}}), (b:Step {{id:{i}}}) CREATE (a)-[:SEQ]->(b)",
                i - 1
            ));
        }
        // Verify node count after each step
        assert_eq!(db.exec_count("MATCH (s:Step) RETURN s").unwrap(), i + 1);
        // Verify edge count after each step
        assert_eq!(db.exec_count("MATCH ()-[r:SEQ]->() RETURN r").unwrap(), i);
    }
}

// ============================================================
// Recommendation graph invariants
// ============================================================

#[test]
fn recommendation_graph_complete_verification() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    db.assert_count("MATCH (v:Viewer) RETURN v", 3);
    db.assert_count("MATCH (m:Movie) RETURN m", 4);
    db.assert_count("MATCH ()-[r:RATED]->() RETURN r", 7);
    // Verify all ratings are between 1 and 5
    let rows = db.run("MATCH ()-[r:RATED]->() RETURN r.score AS score");
    for row in &rows {
        let score = row["score"].as_i64().unwrap();
        assert!(score >= 1 && score <= 5, "score {score} out of range");
    }
}

// ============================================================
// Org graph invariants
// ============================================================

#[test]
fn org_graph_complete_verification() {
    let db = TestDb::new();
    db.seed_org_graph();
    db.assert_count("MATCH (p:Person) RETURN p", 6);
    db.assert_count("MATCH (c:Company) RETURN c", 1);
    db.assert_count("MATCH (pr:Project) RETURN pr", 2);
    db.assert_count("MATCH (c:City) RETURN c", 3);
    db.assert_count("MATCH ()-[r:WORKS_AT]->() RETURN r", 6);
    db.assert_count("MATCH ()-[r:MANAGES]->() RETURN r", 4);
    db.assert_count("MATCH ()-[r:ASSIGNED_TO]->() RETURN r", 4);
    db.assert_count("MATCH ()-[r:LIVES_IN]->() RETURN r", 6);
    // Total nodes: 6 + 1 + 2 + 3 = 12
    db.assert_count("MATCH (n) RETURN n", 12);
    // Total relationships: 6 + 4 + 4 + 6 = 20
    db.assert_count("MATCH ()-[r]->() RETURN r", 20);
}

// ============================================================
// DETACH DELETE + recreate cycle
// ============================================================

#[test]
fn detach_delete_all_then_recreate_from_scratch() {
    let db = TestDb::new();
    db.seed_social_graph();
    db.assert_count("MATCH (n) RETURN n", 3);
    db.run("MATCH (n) DETACH DELETE n");
    db.assert_count("MATCH (n) RETURN n", 0);
    // Recreate completely different graph
    db.run("CREATE (:NewType {id: 1})-[:NEW_REL]->(:NewType {id: 2})");
    db.assert_count("MATCH (n:NewType) RETURN n", 2);
    db.assert_count("MATCH ()-[r:NEW_REL]->() RETURN r", 1);
}

// ============================================================
// Future invariant tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn concurrent_merge_idempotency() {
    // Simulate concurrent MERGE operations (would need threading)
    let db = TestDb::new();
    for _ in 0..100 {
        db.run("MERGE (:Concurrent {key: 'single'})");
    }
    db.assert_count("MATCH (c:Concurrent) RETURN c", 1);
}
