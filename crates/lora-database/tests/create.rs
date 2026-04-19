/// CREATE clause tests — node creation, relationship creation, pattern creation,
/// property types, edge cases.
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// Node creation
// ============================================================

#[test]
fn create_node_no_labels_no_properties() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n) RETURN n");
    assert_eq!(rows.len(), 1);
}

#[test]
fn create_node_with_single_label() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:Person) RETURN n");
    assert_eq!(rows.len(), 1);
    let labels = rows[0]["n"]["labels"].as_array().unwrap();
    assert!(labels.contains(&serde_json::json!("Person")));
}

#[test]
fn create_node_with_multiple_labels() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:Person:Employee) RETURN n");
    let labels = rows[0]["n"]["labels"].as_array().unwrap();
    assert!(labels.contains(&serde_json::json!("Person")));
    assert!(labels.contains(&serde_json::json!("Employee")));
}

#[test]
fn create_and_match_triple_label() {
    let db = TestDb::new();
    db.run("CREATE (:A:B:C {name:'multi'})");
    db.assert_count("MATCH (n:A:B:C) RETURN n", 1);
    db.assert_count("MATCH (n:A:B) RETURN n", 1);
    db.assert_count("MATCH (n:A) RETURN n", 1);
}

#[test]
fn create_node_with_string_property() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:User {name: 'Alice'}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["name"], "Alice");
}

#[test]
fn create_node_with_integer_property() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:User {age: 42}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["age"], 42);
}

#[test]
fn create_node_with_boolean_property() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:User {active: true}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["active"], true);
}

#[test]
fn create_node_with_float_property() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:Metric {score: 3.14}) RETURN n");
    let score = rows[0]["n"]["properties"]["score"].as_f64().unwrap();
    assert!((score - 3.14).abs() < 0.001);
}

#[test]
fn create_node_with_multiple_properties() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:User {name: 'Alice', age: 30, active: true}) RETURN n");
    let props = &rows[0]["n"]["properties"];
    assert_eq!(props["name"], "Alice");
    assert_eq!(props["age"], 30);
    assert_eq!(props["active"], true);
}

#[test]
fn create_node_with_null_property() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:User {name: null}) RETURN n");
    assert_eq!(rows.len(), 1);
}

#[test]
fn create_node_with_list_property() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:User {tags: [1, 2, 3]}) RETURN n");
    assert!(rows[0]["n"]["properties"]["tags"].is_array());
}

#[test]
fn create_node_with_empty_string_property() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:User {name: ''}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["name"], "");
}

#[test]
fn create_multiple_nodes_sequential() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.assert_count("MATCH (n:User) RETURN n", 2);
}

#[test]
fn create_without_return() {
    let db = TestDb::new();
    let result = db.exec("CREATE (n:User {name: 'Alice'})");
    assert!(result.is_ok());
    db.assert_count("MATCH (n:User) RETURN n", 1);
}

#[test]
fn create_node_ids_are_unique() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    let rows = db.run("MATCH (n:User) RETURN n");
    let id_a = rows[0]["n"]["id"].as_i64().unwrap();
    let id_b = rows[1]["n"]["id"].as_i64().unwrap();
    assert_ne!(id_a, id_b);
}

#[test]
fn create_preserves_existing_data() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name:'first'})");
    db.run("CREATE (:Item {name:'second'})");
    db.run("CREATE (:Item {name:'third'})");
    db.assert_count("MATCH (i:Item) RETURN i", 3);
}

// ============================================================
// Relationship creation
// ============================================================

#[test]
fn create_relationship_between_matched_nodes() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:FOLLOWS]->(b)");
    db.assert_count("MATCH (a)-[:FOLLOWS]->(b) RETURN a, b", 1);
}

#[test]
fn create_relationship_with_properties() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:FOLLOWS {since: 2020}]->(b)");
    let rows = db.run("MATCH (a)-[r:FOLLOWS]->(b) RETURN r");
    assert_eq!(rows[0]["r"]["properties"]["since"], 2020);
}

#[test]
fn create_relationship_with_variable() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    let rows = db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[r:FOLLOWS]->(b) RETURN r");
    assert_eq!(rows[0]["r"]["type"], "FOLLOWS");
}

#[test]
fn create_multiple_relationships_same_type() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("CREATE (c:User {name: 'Carol'})");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:FOLLOWS]->(b)");
    db.run("MATCH (a:User {name: 'Alice'}), (c:User {name: 'Carol'}) CREATE (a)-[:FOLLOWS]->(c)");
    db.assert_count("MATCH (a:User {name: 'Alice'})-[:FOLLOWS]->(b) RETURN b", 2);
}

// ============================================================
// Pattern creation
// ============================================================

#[test]
fn create_pattern_node_and_relationship() {
    let db = TestDb::new();
    let rows = db.run("CREATE (a:User {name: 'Alice'})-[:FOLLOWS]->(b:User {name: 'Bob'}) RETURN a, b");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["a"]["properties"]["name"], "Alice");
    assert_eq!(rows[0]["b"]["properties"]["name"], "Bob");
}

#[test]
fn create_chain_pattern() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})-[:FOLLOWS]->(b:User {name: 'Bob'})-[:FOLLOWS]->(c:User {name: 'Carol'}) RETURN a, b, c");
    db.assert_count("MATCH (a)-[:FOLLOWS]->(b) RETURN a, b", 2);
}

#[test]
fn create_self_referential_relationship() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("MATCH (a:User {name: 'Alice'}) CREATE (a)-[:SELF_REF]->(a)");
    db.assert_count("MATCH (a:User {name: 'Alice'})-[:SELF_REF]->(a) RETURN a", 1);
}

#[test]
fn create_parallel_edges_between_same_nodes() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:FOLLOWS]->(b)");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:KNOWS]->(b)");
    db.assert_count("MATCH (a:User {name: 'Alice'})-[r]->(b:User {name: 'Bob'}) RETURN r", 2);
}

#[test]
fn create_duplicate_relationship_type_same_pair() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:FOLLOWS]->(b)");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:FOLLOWS]->(b)");
    db.assert_count("MATCH (a:User {name: 'Alice'})-[r:FOLLOWS]->(b:User {name: 'Bob'}) RETURN r", 2);
}

#[test]
fn create_company_with_employees_and_verify() {
    let db = TestDb::new();
    db.run("CREATE (:Dept {name:'Sales'})");
    db.run("CREATE (:Emp {name:'Alice', salary: 70000})");
    db.run("CREATE (:Emp {name:'Bob', salary: 55000})");
    db.run("MATCH (e:Emp {name:'Alice'}), (d:Dept {name:'Sales'}) CREATE (e)-[:IN_DEPT]->(d)");
    db.run("MATCH (e:Emp {name:'Bob'}),   (d:Dept {name:'Sales'}) CREATE (e)-[:IN_DEPT]->(d)");
    db.assert_count("MATCH (e:Emp)-[:IN_DEPT]->(d:Dept {name:'Sales'}) RETURN e", 2);
}

// ============================================================
// UNWIND + CREATE
// ============================================================

#[test]
fn unwind_create_batch() {
    let db = TestDb::new();
    db.run("UNWIND [1, 2, 3, 4, 5] AS i CREATE (:Num {val: i})");
    db.assert_count("MATCH (n:Num) RETURN n", 5);
}

#[test]
fn unwind_create_with_relationship() {
    let db = TestDb::new();
    db.run("CREATE (:Hub {name:'center'})");
    db.run("UNWIND range(1, 3) AS i MATCH (h:Hub) CREATE (h)-[:SPOKE]->(s:Spoke {idx: i})");
    db.assert_count("MATCH (h:Hub)-[:SPOKE]->(s:Spoke) RETURN h, s", 3);
}

// ============================================================
// CREATE with map/list property
// ============================================================

#[test]
fn create_node_with_map_property() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:Config {settings: {theme: 'dark', lang: 'en'}}) RETURN n");
    assert!(rows[0]["n"]["properties"]["settings"].is_object());
}

#[test]
fn create_node_with_empty_properties() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:Empty {}) RETURN n");
    assert_eq!(rows.len(), 1);
}

// ============================================================
// Longer chain creation
// ============================================================

#[test]
fn create_chain_three_hops() {
    let db = TestDb::new();
    db.run(
        "CREATE (a:Node {id:1})-[:LINK]->(b:Node {id:2})-[:LINK]->(c:Node {id:3})-[:LINK]->(d:Node {id:4})",
    );
    db.assert_count("MATCH (a:Node)-[:LINK]->(b:Node) RETURN a, b", 3);
    db.assert_count("MATCH (n:Node) RETURN n", 4);
}

// ============================================================
// CREATE returning properties
// ============================================================

#[test]
fn create_returning_node_properties() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:User {name: 'Alice', age: 30}) RETURN n.name AS name, n.age AS age");
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[0]["age"], 30);
}

// ============================================================
// CREATE with mixed property types
// ============================================================

#[test]
fn create_node_with_mixed_type_properties() {
    let db = TestDb::new();
    let rows = db.run(
        "CREATE (n:Mixed {str: 'hello', num: 42, flt: 3.14, flag: true, nothing: null}) RETURN n",
    );
    let props = &rows[0]["n"]["properties"];
    assert_eq!(props["str"], "hello");
    assert_eq!(props["num"], 42);
    assert_eq!(props["flag"], true);
}

// ============================================================
// CREATE with bound variables from MATCH
// ============================================================

#[test]
fn create_relationship_reusing_matched_variable() {
    let db = TestDb::new();
    db.run("CREATE (:Person {name:'Alice'})");
    db.run("CREATE (:Person {name:'Bob'})");
    db.run("CREATE (:Person {name:'Carol'})");
    // Create FOLLOWS from everyone to Alice
    db.run(
        "MATCH (a:Person), (b:Person {name:'Alice'}) WHERE a.name <> 'Alice' \
         CREATE (a)-[:FOLLOWS]->(b)",
    );
    db.assert_count("MATCH (a)-[:FOLLOWS]->(b:Person {name:'Alice'}) RETURN a", 2);
}

// ============================================================
// Batch creation via UNWIND
// ============================================================

#[test]
fn create_large_batch_via_unwind() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 50) AS i CREATE (:Batch {id: i})");
    db.assert_count("MATCH (b:Batch) RETURN b", 50);
}

// ============================================================
// CREATE after WITH clause
// ============================================================

#[test]
fn create_after_with_clause() {
    let db = TestDb::new();
    db.run("CREATE (:Score {val: 10})");
    db.run("CREATE (:Score {val: 20})");
    db.run("CREATE (:Score {val: 30})");
    db.run("MATCH (s:Score) WITH sum(s.val) AS total CREATE (:Summary {total: total})");
    let rows = db.run("MATCH (s:Summary) RETURN s");
    assert_eq!(rows[0]["s"]["properties"]["total"], 60);
}

// ============================================================
// CREATE relationship with multiple property types
// ============================================================

#[test]
fn create_relationship_with_multiple_properties() {
    let db = TestDb::new();
    db.run("CREATE (a:N {id:1})-[:REL {weight: 5, label: 'primary', active: true}]->(b:N {id:2})");
    let rows = db.run("MATCH (a)-[r:REL]->(b) RETURN r");
    assert_eq!(rows[0]["r"]["properties"]["weight"], 5);
    assert_eq!(rows[0]["r"]["properties"]["label"], "primary");
    assert_eq!(rows[0]["r"]["properties"]["active"], true);
}

// ============================================================
// CREATE idempotency check — each CREATE always adds
// ============================================================

#[test]
fn create_always_adds_nodes() {
    let db = TestDb::new();
    db.run("CREATE (:Tag {name:'x'})");
    db.run("CREATE (:Tag {name:'x'})");
    db.run("CREATE (:Tag {name:'x'})");
    db.assert_count("MATCH (t:Tag {name:'x'}) RETURN t", 3);
}

// ============================================================
// Complex CREATE patterns
// ============================================================

#[test]
fn create_diamond_graph_in_single_statement() {
    let db = TestDb::new();
    // Diamond: top -> left, top -> right, left -> bottom, right -> bottom
    db.run("CREATE (top:D {name:'top'})");
    db.run("CREATE (left:D {name:'left'})");
    db.run("CREATE (right:D {name:'right'})");
    db.run("CREATE (bottom:D {name:'bottom'})");
    db.run("MATCH (t:D {name:'top'}), (l:D {name:'left'}) CREATE (t)-[:EDGE]->(l)");
    db.run("MATCH (t:D {name:'top'}), (r:D {name:'right'}) CREATE (t)-[:EDGE]->(r)");
    db.run("MATCH (l:D {name:'left'}), (b:D {name:'bottom'}) CREATE (l)-[:EDGE]->(b)");
    db.run("MATCH (r:D {name:'right'}), (b:D {name:'bottom'}) CREATE (r)-[:EDGE]->(b)");
    db.assert_count("MATCH (n:D) RETURN n", 4);
    db.assert_count("MATCH (a:D)-[:EDGE]->(b:D) RETURN a, b", 4);
    // Top has 2 outgoing
    db.assert_count("MATCH (t:D {name:'top'})-[:EDGE]->(x) RETURN x", 2);
    // Bottom has 2 incoming
    db.assert_count("MATCH (x)-[:EDGE]->(b:D {name:'bottom'}) RETURN x", 2);
}

#[test]
fn create_star_graph_hub_with_five_spokes_via_unwind() {
    let db = TestDb::new();
    db.run("CREATE (:Star {name:'hub'})");
    db.run("UNWIND range(1, 5) AS i MATCH (h:Star {name:'hub'}) CREATE (h)-[:ARM]->(s:Star {name:'spoke', idx: i})");
    db.assert_count("MATCH (h:Star {name:'hub'})-[:ARM]->(s) RETURN s", 5);
    db.assert_count("MATCH (n:Star) RETURN n", 6); // hub + 5 spokes
}

#[test]
fn create_with_boolean_and_list_property_combinations() {
    let db = TestDb::new();
    let rows = db.run(
        "CREATE (n:Combo {active: true, scores: [10, 20, 30], verified: false, tags: ['a', 'b']}) RETURN n",
    );
    let props = &rows[0]["n"]["properties"];
    assert_eq!(props["active"], true);
    assert_eq!(props["verified"], false);
    assert!(props["scores"].is_array());
    assert!(props["tags"].is_array());
}

#[test]
fn create_returning_computed_expression_from_created_node() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:Calc {val: 10}) RETURN n.val * 2 AS doubled");
    assert_eq!(rows[0]["doubled"], 20);
}

#[test]
fn create_bidirectional_edges_between_matched_nodes() {
    let db = TestDb::new();
    db.run("CREATE (:Peer {name:'A'})");
    db.run("CREATE (:Peer {name:'B'})");
    db.run("MATCH (a:Peer {name:'A'}), (b:Peer {name:'B'}) CREATE (a)-[:LINK]->(b)");
    db.run("MATCH (a:Peer {name:'A'}), (b:Peer {name:'B'}) CREATE (b)-[:LINK]->(a)");
    db.assert_count("MATCH (a:Peer {name:'A'})-[:LINK]->(b:Peer {name:'B'}) RETURN a", 1);
    db.assert_count("MATCH (b:Peer {name:'B'})-[:LINK]->(a:Peer {name:'A'}) RETURN b", 1);
    db.assert_count("MATCH (x:Peer)-[:LINK]->(y:Peer) RETURN x, y", 2);
}

#[test]
fn create_multiple_patterns_in_single_create() {
    let db = TestDb::new();
    db.run("CREATE (a:Multi {id:1})-[:E]->(b:Multi {id:2})-[:E]->(c:Multi {id:3})");
    db.assert_count("MATCH (n:Multi) RETURN n", 3);
    db.assert_count("MATCH (a:Multi)-[:E]->(b:Multi) RETURN a, b", 2);
}

// ============================================================
// CREATE + read verification patterns
// ============================================================

#[test]
fn create_then_match_with_complex_where_to_verify() {
    let db = TestDb::new();
    db.run("CREATE (:Product {name:'Widget', price: 25, category: 'tools'})");
    db.run("CREATE (:Product {name:'Gadget', price: 50, category: 'electronics'})");
    db.run("CREATE (:Product {name:'Gizmo', price: 15, category: 'tools'})");
    let rows = db.run(
        "MATCH (p:Product) WHERE p.category = 'tools' AND p.price > 20 RETURN p.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Widget");
}

#[test]
fn create_chain_and_verify_hop_count() {
    let db = TestDb::new();
    db.run(
        "CREATE (a:Step {id:1})-[:NEXT]->(b:Step {id:2})-[:NEXT]->(c:Step {id:3})-[:NEXT]->(d:Step {id:4})-[:NEXT]->(e:Step {id:5})",
    );
    // Total nodes: 5
    db.assert_count("MATCH (n:Step) RETURN n", 5);
    // Total edges: 4
    db.assert_count("MATCH (a:Step)-[:NEXT]->(b:Step) RETURN a, b", 4);
}

#[test]
fn create_n_nodes_and_count_them_via_aggregation() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 20) AS i CREATE (:Counter {val: i})");
    let rows = db.run("MATCH (c:Counter) RETURN count(c) AS total");
    assert_eq!(rows[0]["total"], 20);
}

#[test]
fn create_with_string_property_containing_special_chars() {
    let db = TestDb::new();
    let rows = db.run(r#"CREATE (n:Text {msg: "hello 'world'"}) RETURN n"#);
    assert_eq!(rows.len(), 1);
}

#[test]
fn create_with_unicode_property_value() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:Intl {greeting: 'Hallo Welt'}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["greeting"], "Hallo Welt");
}

// ============================================================
// Ignored future CREATE tests
// ============================================================

// Lora: CREATE CONSTRAINT ON (n:Label) ASSERT n.prop IS UNIQUE
#[test]
#[ignore = "pending implementation"]
fn create_constraint_unique_property() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT ON (n:User) ASSERT n.email IS UNIQUE");
    db.run("CREATE (:User {email: 'alice@example.com'})");
    let err = db.run_err("CREATE (:User {email: 'alice@example.com'})");
    assert!(!err.is_empty());
}

// Lora: CREATE INDEX FOR (n:Label) ON (n.prop)
#[test]
#[ignore = "pending implementation"]
fn create_index_on_label_property() {
    let db = TestDb::new();
    db.run("CREATE INDEX FOR (n:User) ON (n.name)");
    db.run("CREATE (:User {name: 'Alice'})");
    db.assert_count("MATCH (n:User {name: 'Alice'}) RETURN n", 1);
}

// Lora: FOREACH (x IN list | CREATE (n:T {val: x}))
#[test]
#[ignore = "pending implementation"]
fn foreach_create_from_list() {
    let db = TestDb::new();
    db.run("FOREACH (x IN [1, 2, 3] | CREATE (:Item {val: x}))");
    db.assert_count("MATCH (i:Item) RETURN i", 3);
}

// Lora: CREATE with temporal properties (date, datetime)
#[test]
#[ignore = "pending implementation"]
fn create_with_temporal_properties() {
    let db = TestDb::new();
    db.run("CREATE (:Event {name: 'Launch', date: date('2025-01-15'), ts: datetime('2025-01-15T10:00:00')})");
    let rows = db.run("MATCH (e:Event {name: 'Launch'}) RETURN e.date AS d");
    assert_eq!(rows.len(), 1);
}

// Lora: CREATE with spatial properties (point)
#[test]
#[ignore = "pending implementation"]
fn create_with_spatial_properties() {
    let db = TestDb::new();
    db.run("CREATE (:Location {name: 'HQ', pos: point({latitude: 52.37, longitude: 4.89})})");
    let rows = db.run("MATCH (l:Location {name: 'HQ'}) RETURN l.pos AS pos");
    assert_eq!(rows.len(), 1);
}

// ============================================================
// CREATE from aggregated WITH pipeline
// ============================================================

#[test]
fn create_summary_from_aggregation() {
    let db = TestDb::new();
    db.seed_org_graph();
    db.run(
        "MATCH (p:Person) \
         WITH p.dept AS dept, count(p) AS cnt \
         CREATE (:DeptSummary {dept: dept, headcount: cnt})",
    );
    let rows = db.run(
        "MATCH (d:DeptSummary) RETURN d.dept AS dept, d.headcount AS cnt ORDER BY d.dept",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["dept"], "Engineering");
    assert_eq!(rows[0]["cnt"], 4);
}

// ============================================================
// CREATE complex graph in one session
// ============================================================

#[test]
fn create_complete_graph_k4() {
    let db = TestDb::new();
    // Create 4 nodes
    for i in 1..=4 {
        db.run(&format!("CREATE (:K {{id:{i}}})"));
    }
    // Create all 12 directed edges (complete directed graph)
    for i in 1..=4 {
        for j in 1..=4 {
            if i != j {
                db.run(&format!(
                    "MATCH (a:K {{id:{i}}}), (b:K {{id:{j}}}) CREATE (a)-[:EDGE]->(b)"
                ));
            }
        }
    }
    db.assert_count("MATCH (n:K) RETURN n", 4);
    db.assert_count("MATCH ()-[r:EDGE]->() RETURN r", 12);
    // Each node has 3 outgoing and 3 incoming
    let rows = db.run("MATCH (n:K {id:1})-[r:EDGE]->() RETURN count(r) AS out");
    assert_eq!(rows[0]["out"], 3);
}

// ============================================================
// CREATE with CASE-derived values
// ============================================================

#[test]
fn create_with_case_expression_value() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 6) AS i \
            CREATE (:Graded {id: i, grade: CASE WHEN i <= 2 THEN 'A' WHEN i <= 4 THEN 'B' ELSE 'C' END})");
    db.assert_count("MATCH (g:Graded {grade: 'A'}) RETURN g", 2);
    db.assert_count("MATCH (g:Graded {grade: 'B'}) RETURN g", 2);
    db.assert_count("MATCH (g:Graded {grade: 'C'}) RETURN g", 2);
}

// ============================================================
// CREATE multiple disconnected patterns in single CREATE
// ============================================================

#[test]
fn create_multiple_disconnected_patterns() {
    let db = TestDb::new();
    db.run("CREATE (:Island {name:'A'}), (:Island {name:'B'}), (:Island {name:'C'})");
    db.assert_count("MATCH (i:Island) RETURN i", 3);
    db.assert_count("MATCH ()-[r]->() RETURN r", 0);
}

// ============================================================
// CREATE + immediate aggregation verification
// ============================================================

#[test]
fn create_batch_then_verify_statistics() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 100) AS i CREATE (:Stat {val: i % 10})");
    let rows = db.run(
        "MATCH (s:Stat) \
         RETURN count(s) AS total, count(DISTINCT s.val) AS unique_vals, min(s.val) AS lo, max(s.val) AS hi",
    );
    assert_eq!(rows[0]["total"], 100);
    assert_eq!(rows[0]["unique_vals"], 10); // 0..9
    assert_eq!(rows[0]["lo"], 0);
    assert_eq!(rows[0]["hi"], 9);
}
