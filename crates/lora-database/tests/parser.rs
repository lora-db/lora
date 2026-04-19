/// Parser-level tests that verify parse_query accepts or rejects specific syntax.
/// These tests don't execute queries - they only check parsing.
use lora_database::parse_query;

// ============================================================
// Valid syntax: basic clauses
// ============================================================

#[test]
fn parse_simple_match_return() {
    assert!(parse_query("MATCH (n) RETURN n").is_ok());
}

#[test]
fn parse_match_with_label() {
    assert!(parse_query("MATCH (n:User) RETURN n").is_ok());
}

#[test]
fn parse_match_with_multiple_labels() {
    assert!(parse_query("MATCH (n:User:Admin) RETURN n").is_ok());
}

#[test]
fn parse_match_with_properties() {
    assert!(parse_query("MATCH (n:User {name: 'Alice', age: 30}) RETURN n").is_ok());
}

#[test]
fn parse_create_node() {
    assert!(parse_query("CREATE (n:User {name: 'Alice'})").is_ok());
}

#[test]
fn parse_create_return() {
    assert!(parse_query("CREATE (n:User {name: 'Alice'}) RETURN n").is_ok());
}

#[test]
fn parse_match_where() {
    assert!(parse_query("MATCH (n:User) WHERE n.age > 18 RETURN n").is_ok());
}

// ============================================================
// Valid syntax: relationships
// ============================================================

#[test]
fn parse_directed_right() {
    assert!(parse_query("MATCH (a)-[:FOLLOWS]->(b) RETURN a, b").is_ok());
}

#[test]
fn parse_directed_left() {
    assert!(parse_query("MATCH (a)<-[:FOLLOWS]-(b) RETURN a, b").is_ok());
}

#[test]
fn parse_undirected() {
    assert!(parse_query("MATCH (a)-[:KNOWS]-(b) RETURN a, b").is_ok());
}

#[test]
fn parse_relationship_with_variable() {
    assert!(parse_query("MATCH (a)-[r:FOLLOWS]->(b) RETURN r").is_ok());
}

#[test]
fn parse_relationship_with_properties() {
    assert!(parse_query("MATCH (a)-[r:FOLLOWS {since: 2020}]->(b) RETURN r").is_ok());
}

#[test]
fn parse_relationship_variable_length() {
    assert!(parse_query("MATCH (a)-[:FOLLOWS*1..3]->(b) RETURN a, b").is_ok());
}

#[test]
fn parse_relationship_variable_length_unbounded() {
    assert!(parse_query("MATCH (a)-[:FOLLOWS*]->(b) RETURN a, b").is_ok());
}

#[test]
fn parse_relationship_variable_length_lower_only() {
    assert!(parse_query("MATCH (a)-[:FOLLOWS*2..]->(b) RETURN a, b").is_ok());
}

#[test]
fn parse_relationship_variable_length_upper_only() {
    assert!(parse_query("MATCH (a)-[:FOLLOWS*..5]->(b) RETURN a, b").is_ok());
}

// ============================================================
// Valid syntax: RETURN variants
// ============================================================

#[test]
fn parse_return_star() {
    assert!(parse_query("MATCH (n) RETURN *").is_ok());
}

#[test]
fn parse_return_distinct() {
    assert!(parse_query("MATCH (n) RETURN DISTINCT n").is_ok());
}

#[test]
fn parse_return_alias() {
    assert!(parse_query("MATCH (n) RETURN n.name AS name").is_ok());
}

#[test]
fn parse_return_order_by() {
    assert!(parse_query("MATCH (n) RETURN n ORDER BY n.name").is_ok());
}

#[test]
fn parse_return_order_by_desc() {
    assert!(parse_query("MATCH (n) RETURN n ORDER BY n.name DESC").is_ok());
}

#[test]
fn parse_return_skip_limit() {
    assert!(parse_query("MATCH (n) RETURN n SKIP 5 LIMIT 10").is_ok());
}

// ============================================================
// Valid syntax: write clauses
// ============================================================

#[test]
fn parse_set_property() {
    assert!(parse_query("MATCH (n) SET n.name = 'Alice' RETURN n").is_ok());
}

#[test]
fn parse_set_variable() {
    assert!(parse_query("MATCH (n) SET n = {name: 'Alice'} RETURN n").is_ok());
}

#[test]
fn parse_set_mutate() {
    assert!(parse_query("MATCH (n) SET n += {age: 30} RETURN n").is_ok());
}

#[test]
fn parse_set_labels() {
    assert!(parse_query("MATCH (n) SET n:Admin RETURN n").is_ok());
}

#[test]
fn parse_remove_property() {
    assert!(parse_query("MATCH (n) REMOVE n.age RETURN n").is_ok());
}

#[test]
fn parse_remove_label() {
    assert!(parse_query("MATCH (n) REMOVE n:Admin RETURN n").is_ok());
}

#[test]
fn parse_delete() {
    assert!(parse_query("MATCH (n) DELETE n").is_ok());
}

#[test]
fn parse_detach_delete() {
    assert!(parse_query("MATCH (n) DETACH DELETE n").is_ok());
}

#[test]
fn parse_merge_basic() {
    assert!(parse_query("MERGE (n:User {name: 'Alice'})").is_ok());
}

#[test]
fn parse_merge_on_match_on_create() {
    assert!(parse_query(
        "MERGE (n:User {name: 'Alice'}) ON MATCH SET n.age = 30 ON CREATE SET n:New"
    )
    .is_ok());
}

// ============================================================
// Valid syntax: expressions and literals
// ============================================================

#[test]
fn parse_unwind() {
    assert!(parse_query("UNWIND [1, 2, 3] AS n RETURN n").is_ok());
}

#[test]
fn parse_with_clause() {
    assert!(parse_query("MATCH (n) WITH n RETURN n").is_ok());
}

#[test]
fn parse_with_where() {
    assert!(parse_query("MATCH (n) WITH n WHERE n.age > 18 RETURN n").is_ok());
}

#[test]
fn parse_case_when() {
    assert!(parse_query("RETURN CASE WHEN 1 = 1 THEN 'yes' ELSE 'no' END").is_ok());
}

#[test]
fn parse_simple_case() {
    assert!(
        parse_query("MATCH (n) RETURN CASE n.age WHEN 25 THEN 'young' ELSE 'other' END").is_ok()
    );
}

#[test]
fn parse_function_call() {
    assert!(parse_query("MATCH (n) RETURN count(n) AS c").is_ok());
}

#[test]
fn parse_function_distinct() {
    assert!(parse_query("MATCH (n) RETURN count(DISTINCT n) AS c").is_ok());
}

#[test]
fn parse_count_star() {
    assert!(parse_query("RETURN count(*)").is_ok());
    assert!(parse_query("MATCH (n) RETURN count(*) AS c").is_ok());
}

#[test]
fn parse_star_in_non_count_function_is_error() {
    assert!(parse_query("RETURN sum(*) AS s").is_err());
    assert!(parse_query("RETURN max(*) AS m").is_err());
}

#[test]
fn parse_parameter() {
    assert!(parse_query("MATCH (n) WHERE n.age > $minAge RETURN n").is_ok());
}

#[test]
fn parse_numeric_parameter() {
    assert!(parse_query("RETURN $1").is_ok());
}

#[test]
fn parse_map_literal() {
    assert!(parse_query("RETURN {name: 'Alice', age: 30}").is_ok());
}

#[test]
fn parse_list_literal() {
    assert!(parse_query("RETURN [1, 2, 3]").is_ok());
}

#[test]
fn parse_arithmetic() {
    assert!(parse_query("RETURN 1 + 2 * 3").is_ok());
}

#[test]
fn parse_string_operators() {
    assert!(parse_query("MATCH (n) WHERE n.name STARTS WITH 'A' RETURN n").is_ok());
    assert!(parse_query("MATCH (n) WHERE n.name ENDS WITH 'z' RETURN n").is_ok());
    assert!(parse_query("MATCH (n) WHERE n.name CONTAINS 'bc' RETURN n").is_ok());
}

#[test]
fn parse_null_check() {
    assert!(parse_query("MATCH (n) WHERE n.name IS NULL RETURN n").is_ok());
    assert!(parse_query("MATCH (n) WHERE n.name IS NOT NULL RETURN n").is_ok());
}

#[test]
fn parse_in_operator() {
    assert!(parse_query("MATCH (n) WHERE n.age IN [1, 2, 3] RETURN n").is_ok());
}

#[test]
fn parse_optional_match() {
    assert!(parse_query("OPTIONAL MATCH (n) RETURN n").is_ok());
}

#[test]
fn parse_semicolon_allowed() {
    assert!(parse_query("MATCH (n) RETURN n;").is_ok());
}

#[test]
fn parse_pattern_binding() {
    assert!(parse_query("MATCH p = (a)-[r]->(b) RETURN p").is_ok());
}

// ============================================================
// Valid syntax: UNION
// ============================================================

#[test]
fn parse_union() {
    assert!(parse_query("MATCH (a) RETURN a UNION MATCH (b) RETURN b").is_ok());
}

#[test]
fn parse_union_all() {
    assert!(parse_query("MATCH (a) RETURN a UNION ALL MATCH (b) RETURN b").is_ok());
}

// ============================================================
// Valid syntax: CALL
// ============================================================

#[test]
fn parse_standalone_call() {
    assert!(parse_query("CALL db.labels()").is_ok());
}

#[test]
fn parse_standalone_call_implicit() {
    assert!(parse_query("CALL db.labels").is_ok());
}

#[test]
fn parse_standalone_call_yield() {
    assert!(parse_query("CALL db.labels() YIELD label").is_ok());
}

#[test]
fn parse_standalone_call_yield_star() {
    assert!(parse_query("CALL db.labels() YIELD *").is_ok());
}

// ============================================================
// Invalid syntax
// ============================================================

#[test]
fn parse_error_empty_input() {
    assert!(parse_query("").is_err());
}

#[test]
fn parse_error_gibberish() {
    assert!(parse_query("THIS IS NOT CYPHER").is_err());
}

#[test]
fn parse_error_incomplete_match() {
    assert!(parse_query("MATCH").is_err());
}

#[test]
fn parse_error_unclosed_paren() {
    assert!(parse_query("MATCH (n RETURN n").is_err());
}

#[test]
fn parse_error_missing_closing_bracket() {
    assert!(parse_query("MATCH (a)-[r:FOLLOWS->(b) RETURN r").is_err());
}

#[test]
fn parse_error_only_return_keyword() {
    assert!(parse_query("RETURN").is_err());
}

// ============================================================
// String escaping
// ============================================================

#[test]
fn parse_single_quoted_string() {
    assert!(parse_query("RETURN 'hello'").is_ok());
}

#[test]
fn parse_double_quoted_string() {
    assert!(parse_query("RETURN \"hello\"").is_ok());
}

#[test]
fn parse_escaped_quote_in_string() {
    assert!(parse_query("RETURN 'it\\'s'").is_ok());
}

// ============================================================
// Number literals
// ============================================================

#[test]
fn parse_integer_literal() {
    assert!(parse_query("RETURN 42").is_ok());
}

#[test]
fn parse_zero_literal() {
    assert!(parse_query("RETURN 0").is_ok());
}

#[test]
fn parse_float_literal() {
    assert!(parse_query("RETURN 3.14").is_ok());
}

#[test]
fn parse_scientific_notation() {
    assert!(parse_query("RETURN 1.5e10").is_ok());
}

#[test]
fn parse_hex_integer() {
    assert!(parse_query("RETURN 0xFF").is_ok());
}
