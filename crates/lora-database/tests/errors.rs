/// Error behavior tests — parse errors, semantic validation, executor
/// constraint violations, schema-awareness rules.
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// Parse errors
// ============================================================

#[test]
fn error_invalid_syntax() {
    assert!(!TestDb::new().run_err("THIS IS NOT CYPHER").is_empty());
}

#[test]
fn error_incomplete_match() {
    assert!(!TestDb::new().run_err("MATCH").is_empty());
}

#[test]
fn error_unclosed_parenthesis() {
    assert!(!TestDb::new().run_err("MATCH (n RETURN n").is_empty());
}

#[test]
fn error_missing_return_or_update() {
    assert!(!TestDb::new().run_err("MATCH (n:User)").is_empty());
}

#[test]
fn error_empty_query() {
    assert!(!TestDb::new().run_err("").is_empty());
}

#[test]
fn error_parse_unmatched_bracket() {
    assert!(TestDb::new().exec("MATCH (a)-[r:X->(b) RETURN a").is_err());
}

#[test]
fn error_parse_only_keyword() {
    assert!(TestDb::new().exec("MATCH").is_err());
}

#[test]
fn error_parse_nonsense() {
    assert!(TestDb::new().exec("SELECT * FROM users").is_err());
}

// ============================================================
// Semantic: unknown variables
// ============================================================

#[test]
fn error_unknown_variable_in_return() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let err = db.run_err("MATCH (n:User) RETURN x");
    assert!(err.contains("Unknown variable") || err.contains("variable"));
}

#[test]
fn error_unknown_variable_in_where() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let err = db.run_err("MATCH (n:User) WHERE x.name = 'Alice' RETURN n");
    assert!(err.contains("Unknown variable") || err.contains("variable"));
}

#[test]
fn error_set_on_unbound_variable() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    let err = db.run_err("MATCH (a:X) SET b.val = 1");
    assert!(err.contains("Unknown variable") || err.contains("variable"));
}

#[test]
fn error_return_unbound_variable() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    let err = db.run_err("MATCH (a:X) RETURN b");
    assert!(err.contains("Unknown variable") || err.contains("variable"));
}

#[test]
fn error_where_unbound_variable() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    let err = db.run_err("MATCH (a:X) WHERE b.id = 1 RETURN a");
    assert!(err.contains("Unknown variable") || err.contains("variable"));
}

// ============================================================
// Semantic: unknown schema elements on non-empty graph
// ============================================================

#[test]
fn error_unknown_label_in_match() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let err = db.run_err("MATCH (n:NonexistentLabel) RETURN n");
    assert!(err.contains("Unknown label"));
}

#[test]
fn error_unknown_relationship_type_in_match() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})-[:FOLLOWS]->(b:User {name: 'Bob'})");
    let err = db.run_err("MATCH (a)-[:NONEXISTENT]->(b) RETURN a, b");
    assert!(err.contains("Unknown relationship type"));
}

#[test]
fn set_allows_new_property_creation() {
    // Lora semantics: SET can create new properties on existing nodes.
    let db = TestDb::new();
    db.run("CREATE (:Whatever {x: 0})");
    db.run("MATCH (n:Whatever) SET n.nonexistent_property_xyz = 1");
    let rows = db.run("MATCH (n:Whatever) RETURN n");
    assert_eq!(rows[0]["n"]["properties"]["nonexistent_property_xyz"], 1);
}

// ============================================================
// Schema: allowed on empty graph and in CREATE
// ============================================================

#[test]
fn unknown_label_allowed_on_empty_graph() {
    let db = TestDb::new();
    db.assert_count("MATCH (n:Whatever) RETURN n", 0);
}

#[test]
fn unknown_label_allowed_in_create() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let result = db.exec("CREATE (n:BrandNewLabel {name: 'test'})");
    assert!(result.is_ok());
}

#[test]
fn create_allows_new_labels_on_nonempty_graph() {
    let db = TestDb::new();
    db.run("CREATE (:Existing {id:1})");
    assert!(db.exec("CREATE (:BrandNew {id:2}) RETURN *").is_ok());
}

#[test]
fn create_allows_new_relationship_type() {
    let db = TestDb::new();
    db.run("CREATE (:A {id:1})-[:EXISTING]->(:B {id:2})");
    assert!(db
        .exec("MATCH (a:A), (b:B) CREATE (a)-[:BRAND_NEW]->(b)")
        .is_ok());
}

// ============================================================
// Feature support verification
// ============================================================

#[test]
fn union_is_supported() {
    let db = TestDb::new();
    assert!(db
        .exec("MATCH (a) RETURN a UNION MATCH (b) RETURN b")
        .is_ok());
}

#[test]
fn error_standalone_call_unsupported() {
    let db = TestDb::new();
    let err = db.run_err("CALL db.labels()");
    assert!(
        err.contains("Unsupported") || err.contains("not yet supported") || err.contains("CALL")
    );
}

// ============================================================
// Delete constraints
// ============================================================

#[test]
fn error_delete_node_with_relationships() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})-[:FOLLOWS]->(b:User {name: 'Bob'})");
    let err = db.run_err("MATCH (n:User {name: 'Alice'}) DELETE n");
    assert!(err.contains("relationships") || err.contains("DETACH"));
}

#[test]
fn error_delete_connected_node_without_detach() {
    let db = TestDb::new();
    db.run("CREATE (:A {id:1})-[:R]->(:B {id:2})");
    let err = db.run_err("MATCH (a:A) DELETE a");
    assert!(err.contains("relationship") || err.contains("DETACH"));
}

// ============================================================
// Duplicate aliases and keys
// ============================================================

#[test]
fn error_duplicate_projection_alias() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice', age: 30})");
    let err = db.run_err("MATCH (n:User) RETURN n.name AS x, n.age AS x");
    assert!(err.contains("Duplicate") || err.contains("alias"));
}

#[test]
fn error_duplicate_map_key() {
    let db = TestDb::new();
    let err = db.run_err("RETURN {name: 'Alice', name: 'Bob'}");
    assert!(err.contains("Duplicate") || err.contains("map key"));
}

// ============================================================
// Invalid range
// ============================================================

#[test]
fn error_invalid_relationship_range() {
    let db = TestDb::new();
    db.run("CREATE (:N {id:1})-[:R]->(:N {id:2})");
    let err = db.run_err("MATCH (a:N)-[:R*5..2]->(b:N) RETURN b");
    assert!(err.contains("range") || err.contains("Invalid") || err.contains("invalid"));
}

// ============================================================
// CREATE relationship without type
// ============================================================

#[test]
fn error_create_relationship_without_type() {
    let db = TestDb::new();
    assert!(db.exec("CREATE (a)-[r]->(b) RETURN r").is_err());
}

// ============================================================
// Invalid function calls
// ============================================================

#[test]
fn error_unknown_function_name() {
    let db = TestDb::new();
    let err = db.run_err("RETURN nonExistentFunction(42)");
    assert!(!err.is_empty());
}

// ============================================================
// Parse errors: malformed patterns
// ============================================================

#[test]
fn error_relationship_without_nodes() {
    assert!(TestDb::new().exec("MATCH -[:R]-> RETURN 1").is_err());
}

#[test]
fn error_double_arrow() {
    assert!(TestDb::new().exec("MATCH (a)-[:R]->>(b) RETURN a").is_err());
}

#[test]
fn error_empty_label() {
    assert!(TestDb::new().exec("MATCH (n:) RETURN n").is_err());
}

// ============================================================
// Semantic: invalid mutation targets
// ============================================================

#[test]
fn error_delete_without_match() {
    // DELETE without MATCH — nothing bound
    let err = TestDb::new().run_err("DELETE n");
    assert!(!err.is_empty());
}

#[test]
fn error_set_without_match() {
    let err = TestDb::new().run_err("SET n.x = 1");
    assert!(!err.is_empty());
}

// ============================================================
// Semantic: variable leakage across UNION
// ============================================================

#[test]
fn error_variable_from_first_union_branch_not_in_second() {
    let db = TestDb::new();
    db.run("CREATE (:A {val:1})");
    // This should just work as a normal union — each branch has its own scope
    let rows = db.run("MATCH (a:A) RETURN a.val AS v UNION ALL MATCH (a:A) RETURN a.val AS v");
    assert_eq!(rows.len(), 2);
}

// ============================================================
// Schema errors with specific messages
// ============================================================

#[test]
fn error_unknown_property_in_where() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    let err = db.run_err("MATCH (n:User) WHERE n.nonexistent = 'x' RETURN n");
    assert!(err.contains("Unknown property"));
}

#[test]
fn error_unknown_property_in_order_by() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    let err = db.run_err("MATCH (n:User) RETURN n.name ORDER BY n.nonexistent");
    assert!(err.contains("Unknown property"));
}

// ============================================================
// Invalid range values
// ============================================================

#[test]
fn error_range_min_greater_than_max() {
    let db = TestDb::new();
    db.run("CREATE (:N {id:1})-[:R]->(:N {id:2})");
    let err = db.run_err("MATCH (a:N)-[:R*10..2]->(b) RETURN b");
    assert!(err.contains("range") || err.contains("Invalid") || err.contains("invalid"));
}

// ============================================================
// CREATE: edge cases
// ============================================================

#[test]
fn error_create_without_parentheses() {
    assert!(TestDb::new().exec("CREATE n:User RETURN n").is_err());
}

#[test]
fn error_star_in_non_count_function() {
    let err = TestDb::new().run_err("RETURN sum(*) AS s");
    assert!(err.contains("count") || err.contains("*"));
}

// ============================================================
// Duplicate variable in same scope
// ============================================================

#[test]
fn error_duplicate_variable_binding() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})-[:R]->(:Y {id:2})");
    let err = db.run_err("MATCH (n:X)-[r:R]->(n:Y) RETURN n");
    assert!(err.contains("Duplicate") || err.contains("variable") || err.contains("already"));
}

// ============================================================
// Read-only violations (attempting mutation without write context)
// ============================================================

#[test]
fn error_create_rel_missing_type_in_pattern() {
    // Relationship without type should fail
    assert!(TestDb::new().exec("CREATE (a)-[r]->(b)").is_err());
}

// ============================================================
// Pending: UNION column mismatch
// ============================================================

#[test]
fn error_union_mismatched_column_count() {
    let db = TestDb::new();
    db.run("CREATE (:A {x:1})");
    db.run("CREATE (:B {x:1, y:2})");
    let err = db.run_err(
        "MATCH (a:A) RETURN a.x AS x \
         UNION \
         MATCH (b:B) RETURN b.x AS x, b.y AS y",
    );
    assert!(!err.is_empty());
}

// ============================================================
// Pending: aggregation in WHERE
// ============================================================

#[test]
fn error_aggregation_in_where() {
    let db = TestDb::new();
    db.run("CREATE (:User {age: 25})");
    db.run("CREATE (:User {age: 30})");
    let err = db.run_err("MATCH (n:User) WHERE count(n) > 1 RETURN n");
    assert!(!err.is_empty());
}

// ============================================================
// More parse error cases
// ============================================================

#[test]
fn error_unterminated_string_literal() {
    assert!(TestDb::new().exec("RETURN 'hello").is_err());
}

#[test]
fn error_invalid_operator() {
    assert!(TestDb::new()
        .exec("MATCH (n:X) WHERE n.val === 1 RETURN n")
        .is_err());
}

#[test]
fn error_missing_return_in_read_query() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    let err = db.run_err("MATCH (n:X) WHERE n.id = 1");
    assert!(!err.is_empty());
}

#[test]
fn error_dangling_comma_in_return() {
    assert!(TestDb::new().exec("MATCH (n) RETURN n.name,").is_err());
}

// ============================================================
// More semantic error cases
// ============================================================

#[test]
fn set_allows_new_property_on_existing_node() {
    // Lora semantics: SET can add new properties not previously on the node.
    let db = TestDb::new();
    db.run("CREATE (:Widget {name: 'sprocket'})");
    db.run("MATCH (w:Widget) SET w.nonexistent_field_xyz = 42");
    let rows = db.run("MATCH (w:Widget) RETURN w");
    assert_eq!(rows[0]["w"]["properties"]["nonexistent_field_xyz"], 42);
}

#[test]
fn error_delete_property_value_not_node() {
    let db = TestDb::new();
    db.run("CREATE (:Item {val: 99})");
    // Bind x to a property value, then try to delete it — should fail
    let err = db.run_err("MATCH (n:Item) WITH n.val AS x DELETE x");
    assert!(!err.is_empty());
}

#[test]
fn error_unknown_label_and_unknown_property_combined() {
    let db = TestDb::new();
    db.run("CREATE (:Known {id: 1})");
    // Unknown label in MATCH on non-empty graph
    let err = db.run_err("MATCH (n:TotallyFakeLabel) RETURN n.also_fake");
    assert!(err.contains("Unknown label") || err.contains("label"));
}

#[test]
fn error_order_by_unbound_variable() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    let err = db.run_err("MATCH (n:X) RETURN n.id ORDER BY z.id");
    assert!(err.contains("Unknown variable") || err.contains("variable"));
}

// ============================================================
// Ignored error tests (pending implementation)
// ============================================================

#[test]
fn error_aggregation_in_where_without_with() {
    // Lora: error for aggregation in WHERE without WITH
    let db = TestDb::new();
    db.run("CREATE (:User {age: 25})");
    db.run("CREATE (:User {age: 30})");
    db.run("CREATE (:User {age: 35})");
    let err = db.run_err("MATCH (n:User) WHERE avg(n.age) > 28 RETURN n");
    assert!(!err.is_empty());
}

#[test]
fn error_union_column_count_mismatch_analysis() {
    // Lora: error for UNION column count mismatch
    let db = TestDb::new();
    db.run("CREATE (:A {x:1})");
    db.run("CREATE (:B {x:1, y:2, z:3})");
    let err = db.run_err(
        "MATCH (a:A) RETURN a.x AS x \
         UNION ALL \
         MATCH (b:B) RETURN b.x AS x, b.y AS y, b.z AS z",
    );
    assert!(!err.is_empty());
}

#[test]
fn error_unknown_function_name_in_return() {
    // Lora: error for unknown function name
    let db = TestDb::new();
    let err = db.run_err("RETURN fooBarBaz(1, 2, 3)");
    assert!(!err.is_empty());
}

#[test]
fn error_wrong_function_arity() {
    // Lora: error for wrong function arity
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    let err = db.run_err("MATCH (n:X) RETURN count(n, n) AS c");
    assert!(!err.is_empty());
}

#[test]
#[ignore = "type validation: type mismatch in comparison not yet detected"]
fn error_type_mismatch_in_comparison() {
    // Lora: error for type mismatch in comparison
    let db = TestDb::new();
    db.run("CREATE (:X {val: 42})");
    let err = db.run_err("MATCH (n:X) WHERE n.val > 'hello' RETURN n");
    assert!(!err.is_empty());
}

// ============================================================
// Extended errors: semantic and runtime errors
// ============================================================

#[test]
fn error_delete_connected_node_without_detach_extended() {
    let db = TestDb::new();
    db.run("CREATE (a:A {name:'a'})-[:R]->(b:B {name:'b'})");
    let err = db.run_err("MATCH (a:A {name:'a'}) DELETE a");
    assert!(!err.is_empty()); // Cannot delete node with relationships
}

#[test]
fn error_return_unbound_variable_extended() {
    let db = TestDb::new();
    let err = db.run_err("MATCH (n) RETURN x");
    assert!(!err.is_empty());
}

#[test]
fn error_unknown_function() {
    let db = TestDb::new();
    let err = db.run_err("RETURN nonExistentFunction(42)");
    assert!(!err.is_empty());
}

#[test]
fn error_invalid_date_string() {
    let db = TestDb::new();
    let err = db.run_err("RETURN date('not-a-date')");
    assert!(!err.is_empty());
}

#[test]
fn error_invalid_point_missing_coords() {
    let db = TestDb::new();
    let err = db.run_err("RETURN point({z: 1.0})");
    assert!(!err.is_empty());
}

#[test]
fn error_division_by_zero_integer() {
    // Division by zero should not crash
    let db = TestDb::new();
    let v = db.scalar("RETURN 1 / 0");
    assert!(v.is_null());
}

#[test]
fn error_modulo_by_zero() {
    let db = TestDb::new();
    let v = db.scalar("RETURN 10 % 0");
    assert!(v.is_null());
}

#[test]
fn error_union_column_count_mismatch() {
    let db = TestDb::new();
    let err = db.run_err("RETURN 1 AS a, 2 AS b UNION RETURN 1 AS a");
    assert!(!err.is_empty());
}

#[test]
fn error_duplicate_variable_in_create() {
    // In a single CREATE pattern, using conflicting patterns should error
    let db = TestDb::new();
    // This should be valid Lora (two separate CREATE clauses, not conflicting)
    db.run("CREATE (:Dup {name: 'a'})");
    db.run("CREATE (:Dup {name: 'b'})");
    db.assert_count("MATCH (d:Dup) RETURN d", 2);
}

// ============================================================
// Extended errors: future / pending features
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn error_constraint_violation() {
    let db = TestDb::new();
    // After creating a uniqueness constraint, duplicate should error
    db.run("CREATE CONSTRAINT FOR (n:User) REQUIRE n.email IS UNIQUE");
    db.run("CREATE (:User {email: 'a@b.com'})");
    let _err = db.run_err("CREATE (:User {email: 'a@b.com'})");
}

#[test]
#[ignore = "pending implementation"]
fn error_read_after_write_in_same_clause() {
    // Lora spec: cannot read and write in the same clause
    let db = TestDb::new();
    let _err = db.run_err("MATCH (n) CREATE (n)-[:R]->(:New) RETURN n");
}

// ============================================================
// Parse error: various malformed queries
// ============================================================

#[test]
fn error_multiple_return_clauses() {
    assert!(TestDb::new().exec("MATCH (n) RETURN n RETURN n").is_err());
}

#[test]
fn error_where_without_match() {
    assert!(TestDb::new().exec("WHERE true RETURN 1").is_err());
}

#[test]
fn error_order_by_without_return() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    assert!(db.exec("MATCH (n:X) ORDER BY n.id").is_err());
}

// ============================================================
// Schema error edge cases
// ============================================================

#[test]
fn error_unknown_label_multiple_types_first_unknown() {
    let db = TestDb::new();
    db.run("CREATE (:Known {id:1})");
    let err = db.run_err("MATCH (n:Unknown) RETURN n");
    assert!(err.contains("Unknown label"));
}

#[test]
fn error_unknown_rel_type_on_populated_graph() {
    let db = TestDb::new();
    db.run("CREATE (:A {id:1})-[:REAL]->(:B {id:2})");
    let err = db.run_err("MATCH ()-[:FAKE]->() RETURN 1");
    assert!(err.contains("Unknown relationship type"));
}

// ============================================================
// Semantic errors: various invalid operations
// ============================================================

#[test]
fn error_delete_scalar_not_node() {
    let db = TestDb::new();
    db.run("CREATE (:X {val: 1})");
    let err = db.run_err("MATCH (n:X) WITH n.val AS v DELETE v");
    assert!(!err.is_empty());
}

#[test]
fn error_set_on_scalar() {
    let db = TestDb::new();
    db.run("CREATE (:X {val: 1})");
    let err = db.run_err("MATCH (n:X) WITH n.val AS v SET v.x = 1");
    assert!(!err.is_empty());
}

// ============================================================
// Runtime safety: division and modulo by zero
// ============================================================

#[test]
fn error_float_division_by_zero() {
    let v = TestDb::new().scalar("RETURN 1.0 / 0.0");
    // Should return null or infinity, not crash
    assert!(v.is_null() || v.is_f64());
}

#[test]
fn error_nested_division_by_zero() {
    let v = TestDb::new().scalar("RETURN 10 + (5 / 0)");
    assert!(v.is_null());
}

// ============================================================
// Error: function arity mismatches
// ============================================================

#[test]
fn error_size_no_argument() {
    let err = TestDb::new().run_err("RETURN size()");
    assert!(!err.is_empty());
}

#[test]
fn error_count_two_arguments() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    let err = db.run_err("MATCH (n:X) RETURN count(n, n)");
    assert!(!err.is_empty());
}

// ============================================================
// Future error tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn error_write_conflict_in_same_clause() {
    // Reading and writing to the same node in same clause
    let db = TestDb::new();
    db.run("CREATE (:X {val: 1})");
    let _err = db.run_err("MATCH (n:X) SET n.val = n.val + 1 DELETE n");
}

#[test]
#[ignore = "pending implementation"]
fn error_property_type_mismatch_at_comparison() {
    let db = TestDb::new();
    db.run("CREATE (:X {val: 42})");
    let _err = db.run_err("MATCH (n:X) WHERE n.val > 'hello' RETURN n");
}
