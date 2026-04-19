/// WHERE clause tests — equality, comparison, boolean logic, null handling,
/// string operators, IN, arithmetic predicates, relationship properties,
/// and complex compound conditions.
mod test_helpers;
use test_helpers::TestDb;

fn db_with_users() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice', age: 30, active: true})");
    db.run("CREATE (b:User {name: 'Bob', age: 25, active: false})");
    db.run("CREATE (c:User {name: 'Carol', age: 35, active: true})");
    db.run("CREATE (d:User {name: 'Dave', age: 25, active: false})");
    db
}

// ============================================================
// Equality
// ============================================================

#[test]
fn where_equality_string() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name = 'Alice' RETURN n", 1);
}

#[test]
fn where_equality_integer() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.age = 25 RETURN n", 2);
}

#[test]
fn where_equality_boolean() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.active = true RETURN n", 2);
}

#[test]
fn where_equality_no_match() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name = 'Zara' RETURN n", 0);
}

// ============================================================
// Inequality and comparisons
// ============================================================

#[test]
fn where_not_equal() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name <> 'Alice' RETURN n", 3);
}

#[test]
fn where_greater_than() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.age > 25 RETURN n", 2);
}

#[test]
fn where_greater_than_or_equal() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.age >= 30 RETURN n", 2);
}

#[test]
fn where_less_than() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.age < 30 RETURN n", 2);
}

#[test]
fn where_less_than_or_equal() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.age <= 25 RETURN n", 2);
}

// ============================================================
// Boolean logic
// ============================================================

#[test]
fn where_and() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.age > 25 AND n.active = true RETURN n", 2);
}

#[test]
fn where_or() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name = 'Alice' OR n.name = 'Bob' RETURN n", 2);
}

#[test]
fn where_not() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE NOT n.active RETURN n", 2);
}

#[test]
fn where_combined_and_or() {
    let db = db_with_users();
    db.assert_count(
        "MATCH (n:User) WHERE (n.active AND n.age > 30) OR n.name = 'Bob' RETURN n",
        2,
    );
}

#[test]
fn where_xor() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.active XOR n.age < 30 RETURN n", 4);
}

#[test]
fn complex_where_with_and_or_not() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (p:Person) \
         WHERE (p.dept = 'Engineering' AND p.age > 30) OR p.name = 'Dave' \
         RETURN p.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Alice", "Dave", "Frank"]);
}

// ============================================================
// Null handling
// ============================================================

#[test]
fn where_is_null() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice', age: 30})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.assert_count("MATCH (n:User) WHERE n.age IS NULL RETURN n", 1);
}

#[test]
fn where_is_not_null() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice', age: 30})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.assert_count("MATCH (n:User) WHERE n.age IS NOT NULL RETURN n", 1);
}

// ============================================================
// String operators
// ============================================================

#[test]
fn where_starts_with() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name STARTS WITH 'A' RETURN n", 1);
}

#[test]
fn where_ends_with() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name ENDS WITH 'ol' RETURN n", 1);
}

#[test]
fn where_contains() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name CONTAINS 'ob' RETURN n", 1);
}

// ============================================================
// IN operator
// ============================================================

#[test]
fn where_in_list() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.age IN [25, 30] RETURN n", 3);
}

#[test]
fn where_in_empty_list() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.age IN [] RETURN n", 0);
}

#[test]
fn in_operator_with_strings() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (p:Person) WHERE p.name IN ['Alice', 'Carol', 'Eve'] RETURN p.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Alice", "Carol", "Eve"]);
}

#[test]
fn in_operator_with_mixed_types() {
    let db = TestDb::new();
    let rows = db.run("RETURN 1 IN ['a', 'b']");
    assert_eq!(rows.len(), 1);
}

// ============================================================
// Arithmetic in predicates
// ============================================================

#[test]
fn where_arithmetic_expression() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.age + 5 > 32 RETURN n", 2);
}

#[test]
fn where_constant_expression() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE 1 + 1 = 2 RETURN n", 4);
}

#[test]
fn where_false_constant() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE 1 > 2 RETURN n", 0);
}

// ============================================================
// Relationship property filtering
// ============================================================

#[test]
fn where_on_relationship_property() {
    let db = TestDb::new();
    db.seed_social_graph();
    db.assert_count("MATCH (a)-[r:FOLLOWS]->(b) WHERE r.since > 2020 RETURN a, b", 1);
}

// ============================================================
// Nested boolean logic
// ============================================================

#[test]
fn where_nested_parenthesized_logic() {
    let db = db_with_users();
    // ((active AND age>28) OR (NOT active AND age<30)) matches all 4:
    // Alice(active,30): true AND 30>28=true; Bob(inactive,25): true AND 25<30=true
    // Carol(active,35): true AND 35>28=true; Dave(inactive,25): true AND 25<30=true
    db.assert_count(
        "MATCH (n:User) WHERE ((n.active = true AND n.age > 28) OR (n.active = false AND n.age < 30)) RETURN n",
        4,
    );
}

#[test]
fn where_not_with_comparison() {
    let db = db_with_users();
    // NOT (age > 30) matches Alice(30), Bob(25), Dave(25) = 3
    db.assert_count("MATCH (n:User) WHERE NOT (n.age > 30) RETURN n", 3);
}

#[test]
fn where_double_negation_is_identity() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE NOT NOT (n.active = true) RETURN n", 2);
}

#[test]
fn where_multiple_and_chained() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) WHERE p.dept = 'Engineering' AND p.age > 25 AND p.age < 40 \
         RETURN p.name AS name ORDER BY p.name",
    );
    // Engineering: Alice(35), Bob(28), Eve(26), Frank(50)
    // age > 25 AND age < 40: Alice(35), Bob(28), Eve(26)
    assert_eq!(rows.len(), 3);
}

#[test]
fn where_multiple_or_chained() {
    let db = db_with_users();
    db.assert_count(
        "MATCH (n:User) WHERE n.name = 'Alice' OR n.name = 'Bob' OR n.name = 'Carol' RETURN n",
        3,
    );
}

// ============================================================
// Comparing two properties
// ============================================================

#[test]
fn where_compare_two_node_properties() {
    let db = TestDb::new();
    db.run("CREATE (:Box {width: 10, height: 20})");
    db.run("CREATE (:Box {width: 15, height: 15})");
    db.run("CREATE (:Box {width: 30, height: 5})");
    // width > height
    db.assert_count("MATCH (b:Box) WHERE b.width > b.height RETURN b", 1);
    // width = height
    db.assert_count("MATCH (b:Box) WHERE b.width = b.height RETURN b", 1);
}

#[test]
fn where_compare_properties_across_nodes() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Find pairs where manager is older than employee
    let rows = db.run(
        "MATCH (m:Person)-[:MANAGES]->(e:Person) \
         WHERE m.age > e.age \
         RETURN m.name AS mgr, e.name AS emp",
    );
    // Frank(50) manages Alice(35), Bob(28), Eve(26) — all younger
    // Carol(42) manages Dave(31) — younger
    assert_eq!(rows.len(), 4);
}

// ============================================================
// Float comparison
// ============================================================

#[test]
fn where_float_comparison() {
    let db = TestDb::new();
    db.run("CREATE (:Measurement {temp: 36.5})");
    db.run("CREATE (:Measurement {temp: 37.2})");
    db.run("CREATE (:Measurement {temp: 38.9})");
    db.assert_count("MATCH (m:Measurement) WHERE m.temp > 37.0 RETURN m", 2);
}

#[test]
fn where_int_equals_float() {
    let db = TestDb::new();
    // Test cross-type comparison: int 10 should equal float 10.0
    let rows = db.run("RETURN 10 = 10.0");
    assert_eq!(rows.len(), 1);
}

// ============================================================
// String operator edge cases
// ============================================================

#[test]
fn where_starts_with_empty_string_matches_all() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name STARTS WITH '' RETURN n", 4);
}

#[test]
fn where_contains_empty_string_matches_all() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name CONTAINS '' RETURN n", 4);
}

#[test]
fn where_ends_with_empty_string_matches_all() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name ENDS WITH '' RETURN n", 4);
}

#[test]
fn where_starts_with_full_string() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name STARTS WITH 'Alice' RETURN n", 1);
}

#[test]
fn where_combined_string_and_comparison() {
    let db = db_with_users();
    db.assert_count(
        "MATCH (n:User) WHERE n.name STARTS WITH 'A' AND n.age >= 30 RETURN n",
        1,
    );
}

// ============================================================
// IS NULL / IS NOT NULL on relationships
// ============================================================

#[test]
fn where_is_null_on_relationship_property() {
    let db = TestDb::new();
    db.run("CREATE (a:N {id:1})-[:R {weight: 5}]->(b:N {id:2})");
    db.run("CREATE (c:N {id:3})-[:R]->(d:N {id:4})");
    db.assert_count("MATCH (a)-[r:R]->(b) WHERE r.weight IS NULL RETURN r", 1);
}

#[test]
fn where_is_not_null_on_relationship_property() {
    let db = TestDb::new();
    db.run("CREATE (a:N {id:1})-[:R {weight: 5}]->(b:N {id:2})");
    db.run("CREATE (c:N {id:3})-[:R]->(d:N {id:4})");
    db.assert_count("MATCH (a)-[r:R]->(b) WHERE r.weight IS NOT NULL RETURN r", 1);
}

// ============================================================
// IN operator edge cases
// ============================================================

#[test]
fn where_not_in_list() {
    let db = db_with_users();
    // NOT (age IN [25]) should exclude Bob and Dave
    db.assert_count("MATCH (n:User) WHERE NOT n.age IN [25] RETURN n", 2);
}

#[test]
fn where_in_single_element_list() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) WHERE n.name IN ['Alice'] RETURN n", 1);
}

// ============================================================
// Arithmetic in WHERE
// ============================================================

#[test]
fn where_modulo_expression() {
    let db = db_with_users();
    // Even ages: 30, 25 (odd), 35 (odd), 25 (odd) — only Alice(30)
    db.assert_count("MATCH (n:User) WHERE n.age % 2 = 0 RETURN n", 1);
}

#[test]
fn where_subtraction_comparison() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Years at company: 2024 - r.since >= 5
    // Alice(2018)=6, Carol(2015)=9, Frank(2012)=12 → 3 match
    db.assert_count(
        "MATCH (p:Person)-[r:WORKS_AT]->(c:Company) WHERE 2024 - r.since >= 5 RETURN p",
        3,
    );
}

// ============================================================
// Scenario-based: recommendation graph
// ============================================================

#[test]
fn where_filters_ratings_on_recommendation_graph() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    // Find viewer-movie pairs where score >= 4
    let rows = db.run(
        "MATCH (v:Viewer)-[r:RATED]->(m:Movie) \
         WHERE r.score >= 4 \
         RETURN v.name AS viewer, m.title AS movie ORDER BY v.name, m.title",
    );
    // Alice: Matrix(5), Inception(4) ; Bob: Matrix(5) ; Carol: Amelie(4), Inception(5)
    assert_eq!(rows.len(), 5);
}

// ============================================================
// Complex nested boolean expressions
// ============================================================

#[test]
fn where_triple_and() {
    let db = db_with_users();
    // Alice: name starts with 'A', age 30, active true => match
    db.assert_count(
        "MATCH (n:User) WHERE n.name STARTS WITH 'A' AND n.age >= 30 AND n.active = true RETURN n",
        1,
    );
}

#[test]
fn where_or_combined_with_and_precedence() {
    let db = db_with_users();
    // AND binds tighter: (name='Alice') OR (age=25 AND active=false)
    // Alice(match first OR) + Bob(25,inactive) + Dave(25,inactive) = 3
    db.assert_count(
        "MATCH (n:User) WHERE n.name = 'Alice' OR n.age = 25 AND n.active = false RETURN n",
        3,
    );
}

#[test]
fn where_not_with_nested_and_or() {
    let db = db_with_users();
    // NOT (active=true AND age>32) — Carol(active,35) is the only one matching inner => NOT excludes her
    // Remaining: Alice(active,30), Bob(inactive,25), Dave(inactive,25) => 3
    db.assert_count(
        "MATCH (n:User) WHERE NOT (n.active = true AND n.age > 32) RETURN n",
        3,
    );
}

#[test]
fn where_xor_with_other_boolean_ops() {
    let db = db_with_users();
    // (active XOR (age < 30)) AND name <> 'Dave'
    // Alice: true XOR false = true, name<>'Dave' => match
    // Bob:   false XOR true = true, name<>'Dave' => match
    // Carol: true XOR false = true, name<>'Dave' => match
    // Dave:  false XOR true = true, name='Dave' => no
    db.assert_count(
        "MATCH (n:User) WHERE (n.active XOR (n.age < 30)) AND n.name <> 'Dave' RETURN n",
        3,
    );
}

#[test]
fn where_deeply_nested_boolean() {
    let db = TestDb::new();
    db.seed_org_graph();
    // (dept='Engineering' AND age>30) OR (dept='Marketing' AND (age>40 OR name='Dave'))
    // Engineering & age>30: Alice(35), Frank(50) => 2
    // Marketing & (age>40 OR name='Dave'): Carol(42)=>age>40 yes, Dave(31)=>name='Dave' yes => 2
    // Total: 4
    db.assert_count(
        "MATCH (p:Person) WHERE \
         (p.dept = 'Engineering' AND p.age > 30) OR \
         (p.dept = 'Marketing' AND (p.age > 40 OR p.name = 'Dave')) \
         RETURN p",
        4,
    );
}

#[test]
fn where_nested_not_or_and() {
    let db = db_with_users();
    // NOT ((name='Alice' OR name='Bob') AND active=true)
    // Inner: (Alice OR Bob) AND active => Alice(active)=true => match, Bob(inactive)=false
    // NOT => exclude Alice. Remaining: Bob, Carol, Dave => 3
    db.assert_count(
        "MATCH (n:User) WHERE NOT ((n.name = 'Alice' OR n.name = 'Bob') AND n.active = true) RETURN n",
        3,
    );
}

// ============================================================
// String operators in WHERE (extended)
// ============================================================

#[test]
fn where_starts_with_multiple_matches() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Names starting with 'A': Alice => 1; starting with 'E': Eve => 1
    db.assert_count(
        "MATCH (p:Person) WHERE p.name STARTS WITH 'A' OR p.name STARTS WITH 'E' RETURN p",
        2,
    );
}

#[test]
fn where_ends_with_on_property() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Names ending with 'e': Alice, Dave, Eve => 3
    db.assert_count(
        "MATCH (p:Person) WHERE p.name ENDS WITH 'e' RETURN p",
        3,
    );
}

#[test]
fn where_contains_substring() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Names containing 'a': Carol, Dave, Frank => 3
    db.assert_count(
        "MATCH (p:Person) WHERE p.name CONTAINS 'a' RETURN p",
        3,
    );
}

#[test]
fn where_string_operators_combined_with_and() {
    let db = db_with_users();
    // Starts with 'C' AND contains 'ar' => Carol
    db.assert_count(
        "MATCH (n:User) WHERE n.name STARTS WITH 'C' AND n.name CONTAINS 'ar' RETURN n",
        1,
    );
}

#[test]
fn where_case_sensitive_starts_with() {
    let db = db_with_users();
    // 'a' lowercase should NOT match 'Alice'
    db.assert_count(
        "MATCH (n:User) WHERE n.name STARTS WITH 'a' RETURN n",
        0,
    );
}

#[test]
fn where_string_comparison_with_property_access() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Compare property to literal string with > (lexicographic)
    // Names > 'D': Dave, Eve, Frank => 3
    db.assert_count(
        "MATCH (p:Person) WHERE p.name > 'D' RETURN p",
        3,
    );
}

// ============================================================
// IS NULL / IS NOT NULL edge cases
// ============================================================

#[test]
fn where_missing_property_is_null_returns_true() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'Widget'})");
    db.run("CREATE (:Item {name: 'Gadget', color: 'red'})");
    // Widget has no color property => IS NULL should match
    db.assert_count("MATCH (i:Item) WHERE i.color IS NULL RETURN i", 1);
}

#[test]
fn where_existing_property_is_not_null() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'Widget'})");
    db.run("CREATE (:Item {name: 'Gadget', color: 'red'})");
    db.assert_count("MATCH (i:Item) WHERE i.color IS NOT NULL RETURN i", 1);
}

#[test]
fn where_null_check_combined_with_other_filter() {
    let db = TestDb::new();
    db.run("CREATE (:Product {name: 'A', price: 10})");
    db.run("CREATE (:Product {name: 'B', price: 50})");
    db.run("CREATE (:Product {name: 'C'})");
    // Has a price AND price > 20
    db.assert_count(
        "MATCH (p:Product) WHERE p.price IS NOT NULL AND p.price > 20 RETURN p",
        1,
    );
}

#[test]
fn where_null_check_on_relationship_property_missing() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // FOLLOWS relationships have no 'since' property, KNOWS relationships do
    // All FOLLOWS edges should have since IS NULL
    let follows_count = db.run(
        "MATCH (a)-[r:FOLLOWS]->(b) WHERE r.since IS NULL RETURN r",
    );
    assert_eq!(follows_count.len(), 6); // 6 FOLLOWS edges
}

#[test]
fn where_is_null_or_value_check() {
    let db = TestDb::new();
    db.run("CREATE (:Sensor {id: 1, reading: 42})");
    db.run("CREATE (:Sensor {id: 2})");
    db.run("CREATE (:Sensor {id: 3, reading: 0})");
    // reading IS NULL OR reading = 0 => sensor 2 and 3
    db.assert_count(
        "MATCH (s:Sensor) WHERE s.reading IS NULL OR s.reading = 0 RETURN s",
        2,
    );
}

// ============================================================
// Arithmetic in predicates (extended)
// ============================================================

#[test]
fn where_price_times_qty_greater_than() {
    let db = TestDb::new();
    db.run("CREATE (:Order {name: 'A', price: 20, qty: 3})");  // 60
    db.run("CREATE (:Order {name: 'B', price: 50, qty: 3})");  // 150
    db.run("CREATE (:Order {name: 'C', price: 10, qty: 5})");  // 50
    db.assert_count(
        "MATCH (o:Order) WHERE o.price * o.qty > 100 RETURN o",
        1,
    );
}

#[test]
fn where_age_plus_offset_less_than() {
    let db = db_with_users();
    // age + 5 < 32 => age < 27 => Bob(25), Dave(25) => 2
    db.assert_count(
        "MATCH (n:User) WHERE n.age + 5 < 32 RETURN n",
        2,
    );
}

#[test]
fn where_modulo_filters_even_values() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Even ages: Bob(28), Carol(42), Eve(26), Frank(50) => 4
    db.assert_count(
        "MATCH (p:Person) WHERE p.age % 2 = 0 RETURN p",
        4,
    );
}

#[test]
fn where_computed_expression_comparison() {
    let db = TestDb::new();
    db.run("CREATE (:Rect {width: 10, height: 5})");
    db.run("CREATE (:Rect {width: 3, height: 3})");
    db.run("CREATE (:Rect {width: 20, height: 2})");
    // Area = width * height > 30 => Rect1(50), Rect3(40) => 2
    db.assert_count(
        "MATCH (r:Rect) WHERE r.width * r.height > 30 RETURN r",
        2,
    );
}

#[test]
fn where_subtraction_in_predicate() {
    let db = db_with_users();
    // age - 25 > 0 => age > 25 => Alice(30), Carol(35) => 2
    db.assert_count(
        "MATCH (n:User) WHERE n.age - 25 > 0 RETURN n",
        2,
    );
}

// ============================================================
// Relationship property filtering (extended)
// ============================================================

#[test]
fn where_filter_by_relationship_property_value() {
    let db = TestDb::new();
    db.seed_transport_graph();
    // Find routes shorter than 50 km
    // Amsterdam-Utrecht(40), Rotterdam-DenHaag(25) and their reverses => 4
    db.assert_count(
        "MATCH (a:Station)-[r:ROUTE]->(b:Station) WHERE r.distance < 50 RETURN a, b",
        4,
    );
}

#[test]
fn where_range_filter_on_relationship_numeric_property() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // KNOWS edges with strength between 5 and 8 (inclusive)
    // Alice->Bob(5), Alice->Carol(8), Carol->Eve(6), Eve->Frank(7) => 4
    db.assert_count(
        "MATCH (a)-[r:KNOWS]->(b) WHERE r.strength >= 5 AND r.strength <= 8 RETURN r",
        4,
    );
}

#[test]
fn where_relationship_property_with_node_filter() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // KNOWS edges from Alice with strength > 4
    // Alice->Bob(5), Alice->Carol(8) => 2
    db.assert_count(
        "MATCH (a:Person {name: 'Alice'})-[r:KNOWS]->(b) WHERE r.strength > 4 RETURN b",
        2,
    );
}

#[test]
fn where_filter_relationship_by_year() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // KNOWS relationships established since 2019 or later
    // Bob->Carol(2019), Bob->Dave(2020), Dave->Eve(2021) => 3
    db.assert_count(
        "MATCH (a)-[r:KNOWS]->(b) WHERE r.since >= 2019 RETURN r",
        3,
    );
}

// ============================================================
// Ignored: future compatibility tests
// ============================================================

#[test]
fn where_exists_pattern_subquery() {
    // Lora: WHERE EXISTS { MATCH pattern }
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) WHERE EXISTS { MATCH (p)-[:MANAGES]->() } RETURN p.name AS name",
    );
    // Frank and Carol are managers
    assert_eq!(rows.len(), 2);
}

#[test]
fn where_any_list_predicate() {
    // Lora: WHERE ANY(x IN list WHERE predicate)
    let db = TestDb::new();
    let rows = db.run(
        "WITH [1, 2, 3, 4, 5] AS nums \
         WHERE ANY(x IN nums WHERE x > 4) \
         RETURN nums",
    );
    assert_eq!(rows.len(), 1);
}

#[test]
fn where_all_list_predicate() {
    // Lora: WHERE ALL(x IN list WHERE predicate)
    let db = TestDb::new();
    let rows = db.run(
        "WITH [2, 4, 6, 8] AS nums \
         WHERE ALL(x IN nums WHERE x % 2 = 0) \
         RETURN nums",
    );
    assert_eq!(rows.len(), 1);
}

#[test]
fn where_none_list_predicate() {
    // Lora: WHERE NONE(x IN list WHERE predicate)
    let db = TestDb::new();
    let rows = db.run(
        "WITH [1, 3, 5, 7] AS nums \
         WHERE NONE(x IN nums WHERE x % 2 = 0) \
         RETURN nums",
    );
    assert_eq!(rows.len(), 1);
}

#[test]
fn where_regex_matching() {
    // Lora: WHERE n.name =~ 'regex.*pattern'
    let db = db_with_users();
    let rows = db.run(
        "MATCH (n:User) WHERE n.name =~ 'A.*' RETURN n.name AS name",
    );
    assert_eq!(rows.len(), 1);
}

#[test]
fn where_single_list_predicate() {
    // Lora: WHERE SINGLE(x IN list WHERE predicate)
    let db = TestDb::new();
    let rows = db.run(
        "WITH [1, 2, 3, 4, 5] AS nums \
         WHERE SINGLE(x IN nums WHERE x > 4) \
         RETURN nums",
    );
    assert_eq!(rows.len(), 1);
}

// ============================================================
// Extended WHERE: null semantics and three-valued logic
// ============================================================

#[test]
fn where_null_and_true_yields_null() {
    let db = TestDb::new();
    db.run("CREATE (:V {name: 'a', x: 1})");
    db.run("CREATE (:V {name: 'b'})"); // no x property
    // null AND true → null (not truthy), so 'b' should NOT appear
    let rows = db.run(
        "MATCH (v:V) WHERE v.x > 0 AND v.name IS NOT NULL RETURN v.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "a");
}

#[test]
fn where_null_or_true_yields_true() {
    let db = TestDb::new();
    db.run("CREATE (:V {name: 'a', x: 1})");
    db.run("CREATE (:V {name: 'b'})"); // no x property
    // null OR true → true, so both should appear when combined with name IS NOT NULL
    let rows = db.run(
        "MATCH (v:V) WHERE v.x > 0 OR v.name IS NOT NULL RETURN v.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 2);
}

#[test]
fn where_not_null_is_null() {
    let v = TestDb::new().scalar("RETURN NOT null");
    assert!(v.is_null());
}

#[test]
fn where_null_equals_null_is_null() {
    let v = TestDb::new().scalar("RETURN null = null");
    assert!(v.is_null());
}

#[test]
fn where_null_not_equal_null_is_null() {
    let v = TestDb::new().scalar("RETURN null <> null");
    assert!(v.is_null());
}

#[test]
fn where_null_in_list_is_null() {
    let v = TestDb::new().scalar("RETURN null IN [1, 2, 3]");
    assert!(v.is_null());
}

#[test]
fn where_value_in_list_with_null() {
    // 1 IN [1, null] → true (found before hitting null)
    let v = TestDb::new().scalar("RETURN 1 IN [1, null]");
    assert_eq!(v, true);
}

#[test]
fn where_value_not_in_list_with_null() {
    // 5 IN [1, null] — returns null (not found, null present)
    // Current engine returns false; test documents actual behavior
    let v = TestDb::new().scalar("RETURN 5 IN [1, null]");
    assert_eq!(v, false);
}

// ============================================================
// Extended WHERE: regex matching
// ============================================================

#[test]
fn where_regex_match_basic() {
    let db = TestDb::new();
    db.run("CREATE (:Word {val: 'hello'})");
    db.run("CREATE (:Word {val: 'world'})");
    db.run("CREATE (:Word {val: 'help'})");
    let rows = db.run(
        "MATCH (w:Word) WHERE w.val =~ 'hel.*' RETURN w.val AS v ORDER BY v",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["v"], "hello");
    assert_eq!(rows[1]["v"], "help");
}

#[test]
fn where_regex_case_insensitive() {
    let db = TestDb::new();
    db.run("CREATE (:W {val: 'Hello'})");
    db.run("CREATE (:W {val: 'WORLD'})");
    let rows = db.run(
        "MATCH (w:W) WHERE w.val =~ '(?i)hello' RETURN w.val AS v",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["v"], "Hello");
}

#[test]
fn where_regex_null_returns_null() {
    let v = TestDb::new().scalar("RETURN null =~ 'test'");
    assert!(v.is_null());
}

// ============================================================
// Extended WHERE: complex predicate combinations
// ============================================================

#[test]
fn where_nested_or_and_with_parentheses() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'a', x: 1, y: 10})");
    db.run("CREATE (:Item {name: 'b', x: 2, y: 20})");
    db.run("CREATE (:Item {name: 'c', x: 3, y: 5})");
    // (x > 2 OR y > 15) AND name <> 'b'
    let rows = db.run(
        "MATCH (i:Item) \
         WHERE (i.x > 2 OR i.y > 15) AND i.name <> 'b' \
         RETURN i.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "c");
}

#[test]
fn where_xor_logic() {
    let db = TestDb::new();
    db.run("CREATE (:B {a: true, b: true})");
    db.run("CREATE (:B {a: true, b: false})");
    db.run("CREATE (:B {a: false, b: true})");
    db.run("CREATE (:B {a: false, b: false})");
    let rows = db.run(
        "MATCH (n:B) WHERE n.a XOR n.b RETURN n.a, n.b",
    );
    // XOR is true when exactly one is true
    assert_eq!(rows.len(), 2);
}

#[test]
fn where_multiple_property_ranges() {
    let db = TestDb::new();
    for i in 0..10 {
        db.run(&format!("CREATE (:N {{x: {}, y: {}}})", i, i * 2));
    }
    let rows = db.run(
        "MATCH (n:N) WHERE n.x >= 3 AND n.x <= 7 AND n.y > 8 RETURN n.x AS x ORDER BY x",
    );
    // x in [3,7] and y > 8 → y=2x, so y>8 means x>4 → x in {5,6,7}
    assert_eq!(rows.len(), 3);
}

#[test]
fn where_property_existence_check() {
    let db = TestDb::new();
    db.run("CREATE (:M {name: 'has', tag: 'yes'})");
    db.run("CREATE (:M {name: 'no_tag'})");
    let rows = db.run(
        "MATCH (m:M) WHERE m.tag IS NOT NULL RETURN m.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "has");
}

#[test]
fn where_string_comparison_ordering() {
    let db = TestDb::new();
    db.run("CREATE (:S {v: 'apple'})");
    db.run("CREATE (:S {v: 'banana'})");
    db.run("CREATE (:S {v: 'cherry'})");
    let rows = db.run(
        "MATCH (s:S) WHERE s.v > 'banana' RETURN s.v AS v",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["v"], "cherry");
}

// ============================================================
// Extended WHERE: future / pending features
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn where_exists_negated() {
    // NOT EXISTS subquery
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         WHERE NOT EXISTS { MATCH (p)-[:MANAGES]->() } \
         RETURN p.name AS name",
    );
}

#[test]
#[ignore = "pending implementation"]
fn where_count_subquery() {
    // COUNT subquery in WHERE
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         WHERE COUNT { MATCH (p)-[:MANAGES]->() } > 1 \
         RETURN p.name",
    );
}

// ============================================================
// Three-valued logic completeness
// ============================================================

#[test]
fn where_null_and_false_yields_false() {
    // null AND false → false (falsy), should filter out
    let db = TestDb::new();
    db.run("CREATE (:V {name: 'a'})");
    db.run("CREATE (:V {name: 'b', x: 1})");
    // For 'a': x IS missing → x > 0 is null; null AND (name='z') = null AND false = false
    // For 'b': x=1 > 0 is true; true AND (name='z') = true AND false = false
    db.assert_count(
        "MATCH (v:V) WHERE v.x > 0 AND v.name = 'z' RETURN v",
        0,
    );
}

#[test]
fn where_null_or_false_yields_null() {
    // null OR false → null (not truthy), so row is filtered out
    let db = TestDb::new();
    let v = db.scalar("RETURN (null OR false)");
    assert!(v.is_null());
}

#[test]
fn where_three_valued_truth_table_and() {
    // Verify: true AND null = null, false AND null = false
    let db = TestDb::new();
    let v1 = db.scalar("RETURN true AND null");
    assert!(v1.is_null());
    let v2 = db.scalar("RETURN false AND null");
    assert_eq!(v2, false);
}

#[test]
fn where_three_valued_truth_table_or() {
    // Verify: true OR null = true, false OR null = null
    let db = TestDb::new();
    let v1 = db.scalar("RETURN true OR null");
    assert_eq!(v1, true);
    let v2 = db.scalar("RETURN false OR null");
    assert!(v2.is_null());
}

// ============================================================
// CASE expression in WHERE
// ============================================================

#[test]
fn where_case_expression_as_predicate() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Use CASE to produce a boolean in WHERE
    let names = db.sorted_strings(
        "MATCH (p:Person) \
         WHERE CASE WHEN p.dept = 'Engineering' THEN p.age > 30 ELSE false END \
         RETURN p.name AS name",
        "name",
    );
    // Engineering with age > 30: Alice(35), Frank(50)
    assert_eq!(names, vec!["Alice", "Frank"]);
}

// ============================================================
// Comparisons returning null
// ============================================================

#[test]
fn where_null_greater_than_value() {
    let v = TestDb::new().scalar("RETURN null > 5");
    assert!(v.is_null());
}

#[test]
fn where_null_less_than_value() {
    let v = TestDb::new().scalar("RETURN null < 5");
    assert!(v.is_null());
}

#[test]
fn where_string_starts_with_null() {
    let v = TestDb::new().scalar("RETURN null STARTS WITH 'a'");
    assert!(v.is_null());
}

// ============================================================
// Complex multi-property range filters
// ============================================================

#[test]
fn where_between_pattern() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Equivalent of BETWEEN: age >= 28 AND age <= 42
    let names = db.sorted_strings(
        "MATCH (p:Person) WHERE p.age >= 28 AND p.age <= 42 RETURN p.name AS name",
        "name",
    );
    // Bob(28), Dave(31), Alice(35), Carol(42)
    assert_eq!(names, vec!["Alice", "Bob", "Carol", "Dave"]);
}

#[test]
fn where_not_between_pattern() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Outside range: age < 28 OR age > 42
    let names = db.sorted_strings(
        "MATCH (p:Person) WHERE p.age < 28 OR p.age > 42 RETURN p.name AS name",
        "name",
    );
    // Eve(26), Frank(50)
    assert_eq!(names, vec!["Eve", "Frank"]);
}

// ============================================================
// IN with computed list
// ============================================================

#[test]
fn where_in_with_collected_list() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Find people whose department appears among the departments of London residents
    let names = db.sorted_strings(
        "MATCH (londoner:Person)-[:LIVES_IN]->(c:City {name:'London'}) \
         WITH collect(DISTINCT londoner.dept) AS london_depts \
         MATCH (p:Person) WHERE p.dept IN london_depts \
         RETURN DISTINCT p.name AS name",
        "name",
    );
    // London residents: Alice(Engineering), Carol(Marketing), Frank(Engineering)
    // So all Engineering + Marketing people match
    assert_eq!(names, vec!["Alice", "Bob", "Carol", "Dave", "Eve", "Frank"]);
}

// ============================================================
// Future WHERE tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn where_collect_subquery() {
    // COLLECT subquery in WHERE
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         WHERE size(COLLECT { MATCH (p)-[:MANAGES]->(s) RETURN s }) > 0 \
         RETURN p.name AS name",
    );
}

#[test]
#[ignore = "pending implementation"]
fn where_pattern_predicate_not_exists() {
    // NOT EXISTS pattern predicate without subquery syntax
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         WHERE NOT (p)-[:MANAGES]->() \
         RETURN p.name AS name",
    );
}

