/// ORDER BY, SKIP, LIMIT, and DISTINCT tests — sort direction, multi-key
/// ordering, null ordering, pagination patterns, distinct deduplication.
mod test_helpers;
use test_helpers::TestDb;

fn db_with_users() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice', age: 30})");
    db.run("CREATE (b:User {name: 'Bob', age: 25})");
    db.run("CREATE (c:User {name: 'Carol', age: 35})");
    db.run("CREATE (d:User {name: 'Dave', age: 25})");
    db
}

// ============================================================
// ORDER BY
// ============================================================

#[test]
fn order_by_property_ascending() {
    let db = db_with_users();
    let names = db.column(
        "MATCH (n:User) RETURN n.name AS name ORDER BY n.name ASC",
        "name",
    );
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s, vec!["Alice", "Bob", "Carol", "Dave"]);
}

#[test]
fn order_by_property_descending() {
    let db = db_with_users();
    let names = db.column(
        "MATCH (n:User) RETURN n.name AS name ORDER BY n.name DESC",
        "name",
    );
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s, vec!["Dave", "Carol", "Bob", "Alice"]);
}

#[test]
fn order_by_numeric_ascending() {
    let db = db_with_users();
    let ages = db.column(
        "MATCH (n:User) RETURN n.age AS age ORDER BY n.age ASC",
        "age",
    );
    let nums: Vec<i64> = ages.iter().map(|v| v.as_i64().unwrap()).collect();
    assert_eq!(nums, vec![25, 25, 30, 35]);
}

#[test]
fn order_by_default_is_ascending() {
    let db = db_with_users();
    let names = db.column("MATCH (n:User) RETURN n.name AS name ORDER BY n.name", "name");
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s, vec!["Alice", "Bob", "Carol", "Dave"]);
}

#[test]
fn order_by_multiple_keys() {
    let db = db_with_users();
    let rows = db.run(
        "MATCH (n:User) RETURN n.name AS name, n.age AS age ORDER BY n.age ASC, n.name DESC",
    );
    let names: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Dave", "Bob", "Alice", "Carol"]);
}

#[test]
fn order_desc_strings() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.column(
        "MATCH (p:Person) RETURN p.name AS name ORDER BY p.name DESC",
        "name",
    );
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s, vec!["Frank", "Eve", "Dave", "Carol", "Bob", "Alice"]);
}

#[test]
fn order_by_computed_expression() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[r:WORKS_AT]->(c:Company) \
         RETURN p.name AS name, r.since AS since ORDER BY r.since ASC",
    );
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0]["name"], "Frank"); // 2012
    assert_eq!(rows[5]["name"], "Eve");   // 2022
}

// ============================================================
// Null ordering
// ============================================================

#[test]
fn order_by_null_values_sort_last_ascending() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name:'A', score: 10})");
    db.run("CREATE (:Item {name:'B', score: 20})");
    db.run("CREATE (:Item {name:'C'})");
    let names = db.column("MATCH (i:Item) RETURN i.name AS name ORDER BY i.score ASC", "name");
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s, vec!["A", "B", "C"]);
}

#[test]
fn order_by_null_values_sort_first_descending() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name:'A', score: 10})");
    db.run("CREATE (:Item {name:'B', score: 20})");
    db.run("CREATE (:Item {name:'C'})");
    let names = db.column("MATCH (i:Item) RETURN i.name AS name ORDER BY i.score DESC", "name");
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s, vec!["C", "B", "A"]);
}

// ============================================================
// LIMIT
// ============================================================

#[test]
fn limit_restricts_rows() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) RETURN n LIMIT 2", 2);
}

#[test]
fn limit_zero() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) RETURN n LIMIT 0", 0);
}

#[test]
fn limit_exceeds_result_count() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) RETURN n LIMIT 100", 4);
}

// ============================================================
// SKIP
// ============================================================

#[test]
fn skip_rows() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) RETURN n SKIP 2", 2);
}

#[test]
fn skip_all_rows() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) RETURN n SKIP 10", 0);
}

#[test]
fn skip_zero() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) RETURN n SKIP 0", 4);
}

#[test]
fn skip_past_all_results() {
    let db = TestDb::new();
    db.seed_org_graph();
    db.assert_count("MATCH (p:Person) RETURN p.name AS name SKIP 100", 0);
}

// ============================================================
// SKIP + LIMIT
// ============================================================

#[test]
fn skip_and_limit_combined() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) RETURN n SKIP 1 LIMIT 2", 2);
}

#[test]
fn order_skip_limit_pipeline() {
    let db = db_with_users();
    let names = db.column(
        "MATCH (n:User) RETURN n.name AS name ORDER BY n.name ASC SKIP 1 LIMIT 2",
        "name",
    );
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s, vec!["Bob", "Carol"]);
}

// ============================================================
// Pagination patterns
// ============================================================

#[test]
fn paginate_page_1() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.column(
        "MATCH (p:Person) RETURN p.name AS name ORDER BY p.name ASC LIMIT 3",
        "name",
    );
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s, vec!["Alice", "Bob", "Carol"]);
}

#[test]
fn paginate_page_2() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.column(
        "MATCH (p:Person) RETURN p.name AS name ORDER BY p.name ASC SKIP 3 LIMIT 3",
        "name",
    );
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s, vec!["Dave", "Eve", "Frank"]);
}

// ============================================================
// DISTINCT
// ============================================================

#[test]
fn distinct_removes_duplicates() {
    let db = db_with_users();
    let rows = db.run("MATCH (n:User) RETURN DISTINCT n.age AS age");
    assert_eq!(rows.len(), 3);
}

#[test]
fn distinct_on_all_same() {
    let db = TestDb::new();
    db.run("CREATE (a:User {age: 30})");
    db.run("CREATE (b:User {age: 30})");
    db.run("CREATE (c:User {age: 30})");
    let rows = db.run("MATCH (n:User) RETURN DISTINCT n.age AS age");
    assert_eq!(rows.len(), 1);
}

#[test]
fn distinct_on_all_different() {
    let db = db_with_users();
    let rows = db.run("MATCH (n:User) RETURN DISTINCT n.name AS name");
    assert_eq!(rows.len(), 4);
}

#[test]
fn distinct_on_department() {
    let db = TestDb::new();
    db.seed_org_graph();
    let depts = db.sorted_strings("MATCH (p:Person) RETURN DISTINCT p.dept AS dept", "dept");
    assert_eq!(depts, vec!["Engineering", "Marketing"]);
}

#[test]
fn distinct_preserves_unique_combos() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) RETURN DISTINCT p.dept AS dept, c.name AS city",
    );
    assert_eq!(rows.len(), 4);
}

// ============================================================
// ORDER BY + aggregation
// ============================================================

#[test]
fn order_aggregation_results() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         RETURN c.name AS city, count(p) AS residents ORDER BY c.name ASC",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["city"], "Berlin");
    assert_eq!(rows[0]["residents"], 2);
    assert_eq!(rows[1]["city"], "London");
    assert_eq!(rows[1]["residents"], 3);
}

#[test]
fn limit_on_aggregation_via_with() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         WITH c.name AS city, count(p) AS n \
         RETURN city, n ORDER BY city ASC LIMIT 2",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["city"], "Berlin");
    assert_eq!(rows[1]["city"], "London");
}

// ============================================================
// Pending: ORDER BY alias
// ============================================================

#[test]
fn order_by_alias_name() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN p.name AS name ORDER BY name ASC LIMIT 3",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], "Alice");
}

// ============================================================
// DISTINCT interaction with ORDER BY
// ============================================================

#[test]
fn distinct_with_order_by() {
    let db = TestDb::new();
    db.seed_org_graph();
    let depts = db.column(
        "MATCH (p:Person) RETURN DISTINCT p.dept AS dept ORDER BY p.dept ASC",
        "dept",
    );
    let s: Vec<&str> = depts.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s, vec!["Engineering", "Marketing"]);
}

#[test]
fn distinct_with_limit() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run("MATCH (p:Person) RETURN DISTINCT p.dept AS dept LIMIT 1");
    assert_eq!(rows.len(), 1);
}

// ============================================================
// Float and boolean ordering
// ============================================================

#[test]
fn order_by_float_values() {
    let db = TestDb::new();
    db.run("CREATE (:M {name:'a', val: 3.14})");
    db.run("CREATE (:M {name:'b', val: 2.71})");
    db.run("CREATE (:M {name:'c', val: 1.41})");
    let names = db.column("MATCH (m:M) RETURN m.name AS name ORDER BY m.val ASC", "name");
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s, vec!["c", "b", "a"]);
}

#[test]
fn order_by_boolean_values() {
    let db = TestDb::new();
    db.run("CREATE (:B {name:'a', flag: false})");
    db.run("CREATE (:B {name:'b', flag: true})");
    db.run("CREATE (:B {name:'c', flag: false})");
    let names = db.column("MATCH (b:B) RETURN b.name AS name ORDER BY b.flag ASC, b.name ASC", "name");
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    // false sorts before true
    assert_eq!(s[0], "a");
    assert_eq!(s[1], "c");
    assert_eq!(s[2], "b");
}

// ============================================================
// Multiple mixed sort directions
// ============================================================

#[test]
fn order_by_mixed_asc_desc() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN p.dept AS dept, p.name AS name \
         ORDER BY p.dept ASC, p.name DESC",
    );
    // Engineering sorted desc by name: Frank, Eve, Bob, Alice
    assert_eq!(rows[0]["dept"], "Engineering");
    assert_eq!(rows[0]["name"], "Frank");
    assert_eq!(rows[3]["name"], "Alice");
    // Marketing sorted desc by name: Dave, Carol
    assert_eq!(rows[4]["name"], "Dave");
    assert_eq!(rows[5]["name"], "Carol");
}

// ============================================================
// Pagination edge cases
// ============================================================

#[test]
fn skip_and_limit_yield_single_row() {
    let db = db_with_users();
    let rows = db.run("MATCH (n:User) RETURN n.name AS name ORDER BY n.name ASC SKIP 2 LIMIT 1");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Carol");
}

#[test]
fn skip_equals_total_returns_zero() {
    let db = db_with_users();
    db.assert_count("MATCH (n:User) RETURN n SKIP 4", 0);
}

#[test]
fn order_by_single_row() {
    let db = TestDb::new();
    db.run("CREATE (:Solo {val: 42})");
    let rows = db.run("MATCH (n:Solo) RETURN n.val AS val ORDER BY n.val ASC");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["val"], 42);
}

// ============================================================
// ORDER BY after aggregation
// ============================================================

#[test]
fn order_by_aggregated_value() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         WITH p.dept AS dept, count(p) AS cnt \
         RETURN dept, cnt ORDER BY cnt DESC",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["dept"], "Engineering");
    assert_eq!(rows[0]["cnt"], 4);
    assert_eq!(rows[1]["dept"], "Marketing");
    assert_eq!(rows[1]["cnt"], 2);
}

// ============================================================
// DISTINCT with multiple columns
// ============================================================

#[test]
fn distinct_multiple_columns_all_unique() {
    let db = TestDb::new();
    db.run("CREATE (:P {x:1, y:'a'})");
    db.run("CREATE (:P {x:1, y:'b'})");
    db.run("CREATE (:P {x:2, y:'a'})");
    let rows = db.run("MATCH (p:P) RETURN DISTINCT p.x AS x, p.y AS y");
    assert_eq!(rows.len(), 3);
}

#[test]
fn distinct_multiple_columns_with_duplicates() {
    let db = TestDb::new();
    db.run("CREATE (:P {x:1, y:'a'})");
    db.run("CREATE (:P {x:1, y:'a'})");
    db.run("CREATE (:P {x:2, y:'b'})");
    let rows = db.run("MATCH (p:P) RETURN DISTINCT p.x AS x, p.y AS y");
    assert_eq!(rows.len(), 2);
}

// ============================================================
// Complex ordering
// ============================================================

#[test]
fn order_by_arithmetic_expression() {
    // Order by computed expression (age * -1 reverses the sort)
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN p.name AS name, p.age AS age ORDER BY p.age * -1",
    );
    // Sorted by age descending (via *-1 trick): Frank(50), Carol(42), Alice(35), Dave(31), Bob(28), Eve(26)
    assert_eq!(rows[0]["name"], "Frank");
    assert_eq!(rows[0]["age"], 50);
    assert_eq!(rows[5]["name"], "Eve");
    assert_eq!(rows[5]["age"], 26);
}

#[test]
fn order_by_aggregate_output_after_with() {
    // Order by aggregate output (after WITH)
    let db = TestDb::new();
    db.seed_recommendation_graph();
    let rows = db.run(
        "MATCH (v:Viewer)-[r:RATED]->(m:Movie) \
         WITH v.name AS viewer, avg(r.score) AS avg_score \
         RETURN viewer, avg_score ORDER BY avg_score DESC",
    );
    // Alice: (5+4+3)/3=4.0, Bob: (5+2)/2=3.5, Carol: (4+5)/2=4.5
    assert_eq!(rows[0]["viewer"], "Carol");
    assert_eq!(rows[2]["viewer"], "Bob");
}

#[test]
fn order_by_multi_key_three_keys() {
    // Multi-key ordering with three keys
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run(
        "MATCH (p:Person)-[r:INTERESTED_IN]->(i:Interest) \
         RETURN p.city AS city, r.level AS level, p.name AS name \
         ORDER BY p.city ASC, r.level ASC, p.name ASC",
    );
    // Berlin city comes first alphabetically
    assert_eq!(rows[0]["city"], "Berlin");
    assert!(rows.len() >= 6);
}

#[test]
fn order_by_null_handling_mixed() {
    // NULL handling in ORDER BY (mixed null and non-null)
    let db = TestDb::new();
    db.run("CREATE (:T {name:'A', rank: 3})");
    db.run("CREATE (:T {name:'B'})");
    db.run("CREATE (:T {name:'C', rank: 1})");
    db.run("CREATE (:T {name:'D'})");
    db.run("CREATE (:T {name:'E', rank: 2})");
    let names = db.column(
        "MATCH (t:T) RETURN t.name AS name ORDER BY t.rank ASC",
        "name",
    );
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    // Non-null values sorted first (1, 2, 3), then nulls last
    assert_eq!(s[0], "C"); // rank 1
    assert_eq!(s[1], "E"); // rank 2
    assert_eq!(s[2], "A"); // rank 3
    // B and D have null rank, should be last
    assert!(s[3] == "B" || s[3] == "D");
    assert!(s[4] == "B" || s[4] == "D");
}

#[test]
fn order_after_distinct() {
    // Order after DISTINCT
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         RETURN DISTINCT c.name AS city ORDER BY c.name DESC",
    );
    let cities: Vec<&str> = rows.iter().map(|r| r["city"].as_str().unwrap()).collect();
    assert_eq!(cities, vec!["Tokyo", "London", "Berlin"]);
}

// ============================================================
// SKIP and LIMIT edge cases
// ============================================================

#[test]
fn skip_greater_than_row_count_returns_empty() {
    // SKIP >= row count returns empty
    let db = TestDb::new();
    db.seed_org_graph();
    db.assert_count("MATCH (p:Person) RETURN p.name AS name SKIP 20", 0);
}

#[test]
fn limit_zero_returns_empty() {
    // LIMIT 0 returns empty
    let db = TestDb::new();
    db.seed_org_graph();
    db.assert_count("MATCH (p:Person) RETURN p.name AS name LIMIT 0", 0);
}

#[test]
fn skip_zero_same_as_no_skip() {
    // SKIP 0 same as no skip
    let db = TestDb::new();
    db.seed_org_graph();
    let with_skip = db.run(
        "MATCH (p:Person) RETURN p.name AS name ORDER BY p.name ASC SKIP 0",
    );
    let without_skip = db.run(
        "MATCH (p:Person) RETURN p.name AS name ORDER BY p.name ASC",
    );
    assert_eq!(with_skip.len(), without_skip.len());
    assert_eq!(with_skip[0]["name"], without_skip[0]["name"]);
}

#[test]
fn skip_limit_pagination_through_results() {
    // SKIP + LIMIT pagination: page through all 6 persons in pages of 2
    let db = TestDb::new();
    db.seed_org_graph();
    let page1 = db.column(
        "MATCH (p:Person) RETURN p.name AS name ORDER BY p.name ASC SKIP 0 LIMIT 2",
        "name",
    );
    let page2 = db.column(
        "MATCH (p:Person) RETURN p.name AS name ORDER BY p.name ASC SKIP 2 LIMIT 2",
        "name",
    );
    let page3 = db.column(
        "MATCH (p:Person) RETURN p.name AS name ORDER BY p.name ASC SKIP 4 LIMIT 2",
        "name",
    );
    assert_eq!(page1.len(), 2);
    assert_eq!(page2.len(), 2);
    assert_eq!(page3.len(), 2);
    assert_eq!(page1[0], "Alice");
    assert_eq!(page1[1], "Bob");
    assert_eq!(page2[0], "Carol");
    assert_eq!(page2[1], "Dave");
    assert_eq!(page3[0], "Eve");
    assert_eq!(page3[1], "Frank");
}

#[test]
fn skip_limit_past_end_returns_partial() {
    // SKIP + LIMIT where LIMIT extends past end of results
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN p.name AS name ORDER BY p.name ASC SKIP 4 LIMIT 10",
    );
    // Only 2 remaining after skip 4 (Eve, Frank)
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "Eve");
    assert_eq!(rows[1]["name"], "Frank");
}

#[test]
fn order_by_alias_from_projection() {
    // Lora: ORDER BY alias from projection
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN p.name AS person_name ORDER BY person_name ASC LIMIT 3",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["person_name"], "Alice");
    assert_eq!(rows[1]["person_name"], "Bob");
    assert_eq!(rows[2]["person_name"], "Carol");
}

#[test]
fn stable_sort_guarantee() {
    // Lora: stable sort guarantee
    let db = TestDb::new();
    db.run("CREATE (:S {group: 1, name: 'A'})");
    db.run("CREATE (:S {group: 1, name: 'B'})");
    db.run("CREATE (:S {group: 1, name: 'C'})");
    db.run("CREATE (:S {group: 2, name: 'D'})");
    // When sorting by group only, items within same group should preserve insertion order
    let rows = db.run(
        "MATCH (s:S) RETURN s.group AS g, s.name AS name ORDER BY s.group ASC",
    );
    assert_eq!(rows.len(), 4);
    // Within group 1, order should be stable: A, B, C
    assert_eq!(rows[0]["name"], "A");
    assert_eq!(rows[1]["name"], "B");
    assert_eq!(rows[2]["name"], "C");
}

#[test]
fn order_by_aggregate_in_return() {
    // Lora: ORDER BY on aggregate in RETURN (without WITH)
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         RETURN p.dept AS dept, count(p) AS cnt ORDER BY count(p) DESC",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["dept"], "Engineering");
    assert_eq!(rows[0]["cnt"], 4);
}

// ============================================================
// ORDER BY with CASE expression
// ============================================================

#[test]
fn order_by_case_expression() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Custom sort: Marketing first, then Engineering
    let rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name AS name, p.dept AS dept \
         ORDER BY CASE p.dept WHEN 'Marketing' THEN 0 ELSE 1 END, p.name ASC",
    );
    // First 2 should be Marketing (Carol, Dave), then Engineering
    assert_eq!(rows[0]["dept"], "Marketing");
    assert_eq!(rows[1]["dept"], "Marketing");
    assert_eq!(rows[2]["dept"], "Engineering");
}

// ============================================================
// DISTINCT with ORDER BY and null interactions
// ============================================================

#[test]
fn distinct_with_nulls_ordered() {
    let db = TestDb::new();
    db.run("CREATE (:V {x: 2})");
    db.run("CREATE (:V {x: 1})");
    db.run("CREATE (:V {x: 3})");
    db.run("CREATE (:V {x: 2})"); // duplicate
    // DISTINCT should deduplicate to 3 distinct values
    let rows = db.run("MATCH (v:V) RETURN DISTINCT v.x AS x");
    assert_eq!(rows.len(), 3);
    let mut vals: Vec<i64> = rows.iter().map(|r| r["x"].as_i64().unwrap()).collect();
    vals.sort();
    assert_eq!(vals, vec![1, 2, 3]);
}

// ============================================================
// ORDER BY with string function
// ============================================================

#[test]
fn order_by_string_function() {
    let db = TestDb::new();
    db.run("CREATE (:W {name: 'Charlie'})");
    db.run("CREATE (:W {name: 'alice'})");
    db.run("CREATE (:W {name: 'Bob'})");
    let rows = db.run(
        "MATCH (w:W) RETURN w.name AS name ORDER BY toLower(w.name) ASC",
    );
    // Alphabetical case-insensitive: alice, Bob, Charlie
    assert_eq!(rows[0]["name"], "alice");
    assert_eq!(rows[1]["name"], "Bob");
    assert_eq!(rows[2]["name"], "Charlie");
}

// ============================================================
// Pagination over relationships
// ============================================================

#[test]
fn paginate_relationships_with_order() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Get the 2nd and 3rd strongest KNOWS relationships
    let rows = db.run(
        "MATCH (a:Person)-[k:KNOWS]->(b:Person) \
         RETURN a.name AS from, b.name AS to, k.strength AS str \
         ORDER BY k.strength DESC SKIP 1 LIMIT 2",
    );
    assert_eq!(rows.len(), 2);
    // Strengths: 8,7,6,5,4,3,2 => skip 8, get 7 and 6
    assert_eq!(rows[0]["str"], 7);
    assert_eq!(rows[1]["str"], 6);
}

// ============================================================
// Top-N per group (via WITH pipeline)
// ============================================================

#[test]
fn top_n_youngest_per_department() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Youngest person per department using min(age) and filtering
    let rows = db.run(
        "MATCH (p:Person) \
         RETURN p.dept AS dept, min(p.age) AS youngest_age \
         ORDER BY dept",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["dept"], "Engineering");
    assert_eq!(rows[0]["youngest_age"], 26); // Eve
    assert_eq!(rows[1]["dept"], "Marketing");
    assert_eq!(rows[1]["youngest_age"], 31); // Dave
}

// ============================================================
// DISTINCT on multiple expressions including computed
// ============================================================

#[test]
fn distinct_on_computed_expression() {
    let db = TestDb::new();
    db.seed_org_graph();
    // DISTINCT on age bracket computed from age
    let rows = db.run(
        "MATCH (p:Person) \
         RETURN DISTINCT CASE \
            WHEN p.age < 30 THEN 'under 30' \
            WHEN p.age < 40 THEN '30-39' \
            ELSE '40+' END AS bracket \
         ORDER BY bracket",
    );
    // under 30: Eve(26), Bob(28); 30-39: Alice(35), Dave(31); 40+: Carol(42), Frank(50)
    assert_eq!(rows.len(), 3);
}

// ============================================================
// Ignored ordering tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn order_by_subquery_count() {
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name AS name \
         ORDER BY COUNT { MATCH (p)-[:MANAGES]->() } DESC",
    );
}
