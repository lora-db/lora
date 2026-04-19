/// WITH clause tests — variable piping, scoping, aggregation pipelines,
/// multi-part queries, top-N patterns, chained filtering.
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// Basic piping
// ============================================================

#[test]
fn with_passes_variables() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User) WITH a RETURN a");
    assert_eq!(rows.len(), 3);
}

#[test]
fn with_renames_variable() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let rows = db.run("MATCH (n:User) WITH n.name AS name RETURN name");
    assert_eq!(rows[0]["name"], "Alice");
}

#[test]
fn with_property_access_returns_value() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice', age: 30})");
    let rows = db.run("MATCH (n:User) WITH n RETURN n.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

// ============================================================
// Filtering
// ============================================================

#[test]
fn with_filters_trivial_true() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (n:User) WITH n WHERE true RETURN n");
    assert_eq!(rows.len(), 3);
}

#[test]
fn with_filters_by_name() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (n:User) WITH n WHERE n.name = 'Alice' RETURN n");
    assert_eq!(rows.len(), 1);
}

#[test]
fn with_filters() {
    let db = TestDb::new();
    db.seed_social_graph();
    let all = db.run("MATCH (n:User) WITH n RETURN n");
    assert_eq!(all.len(), 3);
    let rows = db.run("MATCH (n:User) WITH n WHERE n.age > 28 RETURN n");
    assert_eq!(rows.len(), 2);
}

// ============================================================
// Variable scoping
// ============================================================

#[test]
fn with_hides_unmentioned_variables() {
    let db = TestDb::new();
    db.seed_social_graph();
    let err = db.run_err("MATCH (a:User {name: 'Alice'})-[r:FOLLOWS]->(b) WITH a RETURN b");
    assert!(err.contains("Unknown variable") || err.contains("variable"));
}

#[test]
fn with_hides_unselected_variables() {
    let db = TestDb::new();
    db.seed_org_graph();
    let err = db.run_err("MATCH (p:Person)-[r:WORKS_AT]->(c:Company) WITH p RETURN r");
    assert!(err.contains("Unknown variable") || err.contains("variable"));
}

#[test]
fn with_star_keeps_all_variables() {
    let db = TestDb::new();
    db.run("CREATE (:A {x:1})-[:R]->(:B {y:2})");
    let rows = db.run("MATCH (a:A)-[r:R]->(b:B) WITH * RETURN a.x AS ax, b.y AS by");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["ax"], 1);
    assert_eq!(rows[0]["by"], 2);
}

// ============================================================
// Multi-part queries
// ============================================================

#[test]
fn multi_part_query_with_match_then_match() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run(
        "MATCH (a:User {name: 'Alice'})-[:FOLLOWS]->(b) \
         WITH b \
         MATCH (b)-[:FOLLOWS]->(c) \
         RETURN c",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["c"]["properties"]["name"], "Carol");
}

#[test]
fn pipeline_match_with_then_match() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City {name:'London'}) \
         WITH p \
         MATCH (p)-[:ASSIGNED_TO]->(proj:Project) \
         RETURN p.name AS person, proj.name AS project",
    );
    assert!(rows.len() >= 2);
}

// ============================================================
// WITH + ORDER BY / LIMIT
// ============================================================

#[test]
fn with_order_and_limit() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (n:User) WITH n ORDER BY n.name ASC LIMIT 2 RETURN n.name AS name");
    assert_eq!(rows.len(), 2);
}

#[test]
fn top_n_oldest_employees() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.column(
        "MATCH (p:Person) WITH p ORDER BY p.age DESC LIMIT 3 RETURN p.name AS name",
        "name",
    );
    let s: Vec<&str> = names.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(s.len(), 3);
    assert_eq!(s[0], "Frank");
}

// ============================================================
// WITH + aggregation
// ============================================================

#[test]
fn with_aggregation_then_filter() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice', dept: 'eng'})");
    db.run("CREATE (b:User {name: 'Bob', dept: 'eng'})");
    db.run("CREATE (c:User {name: 'Carol', dept: 'sales'})");
    let rows =
        db.run("MATCH (n:User) WITH n.dept AS dept, count(n) AS c WHERE c > 1 RETURN dept, c");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["dept"], "eng");
}

#[test]
fn pipeline_match_with_aggregate_then_filter() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         WITH c.name AS city, count(p) AS pop \
         WHERE pop > 1 \
         RETURN city, pop ORDER BY city",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["city"], "Berlin");
    assert_eq!(rows[1]["city"], "London");
}

#[test]
fn with_grouped_aggregation_pipeline() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         WITH p.dept AS dept, count(p) AS cnt \
         WHERE cnt >= 3 \
         RETURN dept, cnt",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["dept"], "Engineering");
}

// ============================================================
// WITH + UNWIND
// ============================================================

#[test]
fn with_collect_then_unwind() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows =
        db.run("MATCH (p:Person) WITH collect(p.name) AS names UNWIND names AS name RETURN name");
    assert_eq!(rows.len(), 6);
}

// ============================================================
// Chained WITH
// ============================================================

#[test]
fn chained_with_narrows_progressively() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         WITH p WHERE p.dept = 'Engineering' \
         WITH p WHERE p.age > 30 \
         RETURN p.name AS name ORDER BY p.name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[1]["name"], "Frank");
}

// ============================================================
// WITH + SET
// ============================================================

#[test]
fn with_set_then_return() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    let rows = db.run("MATCH (n:User) SET n:Seen WITH n RETURN n");
    assert_eq!(rows.len(), 1);
    db.assert_count("MATCH (n:Seen) RETURN n", 1);
}

// ============================================================
// Mixed read-write pipeline
// ============================================================

#[test]
fn match_aggregate_then_create() {
    let db = TestDb::new();
    db.run("CREATE (:Score {val: 10})");
    db.run("CREATE (:Score {val: 20})");
    db.run("CREATE (:Score {val: 30})");
    db.run("MATCH (s:Score) WITH sum(s.val) AS total CREATE (:Summary {total: total})");
    let rows = db.run("MATCH (s:Summary) RETURN s");
    assert_eq!(rows[0]["s"]["properties"]["total"], 60);
}

// ============================================================
// WITH pipeline patterns
// ============================================================

#[test]
fn with_aggregation_filter_having_like() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Group persons by city, keep only cities with more than 1 resident
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         WITH c.name AS city, count(p) AS pop \
         WHERE pop >= 3 \
         RETURN city, pop",
    );
    // London has 3 residents (Alice, Carol, Frank)
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["city"], "London");
    assert_eq!(rows[0]["pop"], 3);
}

#[test]
fn multi_step_with_pipeline_match_with_match_return() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Find people in London, then find which projects they work on
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City {name:'London'}) \
         WITH p \
         MATCH (p)-[:ASSIGNED_TO]->(proj:Project) \
         RETURN p.name AS person, proj.name AS project ORDER BY p.name",
    );
    // Alice -> Alpha, Carol -> Beta (Frank has no project assignment)
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["person"], "Alice");
    assert_eq!(rows[0]["project"], "Alpha");
    assert_eq!(rows[1]["person"], "Carol");
    assert_eq!(rows[1]["project"], "Beta");
}

#[test]
fn with_introducing_computed_columns() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         WITH p.name AS name, p.age * 2 AS double_age \
         WHERE double_age > 80 \
         RETURN name, double_age ORDER BY name",
    );
    // Alice 35*2=70 (no), Bob 28*2=56 (no), Carol 42*2=84 (yes), Dave 31*2=62 (no),
    // Eve 26*2=52 (no), Frank 50*2=100 (yes)
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "Carol");
    assert_eq!(rows[0]["double_age"], 84);
    assert_eq!(rows[1]["name"], "Frank");
    assert_eq!(rows[1]["double_age"], 100);
}

#[test]
fn with_pagination_order_by_skip_limit() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Get 2nd and 3rd oldest persons (skip 1, limit 2)
    let names = db.column(
        "MATCH (p:Person) \
         WITH p ORDER BY p.age DESC SKIP 1 LIMIT 2 \
         RETURN p.name AS name",
        "name",
    );
    // Sorted by age DESC: Frank(50), Carol(42), Alice(35), Dave(31), Bob(28), Eve(26)
    // Skip 1 = skip Frank, Limit 2 = Carol, Alice
    assert_eq!(names.len(), 2);
    assert_eq!(names[0].as_str().unwrap(), "Carol");
    assert_eq!(names[1].as_str().unwrap(), "Alice");
}

#[test]
fn with_collect_then_unwind_round_trip() {
    let db = TestDb::new();
    db.run("CREATE (:Tag {name:'a'})");
    db.run("CREATE (:Tag {name:'b'})");
    db.run("CREATE (:Tag {name:'c'})");
    let names = db.sorted_strings(
        "MATCH (t:Tag) \
         WITH collect(t.name) AS tags \
         UNWIND tags AS tag \
         RETURN tag",
        "tag",
    );
    assert_eq!(names, vec!["a", "b", "c"]);
}

// ============================================================
// WITH scope isolation
// ============================================================

#[test]
fn with_variables_before_not_visible_after() {
    let db = TestDb::new();
    db.seed_social_graph();
    // After WITH a, the variable r should not be accessible
    let err = db.run_err(
        "MATCH (a:User)-[r:FOLLOWS]->(b:User) \
         WITH a \
         RETURN r",
    );
    assert!(err.contains("Unknown variable") || err.contains("variable"));
}

#[test]
fn with_passes_only_listed_variables() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Pass only c through WITH — p should not be visible after
    let err = db.run_err(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         WITH c \
         RETURN p.name AS name",
    );
    assert!(err.contains("Unknown variable") || err.contains("variable"));
}

#[test]
fn with_alias_rename() {
    let db = TestDb::new();
    db.run("CREATE (:Item {val: 42})");
    let rows = db.run(
        "MATCH (n:Item) \
         WITH n.val AS renamed_value \
         RETURN renamed_value",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["renamed_value"], 42);
}

#[test]
fn with_alias_rename_hides_original() {
    let db = TestDb::new();
    db.run("CREATE (:Item {val: 42})");
    // After renaming n to x, the original variable n should be gone
    let err = db.run_err(
        "MATCH (n:Item) \
         WITH n AS x \
         RETURN n.val",
    );
    assert!(err.contains("Unknown variable") || err.contains("variable"));
}

// ============================================================
// Ignored WITH tests (pending implementation)
// ============================================================

#[test]
fn with_order_by_within_pipeline() {
    // Lora: WITH ... ORDER BY within pipeline
    let db = TestDb::new();
    db.seed_org_graph();
    // ORDER BY inside WITH without LIMIT should preserve ordering for later stages
    let names = db.column(
        "MATCH (p:Person) \
         WITH p ORDER BY p.name ASC \
         RETURN p.name AS name",
        "name",
    );
    assert_eq!(names[0].as_str().unwrap(), "Alice");
    assert_eq!(names[5].as_str().unwrap(), "Frank");
}

#[test]
fn with_star_passes_all_variables_complex() {
    // Lora: WITH * passes all variables
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[r:WORKS_AT]->(c:Company) \
         WITH * \
         WHERE p.dept = 'Engineering' \
         RETURN p.name AS name, c.name AS company",
    );
    assert_eq!(rows.len(), 4); // Alice, Bob, Eve, Frank
}

#[test]
fn with_distinct_deduplication() {
    // Lora: WITH DISTINCT deduplication
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         WITH DISTINCT c.name AS city \
         RETURN city ORDER BY city",
    );
    // Berlin, London, Tokyo — deduplicated
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["city"], "Berlin");
    assert_eq!(rows[1]["city"], "London");
    assert_eq!(rows[2]["city"], "Tokyo");
}

// ============================================================
// Extended WITH: query pipelining and scoping
// ============================================================

#[test]
fn with_filters_then_aggregates() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:WORKS_AT]->(c:Company) \
         WITH p WHERE p.age > 30 \
         RETURN count(p) AS senior_count",
    );
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["senior_count"].as_i64().unwrap() >= 2);
}

#[test]
fn with_computed_value_in_next_match() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         WITH avg(p.age) AS avg_age \
         MATCH (p2:Person) WHERE p2.age > avg_age \
         RETURN p2.name AS name ORDER BY name",
    );
    assert!(!rows.is_empty());
}

#[test]
fn with_limit_then_collect() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         WITH p ORDER BY p.age DESC LIMIT 3 \
         RETURN collect(p.name) AS top3",
    );
    let names = rows[0]["top3"].as_array().unwrap();
    assert_eq!(names.len(), 3);
}

#[test]
fn with_unwind_then_aggregate() {
    let db = TestDb::new();
    let rows = db.run(
        "WITH [1, 2, 3, 4, 5] AS nums \
         UNWIND nums AS n \
         RETURN sum(n) AS total, count(n) AS cnt",
    );
    assert_eq!(rows[0]["total"], 15);
    assert_eq!(rows[0]["cnt"], 5);
}

#[test]
fn with_multiple_stages() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:WORKS_AT]->(c:Company) \
         WITH p.dept AS dept, count(p) AS cnt \
         WITH dept, cnt WHERE cnt > 1 \
         RETURN dept, cnt ORDER BY dept",
    );
    assert!(!rows.is_empty());
    for row in &rows {
        assert!(row["cnt"].as_i64().unwrap() > 1);
    }
}

#[test]
fn with_distinct_pass_through() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         WITH DISTINCT c.name AS city \
         RETURN city ORDER BY city",
    );
    // Should be deduplicated cities
    let cities: Vec<&str> = rows.iter().map(|r| r["city"].as_str().unwrap()).collect();
    let mut deduped = cities.clone();
    deduped.dedup();
    assert_eq!(cities, deduped);
}

#[test]
fn with_preserves_variables_explicitly() {
    let db = TestDb::new();
    db.run("CREATE (:P {name: 'Alice', age: 30})");
    // After WITH, only passed-through variables are visible
    let rows = db.run(
        "MATCH (p:P) \
         WITH p.name AS name \
         RETURN name",
    );
    assert_eq!(rows[0]["name"], "Alice");
}

// ============================================================
// Complex three-stage pipeline
// ============================================================

#[test]
fn with_three_stage_pipeline() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Stage 1: match + filter. Stage 2: aggregate. Stage 3: filter aggregation result
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         WITH c.name AS city, collect(p.name) AS people, count(p) AS pop \
         WITH city, people, pop WHERE pop >= 2 \
         RETURN city, pop ORDER BY pop DESC",
    );
    // London(3), Berlin(2) — Tokyo(1) filtered out
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["city"], "London");
    assert_eq!(rows[0]["pop"], 3);
    assert_eq!(rows[1]["city"], "Berlin");
    assert_eq!(rows[1]["pop"], 2);
}

// ============================================================
// WITH + OPTIONAL MATCH
// ============================================================

#[test]
fn with_followed_by_optional_match() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Get engineering people, then optionally find their projects
    let rows = db.run(
        "MATCH (p:Person) WHERE p.dept = 'Engineering' \
         WITH p \
         OPTIONAL MATCH (p)-[:ASSIGNED_TO]->(proj:Project) \
         RETURN p.name AS name, proj.name AS project ORDER BY p.name",
    );
    // Alice->Alpha, Bob->Alpha, Eve->Beta, Frank->null
    assert_eq!(rows.len(), 4);
    let frank = rows.iter().find(|r| r["name"] == "Frank").unwrap();
    assert!(frank["project"].is_null());
}

// ============================================================
// WITH + CASE expression
// ============================================================

#[test]
fn with_case_in_pipeline() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         WITH p.name AS name, \
              CASE WHEN p.age >= 40 THEN 'senior' \
                   WHEN p.age >= 30 THEN 'mid' \
                   ELSE 'junior' END AS tier \
         RETURN tier, collect(name) AS people ORDER BY tier",
    );
    assert_eq!(rows.len(), 3);
}

// ============================================================
// WITH for top-N per group pattern
// ============================================================

#[test]
fn with_top_n_per_group() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    // Highest rated movie per viewer using max(score)
    let rows = db.run(
        "MATCH (v:Viewer)-[r:RATED]->(m:Movie) \
         RETURN v.name AS viewer, max(r.score) AS top_score \
         ORDER BY viewer",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["viewer"], "Alice");
    assert_eq!(rows[0]["top_score"], 5); // Matrix
    assert_eq!(rows[1]["viewer"], "Bob");
    assert_eq!(rows[1]["top_score"], 5); // Matrix
    assert_eq!(rows[2]["viewer"], "Carol");
    assert_eq!(rows[2]["top_score"], 5); // Inception
}

// ============================================================
// WITH + UNWIND + aggregation round-trip
// ============================================================

#[test]
fn with_unwind_filter_reaggregate() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Collect all ages, unwind, filter > 30, count
    let rows = db.run(
        "MATCH (p:Person) \
         WITH collect(p.age) AS ages \
         UNWIND ages AS age \
         WITH age WHERE age > 30 \
         RETURN count(age) AS senior_count, min(age) AS youngest_senior",
    );
    // > 30: Alice(35), Carol(42), Dave(31), Frank(50) = 4
    assert_eq!(rows[0]["senior_count"], 4);
    assert_eq!(rows[0]["youngest_senior"], 31);
}

// ============================================================
// Ignored WITH tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn with_call_subquery() {
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         CALL { WITH p MATCH (p)-[:MANAGES]->(s) RETURN count(s) AS subs } \
         RETURN p.name, subs",
    );
}
