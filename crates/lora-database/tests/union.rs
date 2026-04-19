/// UNION and UNION ALL tests — deduplication, branch combination, empty
/// branches, multiple branches, type mixing, aggregation in branches.
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// UNION (with deduplication)
// ============================================================

#[test]
fn union_combines_results() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:Product {name: 'Widget'})");
    let rows = db
        .run("MATCH (n:User) RETURN n.name AS name UNION MATCH (n:Product) RETURN n.name AS name");
    assert_eq!(rows.len(), 2);
}

#[test]
fn union_deduplicates_identical_scalar_rows() {
    let db = TestDb::new();
    db.run("CREATE (:Item {val: 1, grp: 'a'})");
    db.run("CREATE (:Item {val: 1, grp: 'b'})");
    let rows = db.run(
        "MATCH (n:Item {grp:'a'}) RETURN n.val AS v \
         UNION \
         MATCH (n:Item {grp:'b'}) RETURN n.val AS v",
    );
    assert_eq!(rows.len(), 1);
}

#[test]
fn union_combines_different_types() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (p:Person) RETURN p.name AS name \
         UNION MATCH (c:City) RETURN c.name AS name",
        "name",
    );
    assert_eq!(names.len(), 9); // 6 persons + 3 cities
}

// ============================================================
// UNION ALL (no deduplication)
// ============================================================

#[test]
fn union_all_preserves_duplicates() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Alice'})");
    let rows = db
        .run("MATCH (n:User) RETURN n.name AS name UNION ALL MATCH (n:User) RETURN n.name AS name");
    assert_eq!(rows.len(), 4);
}

#[test]
fn union_all_keeps_all_rows() {
    let db = TestDb::new();
    db.run("CREATE (:A {val: 1})");
    db.run("CREATE (:B {val: 1})");
    let rows = db.run("MATCH (n:A) RETURN n.val AS v UNION ALL MATCH (n:B) RETURN n.val AS v");
    assert_eq!(rows.len(), 2);
}

#[test]
fn union_all_preserves_branch_order() {
    let db = TestDb::new();
    db.run("CREATE (:A {v:1})");
    db.run("CREATE (:A {v:2})");
    db.run("CREATE (:B {v:3})");
    db.run("CREATE (:B {v:4})");
    let rows = db.run("MATCH (n:A) RETURN n.v AS v UNION ALL MATCH (n:B) RETURN n.v AS v");
    assert_eq!(rows.len(), 4);
    let vals: Vec<i64> = rows.iter().map(|r| r["v"].as_i64().unwrap()).collect();
    assert!(vals[0] <= 2 && vals[1] <= 2);
    assert!(vals[2] >= 3 && vals[3] >= 3);
}

// ============================================================
// Multiple branches
// ============================================================

#[test]
fn union_three_branches() {
    let db = TestDb::new();
    db.run("CREATE (:X {name:'a'})");
    db.run("CREATE (:Y {name:'b'})");
    db.run("CREATE (:Z {name:'c'})");
    let names = db.sorted_strings(
        "MATCH (n:X) RETURN n.name AS name \
         UNION MATCH (n:Y) RETURN n.name AS name \
         UNION MATCH (n:Z) RETURN n.name AS name",
        "name",
    );
    assert_eq!(names, vec!["a", "b", "c"]);
}

// ============================================================
// Empty branches
// ============================================================

#[test]
fn union_one_empty_branch() {
    let db = TestDb::new();
    let rows =
        db.run("MATCH (n:X) RETURN n.name AS name UNION ALL MATCH (n:Y) RETURN n.name AS name");
    assert_eq!(rows.len(), 0);
}

#[test]
fn union_one_populated_one_empty() {
    let db = TestDb::new();
    db.run("CREATE (:X {name:'exists'})");
    db.run("CREATE (:Y {name:'also'})");
    let rows = db.run(
        "MATCH (n:X) RETURN n.name AS name \
         UNION ALL \
         MATCH (n:Y) WHERE n.name = 'nope' RETURN n.name AS name",
    );
    assert_eq!(rows.len(), 1);
}

#[test]
fn union_both_empty() {
    let db = TestDb::new();
    let rows = db.run("MATCH (a:Nothing) RETURN a.x AS v UNION ALL MATCH (b:Nada) RETURN b.x AS v");
    assert_eq!(rows.len(), 0);
}

// ============================================================
// UNION with aggregation
// ============================================================

#[test]
fn union_with_count_in_each_branch() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN 'persons' AS label, count(p) AS cnt \
         UNION ALL \
         MATCH (c:City) RETURN 'cities' AS label, count(c) AS cnt",
    );
    assert_eq!(rows.len(), 2);
}

// ============================================================
// UNION with WHERE in branches
// ============================================================

#[test]
fn union_with_where_in_each_branch() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (p:Person) WHERE p.dept = 'Engineering' RETURN p.name AS name \
         UNION \
         MATCH (p:Person) WHERE p.dept = 'Marketing' RETURN p.name AS name",
        "name",
    );
    assert_eq!(names.len(), 6);
}

// ============================================================
// UNION ALL with many branches
// ============================================================

#[test]
fn union_all_four_branches() {
    let db = TestDb::new();
    db.run("CREATE (:A {v:1})");
    db.run("CREATE (:B {v:2})");
    db.run("CREATE (:C {v:3})");
    db.run("CREATE (:D {v:4})");
    let rows = db.run(
        "MATCH (n:A) RETURN n.v AS v \
         UNION ALL MATCH (n:B) RETURN n.v AS v \
         UNION ALL MATCH (n:C) RETURN n.v AS v \
         UNION ALL MATCH (n:D) RETURN n.v AS v",
    );
    assert_eq!(rows.len(), 4);
}

// ============================================================
// UNION deduplication with multiple columns
// ============================================================

#[test]
fn union_dedup_multiple_columns() {
    let db = TestDb::new();
    db.run("CREATE (:P {x:1, y:'a'})");
    db.run("CREATE (:Q {x:1, y:'a'})");
    // Both branches return the same (1, 'a') — should dedup to 1 row
    let rows = db.run(
        "MATCH (p:P) RETURN p.x AS x, p.y AS y \
         UNION \
         MATCH (q:Q) RETURN q.x AS x, q.y AS y",
    );
    assert_eq!(rows.len(), 1);
}

#[test]
fn union_keeps_distinct_multi_column_rows() {
    let db = TestDb::new();
    db.run("CREATE (:P {x:1, y:'a'})");
    db.run("CREATE (:Q {x:1, y:'b'})");
    let rows = db.run(
        "MATCH (p:P) RETURN p.x AS x, p.y AS y \
         UNION \
         MATCH (q:Q) RETURN q.x AS x, q.y AS y",
    );
    assert_eq!(rows.len(), 2);
}

// ============================================================
// UNION preserves column naming from first branch
// ============================================================

#[test]
fn union_column_name_from_first_branch() {
    let db = TestDb::new();
    db.run("CREATE (:X {val:1})");
    db.run("CREATE (:Y {val:2})");
    let rows = db.run(
        "MATCH (x:X) RETURN x.val AS result \
         UNION ALL \
         MATCH (y:Y) RETURN y.val AS result",
    );
    // Both should use the column name "result"
    assert!(rows[0].get("result").is_some());
    assert!(rows[1].get("result").is_some());
}

// ============================================================
// UNION: same query in both branches
// ============================================================

#[test]
fn union_same_query_both_branches_deduplicates() {
    let db = TestDb::new();
    db.run("CREATE (:N {v:1})");
    db.run("CREATE (:N {v:2})");
    let rows = db.run(
        "MATCH (n:N) RETURN n.v AS v \
         UNION \
         MATCH (n:N) RETURN n.v AS v",
    );
    assert_eq!(rows.len(), 2); // deduplicates to just {1, 2}
}

#[test]
fn union_all_same_query_both_branches_doubles() {
    let db = TestDb::new();
    db.run("CREATE (:N {v:1})");
    db.run("CREATE (:N {v:2})");
    let rows = db.run(
        "MATCH (n:N) RETURN n.v AS v \
         UNION ALL \
         MATCH (n:N) RETURN n.v AS v",
    );
    assert_eq!(rows.len(), 4); // 2 + 2
}

// ============================================================
// UNION ALL large result set
// ============================================================

#[test]
fn union_all_large_result() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 20) AS i CREATE (:Num {val: i})");
    let rows = db.run(
        "MATCH (n:Num) RETURN n.val AS v \
         UNION ALL \
         MATCH (n:Num) RETURN n.val AS v",
    );
    assert_eq!(rows.len(), 40);
}

// ============================================================
// Advanced UNION patterns
// ============================================================

#[test]
fn union_with_where_filters_in_both_branches() {
    let db = TestDb::new();
    db.seed_social_graph();
    // Alice (age 30) from first branch, Carol (age 35) from second; Bob excluded (age 25)
    let names = db.sorted_strings(
        "MATCH (n:User) WHERE n.age >= 30 RETURN n.name AS name \
         UNION \
         MATCH (n:User) WHERE n.age >= 35 RETURN n.name AS name",
        "name",
    );
    // UNION deduplicates: Carol appears in both branches but only once in result
    assert_eq!(names, vec!["Alice", "Carol"]);
}

#[test]
fn union_all_with_aggregation_in_branches() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) WHERE p.dept = 'Engineering' \
         RETURN p.dept AS dept, count(p) AS cnt \
         UNION ALL \
         MATCH (p:Person) WHERE p.dept = 'Marketing' \
         RETURN p.dept AS dept, count(p) AS cnt",
    );
    assert_eq!(rows.len(), 2);
    // Engineering has 4 (Alice, Bob, Eve, Frank), Marketing has 2 (Carol, Dave)
    let eng = rows.iter().find(|r| r["dept"] == "Engineering").unwrap();
    let mkt = rows.iter().find(|r| r["dept"] == "Marketing").unwrap();
    assert_eq!(eng["cnt"], 4);
    assert_eq!(mkt["cnt"], 2);
}

#[test]
fn union_combining_different_labels() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    // Combine viewer names and movie titles into a single column
    let names = db.sorted_strings(
        "MATCH (v:Viewer) RETURN v.name AS name \
         UNION \
         MATCH (m:Movie) RETURN m.title AS name",
        "name",
    );
    // 3 viewers (Alice, Bob, Carol) + 4 movies (Amelie, Inception, Jaws, Matrix)
    assert_eq!(names.len(), 7);
}

#[test]
fn three_way_union_distinct() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (p:Person) WHERE p.dept = 'Engineering' AND p.age > 30 RETURN p.name AS name \
         UNION \
         MATCH (p:Person) WHERE p.dept = 'Marketing' RETURN p.name AS name \
         UNION \
         MATCH (m:Manager) RETURN m.name AS name",
        "name",
    );
    // Engineering >30: Alice(35), Frank(50). Marketing: Carol, Dave. Manager: Frank.
    // UNION dedup: Alice, Carol, Dave, Frank
    assert_eq!(names, vec!["Alice", "Carol", "Dave", "Frank"]);
}

#[test]
fn union_with_order_by_on_final_result() {
    let db = TestDb::new();
    db.run("CREATE (:X {val: 3})");
    db.run("CREATE (:Y {val: 1})");
    db.run("CREATE (:Z {val: 2})");
    let vals = db.sorted_ints(
        "MATCH (n:X) RETURN n.val AS v \
         UNION ALL \
         MATCH (n:Y) RETURN n.val AS v \
         UNION ALL \
         MATCH (n:Z) RETURN n.val AS v",
        "v",
    );
    assert_eq!(vals, vec![1, 2, 3]);
}

#[test]
fn union_all_with_skip_and_limit_on_final_result() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 5) AS i CREATE (:Num {val: i})");
    db.run("UNWIND range(6, 10) AS i CREATE (:Num2 {val: i})");
    let vals = db.sorted_ints(
        "MATCH (n:Num) RETURN n.val AS v \
         UNION ALL \
         MATCH (n:Num2) RETURN n.val AS v",
        "v",
    );
    // Total 10 values: 1..=5 from Num, 6..=10 from Num2
    assert_eq!(vals.len(), 10);
    assert_eq!(vals[0], 1);
    assert_eq!(vals[9], 10);
}

// ============================================================
// Ignored UNION tests (pending implementation)
// ============================================================

#[test]
fn union_column_count_mismatch_error() {
    // Lora: UNION column count mismatch error
    let db = TestDb::new();
    db.run("CREATE (:A {x:1}), (:B {x:2, y:3})");
    let err = db.run_err(
        "MATCH (a:A) RETURN a.x AS x \
         UNION \
         MATCH (b:B) RETURN b.x AS x, b.y AS y",
    );
    assert!(!err.is_empty());
}

#[test]
fn union_column_name_mismatch_error() {
    // Lora: UNION column name mismatch error
    let db = TestDb::new();
    db.run("CREATE (:A {x:1}), (:B {y:2})");
    let err = db.run_err(
        "MATCH (a:A) RETURN a.x AS foo \
         UNION \
         MATCH (b:B) RETURN b.y AS bar",
    );
    assert!(!err.is_empty());
}

#[test]
fn union_with_with_pipeline_in_branches() {
    // Lora: UNION with WITH pipeline in branches
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City {name:'London'}) \
         WITH p RETURN p.name AS name \
         UNION \
         MATCH (p:Person)-[:LIVES_IN]->(c:City {name:'Berlin'}) \
         WITH p RETURN p.name AS name",
        "name",
    );
    // London: Alice, Carol, Frank; Berlin: Bob, Eve
    assert_eq!(names.len(), 5);
}

#[test]
fn union_with_order_by_on_combined() {
    let db = TestDb::new();
    db.run("CREATE (:A {name: 'Charlie'})");
    db.run("CREATE (:B {name: 'Alice'})");
    let rows = db.run(
        "MATCH (a:A) RETURN a.name AS name \
         UNION ALL \
         MATCH (b:B) RETURN b.name AS name \
         ORDER BY name",
    );
    assert_eq!(rows.len(), 2);
    // After UNION + ORDER BY name, Alice should come first
}

#[test]
fn union_removes_duplicates() {
    let db = TestDb::new();
    db.run("CREATE (:X {v: 1})");
    db.run("CREATE (:Y {v: 1})");
    let rows = db.run(
        "MATCH (x:X) RETURN x.v AS v \
         UNION \
         MATCH (y:Y) RETURN y.v AS v",
    );
    assert_eq!(rows.len(), 1); // UNION deduplicates
    assert_eq!(rows[0]["v"], 1);
}

#[test]
fn union_all_keeps_duplicates() {
    let db = TestDb::new();
    db.run("CREATE (:X {v: 1})");
    db.run("CREATE (:Y {v: 1})");
    let rows = db.run(
        "MATCH (x:X) RETURN x.v AS v \
         UNION ALL \
         MATCH (y:Y) RETURN y.v AS v",
    );
    assert_eq!(rows.len(), 2);
}

#[test]
fn union_three_branches_extended() {
    let db = TestDb::new();
    db.run("CREATE (:A {v: 1})");
    db.run("CREATE (:B {v: 2})");
    db.run("CREATE (:C {v: 3})");
    let rows = db.run(
        "MATCH (a:A) RETURN a.v AS v \
         UNION ALL \
         MATCH (b:B) RETURN b.v AS v \
         UNION ALL \
         MATCH (c:C) RETURN c.v AS v",
    );
    assert_eq!(rows.len(), 3);
}

#[test]
fn union_empty_branch() {
    let db = TestDb::new();
    db.run("CREATE (:Present {v: 42})");
    db.run("CREATE (:Other {v: 99})");
    // Second branch matches nothing (wrong property value)
    let rows = db.run(
        "MATCH (p:Present) RETURN p.v AS v \
         UNION ALL \
         MATCH (n:Other {v: -1}) RETURN n.v AS v",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["v"], 42);
}

#[test]
fn union_mixed_types_in_column() {
    let db = TestDb::new();
    db.run("CREATE (:I {v: 42})");
    db.run("CREATE (:S {v: 'hello'})");
    let rows = db.run(
        "MATCH (i:I) RETURN i.v AS v \
         UNION ALL \
         MATCH (s:S) RETURN s.v AS v",
    );
    assert_eq!(rows.len(), 2);
}

// ============================================================
// UNION with null values
// ============================================================

#[test]
fn union_with_null_in_branch() {
    let db = TestDb::new();
    db.run("CREATE (:A {v: 1})");
    db.run("CREATE (:B {})"); // v is null
    let rows = db.run(
        "MATCH (a:A) RETURN a.v AS v \
         UNION ALL \
         MATCH (b:B) RETURN b.v AS v",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["v"], 1);
    assert!(rows[1]["v"].is_null());
}

// ============================================================
// UNION dedup with null
// ============================================================

#[test]
fn union_dedup_null_values() {
    let db = TestDb::new();
    // Both return the same literal null — UNION should dedup to 1 row
    let rows = db.run(
        "RETURN null AS v \
         UNION \
         RETURN null AS v",
    );
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["v"].is_null());
}

// ============================================================
// UNION with complex aggregation branches
// ============================================================

#[test]
fn union_all_summary_statistics() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN 'people' AS category, count(p) AS cnt \
         UNION ALL \
         MATCH (pr:Project) RETURN 'projects' AS category, count(pr) AS cnt \
         UNION ALL \
         MATCH (c:City) RETURN 'cities' AS category, count(c) AS cnt",
    );
    assert_eq!(rows.len(), 3);
    let people = rows.iter().find(|r| r["category"] == "people").unwrap();
    assert_eq!(people["cnt"], 6);
}

// ============================================================
// Future UNION tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn union_with_limit_on_each_branch() {
    // Per-branch LIMIT before UNION (requires subquery or specific syntax)
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "CALL { MATCH (p:Person) RETURN p.name AS name ORDER BY p.name LIMIT 2 } \
         UNION ALL \
         CALL { MATCH (c:City) RETURN c.name AS name ORDER BY c.name LIMIT 2 }",
    );
}
