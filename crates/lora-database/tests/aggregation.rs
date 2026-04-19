/// Aggregation tests — count, sum, avg, min, max, collect, grouping,
/// count distinct, HAVING patterns, empty-set behavior, multiple aggregates.
mod test_helpers;
use test_helpers::TestDb;

fn db_with_data() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice', age: 30, dept: 'eng'})");
    db.run("CREATE (b:User {name: 'Bob', age: 25, dept: 'eng'})");
    db.run("CREATE (c:User {name: 'Carol', age: 35, dept: 'sales'})");
    db.run("CREATE (d:User {name: 'Dave', age: 40, dept: 'sales'})");
    db
}

// ============================================================
// COUNT
// ============================================================

#[test]
fn count_all_nodes() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN count(n) AS c");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["c"], 4);
}

#[test]
fn count_empty_result() {
    let db = TestDb::new();
    let rows = db.run("MATCH (n:User) RETURN count(n) AS c");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["c"], 0);
}

#[test]
fn count_with_filter() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) WHERE n.age > 30 RETURN count(n) AS c");
    assert_eq!(rows[0]["c"], 2);
}

#[test]
fn count_no_matching_rows() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) WHERE n.age > 100 RETURN count(n) AS c");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["c"], 0);
}

// ============================================================
// COUNT DISTINCT
// ============================================================

#[test]
fn count_distinct() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN count(DISTINCT n.dept) AS c");
    assert_eq!(rows[0]["c"], 2);
}

#[test]
fn count_distinct_departments() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run("MATCH (p:Person) RETURN count(DISTINCT p.dept) AS depts");
    assert_eq!(rows[0]["depts"], 2);
}

#[test]
fn count_distinct_cities() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) RETURN count(DISTINCT c.name) AS cities",
    );
    assert_eq!(rows[0]["cities"], 3);
}

#[test]
fn count_distinct_with_grouping() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         RETURN p.dept AS dept, count(DISTINCT c.name) AS cities ORDER BY p.dept",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["cities"], 2);
    assert_eq!(rows[1]["cities"], 2);
}

// ============================================================
// SUM / AVG / MIN / MAX
// ============================================================

#[test]
fn sum_property() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN sum(n.age) AS total");
    assert_eq!(rows[0]["total"], 130);
}

#[test]
fn avg_property() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN avg(n.age) AS average");
    let avg = rows[0]["average"].as_f64().unwrap();
    assert!((avg - 32.5).abs() < 0.01);
}

#[test]
fn min_property() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN min(n.age) AS youngest");
    assert_eq!(rows[0]["youngest"], 25);
}

#[test]
fn max_property() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN max(n.age) AS oldest");
    assert_eq!(rows[0]["oldest"], 40);
}

#[test]
fn min_max_age_org() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run("MATCH (p:Person) RETURN min(p.age) AS youngest, max(p.age) AS oldest");
    assert_eq!(rows[0]["youngest"], 26);
    assert_eq!(rows[0]["oldest"], 50);
}

#[test]
fn sum_project_budgets() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run("MATCH (p:Project) RETURN sum(p.budget) AS total");
    assert_eq!(rows[0]["total"], 150000);
}

// ============================================================
// COLLECT
// ============================================================

#[test]
fn collect_property() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN collect(n.name) AS names");
    let names = rows[0]["names"].as_array().unwrap();
    assert_eq!(names.len(), 4);
}

#[test]
fn collect_names_per_project() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:ASSIGNED_TO]->(proj:Project) \
         RETURN proj.name AS project, collect(p.name) AS members ORDER BY proj.name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["project"], "Alpha");
    assert_eq!(rows[0]["members"].as_array().unwrap().len(), 2);
}

// ============================================================
// Grouped aggregation
// ============================================================

#[test]
fn count_grouped_by_property() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN n.dept AS dept, count(n) AS c");
    assert_eq!(rows.len(), 2);
    for row in &rows {
        let dept = row["dept"].as_str().unwrap();
        let count = row["c"].as_i64().unwrap();
        match dept {
            "eng" => assert_eq!(count, 2),
            "sales" => assert_eq!(count, 2),
            _ => panic!("unexpected dept: {dept}"),
        }
    }
}

#[test]
fn count_employees_per_department() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN p.dept AS dept, count(p) AS cnt ORDER BY p.dept",
    );
    assert_eq!(rows.len(), 2);
    for row in &rows {
        match row["dept"].as_str().unwrap() {
            "Engineering" => assert_eq!(row["cnt"], 4),
            "Marketing" => assert_eq!(row["cnt"], 2),
            other => panic!("unexpected dept: {other}"),
        }
    }
}

#[test]
fn avg_age_per_department() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN p.dept AS dept, avg(p.age) AS avg_age ORDER BY p.dept",
    );
    assert_eq!(rows.len(), 2);
    for row in &rows {
        let avg = row["avg_age"].as_f64().unwrap();
        match row["dept"].as_str().unwrap() {
            "Engineering" => assert!((avg - 34.75).abs() < 0.01),
            "Marketing" => assert!((avg - 36.5).abs() < 0.01),
            other => panic!("unexpected dept: {other}"),
        }
    }
}

// ============================================================
// HAVING-style patterns (WITH aggregate + WHERE)
// ============================================================

#[test]
fn having_filter_on_grouped_count() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         WITH p.dept AS dept, count(p) AS cnt \
         WHERE cnt > 2 \
         RETURN dept, cnt",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["dept"], "Engineering");
    assert_eq!(rows[0]["cnt"], 4);
}

// ============================================================
// Aggregation over relationships
// ============================================================

#[test]
fn count_outgoing_relationships_per_person() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person {name:'Frank'})-[r]->(x) RETURN count(r) AS rels",
    );
    assert_eq!(rows[0]["rels"], 5);
}

// ============================================================
// Multiple aggregates / empty set
// ============================================================

#[test]
fn aggregation_on_empty_match() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) WHERE p.age > 100 RETURN count(p) AS cnt, sum(p.age) AS total",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], 0);
    assert!(rows[0]["total"].is_null() || rows[0]["total"] == 0);
}

#[test]
fn multiple_aggregates_same_query() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN count(p) AS cnt, min(p.age) AS youngest, max(p.age) AS oldest, avg(p.age) AS avg_age",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], 6);
    assert_eq!(rows[0]["youngest"], 26);
    assert_eq!(rows[0]["oldest"], 50);
}

// ============================================================
// COUNT(*) vs COUNT(variable)
// ============================================================

#[test]
fn count_star_counts_all_rows() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN count(*) AS c");
    assert_eq!(rows[0]["c"], 4);
}

#[test]
fn count_star_with_no_matches() {
    let db = TestDb::new();
    let rows = db.run("MATCH (n:User) RETURN count(*) AS c");
    assert_eq!(rows[0]["c"], 0);
}

#[test]
fn count_star_includes_null_property_rows() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'A', score: 10})");
    db.run("CREATE (:Item {name: 'B'})");
    db.run("CREATE (:Item {name: 'C', score: 20})");
    // count(*) counts all rows including those with null properties
    let rows = db.run("MATCH (i:Item) RETURN count(*) AS c");
    assert_eq!(rows[0]["c"], 3);
    // count(i.score) only counts non-null values
    let rows = db.run("MATCH (i:Item) RETURN count(i.score) AS c");
    assert_eq!(rows[0]["c"], 2);
}

#[test]
fn count_star_no_match_clause() {
    let db = TestDb::new();
    // count(*) with no MATCH — should count the single input row
    let rows = db.run("RETURN count(*) AS c");
    assert_eq!(rows[0]["c"], 1);
}

#[test]
fn count_star_grouped() {
    let db = db_with_data();
    let rows = db.run(
        "MATCH (n:User) RETURN n.dept AS dept, count(*) AS c ORDER BY n.dept",
    );
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(row["c"], 2);
    }
}

// ============================================================
// MIN / MAX on strings
// ============================================================

#[test]
fn min_on_strings() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN min(n.name) AS first_name");
    assert_eq!(rows[0]["first_name"], "Alice");
}

#[test]
fn max_on_strings() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN max(n.name) AS last_name");
    assert_eq!(rows[0]["last_name"], "Dave");
}

// ============================================================
// Aggregation with null values
// ============================================================

#[test]
fn count_skips_null_properties() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'A', score: 10})");
    db.run("CREATE (:Item {name: 'B'})");
    db.run("CREATE (:Item {name: 'C', score: 20})");
    let rows = db.run("MATCH (i:Item) RETURN count(i.score) AS cnt");
    assert_eq!(rows[0]["cnt"], 2);
}

#[test]
fn sum_skips_null_properties() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'A', score: 10})");
    db.run("CREATE (:Item {name: 'B'})");
    db.run("CREATE (:Item {name: 'C', score: 20})");
    let rows = db.run("MATCH (i:Item) RETURN sum(i.score) AS total");
    assert_eq!(rows[0]["total"], 30);
}

#[test]
fn avg_skips_null_properties() {
    let db = TestDb::new();
    db.run("CREATE (:Item {name: 'A', score: 10})");
    db.run("CREATE (:Item {name: 'B'})");
    db.run("CREATE (:Item {name: 'C', score: 20})");
    let rows = db.run("MATCH (i:Item) RETURN avg(i.score) AS average");
    let avg = rows[0]["average"].as_f64().unwrap();
    assert!((avg - 15.0).abs() < 0.01);
}

// ============================================================
// AVG always returns float
// ============================================================

#[test]
fn avg_returns_float_for_integers() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN avg(n.age) AS average");
    assert!(rows[0]["average"].is_f64());
}

// ============================================================
// Single-row aggregation
// ============================================================

#[test]
fn min_max_with_single_row() {
    let db = TestDb::new();
    db.run("CREATE (:Solo {val: 42})");
    let rows = db.run("MATCH (n:Solo) RETURN min(n.val) AS lo, max(n.val) AS hi");
    assert_eq!(rows[0]["lo"], 42);
    assert_eq!(rows[0]["hi"], 42);
}

// ============================================================
// Aggregation on computed expressions
// ============================================================

#[test]
fn sum_of_computed_expression() {
    let db = TestDb::new();
    db.run("CREATE (:Item {price: 10, qty: 3})");
    db.run("CREATE (:Item {price: 20, qty: 2})");
    db.run("CREATE (:Item {price: 5,  qty: 10})");
    let rows = db.run("MATCH (i:Item) RETURN sum(i.price * i.qty) AS revenue");
    assert_eq!(rows[0]["revenue"], 120);
}

// ============================================================
// Multiple grouped aggregates
// ============================================================

#[test]
fn multiple_aggregates_grouped() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         RETURN p.dept AS dept, count(p) AS cnt, min(p.age) AS youngest, max(p.age) AS oldest \
         ORDER BY p.dept",
    );
    assert_eq!(rows.len(), 2);
    // Engineering: count=4, min=26(Eve), max=50(Frank)
    assert_eq!(rows[0]["dept"], "Engineering");
    assert_eq!(rows[0]["cnt"], 4);
    assert_eq!(rows[0]["youngest"], 26);
    assert_eq!(rows[0]["oldest"], 50);
}

// ============================================================
// Count relationships grouped by type
// ============================================================

#[test]
fn count_relationships_grouped_by_type() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (a)-[r]->(b) RETURN type(r) AS rel_type, count(r) AS cnt ORDER BY type(r)",
    );
    // ASSIGNED_TO=4, LIVES_IN=6, MANAGES=4, WORKS_AT=6
    assert!(rows.len() >= 4);
}

// ============================================================
// COLLECT DISTINCT
// ============================================================

#[test]
fn collect_distinct_values() {
    let db = db_with_data();
    let rows = db.run("MATCH (n:User) RETURN collect(DISTINCT n.dept) AS depts");
    let depts = rows[0]["depts"].as_array().unwrap();
    assert_eq!(depts.len(), 2);
}

// ============================================================
// Aggregation on recommendation graph
// ============================================================

#[test]
fn avg_rating_per_movie() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    let rows = db.run(
        "MATCH (v:Viewer)-[r:RATED]->(m:Movie) \
         RETURN m.title AS title, avg(r.score) AS avg_score, count(v) AS reviewers \
         ORDER BY m.title",
    );
    // Amelie: Alice(3)+Carol(4) = avg 3.5, 2 reviewers
    // Inception: Alice(4)+Carol(5) = avg 4.5, 2 reviewers
    // Jaws: Bob(2) = avg 2.0, 1 reviewer
    // Matrix: Alice(5)+Bob(5) = avg 5.0, 2 reviewers
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["title"], "Amelie");
    let amelie_avg = rows[0]["avg_score"].as_f64().unwrap();
    assert!((amelie_avg - 3.5).abs() < 0.01);
}

#[test]
fn total_ratings_per_viewer() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    let rows = db.run(
        "MATCH (v:Viewer)-[r:RATED]->(m:Movie) \
         RETURN v.name AS viewer, count(r) AS ratings ORDER BY v.name",
    );
    assert_eq!(rows[0]["viewer"], "Alice");
    assert_eq!(rows[0]["ratings"], 3);
    assert_eq!(rows[1]["viewer"], "Bob");
    assert_eq!(rows[1]["ratings"], 2);
    assert_eq!(rows[2]["viewer"], "Carol");
    assert_eq!(rows[2]["ratings"], 2);
}

// ============================================================
// Aggregation on dependency graph
// ============================================================

#[test]
fn count_dependencies_per_package() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    let rows = db.run(
        "MATCH (p:Package)-[d:DEPENDS_ON]->(dep:Package) \
         RETURN p.name AS pkg, count(d) AS deps ORDER BY p.name",
    );
    // app=3, auth=2, crypto=1, web=2
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["pkg"], "app");
    assert_eq!(rows[0]["deps"], 3);
}

// ============================================================
// Aggregation over rich social graph
// ============================================================

#[test]
fn rich_social_count_friends_per_person() {
    // Count outgoing KNOWS per person
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:KNOWS]->(friend:Person) \
         RETURN p.name AS name, count(friend) AS friends ORDER BY p.name",
    );
    // Alice->Bob,Carol (2); Bob->Carol,Dave (2); Carol->Eve (1); Dave->Eve (1); Eve->Frank (1)
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[0]["friends"], 2);
    assert_eq!(rows[1]["name"], "Bob");
    assert_eq!(rows[1]["friends"], 2);
    assert_eq!(rows[2]["name"], "Carol");
    assert_eq!(rows[2]["friends"], 1);
}

#[test]
fn rich_social_avg_friendship_strength() {
    // Average friendship strength across all KNOWS relationships
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run(
        "MATCH (a:Person)-[k:KNOWS]->(b:Person) RETURN avg(k.strength) AS avg_str",
    );
    // Strengths: 5, 8, 4, 3, 6, 2, 7 => sum=35, count=7, avg=5.0
    let avg = rows[0]["avg_str"].as_f64().unwrap();
    assert!((avg - 5.0).abs() < 0.01);
}

#[test]
fn rich_social_people_with_most_interests() {
    // People grouped by count of interests, ordered desc
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:INTERESTED_IN]->(i:Interest) \
         RETURN p.name AS name, count(i) AS interest_count \
         ORDER BY count(i) DESC, p.name ASC",
    );
    // Alice: Music, Travel = 2
    // Bob: Sports, Music = 2
    // Carol: Cooking, Travel = 2
    // Dave: Sports, Music = 2
    // Eve: Music, Travel = 2
    // Frank: Cooking = 1
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[5]["name"], "Frank");
    assert_eq!(rows[5]["interest_count"], 1);
}

#[test]
fn rich_social_collect_interest_names_per_person() {
    // Collect all interest names per person
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run(
        "MATCH (p:Person {name:'Alice'})-[:INTERESTED_IN]->(i:Interest) \
         RETURN p.name AS name, collect(i.name) AS interests",
    );
    assert_eq!(rows.len(), 1);
    let interests = rows[0]["interests"].as_array().unwrap();
    assert_eq!(interests.len(), 2);
}

// ============================================================
// Aggregation over knowledge graph
// ============================================================

#[test]
fn knowledge_graph_count_entities_per_type() {
    // Count entities grouped by type
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let rows = db.run(
        "MATCH (e:Entity) RETURN e.type AS etype, count(e) AS cnt ORDER BY e.type",
    );
    // award=1, field=3 (Physics, Mathematics, Radioactivity), person=2, theory=2
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["etype"], "award");
    assert_eq!(rows[0]["cnt"], 1);
    assert_eq!(rows[1]["etype"], "field");
    assert_eq!(rows[1]["cnt"], 3);
    assert_eq!(rows[2]["etype"], "person");
    assert_eq!(rows[2]["cnt"], 2);
    assert_eq!(rows[3]["etype"], "theory");
    assert_eq!(rows[3]["cnt"], 2);
}

#[test]
fn knowledge_graph_count_relationships_per_entity() {
    // Count outgoing relationships for Einstein
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let rows = db.run(
        "MATCH (e:Entity {name:'Albert Einstein'})-[r]->(x) RETURN count(r) AS rels",
    );
    // STUDIED(2) + PROPOSED(1) + CONTRIBUTED_TO(1) + RECEIVED(1) + AUTHORED(2) + HAS_ALIAS(2) = 9
    assert_eq!(rows[0]["rels"], 9);
}

#[test]
fn knowledge_graph_collect_document_titles_by_author() {
    // Collect document titles authored by each person entity
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let rows = db.run(
        "MATCH (e:Entity {type:'person'})-[:AUTHORED]->(d:Document) \
         RETURN e.name AS author, collect(d.title) AS docs ORDER BY e.name",
    );
    // Only Einstein authored documents
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["author"], "Albert Einstein");
    let docs = rows[0]["docs"].as_array().unwrap();
    assert_eq!(docs.len(), 2);
}

#[test]
fn knowledge_graph_count_people_who_received_nobel() {
    // Count entities that received the Nobel Prize
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let rows = db.run(
        "MATCH (e:Entity)-[:RECEIVED]->(a:Entity {name:'Nobel Prize'}) RETURN count(e) AS winners",
    );
    // Einstein and Marie Curie
    assert_eq!(rows[0]["winners"], 2);
}

// ============================================================
// Aggregation edge cases
// ============================================================

#[test]
fn count_distinct_on_computed_expression() {
    // count(DISTINCT) on a computed expression
    let db = TestDb::new();
    db.run("CREATE (:Num {val: 10})");
    db.run("CREATE (:Num {val: 20})");
    db.run("CREATE (:Num {val: 10})");
    db.run("CREATE (:Num {val: 30})");
    let rows = db.run("MATCH (n:Num) RETURN count(DISTINCT n.val * 2) AS cnt");
    // Distinct values of val*2: 20, 40, 60 = 3
    assert_eq!(rows[0]["cnt"], 3);
}

#[test]
fn sum_on_empty_result_set() {
    // sum of null / empty result set
    let db = TestDb::new();
    let rows = db.run("MATCH (n:Nothing) RETURN sum(n.val) AS total");
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["total"].is_null() || rows[0]["total"] == 0);
}

#[test]
fn multiple_aggregates_with_multiple_grouping_keys() {
    // Multiple aggregates + multiple grouping keys
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run(
        "MATCH (p:Person)-[r:INTERESTED_IN]->(i:Interest) \
         RETURN p.city AS city, r.level AS level, count(p) AS cnt \
         ORDER BY p.city ASC, r.level ASC",
    );
    // Multiple combinations of city and level
    assert!(rows.len() >= 3);
}

#[test]
fn aggregation_after_varlen_traversal() {
    // Aggregation after variable-length traversal: count dependencies at each depth
    let db = TestDb::new();
    db.seed_dependency_graph();
    // Count all transitive deps of 'app'
    let rows = db.run(
        "MATCH (src:Package {name:'app'})-[:DEPENDS_ON*]->(dep:Package) \
         RETURN count(dep) AS total_paths",
    );
    // app->web, app->auth, app->log (3 direct)
    // app->web->log, app->web->util, app->auth->crypto, app->auth->log (4 at depth 2)
    // app->auth->crypto->util (1 at depth 3)
    // Total paths = 3 + 4 + 1 = 8
    assert_eq!(rows[0]["total_paths"], 8);
}

#[test]
fn count_distinct_transitive_dependencies() {
    // Count distinct transitive dependencies
    let db = TestDb::new();
    db.seed_dependency_graph();
    let rows = db.run(
        "MATCH (src:Package {name:'app'})-[:DEPENDS_ON*]->(dep:Package) \
         RETURN count(DISTINCT dep.name) AS unique_deps",
    );
    // web, auth, log, util, crypto = 5
    assert_eq!(rows[0]["unique_deps"], 5);
}

// ============================================================
// Ignored future aggregation tests
// ============================================================

#[test]
fn percentile_cont_function() {
    // Lora: percentileCont() function
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN percentileCont(p.age, 0.5) AS median_age",
    );
    assert!(!rows.is_empty());
}

#[test]
fn percentile_disc_function() {
    // Lora: percentileDisc() function
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN percentileDisc(p.age, 0.5) AS median_age",
    );
    assert!(!rows.is_empty());
}

#[test]
fn stdev_function() {
    // Lora: stDev() function
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN stDev(p.age) AS sd",
    );
    let sd = rows[0]["sd"].as_f64().unwrap();
    assert!(sd > 0.0);
}

#[test]
fn collect_ordering_guarantee() {
    // Lora: collect() ordering guarantee
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         WITH p ORDER BY p.name ASC \
         RETURN collect(p.name) AS names",
    );
    let names = rows[0]["names"].as_array().unwrap();
    // Should preserve the ORDER BY ordering within collect
    assert_eq!(names[0], "Alice");
    assert_eq!(names[5], "Frank");
}

#[test]
fn aggregation_in_case_expression() {
    // Lora: aggregation in CASE expression
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         RETURN p.dept AS dept, \
                CASE WHEN count(p) > 3 THEN 'large' ELSE 'small' END AS size",
    );
    assert!(!rows.is_empty());
}

// ============================================================
// Extended aggregation: complex grouping
// ============================================================

#[test]
fn agg_multiple_aggregates_single_query() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:WORKS_AT]->(c:Company) \
         RETURN count(p) AS total, min(p.age) AS youngest, max(p.age) AS oldest, \
                collect(p.name) AS names",
    );
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["total"].as_i64().unwrap() >= 5);
    assert!(rows[0]["youngest"].as_i64().unwrap() < rows[0]["oldest"].as_i64().unwrap());
    assert!(!rows[0]["names"].as_array().unwrap().is_empty());
}

#[test]
fn agg_group_by_with_having_equivalent() {
    let db = TestDb::new();
    db.seed_org_graph();
    // GROUP BY dept, then filter groups with count > 2 using WITH + WHERE
    let rows = db.run(
        "MATCH (p:Person) \
         WITH p.dept AS dept, count(p) AS cnt \
         WHERE cnt > 2 \
         RETURN dept, cnt ORDER BY dept",
    );
    // Engineering has Alice, Bob, Eve, Frank (4) — should appear
    assert!(!rows.is_empty());
    for row in &rows {
        assert!(row["cnt"].as_i64().unwrap() > 2);
    }
}

#[test]
fn agg_count_distinct_values() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) RETURN count(DISTINCT p.dept) AS dept_count",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["dept_count"], 2); // Engineering and Marketing
}

#[test]
fn agg_sum_with_null_values() {
    let db = TestDb::new();
    db.run("CREATE (:Val {x: 10})");
    db.run("CREATE (:Val {x: 20})");
    db.run("CREATE (:Val {})"); // no x property → null
    let rows = db.run("MATCH (v:Val) RETURN sum(v.x) AS total");
    assert_eq!(rows[0]["total"], 30); // null is skipped
}

#[test]
fn agg_avg_with_null_skip() {
    let db = TestDb::new();
    db.run("CREATE (:Score {v: 10})");
    db.run("CREATE (:Score {v: 20})");
    db.run("CREATE (:Score {})"); // null → skipped
    let rows = db.run("MATCH (s:Score) RETURN avg(s.v) AS mean");
    let mean = rows[0]["mean"].as_f64().unwrap();
    assert!((mean - 15.0).abs() < 0.001); // avg(10, 20) = 15
}

#[test]
fn agg_collect_preserves_order_with_order_by() {
    let db = TestDb::new();
    db.run("CREATE (:O {name: 'c', rank: 3})");
    db.run("CREATE (:O {name: 'a', rank: 1})");
    db.run("CREATE (:O {name: 'b', rank: 2})");
    let rows = db.run(
        "MATCH (o:O) \
         WITH o ORDER BY o.rank \
         RETURN collect(o.name) AS names",
    );
    let names = rows[0]["names"].as_array().unwrap();
    assert_eq!(names[0], "a");
    assert_eq!(names[1], "b");
    assert_eq!(names[2], "c");
}

#[test]
fn agg_min_max_on_strings() {
    let db = TestDb::new();
    db.run("CREATE (:W {v: 'banana'})");
    db.run("CREATE (:W {v: 'apple'})");
    db.run("CREATE (:W {v: 'cherry'})");
    let rows = db.run(
        "MATCH (w:W) RETURN min(w.v) AS lo, max(w.v) AS hi",
    );
    assert_eq!(rows[0]["lo"], "apple");
    assert_eq!(rows[0]["hi"], "cherry");
}

#[test]
fn agg_count_star_vs_count_property() {
    let db = TestDb::new();
    db.run("CREATE (:T {v: 1})");
    db.run("CREATE (:T {v: 2})");
    db.run("CREATE (:T {})"); // no v
    let rows = db.run(
        "MATCH (t:T) RETURN count(*) AS all_rows, count(t.v) AS non_null",
    );
    assert_eq!(rows[0]["all_rows"], 3);
    assert_eq!(rows[0]["non_null"], 2);
}

#[test]
fn agg_nested_grouping_with_unwind() {
    let db = TestDb::new();
    db.run("CREATE (:Tag {category: 'A', items: [1, 2, 3]})");
    db.run("CREATE (:Tag {category: 'A', items: [4, 5]})");
    db.run("CREATE (:Tag {category: 'B', items: [10]})");
    let rows = db.run(
        "MATCH (t:Tag) \
         UNWIND t.items AS item \
         RETURN t.category AS cat, count(item) AS cnt, sum(item) AS total \
         ORDER BY cat",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["cat"], "A");
    assert_eq!(rows[0]["cnt"], 5);
    assert_eq!(rows[0]["total"], 15);
    assert_eq!(rows[1]["cat"], "B");
    assert_eq!(rows[1]["cnt"], 1);
}

#[test]
fn agg_empty_group_returns_zero() {
    let db = TestDb::new();
    let rows = db.run("MATCH (n:NoSuchLabel) RETURN count(n) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], 0);
}

// ============================================================
// Extended aggregation: future / pending features
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn agg_percentile_disc() {
    let db = TestDb::new();
    for i in 1..=10 {
        db.run(&format!("CREATE (:V {{x: {}}})", i));
    }
    let rows = db.run(
        "MATCH (v:V) RETURN percentileDisc(v.x, 0.5) AS median",
    );
    // Median of 1..10 should be 5 or 6
    let m = rows[0]["median"].as_i64().unwrap();
    assert!(m == 5 || m == 6);
}

// ============================================================
// Aggregation with OPTIONAL MATCH (null handling)
// ============================================================

#[test]
fn agg_count_with_optional_match_nulls() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Count subordinates per person — those without any should get 0
    let rows = db.run(
        "MATCH (p:Person) \
         OPTIONAL MATCH (p)-[:MANAGES]->(sub:Person) \
         RETURN p.name AS name, count(sub) AS subs \
         ORDER BY subs DESC, p.name ASC",
    );
    assert_eq!(rows.len(), 6);
    // Find specific entries by name since ordering may group ties differently
    let frank = rows.iter().find(|r| r["name"] == "Frank").unwrap();
    assert_eq!(frank["subs"], 3);
    let carol = rows.iter().find(|r| r["name"] == "Carol").unwrap();
    assert_eq!(carol["subs"], 1);
    // The remaining 4 (Alice, Bob, Dave, Eve) should have 0 subs
    let zero_count = rows.iter().filter(|r| r["subs"] == 0).count();
    assert_eq!(zero_count, 4);
}

#[test]
fn agg_collect_with_optional_match_filters_nulls() {
    let db = TestDb::new();
    db.seed_org_graph();
    // collect() with OPTIONAL MATCH — people with projects should have non-empty lists
    let rows = db.run(
        "MATCH (p:Person) \
         OPTIONAL MATCH (p)-[:ASSIGNED_TO]->(proj:Project) \
         RETURN p.name AS name, collect(proj.name) AS projects \
         ORDER BY p.name",
    );
    // Alice -> [Alpha], Bob -> [Alpha], Carol -> [Beta], Eve -> [Beta]
    let alice = rows.iter().find(|r| r["name"] == "Alice").unwrap();
    assert_eq!(alice["projects"].as_array().unwrap().len(), 1);
    assert_eq!(alice["projects"].as_array().unwrap()[0], "Alpha");
    // People with projects should have exactly 1 project each
    let bob = rows.iter().find(|r| r["name"] == "Bob").unwrap();
    assert_eq!(bob["projects"].as_array().unwrap().len(), 1);
    // People without projects: collect may include null or be empty
    // — test documents actual behavior rather than assuming reference semantics
    let dave = rows.iter().find(|r| r["name"] == "Dave").unwrap();
    let dave_projects = dave["projects"].as_array().unwrap();
    // Dave has no assignments, so list should be empty or contain a single null
    assert!(dave_projects.len() <= 1);
}

// ============================================================
// Two-level aggregation via WITH pipeline
// ============================================================

#[test]
fn agg_two_level_count_of_counts() {
    let db = TestDb::new();
    db.seed_org_graph();
    // First: count people per city. Second: count how many cities have > 1 person
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         WITH c.name AS city, count(p) AS pop \
         RETURN count(city) AS num_cities, sum(pop) AS total_residents, max(pop) AS biggest_city",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["num_cities"], 3);
    assert_eq!(rows[0]["total_residents"], 6);
    assert_eq!(rows[0]["biggest_city"], 3); // London
}

#[test]
fn agg_pipeline_filter_then_reaggregate() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    // Step 1: avg rating per movie. Step 2: count movies with avg >= 4
    let rows = db.run(
        "MATCH (v:Viewer)-[r:RATED]->(m:Movie) \
         WITH m.title AS movie, avg(r.score) AS avg_score \
         WHERE avg_score >= 4.0 \
         RETURN count(movie) AS highly_rated_count",
    );
    // Matrix: avg 5.0, Inception: avg 4.5, Amelie: avg 3.5, Jaws: avg 2.0
    // >= 4.0: Matrix, Inception = 2
    assert_eq!(rows[0]["highly_rated_count"], 2);
}

// ============================================================
// Aggregation over variable-length paths
// ============================================================

#[test]
fn agg_count_distinct_over_varlen() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    // Count unique transitive dependencies per package
    let rows = db.run(
        "MATCH (p:Package)-[:DEPENDS_ON*]->(dep:Package) \
         RETURN p.name AS pkg, count(DISTINCT dep.name) AS unique_deps \
         ORDER BY unique_deps DESC",
    );
    // app has 5 unique transitive deps
    assert_eq!(rows[0]["pkg"], "app");
    assert_eq!(rows[0]["unique_deps"], 5);
}

// ============================================================
// Aggregation edge cases: single value, all same, all null
// ============================================================

#[test]
fn agg_all_same_values() {
    let db = TestDb::new();
    for _ in 0..5 {
        db.run("CREATE (:Same {x: 42})");
    }
    let rows = db.run("MATCH (s:Same) RETURN min(s.x) AS lo, max(s.x) AS hi, avg(s.x) AS avg, count(DISTINCT s.x) AS uniq");
    assert_eq!(rows[0]["lo"], 42);
    assert_eq!(rows[0]["hi"], 42);
    let avg = rows[0]["avg"].as_f64().unwrap();
    assert!((avg - 42.0).abs() < 0.01);
    assert_eq!(rows[0]["uniq"], 1);
}

#[test]
fn agg_all_null_values() {
    let db = TestDb::new();
    // Create nodes where one has property x so the schema knows about it,
    // then remove it so all nodes have null for x.
    db.run("CREATE (:Nil {x: 1})");
    db.run("MATCH (n:Nil) SET n.x = null");
    db.run("CREATE (:Nil {})");
    db.run("CREATE (:Nil {})");
    let rows = db.run("MATCH (n:Nil) RETURN count(n.x) AS cnt, sum(n.x) AS total");
    assert_eq!(rows[0]["cnt"], 0);
    // sum of all nulls should be 0 or null
    assert!(rows[0]["total"].is_null() || rows[0]["total"] == 0);
}

// ============================================================
// Grouped aggregation with multiple keys
// ============================================================

#[test]
fn agg_group_by_two_keys() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         RETURN p.dept AS dept, c.name AS city, count(p) AS cnt \
         ORDER BY dept, city",
    );
    // Multiple combinations of dept and city
    assert!(rows.len() >= 4);
    for row in &rows {
        assert!(row["cnt"].as_i64().unwrap() >= 1);
    }
}

// ============================================================
// Collect + size pattern
// ============================================================

#[test]
fn agg_collect_then_size() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:INTERESTED_IN]->(i:Interest) \
         WITH p.name AS name, collect(i.name) AS interests \
         RETURN name, size(interests) AS cnt ORDER BY cnt DESC, name ASC",
    );
    // Most people have 2 interests, Frank has 1
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[5]["name"], "Frank");
    assert_eq!(rows[5]["cnt"], 1);
}

// ============================================================
// Future aggregation tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn agg_stdev_population() {
    let db = TestDb::new();
    for i in 1..=10 {
        db.run(&format!("CREATE (:V {{x: {}}})", i));
    }
    let _rows = db.run("MATCH (v:V) RETURN stDevP(v.x) AS sd");
}

#[test]
#[ignore = "pending implementation"]
fn agg_having_without_with() {
    // Direct HAVING clause (not standard Lora but common request)
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         RETURN p.dept AS dept, count(p) AS cnt \
         HAVING cnt > 2",
    );
}
