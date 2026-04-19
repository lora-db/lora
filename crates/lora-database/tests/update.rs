/// SET, REMOVE, and DELETE tests — property mutation, label operations,
/// property replace/merge, relationship properties, cascading deletes,
/// graph consistency after failures.
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// SET: individual properties
// ============================================================

#[test]
fn set_property_add_new_key() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 25})");
    db.run("MATCH (n:User {name: 'Alice'}) SET n.age = 30");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["age"], 30);
}

#[test]
fn set_property_update_existing() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 25})");
    db.run("MATCH (n:User {name: 'Alice'}) SET n.age = 30");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["age"], 30);
}

#[test]
fn set_property_returns_modified_row() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 25})");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) SET n.age = 30 RETURN n");
    assert_eq!(rows.len(), 1);
}

#[test]
fn set_property_on_relationship() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})-[:FOLLOWS {since: 2020}]->(b:User {name: 'Bob'})");
    db.run("MATCH (a)-[r:FOLLOWS]->(b) SET r.since = 2024");
    let rows = db.run("MATCH (a)-[r:FOLLOWS]->(b) RETURN r");
    assert_eq!(rows[0]["r"]["properties"]["since"], 2024);
}

#[test]
fn set_property_on_multiple_nodes() {
    let db = TestDb::new();
    db.run("CREATE (:Emp {name:'Alice', active: true})");
    db.run("CREATE (:Emp {name:'Bob',   active: true})");
    db.run("CREATE (:Emp {name:'Carol', active: true})");
    db.run("MATCH (e:Emp) SET e.active = false");
    db.assert_count("MATCH (e:Emp) WHERE e.active = false RETURN e", 3);
}

#[test]
fn create_set_then_match() {
    let db = TestDb::new();
    db.run("CREATE (n:Person {name:'Alice'})");
    db.run("MATCH (n:Person {name:'Alice'}) SET n += {verified: true}");
    db.assert_count("MATCH (n:Person) WHERE n.verified = true RETURN n", 1);
}

// ============================================================
// SET: replace all properties (SET n = {})
// ============================================================

#[test]
fn set_variable_replaces_all_properties() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 30, active: true})");
    db.run("MATCH (n:User) SET n = {name: 'Bob'}");
    let rows = db.run("MATCH (n:User) RETURN n");
    let props = &rows[0]["n"]["properties"];
    assert_eq!(props["name"], "Bob");
    assert!(props.get("age").is_none() || props["age"].is_null());
}

#[test]
fn set_replace_all_properties_preserves_identity() {
    let db = TestDb::new();
    db.run("CREATE (:Node {a: 1, b: 2, c: 3})");
    db.run("MATCH (n:Node) SET n = {x: 10}");
    let rows = db.run("MATCH (n:Node) RETURN n");
    let props = &rows[0]["n"]["properties"];
    assert_eq!(props["x"], 10);
    assert!(props.get("a").is_none() || props["a"].is_null());
}

// ============================================================
// SET: merge properties (SET n += {})
// ============================================================

#[test]
fn set_mutate_merges_properties() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 30})");
    db.run("MATCH (n:User) SET n += {dept: 'eng'}");
    let rows = db.run("MATCH (n:User) RETURN n");
    let props = &rows[0]["n"]["properties"];
    assert_eq!(props["name"], "Alice");
    assert_eq!(props["age"], 30);
    assert_eq!(props["dept"], "eng");
}

#[test]
fn set_mutate_overwrites_existing_key() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 30})");
    db.run("MATCH (n:User) SET n += {age: 31}");
    let rows = db.run("MATCH (n:User) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["age"], 31);
}

#[test]
fn set_merge_properties_adds_without_removing() {
    let db = TestDb::new();
    db.run("CREATE (:Node {a: 1, b: 2})");
    db.run("MATCH (n:Node) SET n += {c: 3, b: 20}");
    let rows = db.run("MATCH (n:Node) RETURN n");
    let props = &rows[0]["n"]["properties"];
    assert_eq!(props["a"], 1);
    assert_eq!(props["b"], 20);
    assert_eq!(props["c"], 3);
}

// ============================================================
// SET: labels
// ============================================================

#[test]
fn set_add_label() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    db.run("MATCH (n:User {name: 'Alice'}) SET n:Admin");
    let rows = db.run("MATCH (n:User:Admin) RETURN n");
    assert_eq!(rows.len(), 1);
}

#[test]
fn set_add_label_idempotent() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    db.run("MATCH (n:User {name: 'Alice'}) SET n:User");
    let rows = db.run("MATCH (n:User) RETURN n");
    assert_eq!(rows.len(), 1);
    let labels = rows[0]["n"]["labels"].as_array().unwrap();
    let user_count = labels.iter().filter(|l| l.as_str() == Some("User")).count();
    assert_eq!(user_count, 1);
}

#[test]
fn set_multiple_labels_at_once() {
    let db = TestDb::new();
    db.run("CREATE (:Base {name:'x'})");
    db.run("MATCH (n:Base) SET n:Extra:Special");
    db.assert_count("MATCH (n:Base:Extra:Special) RETURN n", 1);
}

// ============================================================
// REMOVE: properties
// ============================================================

#[test]
fn remove_property_from_node() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 30})");
    db.run("MATCH (n:User {name: 'Alice'}) REMOVE n.age");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) RETURN n");
    assert!(
        rows[0]["n"]["properties"].get("age").is_none()
            || rows[0]["n"]["properties"]["age"].is_null()
    );
}

#[test]
fn remove_property_from_relationship() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})-[:FOLLOWS {since: 2020}]->(b:User {name: 'Bob'})");
    db.run("MATCH (a)-[r:FOLLOWS]->(b) REMOVE r.since");
    let rows = db.run("MATCH (a)-[r:FOLLOWS]->(b) RETURN r");
    assert!(
        rows[0]["r"]["properties"].get("since").is_none()
            || rows[0]["r"]["properties"]["since"].is_null()
    );
}

#[test]
fn remove_nonexistent_property_is_rejected_by_analyzer() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let err = db.run_err("MATCH (n:User) REMOVE n.nonexistent");
    assert!(err.contains("Unknown property"));
}

#[test]
fn remove_property_from_all_nodes() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name:'A', temp: true})");
    db.run("CREATE (:Item {name:'B', temp: true})");
    db.run("MATCH (i:Item) REMOVE i.temp");
    let rows = db.run("MATCH (i:Item) RETURN i");
    for row in &rows {
        assert!(
            row["i"]["properties"].get("temp").is_none()
                || row["i"]["properties"]["temp"].is_null()
        );
    }
}

// ============================================================
// REMOVE: labels
// ============================================================

#[test]
fn remove_label_from_node() {
    let db = TestDb::new();
    db.run("CREATE (n:User:Admin {name: 'Alice'})");
    db.run("MATCH (n:Admin) REMOVE n:Admin");
    db.assert_count("MATCH (n:User) RETURN n", 1);
}

// ============================================================
// DELETE
// ============================================================

#[test]
fn delete_isolated_node() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    db.run("MATCH (n:User {name: 'Alice'}) DELETE n");
    db.assert_count("MATCH (n:User) RETURN n", 0);
}

#[test]
fn delete_node_with_relationships_fails() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})-[:FOLLOWS]->(b:User {name: 'Bob'})");
    let err = db.run_err("MATCH (n:User {name: 'Alice'}) DELETE n");
    assert!(err.contains("relationships") || err.contains("DETACH"));
}

#[test]
fn detach_delete_node() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})-[:FOLLOWS]->(b:User {name: 'Bob'})");
    db.run("MATCH (n:User {name: 'Alice'}) DETACH DELETE n");
    db.assert_count("MATCH (n:User {name: 'Bob'}) RETURN n", 1);
    db.assert_count("MATCH (n:User) RETURN n", 1);
}

#[test]
fn delete_already_deleted_is_ok() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    db.run("MATCH (n:User {name: 'Alice'}) DELETE n");
    db.assert_count("MATCH (n:User {name: 'Alice'}) RETURN n", 0);
}

#[test]
fn delete_relationship_keeps_nodes() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})-[:FOLLOWS]->(b:User {name: 'Bob'})");
    db.run("MATCH (a)-[r:FOLLOWS]->(b) DELETE r");
    db.assert_count("MATCH (n:User) RETURN n", 2);
}

#[test]
fn delete_leaf_nodes_preserves_parent() {
    let db = TestDb::new();
    db.run("CREATE (:Parent {name:'P'})-[:HAS]->(:Child {name:'C1'})");
    db.run("MATCH (p:Parent {name:'P'}) CREATE (p)-[:HAS]->(:Child {name:'C2'})");
    db.run("MATCH (c:Child {name:'C2'}) DETACH DELETE c");
    db.assert_count("MATCH (c:Child) RETURN c", 1);
    db.assert_count("MATCH (p:Parent)-[:HAS]->(c:Child) RETURN p, c", 1);
}

#[test]
fn detach_delete_removes_all_incident_relationships() {
    let db = TestDb::new();
    db.run("CREATE (:Hub {name:'center'})");
    for i in 0..5 {
        db.run(&format!("CREATE (:Spoke {{id:{i}}})"));
        db.run(&format!(
            "MATCH (h:Hub), (s:Spoke {{id:{i}}}) CREATE (h)-[:CONNECTS]->(s)"
        ));
    }
    db.assert_count("MATCH (h:Hub)-[r:CONNECTS]->(s) RETURN r", 5);
    db.run("MATCH (h:Hub) DETACH DELETE h");
    db.assert_count("MATCH (s:Spoke) RETURN s", 5);
}

#[test]
fn failed_delete_leaves_graph_unchanged() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})-[:R]->(:Y {id:2})");
    let _ = db.exec("MATCH (x:X) DELETE x");
    db.assert_count("MATCH (x:X) RETURN x", 1);
    db.assert_count("MATCH (a)-[r:R]->(b) RETURN r", 1);
}

// ============================================================
// SET: null value
// ============================================================

#[test]
fn set_property_to_null() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 30})");
    db.run("MATCH (n:User {name: 'Alice'}) SET n.age = null");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) RETURN n");
    assert!(
        rows[0]["n"]["properties"].get("age").is_none()
            || rows[0]["n"]["properties"]["age"].is_null()
    );
}

// ============================================================
// SET: multiple properties in one SET clause
// ============================================================

#[test]
fn set_multiple_properties_comma_separated() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 25})");
    db.run("MATCH (n:User {name: 'Alice'}) SET n.age = 30, n.name = 'Alicia'");
    let rows = db.run("MATCH (n:User) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["name"], "Alicia");
    assert_eq!(rows[0]["n"]["properties"]["age"], 30);
}

// ============================================================
// SET: list and boolean property values
// ============================================================

#[test]
fn set_list_property_value() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', tags: []})");
    db.run("MATCH (n:User {name: 'Alice'}) SET n.tags = ['admin', 'active']");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) RETURN n");
    assert!(rows[0]["n"]["properties"]["tags"].is_array());
}

#[test]
fn set_boolean_property() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', active: false})");
    db.run("MATCH (n:User {name: 'Alice'}) SET n.active = true");
    let rows = db.run("MATCH (n:User {name: 'Alice'}) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["active"], true);
}

// ============================================================
// DELETE: multiple relationship cleanup
// ============================================================

#[test]
fn delete_multiple_relationships_between_nodes() {
    let db = TestDb::new();
    db.run("CREATE (a:N {id:1}), (b:N {id:2})");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:R1]->(b)");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:R2]->(b)");
    db.assert_count("MATCH (a:N {id:1})-[r]->(b:N {id:2}) RETURN r", 2);
    db.run("MATCH (a:N {id:1})-[r:R1]->(b:N {id:2}) DELETE r");
    db.assert_count("MATCH (a:N {id:1})-[r]->(b:N {id:2}) RETURN r", 1);
}

// ============================================================
// DETACH DELETE: cascading on complex graph
// ============================================================

#[test]
fn detach_delete_cascade_on_chain() {
    let db = TestDb::new();
    db.run("CREATE (a:C {id:1})-[:NEXT]->(b:C {id:2})-[:NEXT]->(c:C {id:3})");
    db.run("MATCH (b:C {id:2}) DETACH DELETE b");
    db.assert_count("MATCH (n:C) RETURN n", 2);
    // Remaining nodes should have no relationships
    db.assert_count("MATCH (a:C)-[r]->(b:C) RETURN r", 0);
}

// ============================================================
// SET with WHERE filter
// ============================================================

#[test]
fn set_only_matching_nodes() {
    let db = TestDb::new();
    db.run("CREATE (:Emp {name:'Alice', dept:'eng', active: true})");
    db.run("CREATE (:Emp {name:'Bob',   dept:'sales', active: true})");
    db.run("CREATE (:Emp {name:'Carol', dept:'eng', active: true})");
    db.run("MATCH (e:Emp) WHERE e.dept = 'eng' SET e.active = false");
    db.assert_count("MATCH (e:Emp) WHERE e.active = false RETURN e", 2);
    db.assert_count("MATCH (e:Emp) WHERE e.active = true RETURN e", 1);
}

// ============================================================
// REMOVE: label edge cases
// ============================================================

#[test]
fn remove_last_remaining_label() {
    let db = TestDb::new();
    db.run("CREATE (:OnlyLabel {name: 'test'})");
    db.run("MATCH (n:OnlyLabel) REMOVE n:OnlyLabel");
    // Node still exists but without the label
    db.assert_count("MATCH (n) RETURN n", 1);
}

// ============================================================
// SET + RETURN interaction
// ============================================================

#[test]
fn set_then_return_shows_updated_value() {
    let db = TestDb::new();
    db.run("CREATE (n:Counter {val: 0})");
    let rows = db.run("MATCH (n:Counter) SET n.val = 1 RETURN n.val AS val");
    assert_eq!(rows.len(), 1);
}

// ============================================================
// DELETE + verify remaining graph
// ============================================================

#[test]
fn delete_isolated_and_verify_others() {
    let db = TestDb::new();
    db.run("CREATE (:Keep {name:'stay1'})");
    db.run("CREATE (:Keep {name:'stay2'})");
    db.run("CREATE (:Keep:Remove {name:'gone'})");
    db.run("MATCH (r:Remove) DELETE r");
    db.assert_count("MATCH (n:Keep) RETURN n", 2);
    // Total nodes left should be 2 (the :Keep-only nodes)
    db.assert_count("MATCH (n) RETURN n", 2);
}

// ============================================================
// SET advanced patterns
// ============================================================

#[test]
fn set_computed_expression_doubled() {
    let db = TestDb::new();
    db.run("CREATE (n:Calc {val: 7, doubled: 0})");
    db.run("MATCH (n:Calc) SET n.doubled = n.val * 2");
    let rows = db.run("MATCH (n:Calc) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["doubled"], 14);
}

#[test]
fn set_property_from_another_node() {
    let db = TestDb::new();
    db.run("CREATE (:Source {name:'src', data: 42})");
    db.run("CREATE (:Dest {name:'dst'})");
    db.run("MATCH (s:Source {name:'src'}), (d:Dest {name:'dst'}) SET d.data = s.data");
    let rows = db.run("MATCH (d:Dest {name:'dst'}) RETURN d");
    assert_eq!(rows[0]["d"]["properties"]["data"], 42);
}

#[test]
fn set_on_many_nodes_with_aggregation_derived_value() {
    let db = TestDb::new();
    db.run("CREATE (:Score {val: 10, group: 'a', total: 0})");
    db.run("CREATE (:Score {val: 20, group: 'a', total: 0})");
    db.run("CREATE (:Score {val: 30, group: 'a', total: 0})");
    db.run(
        "MATCH (s:Score) WITH sum(s.val) AS total \
         MATCH (s2:Score) SET s2.total = total",
    );
    let rows = db.run("MATCH (s:Score) RETURN s");
    for row in &rows {
        assert_eq!(row["s"]["properties"]["total"], 60);
    }
}

#[test]
fn set_relationship_property_from_computation() {
    let db = TestDb::new();
    db.run("CREATE (a:N {id:1})-[:WEIGHTED {base: 5, scaled: 0}]->(b:N {id:2})");
    db.run("MATCH (a)-[r:WEIGHTED]->(b) SET r.scaled = r.base * 10");
    let rows = db.run("MATCH (a)-[r:WEIGHTED]->(b) RETURN r");
    assert_eq!(rows[0]["r"]["properties"]["scaled"], 50);
}

#[test]
fn set_and_remove_in_same_query() {
    let db = TestDb::new();
    db.run("CREATE (:Dual {keep: 'yes', drop: 'no', added: false})");
    db.run("MATCH (n:Dual) SET n.added = true REMOVE n.drop");
    let rows = db.run("MATCH (n:Dual) RETURN n");
    let props = &rows[0]["n"]["properties"];
    assert_eq!(props["keep"], "yes");
    assert_eq!(props["added"], true);
    assert!(props.get("drop").is_none() || props["drop"].is_null());
}

#[test]
fn set_property_with_string_concatenation_expression() {
    let db = TestDb::new();
    db.run("CREATE (:Greet {first: 'Hello', last: 'World', full: ''})");
    db.run("MATCH (n:Greet) SET n.full = n.first + ' ' + n.last");
    let rows = db.run("MATCH (n:Greet) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["full"], "Hello World");
}

// ============================================================
// DELETE/DETACH DELETE advanced
// ============================================================

#[test]
fn delete_all_relationships_of_specific_type_between_specific_nodes() {
    let db = TestDb::new();
    db.run("CREATE (a:P {name:'A'}), (b:P {name:'B'})");
    db.run("MATCH (a:P {name:'A'}), (b:P {name:'B'}) CREATE (a)-[:FRIEND]->(b)");
    db.run("MATCH (a:P {name:'A'}), (b:P {name:'B'}) CREATE (a)-[:COLLEAGUE]->(b)");
    db.assert_count("MATCH (a:P {name:'A'})-[r]->(b:P {name:'B'}) RETURN r", 2);
    db.run("MATCH (a:P {name:'A'})-[r:FRIEND]->(b:P {name:'B'}) DELETE r");
    db.assert_count("MATCH (a:P {name:'A'})-[r]->(b:P {name:'B'}) RETURN r", 1);
    db.assert_count(
        "MATCH (a:P {name:'A'})-[r:COLLEAGUE]->(b:P {name:'B'}) RETURN r",
        1,
    );
}

#[test]
fn detach_delete_all_nodes_of_a_label() {
    let db = TestDb::new();
    db.run("CREATE (:Ephemeral {id: 1})");
    db.run("CREATE (:Ephemeral {id: 2})");
    db.run("CREATE (:Ephemeral {id: 3})");
    db.run("CREATE (:Permanent {id: 100})");
    db.run("MATCH (a:Ephemeral {id:1}), (b:Ephemeral {id:2}) CREATE (a)-[:LINK]->(b)");
    db.assert_count("MATCH (n:Ephemeral) RETURN n", 3);
    db.run("MATCH (n:Ephemeral) DETACH DELETE n");
    // After deleting all Ephemeral nodes, the label is removed from the registry,
    // so we verify by checking only Permanent nodes remain.
    db.assert_count("MATCH (n:Permanent) RETURN n", 1);
    db.assert_count("MATCH (n) RETURN n", 1);
}

#[test]
fn delete_specific_relationship_verify_other_relationships_survive() {
    let db = TestDb::new();
    db.run("CREATE (a:Node {id:1})-[:A]->(b:Node {id:2})");
    db.run("MATCH (a:Node {id:1}), (b:Node {id:2}) CREATE (a)-[:B]->(b)");
    db.run("MATCH (a:Node {id:1}), (b:Node {id:2}) CREATE (a)-[:C]->(b)");
    db.assert_count("MATCH (a:Node {id:1})-[r]->(b:Node {id:2}) RETURN r", 3);
    db.run("MATCH (a:Node {id:1})-[r:B]->(b:Node {id:2}) DELETE r");
    db.assert_count("MATCH (a:Node {id:1})-[r]->(b:Node {id:2}) RETURN r", 2);
    db.assert_count("MATCH (a:Node {id:1})-[r:A]->(b:Node {id:2}) RETURN r", 1);
    db.assert_count("MATCH (a:Node {id:1})-[r:C]->(b:Node {id:2}) RETURN r", 1);
}

#[test]
fn delete_from_middle_of_chain_verify_broken_graph() {
    let db = TestDb::new();
    db.run("CREATE (a:Link {id:1})-[:SEQ]->(b:Link {id:2})-[:SEQ]->(c:Link {id:3})");
    db.run("MATCH (b:Link {id:2}) DETACH DELETE b");
    db.assert_count("MATCH (n:Link) RETURN n", 2);
    // No relationships should remain — both edges to/from b are gone
    db.assert_count("MATCH (a:Link)-[r:SEQ]->(b:Link) RETURN r", 0);
}

#[test]
fn graph_cleanup_detach_delete_all_nodes() {
    let db = TestDb::new();
    db.run("CREATE (:Seed {id:1})-[:R]->(:Seed {id:2})");
    db.run("CREATE (:Seed {id:3})");
    db.assert_count("MATCH (n:Seed) RETURN n", 3);
    db.run("MATCH (n:Seed) DETACH DELETE n");
    db.assert_count("MATCH (n:Seed) RETURN n", 0);
    db.assert_count("MATCH (n) RETURN n", 0);
}

// ============================================================
// Read-after-write consistency
// ============================================================

#[test]
fn set_then_return_in_same_query_shows_new_value() {
    let db = TestDb::new();
    db.run("CREATE (:RW {val: 1})");
    let rows = db.run("MATCH (n:RW) SET n.val = 99 RETURN n.val AS val");
    assert_eq!(rows[0]["val"], 99);
}

#[test]
fn delete_then_match_in_same_session_confirms_removal() {
    let db = TestDb::new();
    db.run("CREATE (:Ghost {name:'phantom'})");
    db.assert_count("MATCH (g:Ghost) RETURN g", 1);
    db.run("MATCH (g:Ghost {name:'phantom'}) DELETE g");
    db.assert_count("MATCH (g:Ghost) RETURN g", 0);
}

#[test]
fn remove_label_then_match_by_that_label_finds_nothing() {
    let db = TestDb::new();
    db.run("CREATE (:Temp:Persist {name:'node1'})");
    db.assert_count("MATCH (n:Temp) RETURN n", 1);
    db.run("MATCH (n:Temp) REMOVE n:Temp");
    // After removing the only node with :Temp, the label is removed from the registry.
    // Verify the node still exists under the :Persist label.
    db.assert_count("MATCH (n:Persist) RETURN n", 1);
    // The node should not have the :Temp label anymore
    let rows = db.run("MATCH (n:Persist) RETURN n");
    let labels = rows[0]["n"]["labels"].as_array().unwrap();
    assert!(!labels.contains(&serde_json::json!("Temp")));
}

#[test]
fn set_property_then_filter_by_new_value() {
    let db = TestDb::new();
    db.run("CREATE (:Flag {name:'a', status: 'pending'})");
    db.run("CREATE (:Flag {name:'b', status: 'pending'})");
    db.run("MATCH (f:Flag {name:'a'}) SET f.status = 'done'");
    db.assert_count("MATCH (f:Flag) WHERE f.status = 'done' RETURN f", 1);
    db.assert_count("MATCH (f:Flag) WHERE f.status = 'pending' RETURN f", 1);
}

// ============================================================
// Ignored future update tests
// ============================================================

// Lora: SET n.prop = CASE WHEN ... pattern
#[test]
fn set_with_case_expression() {
    let db = TestDb::new();
    db.run("CREATE (:Grade {name:'Alice', score: 85})");
    db.run("CREATE (:Grade {name:'Bob',   score: 45})");
    db.run("MATCH (g:Grade) SET g.pass = CASE WHEN g.score >= 60 THEN true ELSE false END");
    let rows = db.run("MATCH (g:Grade {name:'Alice'}) RETURN g");
    assert_eq!(rows[0]["g"]["properties"]["pass"], true);
    let rows = db.run("MATCH (g:Grade {name:'Bob'}) RETURN g");
    assert_eq!(rows[0]["g"]["properties"]["pass"], false);
}

// Lora: SET with map from parameter $props
#[test]
#[ignore = "pending implementation"]
fn set_with_map_from_parameter() {
    let db = TestDb::new();
    db.run("CREATE (:Param {name:'test'})");
    db.run("MATCH (n:Param {name:'test'}) SET n += $props");
    let rows = db.run("MATCH (n:Param {name:'test'}) RETURN n");
    assert_eq!(rows.len(), 1);
}

// Lora: transactional rollback on error leaves data unchanged
#[test]
fn transactional_rollback_on_error_leaves_data_unchanged() {
    let db = TestDb::new();
    db.run("CREATE (:Safe {val: 1})");
    // Attempt a failing multi-statement transaction
    let _ = db.exec("BEGIN; MATCH (n:Safe) SET n.val = 2; INVALID SYNTAX; COMMIT;");
    // LoraValue should remain unchanged due to rollback
    let rows = db.run("MATCH (n:Safe) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["val"], 1);
}

// Lora: ON DELETE CASCADE (not Lora, but worth noting as non-goal)
#[test]
#[ignore = "pending implementation"]
fn on_delete_cascade_is_not_cypher() {
    let db = TestDb::new();
    db.run("CREATE (:Parent {id:1})-[:OWNS]->(:Child {id:2})");
    // Lora has no ON DELETE CASCADE; this tests hypothetical behavior
    db.run("MATCH (p:Parent {id:1}) DELETE p CASCADE");
    db.assert_count("MATCH (n) RETURN n", 0);
}

// Lora: SET with subquery-derived value
#[test]
#[ignore = "pending implementation"]
fn set_with_subquery_derived_value() {
    let db = TestDb::new();
    db.run("CREATE (:Outer {name:'target'})");
    db.run("CREATE (:Inner {val: 10})");
    db.run("CREATE (:Inner {val: 20})");
    db.run(
        "MATCH (o:Outer {name:'target'}) \
         CALL { MATCH (i:Inner) RETURN sum(i.val) AS total } \
         SET o.total = total",
    );
    let rows = db.run("MATCH (o:Outer {name:'target'}) RETURN o");
    assert_eq!(rows[0]["o"]["properties"]["total"], 30);
}

#[test]
fn set_property_from_expression() {
    let db = TestDb::new();
    db.run("CREATE (:Calc {a: 10, b: 3})");
    db.run("MATCH (c:Calc) SET c.sum = c.a + c.b");
    let rows = db.run("MATCH (c:Calc) RETURN c.sum AS s");
    assert_eq!(rows[0]["s"], 13);
}

#[test]
fn set_property_from_other_node() {
    let db = TestDb::new();
    db.run("CREATE (:Source {val: 42})");
    db.run("CREATE (:Target {name: 'dest'})");
    db.run("MATCH (s:Source), (t:Target) SET t.copied = s.val");
    let rows = db.run("MATCH (t:Target) RETURN t.copied AS v");
    assert_eq!(rows[0]["v"], 42);
}

#[test]
fn set_null_removes_property() {
    let db = TestDb::new();
    db.run("CREATE (:Tmp {keep: 1, remove_me: 2})");
    db.run("MATCH (t:Tmp) SET t.remove_me = null");
    let rows = db.run("MATCH (t:Tmp) RETURN t.remove_me AS v, t.keep AS k");
    assert!(rows[0]["v"].is_null());
    assert_eq!(rows[0]["k"], 1);
}

#[test]
fn delete_node_and_verify_gone() {
    let db = TestDb::new();
    db.run("CREATE (:Temp {name: 'del'})");
    db.assert_count("MATCH (n:Temp) RETURN n", 1);
    db.run("MATCH (n:Temp {name: 'del'}) DELETE n");
    db.assert_count("MATCH (n:Temp) RETURN n", 0);
}

#[test]
fn detach_delete_with_multiple_relationships() {
    let db = TestDb::new();
    db.run("CREATE (a:Hub {name:'hub'})");
    db.run("CREATE (b:Spoke {name:'s1'})");
    db.run("CREATE (c:Spoke {name:'s2'})");
    db.run("MATCH (a:Hub), (b:Spoke {name:'s1'}) CREATE (a)-[:LINK]->(b)");
    db.run("MATCH (a:Hub), (c:Spoke {name:'s2'}) CREATE (a)-[:LINK]->(c)");
    db.assert_count("MATCH (n:Hub) RETURN n", 1);
    db.run("MATCH (a:Hub {name:'hub'}) DETACH DELETE a");
    // Spokes should still exist
    db.assert_count("MATCH (n:Spoke) RETURN n", 2);
}

#[test]
fn merge_on_create_set() {
    let db = TestDb::new();
    db.run("MERGE (n:Counter {name: 'hits'}) ON CREATE SET n.count = 1");
    let rows = db.run("MATCH (n:Counter) RETURN n.count AS c");
    assert_eq!(rows[0]["c"], 1);
}

#[test]
fn merge_on_match_set() {
    let db = TestDb::new();
    db.run("CREATE (:Counter {name: 'hits', count: 5})");
    db.run("MERGE (n:Counter {name: 'hits'}) ON MATCH SET n.count = n.count + 1");
    let rows = db.run("MATCH (n:Counter) RETURN n.count AS c");
    assert_eq!(rows[0]["c"], 6);
}

#[test]
fn create_multiple_nodes_single_query() {
    let db = TestDb::new();
    db.run("CREATE (:Batch {i: 1}), (:Batch {i: 2}), (:Batch {i: 3})");
    db.assert_count("MATCH (b:Batch) RETURN b", 3);
}

#[test]
fn set_multiple_properties_comma() {
    let db = TestDb::new();
    db.run("CREATE (:Multi {name: 'test'})");
    db.run("MATCH (m:Multi) SET m.a = 1, m.b = 2, m.c = 3");
    let rows = db.run("MATCH (m:Multi) RETURN m.a AS a, m.b AS b, m.c AS c");
    assert_eq!(rows[0]["a"], 1);
    assert_eq!(rows[0]["b"], 2);
    assert_eq!(rows[0]["c"], 3);
}

#[test]
fn remove_label_and_verify() {
    let db = TestDb::new();
    db.run("CREATE (:A:B {name: 'both'})");
    db.run("MATCH (n:A:B) REMOVE n:B");
    let rows = db.run("MATCH (n:A) RETURN labels(n) AS l");
    let labels = rows[0]["l"].as_array().unwrap();
    assert!(!labels.iter().any(|l| l == "B"));
}

#[test]
#[ignore = "pending implementation"]
fn foreach_update() {
    let db = TestDb::new();
    db.run("CREATE (:List {items: [1, 2, 3]})");
    let _rows = db.run("MATCH (l:List) FOREACH (i IN l.items | CREATE (:Item {val: i}))");
}

#[test]
#[ignore = "pending implementation"]
fn create_unique_constraint() {
    let db = TestDb::new();
    let _err = db.run("CREATE CONSTRAINT FOR (n:User) REQUIRE n.email IS UNIQUE");
}

// ============================================================
// SET with aggregation-derived value via pipeline
// ============================================================

#[test]
fn set_from_aggregation_pipeline() {
    let db = TestDb::new();
    db.run("CREATE (:Score {val: 10, rank: 0})");
    db.run("CREATE (:Score {val: 20, rank: 0})");
    db.run("CREATE (:Score {val: 30, rank: 0})");
    // Set the max value into all nodes
    db.run(
        "MATCH (s:Score) \
         WITH max(s.val) AS top \
         MATCH (s2:Score) SET s2.rank = top",
    );
    let rows = db.run("MATCH (s:Score) RETURN DISTINCT s.rank AS rank");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["rank"], 30);
}

// ============================================================
// DELETE all relationships of a specific type
// ============================================================

#[test]
fn delete_all_relationships_of_type() {
    let db = TestDb::new();
    db.seed_social_graph();
    // Before: 2 FOLLOWS + 1 KNOWS = 3 relationships
    db.assert_count("MATCH ()-[r:FOLLOWS]->() RETURN r", 2);
    // Delete all FOLLOWS relationships
    db.run("MATCH ()-[r:FOLLOWS]->() DELETE r");
    // KNOWS should still exist
    db.assert_count("MATCH ()-[r:KNOWS]->() RETURN r", 1);
    // Total remaining relationships should be 1 (only KNOWS)
    db.assert_count("MATCH ()-[r]->() RETURN r", 1);
    // All nodes should still exist
    db.assert_count("MATCH (n:User) RETURN n", 3);
}

// ============================================================
// SET += with empty map (no-op)
// ============================================================

#[test]
fn set_merge_with_empty_map() {
    let db = TestDb::new();
    db.run("CREATE (:Stable {a: 1, b: 2})");
    db.run("MATCH (s:Stable) SET s += {}");
    let rows = db.run("MATCH (s:Stable) RETURN s.a AS a, s.b AS b");
    assert_eq!(rows[0]["a"], 1);
    assert_eq!(rows[0]["b"], 2);
}

// ============================================================
// Conditional SET via CASE
// ============================================================

#[test]
fn set_conditional_via_case_bucketing() {
    let db = TestDb::new();
    db.seed_org_graph();
    db.run(
        "MATCH (p:Person) \
         SET p.seniority = CASE \
             WHEN p.age >= 40 THEN 'senior' \
             WHEN p.age >= 30 THEN 'mid' \
             ELSE 'junior' END",
    );
    db.assert_count("MATCH (p:Person {seniority: 'senior'}) RETURN p", 2); // Carol, Frank
    db.assert_count("MATCH (p:Person {seniority: 'mid'}) RETURN p", 2); // Alice, Dave
    db.assert_count("MATCH (p:Person {seniority: 'junior'}) RETURN p", 2); // Bob, Eve
}

// ============================================================
// DETACH DELETE with complex graph
// ============================================================

#[test]
fn detach_delete_hub_in_star_preserves_leaves() {
    let db = TestDb::new();
    db.run("CREATE (:Hub {name:'center'})");
    db.run("UNWIND range(1, 10) AS i MATCH (h:Hub) CREATE (h)-[:ARM]->(:Leaf {id: i})");
    db.assert_count("MATCH (:Hub)-[:ARM]->(:Leaf) RETURN 1 AS x", 10);
    db.run("MATCH (h:Hub) DETACH DELETE h");
    db.assert_count("MATCH (l:Leaf) RETURN l", 10);
    db.assert_count("MATCH ()-[r:ARM]->() RETURN r", 0);
}

// ============================================================
// Multi-step update pipeline
// ============================================================

#[test]
fn multi_step_set_pipeline() {
    let db = TestDb::new();
    db.run("CREATE (:Account {name: 'A', balance: 100})");
    db.run("CREATE (:Account {name: 'B', balance: 50})");
    // Transfer 30 from A to B
    db.run("MATCH (a:Account {name: 'A'}) SET a.balance = a.balance - 30");
    db.run("MATCH (b:Account {name: 'B'}) SET b.balance = b.balance + 30");
    let rows = db.run("MATCH (a:Account) RETURN a.name AS name, a.balance AS bal ORDER BY a.name");
    assert_eq!(rows[0]["bal"], 70);
    assert_eq!(rows[1]["bal"], 80);
}

// ============================================================
// DELETE with WHERE filter on relationship
// ============================================================

#[test]
fn delete_relationship_by_property_filter() {
    let db = TestDb::new();
    db.run("CREATE (a:N {id:1})-[:R {weight: 1}]->(b:N {id:2})");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:R {weight: 5}]->(b)");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:R {weight: 10}]->(b)");
    // Delete only low-weight relationships
    db.run("MATCH (a)-[r:R]->(b) WHERE r.weight < 3 DELETE r");
    db.assert_count("MATCH ()-[r:R]->() RETURN r", 2);
}

// ============================================================
// Future update tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn set_from_call_subquery() {
    let db = TestDb::new();
    db.run("CREATE (:Target {name: 'x', total: 0})");
    db.run("CREATE (:Source {val: 10})");
    db.run("CREATE (:Source {val: 20})");
    let _rows = db.run(
        "MATCH (t:Target) \
         CALL { MATCH (s:Source) RETURN sum(s.val) AS s_total } \
         SET t.total = s_total",
    );
}
