/// Extended function coverage — edge cases for string, numeric, list, and
/// conversion functions, plus tests for functions not yet implemented.
///
/// Active tests verify currently supported behavior.
/// Ignored tests specify desired future behavior.
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// 1. String functions — edge cases
// ============================================================

#[test]
fn tolower_empty_string() {
    assert_eq!(TestDb::new().scalar("RETURN toLower('')"), "");
}

#[test]
fn toupper_empty_string() {
    assert_eq!(TestDb::new().scalar("RETURN toUpper('')"), "");
}

#[test]
fn tolower_already_lower() {
    assert_eq!(TestDb::new().scalar("RETURN toLower('hello')"), "hello");
}

#[test]
fn toupper_already_upper() {
    assert_eq!(TestDb::new().scalar("RETURN toUpper('HELLO')"), "HELLO");
}

#[test]
fn tolower_mixed_case() {
    assert_eq!(TestDb::new().scalar("RETURN toLower('HeLLo WoRLd')"), "hello world");
}

#[test]
fn tolower_null_returns_null() {
    assert!(TestDb::new().scalar("RETURN toLower(null)").is_null());
}

#[test]
fn toupper_null_returns_null() {
    assert!(TestDb::new().scalar("RETURN toUpper(null)").is_null());
}

// ============================================================
// 2. Trim functions — edge cases
// ============================================================

#[test]
fn trim_whitespace() {
    assert_eq!(TestDb::new().scalar("RETURN trim('  hello  ')"), "hello");
}

#[test]
fn ltrim_whitespace() {
    assert_eq!(TestDb::new().scalar("RETURN ltrim('  hello  ')"), "hello  ");
}

#[test]
fn rtrim_whitespace() {
    assert_eq!(TestDb::new().scalar("RETURN rtrim('  hello  ')"), "  hello");
}

#[test]
fn trim_no_whitespace() {
    assert_eq!(TestDb::new().scalar("RETURN trim('hello')"), "hello");
}

#[test]
fn trim_empty_string() {
    assert_eq!(TestDb::new().scalar("RETURN trim('')"), "");
}

#[test]
fn trim_all_whitespace() {
    assert_eq!(TestDb::new().scalar("RETURN trim('   ')"), "");
}

#[test]
fn trim_tabs_and_newlines() {
    assert_eq!(TestDb::new().scalar("RETURN trim('\t hello \n')"), "hello");
}

// ============================================================
// 3. Replace function — edge cases
// ============================================================

#[test]
fn replace_basic() {
    assert_eq!(
        TestDb::new().scalar("RETURN replace('hello world', 'world', 'earth')"),
        "hello earth"
    );
}

#[test]
fn replace_multiple_occurrences() {
    assert_eq!(
        TestDb::new().scalar("RETURN replace('abcabc', 'a', 'x')"),
        "xbcxbc"
    );
}

#[test]
fn replace_no_match() {
    assert_eq!(
        TestDb::new().scalar("RETURN replace('hello', 'xyz', 'abc')"),
        "hello"
    );
}

#[test]
fn replace_empty_search() {
    // Replacing empty string inserts between each character
    let v = TestDb::new().scalar("RETURN replace('ab', '', '-')");
    assert_eq!(v, "-a-b-");
}

#[test]
fn replace_with_empty_replacement() {
    assert_eq!(
        TestDb::new().scalar("RETURN replace('hello', 'l', '')"),
        "heo"
    );
}

// ============================================================
// 4. Split function — edge cases
// ============================================================

#[test]
fn split_basic() {
    let v = TestDb::new().scalar("RETURN split('a,b,c', ',')");
    let arr = v.as_array().unwrap();
    assert_eq!(arr, &["a", "b", "c"]);
}

#[test]
fn split_no_delimiter_found() {
    let v = TestDb::new().scalar("RETURN split('hello', ',')");
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0], "hello");
}

#[test]
fn split_empty_string() {
    let v = TestDb::new().scalar("RETURN split('', ',')");
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0], "");
}

#[test]
fn split_consecutive_delimiters() {
    let v = TestDb::new().scalar("RETURN split('a,,b', ',')");
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0], "a");
    assert_eq!(arr[1], "");
    assert_eq!(arr[2], "b");
}

// ============================================================
// 5. Substring function — edge cases
// ============================================================

#[test]
fn substring_basic() {
    assert_eq!(TestDb::new().scalar("RETURN substring('hello world', 0, 5)"), "hello");
}

#[test]
fn substring_from_middle() {
    assert_eq!(TestDb::new().scalar("RETURN substring('hello world', 6, 5)"), "world");
}

#[test]
fn substring_two_arg_to_end() {
    assert_eq!(TestDb::new().scalar("RETURN substring('hello world', 6)"), "world");
}

#[test]
fn substring_start_beyond_length() {
    assert_eq!(TestDb::new().scalar("RETURN substring('hello', 100, 5)"), "");
}

#[test]
fn substring_length_beyond_end() {
    assert_eq!(TestDb::new().scalar("RETURN substring('hello', 3, 100)"), "lo");
}

#[test]
fn substring_zero_length() {
    assert_eq!(TestDb::new().scalar("RETURN substring('hello', 0, 0)"), "");
}

// ============================================================
// 6. Left / Right functions — edge cases
// ============================================================

#[test]
fn left_basic() {
    assert_eq!(TestDb::new().scalar("RETURN left('hello', 3)"), "hel");
}

#[test]
fn left_zero() {
    assert_eq!(TestDb::new().scalar("RETURN left('hello', 0)"), "");
}

#[test]
fn left_beyond_length() {
    assert_eq!(TestDb::new().scalar("RETURN left('hello', 100)"), "hello");
}

#[test]
fn right_basic() {
    assert_eq!(TestDb::new().scalar("RETURN right('hello', 3)"), "llo");
}

#[test]
fn right_zero() {
    assert_eq!(TestDb::new().scalar("RETURN right('hello', 0)"), "");
}

#[test]
fn right_beyond_length() {
    assert_eq!(TestDb::new().scalar("RETURN right('hello', 100)"), "hello");
}

// ============================================================
// 7. Reverse function — edge cases
// ============================================================

#[test]
fn reverse_string() {
    assert_eq!(TestDb::new().scalar("RETURN reverse('hello')"), "olleh");
}

#[test]
fn reverse_empty_string() {
    assert_eq!(TestDb::new().scalar("RETURN reverse('')"), "");
}

#[test]
fn reverse_single_char() {
    assert_eq!(TestDb::new().scalar("RETURN reverse('x')"), "x");
}

#[test]
fn reverse_palindrome() {
    assert_eq!(TestDb::new().scalar("RETURN reverse('racecar')"), "racecar");
}

// ============================================================
// 8. Size / Length function — edge cases
// ============================================================

#[test]
fn size_of_string() {
    assert_eq!(TestDb::new().scalar("RETURN size('hello')"), 5);
}

#[test]
fn size_of_empty_string() {
    assert_eq!(TestDb::new().scalar("RETURN size('')"), 0);
}

#[test]
fn length_alias_works() {
    assert_eq!(TestDb::new().scalar("RETURN length('hello')"), 5);
}

#[test]
fn size_of_list() {
    assert_eq!(TestDb::new().scalar("RETURN size([1, 2, 3])"), 3);
}

#[test]
fn size_of_empty_list() {
    assert_eq!(TestDb::new().scalar("RETURN size([])"), 0);
}

// ============================================================
// 9. Numeric functions — edge cases
// ============================================================

#[test]
fn abs_zero() {
    assert_eq!(TestDb::new().scalar("RETURN abs(0)"), 0);
}

#[test]
fn abs_negative_float() {
    let v = TestDb::new().scalar("RETURN abs(-3.14)").as_f64().unwrap();
    assert!((v - 3.14).abs() < 0.001);
}

#[test]
fn abs_positive() {
    assert_eq!(TestDb::new().scalar("RETURN abs(42)"), 42);
}

#[test]
fn ceil_already_integer() {
    assert_eq!(TestDb::new().scalar("RETURN ceil(5.0)"), 5);
}

#[test]
fn ceil_positive() {
    assert_eq!(TestDb::new().scalar("RETURN ceil(4.1)"), 5);
}

#[test]
fn ceil_negative() {
    assert_eq!(TestDb::new().scalar("RETURN ceil(-4.9)"), -4);
}

#[test]
fn floor_positive() {
    assert_eq!(TestDb::new().scalar("RETURN floor(4.9)"), 4);
}

#[test]
fn floor_negative() {
    assert_eq!(TestDb::new().scalar("RETURN floor(-4.1)"), -5);
}

#[test]
fn round_half_up() {
    assert_eq!(TestDb::new().scalar("RETURN round(2.5)"), 3);
}

#[test]
fn round_half_down() {
    assert_eq!(TestDb::new().scalar("RETURN round(2.4)"), 2);
}

#[test]
fn sqrt_of_zero() {
    let v = TestDb::new().scalar("RETURN sqrt(0)").as_f64().unwrap();
    assert!(v.abs() < 0.001);
}

#[test]
fn sqrt_of_one() {
    let v = TestDb::new().scalar("RETURN sqrt(1)").as_f64().unwrap();
    assert!((v - 1.0).abs() < 0.001);
}

#[test]
fn sqrt_negative_null() {
    assert!(TestDb::new().scalar("RETURN sqrt(-1)").is_null());
}

#[test]
fn sign_positive_int() {
    assert_eq!(TestDb::new().scalar("RETURN sign(42)"), 1);
}

#[test]
fn sign_negative_int() {
    assert_eq!(TestDb::new().scalar("RETURN sign(-42)"), -1);
}

#[test]
fn sign_zero_int() {
    assert_eq!(TestDb::new().scalar("RETURN sign(0)"), 0);
}

// ============================================================
// 10. Type conversion — toString edge cases
// ============================================================

#[test]
fn tostring_integer() {
    assert_eq!(TestDb::new().scalar("RETURN toString(42)"), "42");
}

#[test]
fn tostring_negative_integer() {
    assert_eq!(TestDb::new().scalar("RETURN toString(-7)"), "-7");
}

#[test]
fn tostring_float() {
    let v = TestDb::new().scalar("RETURN toString(3.14)");
    assert!(v.as_str().unwrap().starts_with("3.14"));
}

#[test]
fn tostring_boolean_true() {
    assert_eq!(TestDb::new().scalar("RETURN toString(true)"), "true");
}

#[test]
fn tostring_boolean_false() {
    assert_eq!(TestDb::new().scalar("RETURN toString(false)"), "false");
}

#[test]
fn tostring_string_passthrough() {
    assert_eq!(TestDb::new().scalar("RETURN toString('hello')"), "hello");
}

#[test]
fn tostring_null_returns_null() {
    assert!(TestDb::new().scalar("RETURN toString(null)").is_null());
}

// ============================================================
// 11. Type conversion — toInteger edge cases
// ============================================================

#[test]
fn tointeger_string_positive() {
    assert_eq!(TestDb::new().scalar("RETURN toInteger('42')"), 42);
}

#[test]
fn tointeger_string_negative() {
    assert_eq!(TestDb::new().scalar("RETURN toInteger('-7')"), -7);
}

#[test]
fn tointeger_float_truncates() {
    assert_eq!(TestDb::new().scalar("RETURN toInteger(3.9)"), 3);
}

#[test]
fn tointeger_negative_float_truncates() {
    assert_eq!(TestDb::new().scalar("RETURN toInteger(-3.9)"), -3);
}

#[test]
fn tointeger_already_int() {
    assert_eq!(TestDb::new().scalar("RETURN toInteger(42)"), 42);
}

#[test]
fn tointeger_null_returns_null() {
    assert!(TestDb::new().scalar("RETURN toInteger(null)").is_null());
}

// ============================================================
// 12. Type conversion — toFloat edge cases
// ============================================================

#[test]
fn tofloat_string() {
    let v = TestDb::new().scalar("RETURN toFloat('3.14')").as_f64().unwrap();
    assert!((v - 3.14).abs() < 0.001);
}

#[test]
fn tofloat_integer_to_float() {
    let v = TestDb::new().scalar("RETURN toFloat(42)").as_f64().unwrap();
    assert!((v - 42.0).abs() < 0.001);
}

#[test]
fn tofloat_already_float() {
    let v = TestDb::new().scalar("RETURN toFloat(3.14)").as_f64().unwrap();
    assert!((v - 3.14).abs() < 0.001);
}

#[test]
fn tofloat_null_returns_null() {
    assert!(TestDb::new().scalar("RETURN toFloat(null)").is_null());
}

// ============================================================
// 13. Coalesce with property access
// ============================================================

#[test]
fn coalesce_missing_property_to_default() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice', nickname: 'Ali'})");
    db.run("CREATE (:User {name: 'Bob'})");
    // Bob has no nickname: property access returns null → coalesce to 'N/A'
    let rows = db.run(
        "MATCH (u:User) RETURN u.name AS name, coalesce(u.nickname, 'N/A') AS nick ORDER BY u.name",
    );
    assert_eq!(rows[0]["nick"], "Ali");
    assert_eq!(rows[1]["nick"], "N/A");
}

#[test]
fn coalesce_existing_property() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice', nickname: 'Ali'})");
    let rows = db.run(
        "MATCH (u:User) RETURN coalesce(u.nickname, 'N/A') AS nick",
    );
    assert_eq!(rows[0]["nick"], "Ali");
}

// ============================================================
// 14. Keys function
// ============================================================

#[test]
fn keys_of_node() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'Widget', price: 42})");
    let rows = db.run("MATCH (i:Item) RETURN keys(i) AS k");
    let keys = rows[0]["k"].as_array().unwrap();
    assert_eq!(keys.len(), 2);
}

#[test]
fn keys_of_relationship() {
    let db = TestDb::new();
    db.run("CREATE (a:Person {name:'A'})-[:KNOWS {since:2020, strength:5}]->(b:Person {name:'B'})");
    let rows = db.run("MATCH ()-[r:KNOWS]->() RETURN keys(r) AS k");
    let keys = rows[0]["k"].as_array().unwrap();
    assert_eq!(keys.len(), 2);
}

#[test]
fn keys_of_empty_node() {
    let db = TestDb::new();
    db.run("CREATE (:Empty)");
    let rows = db.run("MATCH (n:Empty) RETURN keys(n) AS k");
    let keys = rows[0]["k"].as_array().unwrap();
    assert_eq!(keys.len(), 0);
}

// ============================================================
// 15. Labels function
// ============================================================

#[test]
fn labels_single() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    let rows = db.run("MATCH (n:User) RETURN labels(n) AS l");
    let labels = rows[0]["l"].as_array().unwrap();
    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0], "User");
}

#[test]
fn labels_multiple() {
    let db = TestDb::new();
    db.run("CREATE (:Person:Employee {name: 'Alice'})");
    let rows = db.run("MATCH (n:Person) RETURN labels(n) AS l");
    let labels = rows[0]["l"].as_array().unwrap();
    assert_eq!(labels.len(), 2);
}

// ============================================================
// 16. Type function (relationship type)
// ============================================================

#[test]
fn type_of_relationship() {
    let db = TestDb::new();
    db.run("CREATE (a:A)-[:KNOWS]->(b:B)");
    let rows = db.run("MATCH ()-[r]->() RETURN type(r) AS t");
    assert_eq!(rows[0]["t"], "KNOWS");
}

// ============================================================
// 17. ID function
// ============================================================

#[test]
fn id_of_node_is_integer() {
    let db = TestDb::new();
    db.run("CREATE (:N {name: 'x'})");
    let v = db.scalar("MATCH (n:N) RETURN id(n) AS i");
    assert!(v.is_i64());
}

#[test]
fn id_of_relationship_is_integer() {
    let db = TestDb::new();
    db.run("CREATE (a:A)-[:R]->(b:B)");
    let v = db.scalar("MATCH ()-[r]->() RETURN id(r) AS i");
    assert!(v.is_i64());
}

// ============================================================
// 18. Timestamp function
// ============================================================

#[test]
fn timestamp_returns_positive_integer() {
    let v = TestDb::new().scalar("RETURN timestamp() AS ts");
    assert!(v.as_i64().unwrap() > 0);
}

#[test]
fn timestamp_is_recent() {
    let v = TestDb::new().scalar("RETURN timestamp() AS ts");
    let ts = v.as_i64().unwrap();
    // Should be after 2024-01-01 in milliseconds
    assert!(ts > 1_704_067_200_000);
}

// ============================================================
// 19. Functions on paths
// ============================================================

#[test]
fn length_of_path() {
    let db = TestDb::new();
    db.run("CREATE (:A {id:1})-[:R]->(:B {id:2})");
    let v = db.scalar(
        "MATCH p = (:A)-[:R]->(:B) RETURN length(p) AS len",
    );
    assert_eq!(v, 1);
}

#[test]
fn nodes_of_path() {
    let db = TestDb::new();
    db.run("CREATE (:A {id:1})-[:R]->(:B {id:2})");
    let rows = db.run("MATCH p = (:A)-[:R]->(:B) RETURN nodes(p) AS ns");
    let ns = rows[0]["ns"].as_array().unwrap();
    assert_eq!(ns.len(), 2);
}

#[test]
fn relationships_of_path() {
    let db = TestDb::new();
    db.run("CREATE (:A {id:1})-[:R {w:1}]->(:B {id:2})");
    let rows = db.run(
        "MATCH p = (:A)-[:R]->(:B) RETURN relationships(p) AS rels",
    );
    let rels = rows[0]["rels"].as_array().unwrap();
    assert_eq!(rels.len(), 1);
}

// ============================================================
// 20. Aggregation functions — edge cases
// ============================================================

#[test]
fn count_star_empty_graph() {
    let v = TestDb::new().scalar("MATCH (n:Nothing) RETURN count(*) AS c");
    assert_eq!(v, 0);
}

#[test]
fn sum_empty_set() {
    let v = TestDb::new().scalar("MATCH (n:Nothing) RETURN sum(n.val) AS s");
    // sum of empty set is 0 or null — depends on implementation
    // Lora returns 0 for sum of empty
    assert!(v == 0 || v.is_null());
}

#[test]
fn avg_empty_set() {
    let v = TestDb::new().scalar("MATCH (n:Nothing) RETURN avg(n.val) AS a");
    assert!(v.is_null());
}

#[test]
fn min_empty_set() {
    let v = TestDb::new().scalar("MATCH (n:Nothing) RETURN min(n.val) AS m");
    assert!(v.is_null());
}

#[test]
fn max_empty_set() {
    let v = TestDb::new().scalar("MATCH (n:Nothing) RETURN max(n.val) AS m");
    assert!(v.is_null());
}

#[test]
fn collect_empty_set() {
    let v = TestDb::new().scalar("MATCH (n:Nothing) RETURN collect(n.val) AS c");
    assert_eq!(v.as_array().unwrap().len(), 0);
}

#[test]
fn count_with_nulls() {
    let db = TestDb::new();
    db.run("CREATE (:Item {val: 1})");
    db.run("CREATE (:Item {val: 2})");
    db.run("CREATE (:Item)"); // no val property -> null
    let v = db.scalar("MATCH (i:Item) RETURN count(i.val) AS c");
    // count excludes nulls
    assert_eq!(v, 2);
}

#[test]
fn count_star_includes_nulls() {
    let db = TestDb::new();
    db.run("CREATE (:Item {val: 1})");
    db.run("CREATE (:Item)");
    let v = db.scalar("MATCH (i:Item) RETURN count(*) AS c");
    assert_eq!(v, 2);
}

// ============================================================
// 21. Statistical aggregations
// ============================================================

#[test]
fn stdev_basic() {
    let db = TestDb::new();
    for v in [2, 4, 4, 4, 5, 5, 7, 9] {
        db.run(&format!("CREATE (:D {{val: {v}}})"));
    }
    let v = db.scalar("MATCH (d:D) RETURN stdev(d.val) AS s");
    let s = v.as_f64().unwrap();
    // Sample stdev of [2,4,4,4,5,5,7,9] ~ 2.0
    assert!(s > 1.5 && s < 2.5);
}

#[test]
fn percentile_cont_median() {
    let db = TestDb::new();
    for v in [1, 2, 3, 4, 5] {
        db.run(&format!("CREATE (:D {{val: {v}}})"));
    }
    let v = db.scalar("MATCH (d:D) RETURN percentileCont(d.val, 0.5) AS p");
    let p = v.as_f64().unwrap();
    assert!((p - 3.0).abs() < 0.001);
}

// ============================================================
// 22. Missing string functions (future)
// ============================================================

#[test]
fn lpad_function() {
    // Lora: lpad(string, length, padChar)
    let v = TestDb::new().scalar("RETURN lpad('42', 5, '0')");
    assert_eq!(v, "00042");
}

#[test]
fn rpad_function() {
    let v = TestDb::new().scalar("RETURN rpad('hi', 5, '.')");
    assert_eq!(v, "hi...");
}

#[test]
fn char_length_function() {
    // Lora: char_length handles unicode properly
    let v = TestDb::new().scalar("RETURN char_length('hello')");
    assert_eq!(v, 5);
}

#[test]
fn normalize_function() {
    // Lora: normalize(string) — Unicode normalization
    let v = TestDb::new().scalar("RETURN normalize('hello')");
    assert_eq!(v, "hello");
}

// ============================================================
// 23. Missing list functions (future)
// ============================================================

#[test]
fn last_function() {
    // Lora: last(list) — like head but for the end
    let v = TestDb::new().scalar("RETURN last([1, 2, 3])");
    assert_eq!(v, 3);
}

#[test]
fn last_of_empty_list_is_null() {
    assert!(TestDb::new().scalar("RETURN last([])").is_null());
}

#[test]
fn list_contains_function() {
    // Lora doesn't have a contains() for lists, but CONTAINS for strings
    // This tests potential future list.contains()
    let v = TestDb::new().scalar("RETURN [1, 2, 3] CONTAINS 2");
    assert_eq!(v, true);
}

// ============================================================
// 24. Missing numeric functions (future)
// ============================================================

#[test]
fn log_function() {
    let v = TestDb::new().scalar("RETURN log(2.718281828)").as_f64().unwrap();
    assert!((v - 1.0).abs() < 0.01);
}

#[test]
fn log10_function() {
    let v = TestDb::new().scalar("RETURN log10(100)").as_f64().unwrap();
    assert!((v - 2.0).abs() < 0.01);
}

#[test]
fn exp_function() {
    let v = TestDb::new().scalar("RETURN exp(1)").as_f64().unwrap();
    assert!((v - 2.718).abs() < 0.01);
}

#[test]
fn sin_function() {
    let v = TestDb::new().scalar("RETURN sin(0)").as_f64().unwrap();
    assert!(v.abs() < 0.001);
}

#[test]
fn cos_function() {
    let v = TestDb::new().scalar("RETURN cos(0)").as_f64().unwrap();
    assert!((v - 1.0).abs() < 0.001);
}

#[test]
fn pi_function() {
    let v = TestDb::new().scalar("RETURN pi()").as_f64().unwrap();
    assert!((v - std::f64::consts::PI).abs() < 0.0001);
}

#[test]
fn e_function() {
    let v = TestDb::new().scalar("RETURN e()").as_f64().unwrap();
    assert!((v - std::f64::consts::E).abs() < 0.0001);
}

#[test]
fn rand_function() {
    // rand() returns a float between 0.0 (inclusive) and 1.0 (exclusive)
    let v = TestDb::new().scalar("RETURN rand()").as_f64().unwrap();
    assert!(v >= 0.0 && v < 1.0);
}

// ============================================================
// 25. Function composition
// ============================================================

#[test]
fn nested_tolower_replace() {
    assert_eq!(
        TestDb::new().scalar("RETURN toLower(replace('Hello World', ' ', '_'))"),
        "hello_world"
    );
}

#[test]
fn nested_tostring_tointeger() {
    assert_eq!(
        TestDb::new().scalar("RETURN toInteger(toString(42))"),
        42
    );
}

#[test]
fn size_of_split_result() {
    assert_eq!(
        TestDb::new().scalar("RETURN size(split('a.b.c', '.'))"),
        3
    );
}

#[test]
fn head_of_split() {
    assert_eq!(
        TestDb::new().scalar("RETURN head(split('hello.world', '.'))"),
        "hello"
    );
}

// ============================================================
// 26. Functions with graph data
// ============================================================

#[test]
fn tolower_on_node_property() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'ALICE'})");
    let rows = db.run("MATCH (u:User) RETURN toLower(u.name) AS name");
    assert_eq!(rows[0]["name"], "alice");
}

#[test]
fn size_of_node_string_property() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    let rows = db.run("MATCH (u:User) RETURN size(u.name) AS len");
    assert_eq!(rows[0]["len"], 5);
}

#[test]
fn string_functions_in_where_clause() {
    let db = TestDb::new();
    db.run("CREATE (:User {name: 'Alice'})");
    db.run("CREATE (:User {name: 'BOB'})");
    let rows = db.run(
        "MATCH (u:User) WHERE toLower(u.name) STARTS WITH 'a' RETURN u.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

#[test]
fn string_concat_in_return() {
    let db = TestDb::new();
    db.run("CREATE (:Person {first: 'John', last: 'Doe'})");
    let rows = db.run(
        "MATCH (p:Person) RETURN p.first + ' ' + p.last AS fullName",
    );
    assert_eq!(rows[0]["fullName"], "John Doe");
}

// ============================================================
// 27. CASE with functions
// ============================================================

#[test]
fn case_with_size_function() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'A', tags: ['x']})");
    db.run("CREATE (:Item {name: 'B', tags: ['x', 'y', 'z']})");
    let rows = db.run(
        "MATCH (i:Item) \
         RETURN i.name AS name, \
                CASE WHEN size(i.tags) > 2 THEN 'many' ELSE 'few' END AS count \
         ORDER BY i.name",
    );
    assert_eq!(rows[0]["count"], "few");
    assert_eq!(rows[1]["count"], "many");
}

// ============================================================
// 28. Expression chaining with WITH
// ============================================================

#[test]
fn with_computed_values_reused() {
    let rows = TestDb::new().run(
        "WITH 'hello world' AS text \
         WITH text, size(text) AS len, toUpper(text) AS upper \
         RETURN len, upper",
    );
    assert_eq!(rows[0]["len"], 11);
    assert_eq!(rows[0]["upper"], "HELLO WORLD");
}
