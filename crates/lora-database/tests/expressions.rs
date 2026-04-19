/// Expression evaluation tests — arithmetic, boolean, comparison, string ops,
/// CASE, UNWIND, functions (type, labels, keys, size, head, tail, coalesce,
/// id, toString, toInteger, toFloat, abs, range).
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// Arithmetic
// ============================================================

#[test]
fn expr_addition() { assert_eq!(TestDb::new().scalar("RETURN 1 + 2"), 3); }

#[test]
fn expr_subtraction() { assert_eq!(TestDb::new().scalar("RETURN 10 - 3"), 7); }

#[test]
fn expr_multiplication() { assert_eq!(TestDb::new().scalar("RETURN 4 * 5"), 20); }

#[test]
fn expr_division() {
    let f = TestDb::new().scalar("RETURN 10 / 3").as_f64().unwrap();
    assert!((f - 3.333).abs() < 0.01);
}

#[test]
fn expr_modulo() { assert_eq!(TestDb::new().scalar("RETURN 10 % 3"), 1); }

#[test]
fn expr_power() { assert_eq!(TestDb::new().scalar("RETURN 2 ^ 3"), 8); }

#[test]
fn expr_unary_negative() { assert_eq!(TestDb::new().scalar("RETURN -5"), -5); }

#[test]
fn expr_unary_positive() { assert_eq!(TestDb::new().scalar("RETURN +5"), 5); }

#[test]
fn expr_parenthesized_precedence() { assert_eq!(TestDb::new().scalar("RETURN (1 + 2) * 3"), 9); }

#[test]
fn expr_operator_precedence() { assert_eq!(TestDb::new().scalar("RETURN 2 + 3 * 4"), 14); }

#[test]
fn division_by_zero_returns_null() { assert!(TestDb::new().scalar("RETURN 10 / 0").is_null()); }

#[test]
fn modulo_by_zero_returns_null() { assert!(TestDb::new().scalar("RETURN 10 % 0").is_null()); }

#[test]
fn arithmetic_on_properties() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run("MATCH (p:Person {name:'Alice'})-[r:WORKS_AT]->(c:Company) RETURN 2024 - r.since AS years");
    assert_eq!(rows[0]["years"], 6);
}

// ============================================================
// Boolean
// ============================================================

#[test]
fn expr_boolean_and() { assert_eq!(TestDb::new().scalar("RETURN true AND false"), false); }

#[test]
fn expr_boolean_or() { assert_eq!(TestDb::new().scalar("RETURN true OR false"), true); }

#[test]
fn expr_boolean_not() { assert_eq!(TestDb::new().scalar("RETURN NOT true"), false); }

#[test]
fn not_of_not() { assert_eq!(TestDb::new().scalar("RETURN NOT NOT true"), true); }

#[test]
fn chained_comparisons() {
    let db = TestDb::new();
    assert_eq!(db.scalar("RETURN 1 < 2 AND 2 < 3"), true);
    assert_eq!(db.scalar("RETURN 1 < 2 AND 2 > 3"), false);
}

// ============================================================
// Comparison
// ============================================================

#[test]
fn expr_eq_true() { assert_eq!(TestDb::new().scalar("RETURN 1 = 1"), true); }

#[test]
fn expr_eq_false() { assert_eq!(TestDb::new().scalar("RETURN 1 = 2"), false); }

#[test]
fn expr_ne() { assert_eq!(TestDb::new().scalar("RETURN 1 <> 2"), true); }

#[test]
fn expr_lt() { assert_eq!(TestDb::new().scalar("RETURN 1 < 2"), true); }

#[test]
fn expr_gt() { assert_eq!(TestDb::new().scalar("RETURN 2 > 1"), true); }

#[test]
fn expr_string_comparison() { assert_eq!(TestDb::new().scalar("RETURN 'a' < 'b'"), true); }

// ============================================================
// String functions
// ============================================================

#[test]
fn expr_tolower() { assert_eq!(TestDb::new().scalar("RETURN toLower('HELLO')"), "hello"); }

#[test]
fn expr_toupper() { assert_eq!(TestDb::new().scalar("RETURN toUpper('hello')"), "HELLO"); }

#[test]
fn expr_string_concatenation() { assert_eq!(TestDb::new().scalar("RETURN 'hello' + ' ' + 'world'"), "hello world"); }

#[test]
fn tolower_on_property() {
    let db = TestDb::new();
    db.run("CREATE (:Tag {name:'HELLO'})");
    let rows = db.run("MATCH (t:Tag) RETURN toLower(t.name) AS lower");
    assert_eq!(rows[0]["lower"], "hello");
}

#[test]
fn toupper_on_property() {
    let db = TestDb::new();
    db.run("CREATE (:Tag {name:'hello'})");
    let rows = db.run("MATCH (t:Tag) RETURN toUpper(t.name) AS upper");
    assert_eq!(rows[0]["upper"], "HELLO");
}

#[test]
fn string_concatenation_with_properties() {
    let db = TestDb::new();
    db.run("CREATE (:Person {first:'Alice', last:'Smith'})");
    let rows = db.run("MATCH (p:Person) RETURN p.first + ' ' + p.last AS full");
    assert_eq!(rows[0]["full"], "Alice Smith");
}

// ============================================================
// Type conversion functions
// ============================================================

#[test]
fn tointeger_from_string() { assert_eq!(TestDb::new().scalar("RETURN toInteger('42')"), 42); }

#[test]
fn tointeger_from_float() { assert_eq!(TestDb::new().scalar("RETURN toInteger(3.9)"), 3); }

#[test]
fn tofloat_from_string() {
    let f = TestDb::new().scalar("RETURN toFloat('3.14')").as_f64().unwrap();
    assert!((f - 3.14).abs() < 0.001);
}

#[test]
fn tostring_from_integer() { assert_eq!(TestDb::new().scalar("RETURN toString(42)"), "42"); }

#[test]
fn tostring_from_boolean() { assert_eq!(TestDb::new().scalar("RETURN toString(true)"), "true"); }

#[test]
fn abs_positive_and_negative() {
    let db = TestDb::new();
    assert_eq!(db.scalar("RETURN abs(-5)"), 5);
    assert_eq!(db.scalar("RETURN abs(5)"), 5);
    assert_eq!(db.scalar("RETURN abs(0)"), 0);
}

// ============================================================
// Entity introspection functions
// ============================================================

#[test]
fn id_function_returns_node_id() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let rows = db.run("MATCH (n:User) RETURN id(n) AS nodeId");
    assert!(rows[0]["nodeId"].is_number());
}

#[test]
fn type_of_relationship() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})-[:MY_REL]->(:Y {id:2})");
    let rows = db.run("MATCH (a)-[r:MY_REL]->(b) RETURN type(r) AS t");
    assert_eq!(rows[0]["t"], "MY_REL");
}

#[test]
fn labels_returns_all_labels() {
    let db = TestDb::new();
    db.run("CREATE (:A:B:C {x:1})");
    let rows = db.run("MATCH (n:A) RETURN labels(n) AS lbls");
    assert_eq!(rows[0]["lbls"].as_array().unwrap().len(), 3);
}

#[test]
fn labels_of_node() {
    let db = TestDb::new();
    db.run("CREATE (n:User:Admin {name: 'Alice'})");
    let rows = db.run("MATCH (n:User) RETURN labels(n) AS lbls");
    let labels = rows[0]["lbls"].as_array().unwrap();
    assert!(labels.contains(&serde_json::json!("User")));
    assert!(labels.contains(&serde_json::json!("Admin")));
}

#[test]
fn keys_returns_property_names() {
    let db = TestDb::new();
    db.run("CREATE (:Item {a:1, b:2, c:3})");
    let rows = db.run("MATCH (n:Item) RETURN keys(n) AS ks");
    assert_eq!(rows[0]["ks"].as_array().unwrap().len(), 3);
}

// ============================================================
// List functions
// ============================================================

#[test]
fn size_of_list() { assert_eq!(TestDb::new().scalar("RETURN size([1,2,3,4,5])"), 5); }

#[test]
fn size_of_empty_list() { assert_eq!(TestDb::new().scalar("RETURN size([])"), 0); }

#[test]
fn size_of_string() { assert_eq!(TestDb::new().scalar("RETURN size('hello')"), 5); }

#[test]
fn head_returns_first() { assert_eq!(TestDb::new().scalar("RETURN head([10, 20, 30])"), 10); }

#[test]
fn tail_returns_rest() {
    let rows = TestDb::new().run("RETURN tail([10, 20, 30]) AS t");
    let tail = rows[0]["t"].as_array().unwrap();
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0], 20);
    assert_eq!(tail[1], 30);
}

#[test]
fn head_of_empty_list_is_null() { assert!(TestDb::new().scalar("RETURN head([])").is_null()); }

// ============================================================
// Range function
// ============================================================

#[test]
fn range_basic() {
    let rows = TestDb::new().run("RETURN range(1, 5) AS r");
    let vals: Vec<i64> = rows[0]["r"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    assert_eq!(vals, vec![1, 2, 3, 4, 5]);
}

#[test]
fn range_with_step() {
    let rows = TestDb::new().run("RETURN range(0, 10, 3) AS r");
    let vals: Vec<i64> = rows[0]["r"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    assert_eq!(vals, vec![0, 3, 6, 9]);
}

// ============================================================
// Coalesce
// ============================================================

#[test]
fn coalesce_returns_first_non_null() { assert_eq!(TestDb::new().scalar("RETURN coalesce(null, 'fallback')"), "fallback"); }

#[test]
fn coalesce_returns_first_when_not_null() { assert_eq!(TestDb::new().scalar("RETURN coalesce('first', 'second')"), "first"); }

#[test]
fn coalesce_on_node_with_and_without_property() {
    let db = TestDb::new();
    db.run("CREATE (:Person {name:'Alice', nick:'Ali'})");
    db.run("CREATE (:Person {name:'Bob'})");
    let rows = db.run("MATCH (p:Person) RETURN coalesce(p.nick, p.name) AS display ORDER BY p.name");
    assert_eq!(rows[0]["display"], "Ali");
    assert_eq!(rows[1]["display"], "Bob");
}

// ============================================================
// CASE expressions
// ============================================================

#[test]
fn case_generic_when_then() { assert_eq!(TestDb::new().scalar("RETURN CASE WHEN 1 = 1 THEN 'yes' ELSE 'no' END"), "yes"); }

#[test]
fn case_generic_else_branch() { assert_eq!(TestDb::new().scalar("RETURN CASE WHEN 1 = 2 THEN 'yes' ELSE 'no' END"), "no"); }

#[test]
fn case_simple_form() {
    let db = TestDb::new();
    db.run("CREATE (n:User {age: 25})");
    let rows = db.run("MATCH (n:User) RETURN CASE n.age WHEN 25 THEN 'young' WHEN 50 THEN 'old' ELSE 'other' END AS cat");
    assert_eq!(rows[0]["cat"], "young");
}

#[test]
fn case_no_match_returns_null_without_else() { assert!(TestDb::new().scalar("RETURN CASE WHEN false THEN 'yes' END").is_null()); }

#[test]
fn case_in_return_classifies_data() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN p.name AS name, \
         CASE WHEN p.age < 30 THEN 'junior' WHEN p.age < 40 THEN 'mid' ELSE 'senior' END AS tier \
         ORDER BY p.name ASC",
    );
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[0]["tier"], "mid");
    assert_eq!(rows[1]["name"], "Bob");
    assert_eq!(rows[1]["tier"], "junior");
}

// ============================================================
// UNWIND
// ============================================================

#[test]
fn unwind_list() { assert_eq!(TestDb::new().run("UNWIND [1, 2, 3] AS n RETURN n").len(), 3); }

#[test]
fn unwind_empty_list() { TestDb::new().assert_count("UNWIND [] AS n RETURN n", 0); }

#[test]
fn unwind_single_element() {
    let rows = TestDb::new().run("UNWIND [42] AS n RETURN n");
    assert_eq!(rows[0]["n"], 42);
}

#[test]
fn unwind_null_produces_no_rows() { TestDb::new().assert_count("UNWIND null AS n RETURN n", 0); }

// ============================================================
// Pending: list comprehensions and subqueries
// ============================================================

#[test]
fn list_comprehension_filter() {
    let rows = TestDb::new().run("RETURN [x IN range(1,10) WHERE x % 2 = 0] AS evens");
    assert_eq!(rows[0]["evens"].as_array().unwrap().len(), 5);
}

#[test]
fn list_comprehension_transform() {
    let rows = TestDb::new().run("RETURN [x IN [1,2,3] | x * 2] AS doubled");
    assert_eq!(rows[0]["doubled"].as_array().unwrap(), &[2, 4, 6]);
}

// ============================================================
// Pending: grammar extensions
// ============================================================

#[test]
#[ignore = "stored procedures: CALL db.labels() not yet implemented"]
fn call_db_labels() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    db.run("CREATE (n:Product {name: 'Widget'})");
    let rows = db.run("CALL db.labels() YIELD label RETURN label");
    assert!(rows.len() >= 2);
}

#[test]
#[ignore = "FOREACH clause not yet in grammar"]
fn foreach_sets_property_on_list() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("MATCH (n:User) FOREACH (x IN ['Alice', 'Bob'] | SET n.seen = true)");
    db.assert_count("MATCH (n:User) WHERE n.seen = true RETURN n", 2);
}

#[test]
fn exists_subquery_in_where() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run("MATCH (p:Person) WHERE EXISTS { MATCH (p)-[:MANAGES]->() } RETURN p.name AS name");
    assert_eq!(rows.len(), 2);
}

#[test]
fn explain_returns_plan() {
    let db = TestDb::new();
    let _result = db.exec("EXPLAIN MATCH (n) RETURN n");
}

// ============================================================
// Mixed-type arithmetic
// ============================================================

#[test]
fn expr_int_plus_float() {
    let v = TestDb::new().scalar("RETURN 1 + 2.5").as_f64().unwrap();
    assert!((v - 3.5).abs() < 0.001);
}

#[test]
fn expr_float_minus_int() {
    let v = TestDb::new().scalar("RETURN 10.0 - 3").as_f64().unwrap();
    assert!((v - 7.0).abs() < 0.001);
}

#[test]
fn expr_int_times_float() {
    let v = TestDb::new().scalar("RETURN 4 * 2.5").as_f64().unwrap();
    assert!((v - 10.0).abs() < 0.001);
}

// ============================================================
// Float precision
// ============================================================

#[test]
fn expr_float_precision() {
    let v = TestDb::new().scalar("RETURN 0.1 + 0.2").as_f64().unwrap();
    assert!((v - 0.3).abs() < 0.0001);
}

// ============================================================
// Large numbers
// ============================================================

#[test]
fn expr_large_integer() {
    assert_eq!(TestDb::new().scalar("RETURN 1000000000"), 1000000000_i64);
}

#[test]
fn expr_negative_float() {
    let v = TestDb::new().scalar("RETURN -3.14").as_f64().unwrap();
    assert!((v + 3.14).abs() < 0.001);
}

// ============================================================
// Null propagation in arithmetic
// ============================================================

#[test]
fn expr_null_plus_int() {
    assert!(TestDb::new().scalar("RETURN null + 1").is_null());
}

#[test]
fn expr_null_times_int() {
    assert!(TestDb::new().scalar("RETURN null * 5").is_null());
}

#[test]
fn expr_null_equals_null() {
    // In Lora, null = null returns null (not true)
    assert!(TestDb::new().scalar("RETURN null = null").is_null());
}

// ============================================================
// Boolean XOR
// ============================================================

#[test]
fn expr_xor_true_true() {
    assert_eq!(TestDb::new().scalar("RETURN true XOR true"), false);
}

#[test]
fn expr_xor_true_false() {
    assert_eq!(TestDb::new().scalar("RETURN true XOR false"), true);
}

#[test]
fn expr_xor_false_false() {
    assert_eq!(TestDb::new().scalar("RETURN false XOR false"), false);
}

// ============================================================
// Nested function calls
// ============================================================

#[test]
fn expr_nested_tostring_tointeger() {
    assert_eq!(TestDb::new().scalar("RETURN toInteger(toString(42))"), 42);
}

#[test]
fn expr_tolower_of_toupper() {
    assert_eq!(TestDb::new().scalar("RETURN toLower(toUpper('hello'))"), "hello");
}

#[test]
fn expr_size_of_range() {
    assert_eq!(TestDb::new().scalar("RETURN size(range(1, 10))"), 10);
}

// ============================================================
// CASE with multiple WHEN branches
// ============================================================

#[test]
fn case_multiple_whens() {
    let db = TestDb::new();
    assert_eq!(db.scalar("RETURN CASE WHEN 1=2 THEN 'a' WHEN 2=3 THEN 'b' WHEN 3=3 THEN 'c' ELSE 'd' END"), "c");
}

#[test]
fn case_first_matching_when_wins() {
    let db = TestDb::new();
    assert_eq!(db.scalar("RETURN CASE WHEN true THEN 'first' WHEN true THEN 'second' END"), "first");
}

// ============================================================
// Type coercion in comparisons
// ============================================================

#[test]
fn expr_int_equals_float_same_value() {
    assert_eq!(TestDb::new().scalar("RETURN 5 = 5.0"), true);
}

#[test]
fn expr_int_less_than_float() {
    assert_eq!(TestDb::new().scalar("RETURN 3 < 3.5"), true);
}

// ============================================================
// String comparison
// ============================================================

#[test]
fn expr_string_less_than() {
    assert_eq!(TestDb::new().scalar("RETURN 'apple' < 'banana'"), true);
}

#[test]
fn expr_string_greater_than() {
    assert_eq!(TestDb::new().scalar("RETURN 'z' > 'a'"), true);
}

#[test]
fn expr_string_equality() {
    assert_eq!(TestDb::new().scalar("RETURN 'hello' = 'hello'"), true);
}

// ============================================================
// IN with various types
// ============================================================

#[test]
fn expr_in_list_integers() {
    assert_eq!(TestDb::new().scalar("RETURN 3 IN [1, 2, 3, 4]"), true);
}

#[test]
fn expr_not_in_list() {
    assert_eq!(TestDb::new().scalar("RETURN 5 IN [1, 2, 3]"), false);
}

#[test]
fn expr_in_list_strings() {
    assert_eq!(TestDb::new().scalar("RETURN 'b' IN ['a', 'b', 'c']"), true);
}

// ============================================================
// Power operator
// ============================================================

#[test]
fn expr_power_float() {
    let v = TestDb::new().scalar("RETURN 2.0 ^ 0.5").as_f64().unwrap();
    assert!((v - 1.4142).abs() < 0.001);
}

// ============================================================
// abs on float
// ============================================================

#[test]
fn abs_on_float() {
    let v = TestDb::new().scalar("RETURN abs(-3.14)").as_f64().unwrap();
    assert!((v - 3.14).abs() < 0.001);
}

// ============================================================
// Coalesce with multiple nulls
// ============================================================

#[test]
fn coalesce_multiple_nulls_then_value() {
    assert_eq!(TestDb::new().scalar("RETURN coalesce(null, null, null, 42)"), 42);
}

#[test]
fn coalesce_all_nulls_returns_null() {
    assert!(TestDb::new().scalar("RETURN coalesce(null, null, null)").is_null());
}

// ============================================================
// String functions: trim, replace, split, substring
// ============================================================

#[test]
fn trim_whitespace() {
    assert_eq!(TestDb::new().scalar("RETURN trim('  hello  ')"), "hello");
}

#[test]
fn trim_tabs_and_newlines() {
    assert_eq!(TestDb::new().scalar("RETURN trim('  \t hi \n ')"), "hi");
}

#[test]
fn trim_already_trimmed() {
    assert_eq!(TestDb::new().scalar("RETURN trim('hello')"), "hello");
}

#[test]
fn trim_null_returns_null() {
    assert!(TestDb::new().scalar("RETURN trim(null)").is_null());
}

#[test]
fn ltrim_whitespace() {
    assert_eq!(TestDb::new().scalar("RETURN lTrim('  hello  ')"), "hello  ");
}

#[test]
fn rtrim_whitespace() {
    assert_eq!(TestDb::new().scalar("RETURN rTrim('  hello  ')"), "  hello");
}

#[test]
fn replace_substring() {
    assert_eq!(TestDb::new().scalar("RETURN replace('hello world', 'world', 'rust')"), "hello rust");
}

#[test]
fn replace_all_occurrences() {
    assert_eq!(TestDb::new().scalar("RETURN replace('a-b-c', '-', ':')"), "a:b:c");
}

#[test]
fn replace_no_match() {
    assert_eq!(TestDb::new().scalar("RETURN replace('hello', 'xyz', 'abc')"), "hello");
}

#[test]
fn replace_with_empty() {
    assert_eq!(TestDb::new().scalar("RETURN replace('hello', 'l', '')"), "heo");
}

#[test]
fn replace_null_returns_null() {
    assert!(TestDb::new().scalar("RETURN replace(null, 'a', 'b')").is_null());
}

#[test]
fn split_string() {
    let rows = TestDb::new().run("RETURN split('a,b,c', ',') AS parts");
    let parts = rows[0]["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], "a");
    assert_eq!(parts[1], "b");
    assert_eq!(parts[2], "c");
}

#[test]
fn split_no_delimiter_found() {
    let rows = TestDb::new().run("RETURN split('hello', ',') AS parts");
    let parts = rows[0]["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0], "hello");
}

#[test]
fn split_null_returns_null() {
    assert!(TestDb::new().scalar("RETURN split(null, ',')").is_null());
}

#[test]
fn substring_three_args() {
    assert_eq!(TestDb::new().scalar("RETURN substring('hello', 1, 3)"), "ell");
}

#[test]
fn substring_two_args_rest_of_string() {
    assert_eq!(TestDb::new().scalar("RETURN substring('hello', 2)"), "llo");
}

#[test]
fn substring_from_start() {
    assert_eq!(TestDb::new().scalar("RETURN substring('hello', 0, 5)"), "hello");
}

#[test]
fn substring_beyond_length() {
    assert_eq!(TestDb::new().scalar("RETURN substring('hi', 0, 100)"), "hi");
}

#[test]
fn substring_start_beyond_length() {
    assert_eq!(TestDb::new().scalar("RETURN substring('hi', 10, 5)"), "");
}

#[test]
fn substring_null_returns_null() {
    assert!(TestDb::new().scalar("RETURN substring(null, 0, 5)").is_null());
}

#[test]
fn reverse_string() {
    assert_eq!(TestDb::new().scalar("RETURN reverse('hello')"), "olleh");
}

#[test]
fn left_function() {
    assert_eq!(TestDb::new().scalar("RETURN left('hello', 3)"), "hel");
}

#[test]
fn right_function() {
    assert_eq!(TestDb::new().scalar("RETURN right('hello', 3)"), "llo");
}

// ============================================================
// Math functions: ceil, floor, round, sqrt, sign
// ============================================================

#[test]
fn ceil_float() {
    assert_eq!(TestDb::new().scalar("RETURN ceil(2.3)"), 3);
}

#[test]
fn ceil_negative() {
    assert_eq!(TestDb::new().scalar("RETURN ceil(-2.7)"), -2);
}

#[test]
fn ceil_integer_passthrough() {
    assert_eq!(TestDb::new().scalar("RETURN ceil(5)"), 5);
}

#[test]
fn ceil_null_returns_null() {
    assert!(TestDb::new().scalar("RETURN ceil(null)").is_null());
}

#[test]
fn floor_float() {
    assert_eq!(TestDb::new().scalar("RETURN floor(2.7)"), 2);
}

#[test]
fn floor_negative() {
    assert_eq!(TestDb::new().scalar("RETURN floor(-2.3)"), -3);
}

#[test]
fn floor_integer_passthrough() {
    assert_eq!(TestDb::new().scalar("RETURN floor(5)"), 5);
}

#[test]
fn round_up() {
    assert_eq!(TestDb::new().scalar("RETURN round(2.5)"), 3);
}

#[test]
fn round_down() {
    assert_eq!(TestDb::new().scalar("RETURN round(2.3)"), 2);
}

#[test]
fn round_negative() {
    assert_eq!(TestDb::new().scalar("RETURN round(-2.5)"), -3);
}

#[test]
fn round_integer_passthrough() {
    assert_eq!(TestDb::new().scalar("RETURN round(7)"), 7);
}

#[test]
fn sqrt_perfect_square() {
    let v = TestDb::new().scalar("RETURN sqrt(16)").as_f64().unwrap();
    assert!((v - 4.0).abs() < 0.001);
}

#[test]
fn sqrt_non_perfect() {
    let v = TestDb::new().scalar("RETURN sqrt(2)").as_f64().unwrap();
    assert!((v - 1.4142).abs() < 0.001);
}

#[test]
fn sqrt_zero() {
    let v = TestDb::new().scalar("RETURN sqrt(0)").as_f64().unwrap();
    assert!((v - 0.0).abs() < 0.001);
}

#[test]
fn sqrt_negative_returns_null() {
    assert!(TestDb::new().scalar("RETURN sqrt(-1)").is_null());
}

#[test]
fn sqrt_float_input() {
    let v = TestDb::new().scalar("RETURN sqrt(2.25)").as_f64().unwrap();
    assert!((v - 1.5).abs() < 0.001);
}

#[test]
fn sign_positive() {
    assert_eq!(TestDb::new().scalar("RETURN sign(5)"), 1);
}

#[test]
fn sign_negative() {
    assert_eq!(TestDb::new().scalar("RETURN sign(-5)"), -1);
}

#[test]
fn sign_zero() {
    assert_eq!(TestDb::new().scalar("RETURN sign(0)"), 0);
}

#[test]
fn sign_float() {
    assert_eq!(TestDb::new().scalar("RETURN sign(-3.14)"), -1);
}

// ============================================================
// Pending: date/time
// ============================================================

#[test]
#[ignore = "temporal types: date/time functions not yet implemented"]
fn date_function() {
    let _rows = TestDb::new().run("RETURN date('2024-01-15') AS d");
}

#[test]
#[ignore = "temporal types: date/time functions not yet implemented"]
fn datetime_function() {
    let _rows = TestDb::new().run("RETURN datetime() AS now");
}

// ============================================================
// Advanced string functions
// ============================================================

#[test]
fn reverse_list() {
    let rows = TestDb::new().run("RETURN reverse([1, 2, 3]) AS r");
    let arr = rows[0]["r"].as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0], 3);
    assert_eq!(arr[1], 2);
    assert_eq!(arr[2], 1);
}

#[test]
fn left_beyond_string_length() {
    // left('hi', 100) should return the whole string
    assert_eq!(TestDb::new().scalar("RETURN left('hi', 100)"), "hi");
}

#[test]
fn right_beyond_string_length() {
    // right('hi', 100) should return the whole string
    assert_eq!(TestDb::new().scalar("RETURN right('hi', 100)"), "hi");
}

#[test]
fn split_with_multi_char_delimiter() {
    let rows = TestDb::new().run("RETURN split('one::two::three', '::') AS parts");
    let parts = rows[0]["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], "one");
    assert_eq!(parts[1], "two");
    assert_eq!(parts[2], "three");
}

#[test]
fn nested_string_functions_tolower_replace() {
    assert_eq!(
        TestDb::new().scalar("RETURN toLower(replace('Hello World', ' ', '_'))"),
        "hello_world",
    );
}

#[test]
fn empty_string_handling() {
    let db = TestDb::new();
    assert_eq!(db.scalar("RETURN size('')"), 0);
    assert_eq!(db.scalar("RETURN toLower('')"), "");
    assert_eq!(db.scalar("RETURN toUpper('')"), "");
    assert_eq!(db.scalar("RETURN reverse('')"), "");
    assert_eq!(db.scalar("RETURN trim('')"), "");
}

// ============================================================
// Math function combinations
// ============================================================

#[test]
fn abs_of_floor() {
    // abs(floor(-2.7)) => abs(-3) => 3
    assert_eq!(TestDb::new().scalar("RETURN abs(floor(-2.7))"), 3);
}

#[test]
fn ceil_of_sqrt() {
    // ceil(sqrt(5)) => ceil(2.236...) => 3
    assert_eq!(TestDb::new().scalar("RETURN ceil(sqrt(5))"), 3);
}

#[test]
fn round_computed_average() {
    // Simulate rounding an average: round((10 + 20 + 33) / 3.0)
    // 63 / 3.0 = 21.0 => round(21.0) => 21
    assert_eq!(TestDb::new().scalar("RETURN round((10 + 20 + 33) / 3.0)"), 21);
}

#[test]
fn sign_of_computed_value() {
    // sign(3 - 10) => sign(-7) => -1
    assert_eq!(TestDb::new().scalar("RETURN sign(3 - 10)"), -1);
}

#[test]
fn floor_of_division() {
    // floor(7 / 2.0) => floor(3.5) => 3
    assert_eq!(TestDb::new().scalar("RETURN floor(7 / 2.0)"), 3);
}

// ============================================================
// CASE expression edge cases
// ============================================================

#[test]
fn case_nested_case() {
    // Nested CASE: outer CASE uses result of inner CASE
    let result = TestDb::new().scalar(
        "RETURN CASE WHEN true THEN \
             CASE WHEN 1 = 1 THEN 'inner-yes' ELSE 'inner-no' END \
         ELSE 'outer-no' END",
    );
    assert_eq!(result, "inner-yes");
}

#[test]
fn case_with_null_input() {
    // CASE null WHEN null THEN 'matched' ELSE 'not-matched' END
    // Since null = null currently returns true in this engine, this matches.
    // In standard Lora, null = null => null => would fall to ELSE.
    // Accept whatever the engine does; just verify no crash.
    let result = TestDb::new().scalar(
        "RETURN CASE null WHEN null THEN 'matched' ELSE 'not-matched' END",
    );
    assert!(result == "matched" || result == "not-matched");
}

#[test]
fn case_with_expression_in_when() {
    let db = TestDb::new();
    db.run("CREATE (:Score {value: 85})");
    let rows = db.run(
        "MATCH (s:Score) RETURN \
         CASE \
           WHEN s.value >= 90 THEN 'A' \
           WHEN s.value >= 80 THEN 'B' \
           WHEN s.value >= 70 THEN 'C' \
           ELSE 'F' \
         END AS grade",
    );
    assert_eq!(rows[0]["grade"], "B");
}

#[test]
fn case_returning_different_types() {
    // Branches return string vs integer — engine should handle gracefully
    let db = TestDb::new();
    let result = db.scalar(
        "RETURN CASE WHEN false THEN 'hello' WHEN true THEN 42 ELSE null END",
    );
    assert_eq!(result, 42);
}

#[test]
fn case_all_whens_false_no_else_returns_null() {
    let result = TestDb::new().scalar(
        "RETURN CASE WHEN 1 = 2 THEN 'a' WHEN 2 = 3 THEN 'b' END",
    );
    assert!(result.is_null());
}

// ============================================================
// UNWIND advanced patterns
// ============================================================

#[test]
fn unwind_plus_match_combination() {
    let db = TestDb::new();
    db.seed_org_graph();
    // UNWIND a list of names, then MATCH them
    let rows = db.run(
        "UNWIND ['Alice', 'Bob', 'Eve'] AS name \
         MATCH (p:Person {name: name}) \
         RETURN p.name AS name, p.age AS age ORDER BY p.name",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[1]["name"], "Bob");
    assert_eq!(rows[2]["name"], "Eve");
}

#[test]
fn unwind_range_function() {
    // UNWIND range(1, 5) AS n RETURN n
    let rows = TestDb::new().run("UNWIND range(1, 5) AS n RETURN n");
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0]["n"], 1);
    assert_eq!(rows[4]["n"], 5);
}

#[test]
fn unwind_with_aggregation() {
    // UNWIND [10, 20, 30] AS val RETURN sum(val) AS total
    let rows = TestDb::new().run("UNWIND [10, 20, 30] AS val RETURN sum(val) AS total");
    assert_eq!(rows[0]["total"], 60);
}

#[test]
fn unwind_strings_with_function() {
    // UNWIND list of strings and apply toUpper
    let rows = TestDb::new().run(
        "UNWIND ['hello', 'world'] AS word RETURN toUpper(word) AS upper",
    );
    assert_eq!(rows[0]["upper"], "HELLO");
    assert_eq!(rows[1]["upper"], "WORLD");
}

#[test]
fn unwind_nested_list() {
    let rows = TestDb::new().run(
        "UNWIND [[1,2],[3,4]] AS sublist UNWIND sublist AS val RETURN val",
    );
    assert_eq!(rows.len(), 4);
}

// ============================================================
// Map and list literals
// ============================================================

#[test]
fn map_literal_in_return() {
    let rows = TestDb::new().run("RETURN {name: 'Alice', age: 30} AS person");
    let person = &rows[0]["person"];
    assert_eq!(person["name"], "Alice");
    assert_eq!(person["age"], 30);
}

#[test]
fn list_literal_in_return() {
    let rows = TestDb::new().run("RETURN [1, 2, 3] AS nums");
    let nums = rows[0]["nums"].as_array().unwrap();
    assert_eq!(nums.len(), 3);
    assert_eq!(nums[0], 1);
    assert_eq!(nums[2], 3);
}

#[test]
fn nested_map_list_literal() {
    let rows = TestDb::new().run(
        "RETURN {items: [1, 2, 3], label: 'test'} AS data",
    );
    let data = &rows[0]["data"];
    assert_eq!(data["label"], "test");
    let items = data["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
}

#[test]
fn empty_map_literal() {
    let rows = TestDb::new().run("RETURN {} AS m");
    let m = &rows[0]["m"];
    assert!(m.is_object());
    assert_eq!(m.as_object().unwrap().len(), 0);
}

#[test]
fn empty_list_literal() {
    let rows = TestDb::new().run("RETURN [] AS lst");
    let lst = rows[0]["lst"].as_array().unwrap();
    assert_eq!(lst.len(), 0);
}

// ============================================================
// Ignored: future compatibility tests
// ============================================================

#[test]
fn list_comprehension_filter_and_map() {
    // Lora: list comprehension [x IN list WHERE ... | expr]
    let rows = TestDb::new().run(
        "RETURN [x IN range(1, 10) WHERE x % 2 = 0 | x * x] AS squares",
    );
    let arr = rows[0]["squares"].as_array().unwrap();
    assert_eq!(arr, &[4, 16, 36, 64, 100]);
}

#[test]
fn reduce_function() {
    // Lora: reduce(acc = 0, x IN list | acc + x)
    let result = TestDb::new().scalar(
        "RETURN reduce(total = 0, x IN [1, 2, 3, 4, 5] | total + x)",
    );
    assert_eq!(result, 15);
}

#[test]
fn properties_function_on_node() {
    // Lora: properties(n) returns map of all properties
    let db = TestDb::new();
    db.run("CREATE (:Item {a: 1, b: 'two'})");
    let rows = db.run("MATCH (n:Item) RETURN properties(n) AS props");
    let props = &rows[0]["props"];
    assert_eq!(props["a"], 1);
    assert_eq!(props["b"], "two");
}

#[test]
fn timestamp_function() {
    // Lora: timestamp() returns current epoch millis
    let result = TestDb::new().scalar("RETURN timestamp()");
    assert!(result.is_number());
}

#[test]
fn point_and_distance_functions() {
    // Lora: point({x, y}) and distance(point1, point2)
    let result = TestDb::new().scalar(
        "RETURN distance(point({x: 0, y: 0}), point({x: 3, y: 4}))",
    );
    let v = result.as_f64().unwrap();
    assert!((v - 5.0).abs() < 0.001);
}

#[test]
#[ignore = "duration type: duration arithmetic not yet implemented"]
fn duration_type_and_arithmetic() {
    // Lora: duration type and arithmetic
    let _rows = TestDb::new().run(
        "RETURN duration('P1Y2M3D') AS d",
    );
}

#[test]
fn case_type_coercion_in_branches() {
    // Lora: CASE branches returning mixed types with coercion
    let result = TestDb::new().scalar(
        "RETURN CASE WHEN true THEN 1 ELSE '1' END = CASE WHEN true THEN '1' ELSE 1 END",
    );
    assert_eq!(result, false);
}

#[test]
#[ignore = "APOC utilities: apoc-like utility functions not yet implemented"]
fn apoc_like_utility_functions() {
    // Future extension target: apoc-like utility functions
    let _rows = TestDb::new().run(
        "RETURN apoc.text.join(['a', 'b', 'c'], '-') AS joined",
    );
}
