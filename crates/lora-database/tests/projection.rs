/// RETURN / projection tests — variable access, property access, aliases,
/// star projection, literals, expressions, and schema validation on properties.
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// Basic projection
// ============================================================

#[test]
fn return_node_variable() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let rows = db.run("MATCH (n:User) RETURN n");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].get("n").is_some());
}

#[test]
fn return_property_access() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 30})");
    let rows = db.run("MATCH (n:User) RETURN n.name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

#[test]
fn return_multiple_properties() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 30})");
    let rows = db.run("MATCH (n:User) RETURN n.name, n.age");
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[0]["age"], 30);
}

#[test]
fn return_with_alias() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let rows = db.run("MATCH (n:User) RETURN n.name AS userName");
    assert!(rows[0].get("userName").is_some());
    assert_eq!(rows[0]["userName"], "Alice");
}

#[test]
fn return_expression() {
    let db = TestDb::new();
    db.run("CREATE (n:User {age: 30})");
    let rows = db.run("MATCH (n:User) RETURN n.age + 5 AS older");
    assert_eq!(rows[0]["older"], 35);
}

// ============================================================
// Unknown property validation
// ============================================================

#[test]
fn return_missing_property_is_rejected() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let err = db.run_err("MATCH (n:User) RETURN n.nonexistent");
    assert!(err.contains("Unknown property"));
}

#[test]
fn return_existing_property_not_on_specific_node() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice', age: 30})");
    db.run("CREATE (b:User {name: 'Bob'})");
    let rows = db.run("MATCH (n:User) RETURN n.name AS name, n.age AS age ORDER BY n.name ASC");
    assert_eq!(rows.len(), 2);
}

// ============================================================
// Star projection
// ============================================================

#[test]
fn return_star_single_variable() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let rows = db.run("MATCH (n:User) RETURN *");
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].as_object().unwrap().is_empty());
}

#[test]
fn return_star_with_relationship() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name: 'Alice'})-[r:FOLLOWS]->(b) RETURN *");
    assert_eq!(rows.len(), 1);
    let obj = rows[0].as_object().unwrap();
    assert!(obj.len() >= 3);
}

#[test]
fn return_star_plus_expression() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name: 'Alice'})-[r:FOLLOWS]->(b) RETURN *, a.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

// ============================================================
// Literal return values
// ============================================================

#[test]
fn return_integer_literal() {
    let db = TestDb::new();
    let rows = db.run("RETURN 42");
    assert_eq!(rows.len(), 1);
}

#[test]
fn return_string_literal() {
    let db = TestDb::new();
    let rows = db.run("RETURN 'hello'");
    assert_eq!(rows.len(), 1);
}

#[test]
fn return_boolean_literal() {
    let db = TestDb::new();
    let rows = db.run("RETURN true");
    assert_eq!(rows.len(), 1);
}

#[test]
fn return_null_literal() {
    let db = TestDb::new();
    let rows = db.run("RETURN null");
    assert_eq!(rows.len(), 1);
}

#[test]
fn return_list_literal() {
    let db = TestDb::new();
    let rows = db.run("RETURN [1, 2, 3]");
    assert_eq!(rows.len(), 1);
}

#[test]
fn return_map_literal() {
    let db = TestDb::new();
    let rows = db.run("RETURN {name: 'Alice', age: 30}");
    assert_eq!(rows.len(), 1);
}

#[test]
fn return_multiple_expressions() {
    let db = TestDb::new();
    let rows = db.run("RETURN 42, 'hello', true, null");
    assert_eq!(rows.len(), 1);
}

// ============================================================
// Computed expressions in projection
// ============================================================

#[test]
fn return_computed_arithmetic_expression() {
    let db = TestDb::new();
    db.run("CREATE (:Item {price: 100, quantity: 3})");
    let rows = db.run("MATCH (i:Item) RETURN i.price * i.quantity AS total");
    assert_eq!(rows[0]["total"], 300);
}

#[test]
fn return_float_expression() {
    let db = TestDb::new();
    let rows = db.run("RETURN 10 / 3.0 AS result");
    let v = rows[0]["result"].as_f64().unwrap();
    assert!((v - 3.333).abs() < 0.01);
}

#[test]
fn return_boolean_expression() {
    let db = TestDb::new();
    let rows = db.run("RETURN 1 < 2 AS cmp");
    assert_eq!(rows[0]["cmp"], true);
}

// ============================================================
// Relationship in projection
// ============================================================

#[test]
fn return_relationship_variable() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name:'Alice'})-[r:FOLLOWS]->(b) RETURN r");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["r"]["type"], "FOLLOWS");
}

#[test]
fn return_relationship_property() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name:'Alice'})-[r:FOLLOWS]->(b) RETURN r.since AS since");
    assert_eq!(rows[0]["since"], 2020);
}

// ============================================================
// Mixed return types
// ============================================================

#[test]
fn return_mixed_node_rel_and_scalar() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run(
        "MATCH (a:User {name:'Alice'})-[r:FOLLOWS]->(b:User) \
         RETURN a, r, b, a.name AS name, r.since AS since",
    );
    assert_eq!(rows.len(), 1);
    assert!(rows[0].get("a").is_some());
    assert!(rows[0].get("r").is_some());
    assert!(rows[0].get("b").is_some());
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[0]["since"], 2020);
}

#[test]
fn return_multiple_aliases_different_expressions() {
    let db = TestDb::new();
    db.run("CREATE (:P {first:'John', last:'Doe', age:40})");
    let rows =
        db.run("MATCH (p:P) RETURN p.first + ' ' + p.last AS full_name, p.age + 10 AS future_age");
    assert_eq!(rows[0]["full_name"], "John Doe");
    assert_eq!(rows[0]["future_age"], 50);
}

// ============================================================
// Function calls in projection
// ============================================================

#[test]
fn return_function_in_projection() {
    let db = TestDb::new();
    db.run("CREATE (:Tag {label:'IMPORTANT'})");
    let rows = db.run("MATCH (t:Tag) RETURN toLower(t.label) AS lower");
    assert_eq!(rows[0]["lower"], "important");
}

#[test]
fn return_count_in_projection() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run("MATCH (p:Person) RETURN count(p) AS total");
    assert_eq!(rows[0]["total"], 6);
}

// ============================================================
// DISTINCT in RETURN
// ============================================================

#[test]
fn return_distinct_property_values() {
    let db = TestDb::new();
    db.seed_org_graph();
    let depts = db.sorted_strings("MATCH (p:Person) RETURN DISTINCT p.dept AS dept", "dept");
    assert_eq!(depts, vec!["Engineering", "Marketing"]);
}

// ============================================================
// Edge cases
// ============================================================

#[test]
fn return_same_property_twice_with_different_aliases() {
    let db = TestDb::new();
    db.run("CREATE (:V {x: 42})");
    let rows = db.run("MATCH (v:V) RETURN v.x AS a, v.x AS b");
    assert_eq!(rows[0]["a"], 42);
    assert_eq!(rows[0]["b"], 42);
}

#[test]
fn return_null_property_value() {
    let db = TestDb::new();
    db.run("CREATE (:P {name: 'Alice'})");
    db.run("CREATE (:P {name: 'Bob', age: 30})");
    let rows = db.run("MATCH (p:P) RETURN p.name AS name, p.age AS age ORDER BY p.name");
    assert_eq!(rows[0]["name"], "Alice");
    assert!(rows[0]["age"].is_null());
    assert_eq!(rows[1]["age"], 30);
}

// ============================================================
// Complex projections
// ============================================================

#[test]
fn return_computed_age_doubled() {
    // Computed expression in RETURN: n.age * 2 AS doubled
    let db = TestDb::new();
    db.run("CREATE (:Person {name: 'Alice', age: 30})");
    let rows = db.run("MATCH (n:Person) RETURN n.age * 2 AS doubled");
    assert_eq!(rows[0]["doubled"], 60);
}

#[test]
fn return_multiple_computed_columns() {
    // Multiple computed columns
    let db = TestDb::new();
    db.run("CREATE (:Item {price: 50, qty: 4})");
    let rows = db.run(
        "MATCH (i:Item) RETURN i.price * i.qty AS total, i.price + 10 AS adjusted, i.qty - 1 AS remaining",
    );
    assert_eq!(rows[0]["total"], 200);
    assert_eq!(rows[0]["adjusted"], 60);
    assert_eq!(rows[0]["remaining"], 3);
}

#[test]
fn return_distinct_on_property_expression() {
    // DISTINCT on property expression
    let db = TestDb::new();
    db.seed_org_graph();
    let depts = db.sorted_strings("MATCH (p:Person) RETURN DISTINCT p.dept AS dept", "dept");
    assert_eq!(depts, vec!["Engineering", "Marketing"]);
}

#[test]
fn return_literal_and_variable_mix() {
    // Returning literal + variable mix
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    let rows = db.run("MATCH (n:User) RETURN n.name AS name, 42 AS magic, 'hello' AS greeting");
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[0]["magic"], 42);
    assert_eq!(rows[0]["greeting"], "hello");
}

#[test]
fn return_relationship_type_in_projection() {
    // Returning relationship type() in projection
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db
        .run("MATCH (a:User {name:'Alice'})-[r]->(b) RETURN type(r) AS rel_type ORDER BY type(r)");
    // Alice has FOLLOWS->Bob and KNOWS->Carol
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["rel_type"], "FOLLOWS");
    assert_eq!(rows[1]["rel_type"], "KNOWS");
}

#[test]
fn return_string_concatenation_computed() {
    // String concatenation as computed expression
    let db = TestDb::new();
    db.run("CREATE (:Person {first: 'John', last: 'Smith'})");
    let rows = db.run("MATCH (p:Person) RETURN p.first + ' ' + p.last AS full_name");
    assert_eq!(rows[0]["full_name"], "John Smith");
}

// ============================================================
// Alias scope and interaction
// ============================================================

#[test]
fn alias_used_in_order_by_via_property() {
    // Alias used alongside ORDER BY (ordering by the original property expression)
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN p.name AS person_name, p.age AS person_age \
         ORDER BY p.age ASC",
    );
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0]["person_name"], "Eve"); // age 26 is youngest
    assert_eq!(rows[0]["person_age"], 26);
}

#[test]
fn mixed_aliases_and_raw_expressions() {
    // Mixed aliases and raw property expressions
    let db = TestDb::new();
    db.run("CREATE (:Widget {name: 'Gadget', weight: 15, color: 'red'})");
    let rows = db.run("MATCH (w:Widget) RETURN w.name AS label, w.weight, w.color AS hue");
    assert_eq!(rows[0]["label"], "Gadget");
    assert_eq!(rows[0]["weight"], 15);
    assert_eq!(rows[0]["hue"], "red");
}

#[test]
fn return_count_with_alias() {
    // Aggregation with alias in return
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db
        .run("MATCH (p:Person) RETURN p.dept AS department, count(p) AS headcount ORDER BY p.dept");
    assert_eq!(rows[0]["department"], "Engineering");
    assert_eq!(rows[0]["headcount"], 4);
    assert_eq!(rows[1]["department"], "Marketing");
    assert_eq!(rows[1]["headcount"], 2);
}

#[test]
fn return_with_expression_on_relationship_property() {
    // Computed expression on relationship property
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person {name:'Alice'})-[r:WORKS_AT]->(c:Company) \
         RETURN p.name AS name, 2026 - r.since AS years_employed",
    );
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[0]["years_employed"], 8); // 2026 - 2018
}

// ============================================================
// Ignored projection tests
// ============================================================

#[test]
fn map_projection() {
    // Lora: map projection n{.name, .age}
    let db = TestDb::new();
    db.run("CREATE (:Person {name: 'Alice', age: 30, dept: 'eng'})");
    let rows = db.run("MATCH (n:Person) RETURN n{.name, .age} AS person");
    let person = &rows[0]["person"];
    assert_eq!(person["name"], "Alice");
    assert_eq!(person["age"], 30);
    assert!(person.get("dept").is_none());
}

#[test]
fn extended_map_projection() {
    // Lora: RETURN n{.*, extra: 'val'} extended map projection
    let db = TestDb::new();
    db.run("CREATE (:Person {name: 'Bob', age: 25})");
    let rows = db.run("MATCH (n:Person) RETURN n{.*, extra: 'val'} AS person");
    let person = &rows[0]["person"];
    assert_eq!(person["name"], "Bob");
    assert_eq!(person["extra"], "val");
}

#[test]
fn pattern_comprehension_in_return() {
    // Lora: pattern comprehension in RETURN
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person {name:'Frank'}) \
         RETURN p.name AS name, [(p)-[:MANAGES]->(s) | s.name] AS subordinates",
    );
    let subs = rows[0]["subordinates"].as_array().unwrap();
    assert_eq!(subs.len(), 3);
}

#[test]
fn return_with_list_slicing() {
    // Lora: RETURN with list slicing [0..3]
    let db = TestDb::new();
    let rows = db.run("RETURN [1, 2, 3, 4, 5][0..3] AS slice");
    let slice = rows[0]["slice"].as_array().unwrap();
    assert_eq!(slice.len(), 3);
    assert_eq!(slice[0], 1);
    assert_eq!(slice[2], 3);
}

#[test]
fn distinct_on_complex_expression_returning_nodes() {
    // Lora: DISTINCT on complex expressions returning nodes
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run("MATCH (p:Person)-[:LIVES_IN]->(c:City) RETURN DISTINCT c");
    assert_eq!(rows.len(), 3); // London, Berlin, Tokyo
}

// ============================================================
// CASE expression in projection
// ============================================================

#[test]
fn return_case_when_categorization() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name AS name, \
                CASE WHEN p.age >= 40 THEN 'senior' \
                     WHEN p.age >= 30 THEN 'mid' \
                     ELSE 'junior' END AS level \
         ORDER BY p.name",
    );
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0]["name"], "Alice"); // 35 -> mid
    assert_eq!(rows[0]["level"], "mid");
    assert_eq!(rows[2]["name"], "Carol"); // 42 -> senior
    assert_eq!(rows[2]["level"], "senior");
    assert_eq!(rows[4]["name"], "Eve"); // 26 -> junior
    assert_eq!(rows[4]["level"], "junior");
}

#[test]
fn return_simple_case_form() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name AS name, \
                CASE p.dept WHEN 'Engineering' THEN 'ENG' \
                            WHEN 'Marketing' THEN 'MKT' \
                            ELSE 'OTHER' END AS code \
         ORDER BY p.name",
    );
    assert_eq!(rows[0]["code"], "ENG"); // Alice
    assert_eq!(rows[2]["code"], "MKT"); // Carol
}

// ============================================================
// Complex nested expressions in projection
// ============================================================

#[test]
fn return_nested_arithmetic() {
    let db = TestDb::new();
    let rows = db.run("RETURN (2 + 3) * (4 - 1) AS result");
    assert_eq!(rows[0]["result"], 15);
}

#[test]
fn return_conditional_string_building() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name + ' (' + p.dept + ')' AS label \
         ORDER BY p.name LIMIT 2",
    );
    assert_eq!(rows[0]["label"], "Alice (Engineering)");
    assert_eq!(rows[1]["label"], "Bob (Engineering)");
}

#[test]
fn return_coalesce_with_default() {
    let db = TestDb::new();
    db.run("CREATE (:P {name: 'Alice', title: 'Dr.'})");
    db.run("CREATE (:P {name: 'Bob'})");
    let rows = db.run(
        "MATCH (p:P) RETURN p.name AS name, coalesce(p.title, 'N/A') AS title ORDER BY p.name",
    );
    assert_eq!(rows[0]["title"], "Dr.");
    assert_eq!(rows[1]["title"], "N/A");
}

// ============================================================
// DISTINCT with null values
// ============================================================

#[test]
fn return_distinct_with_nulls() {
    let db = TestDb::new();
    db.run("CREATE (:V {x: 1})");
    db.run("CREATE (:V {x: 1})");
    db.run("CREATE (:V {})");
    db.run("CREATE (:V {})");
    db.run("CREATE (:V {x: 2})");
    let rows = db.run("MATCH (v:V) RETURN DISTINCT v.x AS x ORDER BY x");
    // Should be: 1, 2, null (3 distinct values)
    assert_eq!(rows.len(), 3);
}

// ============================================================
// Projection with labels/type functions
// ============================================================

#[test]
fn return_labels_function_in_projection() {
    let db = TestDb::new();
    db.run("CREATE (:Person:Admin {name: 'Alice'})");
    let rows = db.run("MATCH (n:Person) RETURN n.name AS name, labels(n) AS labels");
    let labels = rows[0]["labels"].as_array().unwrap();
    assert!(labels.contains(&serde_json::json!("Person")));
    assert!(labels.contains(&serde_json::json!("Admin")));
}

#[test]
fn return_keys_function_in_projection() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'Widget', price: 10, color: 'red'})");
    let rows = db.run("MATCH (i:Item) RETURN keys(i) AS k");
    let keys = rows[0]["k"].as_array().unwrap();
    assert_eq!(keys.len(), 3);
}

// ============================================================
// Ignored projection tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn return_collect_subquery_in_projection() {
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name AS name, \
                COUNT { MATCH (p)-[:MANAGES]->() } AS report_count",
    );
}

#[test]
#[ignore = "pending implementation"]
fn return_exists_subquery_in_projection() {
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name AS name, \
                EXISTS { MATCH (p)-[:MANAGES]->() } AS is_manager",
    );
}
