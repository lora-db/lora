/// Parameter binding tests — $param syntax resolves from a runtime parameter map.
mod test_helpers;
use std::collections::BTreeMap;
use test_helpers::TestDb;

use lora_database::LoraValue;

/// Helper to build a parameter map from key-value pairs.
fn params(entries: &[(&str, LoraValue)]) -> BTreeMap<String, LoraValue> {
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

// ============================================================
// Scalar parameter types
// ============================================================

#[test]
fn parameter_scalar_string() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    db.run("CREATE (:User {name: 'Bob'})");
    let rows = db.run_with_params(
        "MATCH (n:User) WHERE n.name = $name RETURN n.name AS name",
        params(&[("name", LoraValue::String("Alice".into()))]),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

#[test]
fn parameter_scalar_integer() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice', age: 30})");
    db.run("CREATE (:User {name: 'Bob', age: 25})");
    let rows = db.run_with_params(
        "MATCH (n:User) WHERE n.age = $age RETURN n.name AS name",
        params(&[("age", LoraValue::Int(30))]),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

#[test]
fn parameter_scalar_boolean() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice', active: true})");
    db.run("CREATE (:User {name: 'Bob', active: false})");
    let rows = db.run_with_params(
        "MATCH (n:User) WHERE n.active = $active RETURN n.name AS name",
        params(&[("active", LoraValue::Bool(true))]),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

// ============================================================
// Parameters in RETURN / projection
// ============================================================

#[test]
fn parameter_in_return_expression() {
    let db = TestDb::new();
    let rows = db.run_with_params(
        "RETURN $val AS v",
        params(&[("val", LoraValue::Int(42))]),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["v"], 42);
}

#[test]
fn parameter_string_in_return() {
    let db = TestDb::new();
    let rows = db.run_with_params(
        "RETURN $greeting AS msg",
        params(&[("greeting", LoraValue::String("hello".into()))]),
    );
    assert_eq!(rows[0]["msg"], "hello");
}

// ============================================================
// Parameter reuse
// ============================================================

#[test]
fn parameter_reused_multiple_times() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice', nickname: 'Alice'})");
    db.run("CREATE (:User {name: 'Bob', nickname: 'Bobby'})");
    let rows = db.run_with_params(
        "MATCH (n:User) WHERE n.name = $val AND n.nickname = $val RETURN n.name AS name",
        params(&[("val", LoraValue::String("Alice".into()))]),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

// ============================================================
// Numeric parameters ($1 syntax)
// ============================================================

#[test]
fn parameter_numeric_index() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    let rows = db.run_with_params(
        "MATCH (n:User) WHERE n.name = $1 RETURN n.name AS name",
        params(&[("1", LoraValue::String("Alice".into()))]),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

// ============================================================
// Missing parameter resolves to null
// ============================================================

#[test]
fn parameter_missing_resolves_to_null() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    // $undefined is not in the params map — resolves to null, so no match
    let rows = db.run_with_params(
        "MATCH (n:User) WHERE n.name = $undefined RETURN n",
        params(&[("other", LoraValue::String("irrelevant".into()))]),
    );
    assert_eq!(rows.len(), 0);
}

#[test]
fn parameter_missing_with_empty_params() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    // No params at all — $name resolves to null
    let rows = db.run_with_params(
        "MATCH (n:User) WHERE n.name = $name RETURN n",
        BTreeMap::new(),
    );
    assert_eq!(rows.len(), 0);
}

// ============================================================
// Parameters in CREATE property maps
// ============================================================

#[test]
fn parameter_in_create_property_value() {
    let db = TestDb::new();
    db.run_with_params(
        "CREATE (:Item {name: $name})",
        params(&[("name", LoraValue::String("Widget".into()))]),
    );
    let rows = db.run("MATCH (i:Item) RETURN i.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Widget");
}

// ============================================================
// Parameters in comparison expressions
// ============================================================

#[test]
fn parameter_in_greater_than() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice', age: 30})");
    db.run("CREATE (:User {name: 'Bob', age: 25})");
    db.run("CREATE (:User {name: 'Carol', age: 35})");
    let rows = db.run_with_params(
        "MATCH (n:User) WHERE n.age > $minAge RETURN n.name AS name ORDER BY n.name",
        params(&[("minAge", LoraValue::Int(28))]),
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[1]["name"], "Carol");
}

#[test]
fn parameter_in_list_membership() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    db.run("CREATE (:User {name: 'Bob'})");
    db.run("CREATE (:User {name: 'Carol'})");
    let rows = db.run_with_params(
        "MATCH (n:User) WHERE n.name IN $names RETURN n.name AS name ORDER BY n.name",
        params(&[(
            "names",
            LoraValue::List(vec![
                LoraValue::String("Alice".into()),
                LoraValue::String("Carol".into()),
            ]),
        )]),
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[1]["name"], "Carol");
}

// ============================================================
// Parameters with no data interaction (backward compatibility)
// ============================================================

#[test]
fn queries_without_params_still_work() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    // Using run() (no params) should still work exactly as before
    let rows = db.run("MATCH (n:User) RETURN n.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

// ============================================================
// Parameter edge cases
// ============================================================

#[test]
fn parameter_float_value() {
    let db = TestDb::new();
    db.run("CREATE (:Metric {name: 'temp', value: 36.6})");
    db.run("CREATE (:Metric {name: 'pressure', value: 120.5})");
    let rows = db.run_with_params(
        "MATCH (m:Metric) WHERE m.value > $threshold RETURN m.name AS name",
        params(&[("threshold", LoraValue::Float(100.0))]),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "pressure");
}

#[test]
fn parameter_null_in_where() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice', age: 30})");
    db.run("CREATE (:User {name: 'Bob', age: 25})");
    // Passing Null as parameter — no rows should match since null != anything
    let rows = db.run_with_params(
        "MATCH (n:User) WHERE n.name = $name RETURN n",
        params(&[("name", LoraValue::Null)]),
    );
    assert_eq!(rows.len(), 0);
}

#[test]
fn parameter_in_arithmetic_expression() {
    let db = TestDb::new();
    db.run("CREATE (:Item {price: 100})");
    db.run("CREATE (:Item {price: 200})");
    db.run("CREATE (:Item {price: 300})");
    let rows = db.run_with_params(
        "MATCH (i:Item) WHERE i.price > $base + 50 RETURN i.price AS price ORDER BY i.price",
        params(&[("base", LoraValue::Int(100))]),
    );
    // base + 50 = 150, so items with price > 150: 200, 300
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["price"], 200);
    assert_eq!(rows[1]["price"], 300);
}

#[test]
fn multiple_different_params_in_same_query() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice', age: 30})");
    db.run("CREATE (:User {name: 'Bob', age: 25})");
    db.run("CREATE (:User {name: 'Carol', age: 35})");
    let rows = db.run_with_params(
        "MATCH (n:User) WHERE n.name = $name AND n.age = $age RETURN n.name AS name",
        params(&[
            ("name", LoraValue::String("Alice".into())),
            ("age", LoraValue::Int(30)),
        ]),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

#[test]
fn parameter_as_list_in_unwind() {
    let db = TestDb::new();
    let rows = db.run_with_params(
        "UNWIND $items AS item RETURN item",
        params(&[(
            "items",
            LoraValue::List(vec![
                LoraValue::Int(10),
                LoraValue::Int(20),
                LoraValue::Int(30),
            ]),
        )]),
    );
    assert_eq!(rows.len(), 3);
}

// ============================================================
// Parameters with seed graphs
// ============================================================

#[test]
fn parameter_match_on_org_graph() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (p:Person) WHERE p.dept = $dept RETURN p.name AS name",
        "name",
    );
    // Without params this would fail, but sorted_strings uses run() not run_with_params.
    // Use run_with_params instead:
    let rows = db.run_with_params(
        "MATCH (p:Person) WHERE p.dept = $dept RETURN p.name AS name ORDER BY p.name",
        params(&[("dept", LoraValue::String("Marketing".into()))]),
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "Carol");
    assert_eq!(rows[1]["name"], "Dave");
}

#[test]
fn parameter_in_order_by_expression() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'A', score: 10})");
    db.run("CREATE (:Item {name: 'B', score: 20})");
    db.run("CREATE (:Item {name: 'C', score: 30})");
    let rows = db.run_with_params(
        "MATCH (i:Item) WHERE i.score >= $min RETURN i.name AS name ORDER BY i.score DESC",
        params(&[("min", LoraValue::Int(15))]),
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "C");
    assert_eq!(rows[1]["name"], "B");
}

#[test]
fn parameter_threshold_filtering_on_transport_graph() {
    let db = TestDb::new();
    db.seed_transport_graph();
    // Find routes with distance greater than a parameter threshold
    let rows = db.run_with_params(
        "MATCH (s1:Station)-[r:ROUTE]->(s2:Station) \
         WHERE r.distance > $min_dist \
         RETURN s1.name AS from, s2.name AS to, r.distance AS dist \
         ORDER BY r.distance DESC",
        params(&[("min_dist", LoraValue::Int(50))]),
    );
    // Routes with distance > 50: Amsterdam<->Rotterdam(60), Utrecht<->Rotterdam(55), Utrecht<->Eindhoven(100)
    // Bidirectional so 6 directed edges
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0]["dist"], 100); // Utrecht->Eindhoven or Eindhoven->Utrecht
}

// ============================================================
// Ignored parameter tests (pending implementation)
// ============================================================

#[test]
fn parameter_as_property_map_in_create() {
    // Lora: parameter as property map in CREATE {$props}
    let db = TestDb::new();
    let mut props = BTreeMap::new();
    props.insert("name".to_string(), LoraValue::String("Widget".into()));
    props.insert("price".to_string(), LoraValue::Int(42));
    db.run_with_params(
        "CREATE (n:Item $props) RETURN n",
        params(&[("props", LoraValue::Map(props))]),
    );
    let rows = db.run("MATCH (i:Item) RETURN i.name AS name, i.price AS price");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Widget");
    assert_eq!(rows[0]["price"], 42);
}

#[test]
#[ignore = "parameter as label: dynamic labels via parameters not standard Lora"]
fn parameter_as_label_name() {
    // Lora: parameter as label name (not standard Lora)
    let db = TestDb::new();
    db.run("CREATE (:DynamicLabel {name: 'test'})");
    let rows = db.run_with_params(
        "MATCH (n:$label) RETURN n.name AS name",
        params(&[("label", LoraValue::String("DynamicLabel".into()))]),
    );
    assert_eq!(rows.len(), 1);
}

#[test]
#[ignore = "parameter validation: type checking at parse time not yet implemented"]
fn parameter_type_checking_at_parse_time() {
    // Lora: parameter validation/type checking at parse time
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice', age: 30})");
    // Passing a string where an integer is expected in comparison
    let err = db.run_err("MATCH (n:User) WHERE n.age > $age RETURN n");
    // Should detect type mismatch at parse/plan time
    assert!(!err.is_empty());
}

#[test]
fn param_list_value() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'A', tag: 'x'})");
    db.run("CREATE (:Item {name: 'B', tag: 'y'})");
    db.run("CREATE (:Item {name: 'C', tag: 'x'})");

    use std::collections::BTreeMap;
    use lora_database::LoraValue;

    let mut params = BTreeMap::new();
    params.insert(
        "tags".to_string(),
        LoraValue::List(vec![LoraValue::String("x".to_string())]),
    );
    let rows = db.run_with_params(
        "MATCH (i:Item) WHERE i.tag IN $tags RETURN i.name AS name ORDER BY name",
        params,
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "A");
    assert_eq!(rows[1]["name"], "C");
}

#[test]
fn param_in_create() {
    let db = TestDb::new();

    use std::collections::BTreeMap;
    use lora_database::LoraValue;

    let mut params = BTreeMap::new();
    params.insert("name".to_string(), LoraValue::String("Parameterized".to_string()));
    params.insert("age".to_string(), LoraValue::Int(25));
    db.run_with_params(
        "CREATE (:Person {name: $name, age: $age})",
        params,
    );
    let rows = db.run("MATCH (p:Person {name: 'Parameterized'}) RETURN p.age AS age");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["age"], 25);
}

#[test]
fn param_boolean_in_where() {
    let db = TestDb::new();
    db.run("CREATE (:Flag {active: true, name: 'on'})");
    db.run("CREATE (:Flag {active: false, name: 'off'})");

    use std::collections::BTreeMap;
    use lora_database::LoraValue;

    let mut params = BTreeMap::new();
    params.insert("active".to_string(), LoraValue::Bool(true));
    let rows = db.run_with_params(
        "MATCH (f:Flag) WHERE f.active = $active RETURN f.name AS name",
        params,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "on");
}

#[test]
#[ignore = "pending implementation"]
fn param_map_as_properties() {
    let db = TestDb::new();

    use std::collections::BTreeMap;
    use lora_database::LoraValue;

    let mut props = BTreeMap::new();
    props.insert("name".to_string(), LoraValue::String("Dynamic".to_string()));
    props.insert("score".to_string(), LoraValue::Int(99));
    let mut params = BTreeMap::new();
    params.insert("props".to_string(), LoraValue::Map(props));
    db.run_with_params("CREATE (:Dynamic $props)", params);
    let rows = db.run("MATCH (d:Dynamic) RETURN d.name AS name, d.score AS score");
    assert_eq!(rows[0]["name"], "Dynamic");
    assert_eq!(rows[0]["score"], 99);
}

// ============================================================
// Parameters in SET clauses
// ============================================================

#[test]
fn parameter_in_set_value() {
    let db = TestDb::new();
    db.run("CREATE (:Target {name: 'x', val: 0})");
    db.run_with_params(
        "MATCH (t:Target {name: 'x'}) SET t.val = $newval",
        params(&[("newval", LoraValue::Int(42))]),
    );
    let rows = db.run("MATCH (t:Target {name: 'x'}) RETURN t.val AS val");
    assert_eq!(rows[0]["val"], 42);
}

// ============================================================
// Parameters in MERGE
// ============================================================

#[test]
fn parameter_in_merge_pattern() {
    let db = TestDb::new();
    db.run_with_params(
        "MERGE (n:Config {key: $key}) ON CREATE SET n.val = $val",
        params(&[
            ("key", LoraValue::String("timeout".into())),
            ("val", LoraValue::Int(30)),
        ]),
    );
    let rows = db.run("MATCH (c:Config {key: 'timeout'}) RETURN c.val AS val");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["val"], 30);

    // Second merge should not create duplicate
    db.run_with_params(
        "MERGE (n:Config {key: $key}) ON CREATE SET n.val = $val ON MATCH SET n.val = $val + 10",
        params(&[
            ("key", LoraValue::String("timeout".into())),
            ("val", LoraValue::Int(30)),
        ]),
    );
    db.assert_count("MATCH (c:Config) RETURN c", 1);
}

// ============================================================
// Parameters in complex expressions
// ============================================================

#[test]
fn parameter_in_string_concatenation() {
    let db = TestDb::new();
    let rows = db.run_with_params(
        "RETURN 'Hello, ' + $name + '!' AS greeting",
        params(&[("name", LoraValue::String("World".into()))]),
    );
    assert_eq!(rows[0]["greeting"], "Hello, World!");
}

#[test]
fn parameter_in_case_expression() {
    let db = TestDb::new();
    db.run("CREATE (:Item {score: 85})");
    db.run("CREATE (:Item {score: 45})");
    let rows = db.run_with_params(
        "MATCH (i:Item) \
         RETURN i.score AS score, \
                CASE WHEN i.score >= $threshold THEN 'pass' ELSE 'fail' END AS result \
         ORDER BY i.score DESC",
        params(&[("threshold", LoraValue::Int(60))]),
    );
    assert_eq!(rows[0]["result"], "pass");
    assert_eq!(rows[1]["result"], "fail");
}

// ============================================================
// Parameters with UNWIND
// ============================================================

#[test]
fn parameter_list_in_unwind_create() {
    let db = TestDb::new();
    db.run_with_params(
        "UNWIND $names AS name CREATE (:Person {name: name})",
        params(&[(
            "names",
            LoraValue::List(vec![
                LoraValue::String("Alice".into()),
                LoraValue::String("Bob".into()),
                LoraValue::String("Carol".into()),
            ]),
        )]),
    );
    db.assert_count("MATCH (p:Person) RETURN p", 3);
}

// ============================================================
// Future parameter tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn parameter_in_skip_limit() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 10) AS i CREATE (:N {id: i})");
    let _rows = db.run_with_params(
        "MATCH (n:N) RETURN n.id AS id ORDER BY id SKIP $skip LIMIT $limit",
        params(&[
            ("skip", LoraValue::Int(2)),
            ("limit", LoraValue::Int(3)),
        ]),
    );
}
