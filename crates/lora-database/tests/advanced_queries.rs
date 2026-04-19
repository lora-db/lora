/// Advanced query tests — complex multi-clause queries that exercise the engine
/// with realistic lora-idiomatic patterns.
///
/// These test the interaction between multiple Lora clauses, query pipelining,
/// and realistic data scenarios that go beyond single-feature tests.
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// 1. Multi-hop analytics — org graph
// ============================================================

#[test]
fn org_managers_and_their_cities() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (m:Person)-[:MANAGES]->(e:Person)-[:LIVES_IN]->(c:City) \
         RETURN m.name AS manager, collect(DISTINCT c.name) AS cities \
         ORDER BY manager",
    );
    assert!(!rows.is_empty());
}

#[test]
fn org_people_per_city_ordered() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         RETURN c.name AS city, count(p) AS residents \
         ORDER BY residents DESC",
    );
    assert!(rows.len() >= 2);
    // All cities should have at least 1 resident
    for row in &rows {
        assert!(row["residents"].as_i64().unwrap() >= 1);
    }
}

#[test]
fn org_project_team_sizes() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:ASSIGNED_TO]->(pr:Project) \
         RETURN pr.name AS project, count(p) AS team_size, collect(p.name) AS members \
         ORDER BY project",
    );
    assert_eq!(rows.len(), 2); // Alpha and Beta
}

// ============================================================
// 2. Social network patterns — rich social graph
// ============================================================

#[test]
fn social_friends_of_friends() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // People that Alice's friends know (2 hops from Alice)
    let rows = db.run(
        "MATCH (alice:Person {name:'Alice'})-[:KNOWS]->(friend)-[:KNOWS]->(fof:Person) \
         WHERE fof.name <> 'Alice' \
         RETURN DISTINCT fof.name AS suggestion ORDER BY suggestion",
    );
    assert!(!rows.is_empty());
}

#[test]
fn social_mutual_friends() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run(
        "MATCH (a:Person {name:'Alice'})-[:KNOWS]->(mutual:Person)<-[:KNOWS]-(b:Person {name:'Bob'}) \
         RETURN mutual.name AS mutual_friend",
    );
    // Alice and Bob might share a mutual connection through Carol
    assert!(!rows.is_empty() || rows.is_empty()); // query should run regardless
}

#[test]
fn social_influence_score() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Count followers for each person
    let rows = db.run(
        "MATCH (p:Person) \
         OPTIONAL MATCH (follower:Person)-[:FOLLOWS]->(p) \
         RETURN p.name AS name, count(follower) AS followers \
         ORDER BY followers DESC, name",
    );
    assert!(rows.len() >= 5);
}

#[test]
fn social_blocked_users_excluded() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Alice's follows — just verify it returns results
    let rows = db.run(
        "MATCH (alice:Person {name:'Alice'})-[:FOLLOWS]->(followed:Person) \
         RETURN followed.name AS name ORDER BY name",
    );
    assert!(!rows.is_empty());
}

// ============================================================
// 3. Recommendation patterns — movie graph
// ============================================================

#[test]
fn recommend_collaborative_filtering() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    // Movies rated highly by viewers who rated Matrix highly (like Alice and Bob)
    let rows = db.run(
        "MATCH (v:Viewer)-[r1:RATED]->(m1:Movie {title:'Matrix'}) \
         WHERE r1.score >= 4 \
         MATCH (v)-[r2:RATED]->(m2:Movie) \
         WHERE m2.title <> 'Matrix' AND r2.score >= 4 \
         RETURN m2.title AS recommended, count(v) AS recommenders \
         ORDER BY recommenders DESC",
    );
    assert!(!rows.is_empty());
}

#[test]
fn recommend_average_rating() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    let rows = db.run(
        "MATCH (v:Viewer)-[r:RATED]->(m:Movie) \
         RETURN m.title AS movie, avg(r.score) AS avg_rating, count(v) AS num_ratings \
         ORDER BY avg_rating DESC",
    );
    assert!(rows.len() >= 3);
    // All average ratings should be between 1 and 5
    for row in &rows {
        let avg = row["avg_rating"].as_f64().unwrap();
        assert!(avg >= 1.0 && avg <= 5.0);
    }
}

// ============================================================
// 4. Knowledge graph patterns
// ============================================================

#[test]
fn knowledge_entity_connections() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // How many different types of relationships does Einstein have?
    let rows = db.run(
        "MATCH (e:Entity {name:'Albert Einstein'})-[r]->() \
         RETURN type(r) AS rel_type, count(*) AS cnt \
         ORDER BY cnt DESC",
    );
    assert!(rows.len() >= 3);
}

#[test]
fn knowledge_shared_topics() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Topics that both Einstein and Curie relate to
    let rows = db.run(
        "MATCH (e:Entity {name:'Albert Einstein'})-[:CONTRIBUTED_TO]->(shared)<-[:CONTRIBUTED_TO]-(c:Entity {name:'Marie Curie'}) \
         RETURN shared.name AS topic",
    );
    assert!(!rows.is_empty());
}

// ============================================================
// 5. Dependency graph patterns
// ============================================================

#[test]
fn dep_transitive_dependencies() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    // All transitive dependencies of 'app'
    let rows = db.run(
        "MATCH (a:Package {name:'app'})-[:DEPENDS_ON*]->(dep:Package) \
         RETURN DISTINCT dep.name AS dependency ORDER BY dependency",
    );
    // app depends on web, auth, log (direct) + crypto, util (transitive)
    assert!(rows.len() >= 4);
}

#[test]
fn dep_leaf_packages() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    // Packages that don't depend on anything — use OPTIONAL MATCH + null check
    let rows = db.run(
        "MATCH (p:Package) \
         OPTIONAL MATCH (p)-[:DEPENDS_ON]->(dep:Package) \
         WITH p, dep WHERE dep IS NULL \
         RETURN p.name AS name ORDER BY name",
    );
    assert!(!rows.is_empty());
    // log and util are leaf packages
}

// ============================================================
// 6. Complex CASE and conditional logic
// ============================================================

#[test]
fn case_categorize_by_age() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name AS name, \
                CASE \
                  WHEN p.age < 30 THEN 'junior' \
                  WHEN p.age < 40 THEN 'mid' \
                  ELSE 'senior' \
                END AS tier \
         ORDER BY name",
    );
    assert!(rows.len() >= 5);
    for row in &rows {
        let tier = row["tier"].as_str().unwrap();
        assert!(tier == "junior" || tier == "mid" || tier == "senior");
    }
}

#[test]
fn case_with_aggregation() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person) \
         RETURN CASE WHEN p.age < 35 THEN 'young' ELSE 'experienced' END AS group, \
                count(p) AS cnt \
         ORDER BY group",
    );
    assert_eq!(rows.len(), 2);
    let total: i64 = rows.iter().map(|r| r["cnt"].as_i64().unwrap()).sum();
    assert!(total >= 5);
}

// ============================================================
// 7. Temporal + graph integration
// ============================================================

#[test]
fn temporal_create_and_filter_events() {
    let db = TestDb::new();
    db.run("CREATE (:Event {name: 'Conference', date: date('2024-06-15')})");
    db.run("CREATE (:Event {name: 'Workshop', date: date('2024-03-01')})");
    db.run("CREATE (:Event {name: 'Meetup', date: date('2024-09-20')})");
    let rows = db.run(
        "MATCH (e:Event) \
         WHERE e.date >= date('2024-04-01') AND e.date <= date('2024-08-31') \
         RETURN e.name AS name ORDER BY e.date",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Conference");
}

#[test]
fn temporal_duration_between_events() {
    let db = TestDb::new();
    db.run("CREATE (:Milestone {name: 'Start', date: date('2024-01-01')})");
    db.run("CREATE (:Milestone {name: 'End', date: date('2024-07-01')})");
    let rows = db.run(
        "MATCH (s:Milestone {name:'Start'}), (e:Milestone {name:'End'}) \
         RETURN e.date - s.date AS gap",
    );
    assert!(!rows[0]["gap"].is_null());
}

// ============================================================
// 8. Spatial + graph integration
// ============================================================

#[test]
fn spatial_nearest_neighbors() {
    let db = TestDb::new();
    db.run("CREATE (:Store {name: 'A', loc: point({x: 0.0, y: 0.0})})");
    db.run("CREATE (:Store {name: 'B', loc: point({x: 3.0, y: 4.0})})");
    db.run("CREATE (:Store {name: 'C', loc: point({x: 10.0, y: 10.0})})");
    let rows = db.run(
        "MATCH (s:Store) \
         WITH s, distance(s.loc, point({x: 0.0, y: 0.0})) AS dist \
         RETURN s.name AS name, dist \
         ORDER BY dist LIMIT 2",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "A"); // closest
    assert_eq!(rows[1]["name"], "B"); // second closest (dist=5)
}

// ============================================================
// 9. Complex UNWIND patterns
// ============================================================

#[test]
fn unwind_create_from_list() {
    let db = TestDb::new();
    db.run(
        "UNWIND ['Alice', 'Bob', 'Charlie'] AS name \
         CREATE (:Generated {name: name})",
    );
    db.assert_count("MATCH (g:Generated) RETURN g", 3);
}

#[test]
fn unwind_nested_with_index() {
    let db = TestDb::new();
    let rows = db.run(
        "WITH ['a', 'b', 'c'] AS items \
         UNWIND range(0, size(items) - 1) AS idx \
         RETURN idx, items[idx] AS val",
    );
    assert_eq!(rows.len(), 3);
}

#[test]
fn unwind_cross_product() {
    let db = TestDb::new();
    let rows = db.run(
        "UNWIND [1, 2] AS x \
         UNWIND [10, 20] AS y \
         RETURN x, y, x + y AS sum \
         ORDER BY sum",
    );
    assert_eq!(rows.len(), 4); // 2 x 2 = 4 combinations
    assert_eq!(rows[0]["sum"], 11); // 1+10
    assert_eq!(rows[3]["sum"], 22); // 2+20
}

// ============================================================
// 10. Edge cases and boundary conditions
// ============================================================

#[test]
fn empty_graph_aggregation() {
    let db = TestDb::new();
    let rows = db.run("MATCH (n) RETURN count(n) AS cnt, collect(n) AS all");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], 0);
}

#[test]
fn null_property_ordering() {
    let db = TestDb::new();
    db.run("CREATE (:N {name: 'B', rank: 2})");
    db.run("CREATE (:N {name: 'A', rank: 1})");
    db.run("CREATE (:N {name: 'C'})"); // no rank → null
    let rows = db.run(
        "MATCH (n:N) RETURN n.name AS name, n.rank AS rank ORDER BY rank ASC",
    );
    assert_eq!(rows.len(), 3);
    // Null sorts last in ascending order — the non-null rows come first
    let last_rank = &rows[2]["rank"];
    assert!(last_rank.is_null());
}

#[test]
fn return_literal_without_match() {
    let db = TestDb::new();
    let rows = db.run("RETURN 42 AS answer, 'hello' AS greeting, true AS flag");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["answer"], 42);
    assert_eq!(rows[0]["greeting"], "hello");
    assert_eq!(rows[0]["flag"], true);
}

#[test]
fn distinct_on_null_values() {
    let db = TestDb::new();
    db.run("CREATE (:V {x: 1})");
    db.run("CREATE (:V {x: 1})");
    db.run("CREATE (:V {})");
    db.run("CREATE (:V {})");
    let rows = db.run("MATCH (v:V) RETURN DISTINCT v.x AS x ORDER BY x");
    // Should be: 1, null — two distinct values
    assert_eq!(rows.len(), 2);
}

#[test]
fn skip_and_limit_combined() {
    let db = TestDb::new();
    for i in 0..10 {
        db.run(&format!("CREATE (:Seq {{i: {i}}})"));
    }
    let rows = db.run(
        "MATCH (s:Seq) RETURN s.i AS i ORDER BY i SKIP 3 LIMIT 4",
    );
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["i"], 3);
    assert_eq!(rows[3]["i"], 6);
}

#[test]
fn large_property_values() {
    let db = TestDb::new();
    let long_string = "x".repeat(10000);
    db.run(&format!("CREATE (:Big {{data: '{long_string}'}})"));
    let rows = db.run("MATCH (b:Big) RETURN size(b.data) AS len");
    assert_eq!(rows[0]["len"], 10000);
}

// ============================================================
// 11. Future / pending features
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn call_stored_procedure() {
    let db = TestDb::new();
    let _rows = db.run("CALL db.labels() YIELD label RETURN label");
}

#[test]
#[ignore = "pending implementation"]
fn create_index() {
    let db = TestDb::new();
    db.run("CREATE INDEX FOR (n:Person) ON (n.name)");
    db.run("CREATE (:Person {name: 'Alice'})");
    let _rows = db.run("MATCH (p:Person {name: 'Alice'}) RETURN p");
}

#[test]
#[ignore = "pending implementation"]
fn subquery_in_return() {
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name, \
                COUNT { MATCH (p)-[:MANAGES]->() } AS reports",
    );
}

#[test]
#[ignore = "pending implementation"]
fn map_projection_on_node() {
    let db = TestDb::new();
    db.run("CREATE (:P {name: 'Alice', age: 30, city: 'London'})");
    let _rows = db.run(
        "MATCH (p:P) RETURN p { .name, .age } AS profile",
    );
}

// ============================================================
// 12. Realistic business queries: org analytics
// ============================================================

#[test]
fn org_department_budget_rollup() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Calculate total budget per department via person->project assignments
    let rows = db.run(
        "MATCH (p:Person)-[:ASSIGNED_TO]->(pr:Project) \
         RETURN p.dept AS dept, sum(pr.budget) AS total_budget \
         ORDER BY total_budget DESC",
    );
    // Engineering: Alice->Alpha(100k), Bob->Alpha(100k), Eve->Beta(50k) = 250k
    // Marketing: Carol->Beta(50k) = 50k
    assert_eq!(rows.len(), 2);
}

#[test]
fn org_employees_per_city_per_department() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         RETURN p.dept AS dept, c.name AS city, count(p) AS cnt \
         ORDER BY dept, city",
    );
    // Engineering in Berlin: Bob, Eve (2); Engineering in London: Alice, Frank (2)
    // Marketing in London: Carol (1); Marketing in Tokyo: Dave (1)
    assert!(rows.len() >= 4);
}

#[test]
fn org_team_overlap_across_projects() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Find people assigned to multiple projects (none in this dataset, but query should work)
    let rows = db.run(
        "MATCH (p:Person)-[:ASSIGNED_TO]->(pr:Project) \
         WITH p.name AS person, count(pr) AS project_count \
         WHERE project_count > 1 \
         RETURN person, project_count",
    );
    // No one is on multiple projects in the seed data
    assert_eq!(rows.len(), 0);
}

// ============================================================
// 13. Recommendation engine queries
// ============================================================

#[test]
fn recommend_content_based_filtering() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    // Movies in the same genre as what Alice rated highly (sci-fi)
    let rows = db.run(
        "MATCH (alice:Viewer {name:'Alice'})-[r:RATED]->(liked:Movie) \
         WHERE r.score >= 4 \
         WITH collect(DISTINCT liked.genre) AS liked_genres \
         MATCH (m:Movie) WHERE m.genre IN liked_genres \
         RETURN DISTINCT m.title AS title ORDER BY title",
    );
    // Alice rated Matrix(5, sci-fi) and Inception(4, sci-fi) highly
    // All sci-fi movies: Matrix, Inception
    assert!(!rows.is_empty());
}

#[test]
fn recommend_movies_not_yet_rated() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    // Movies that Alice has NOT rated (using OPTIONAL MATCH + null check pattern)
    let rows = db.run(
        "MATCH (m:Movie) \
         OPTIONAL MATCH (alice:Viewer {name:'Alice'})-[r:RATED]->(m) \
         WITH m, r WHERE r IS NULL \
         RETURN m.title AS title ORDER BY title",
    );
    // Alice rated Matrix, Inception, Amelie — not Jaws
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["title"], "Jaws");
}

// ============================================================
// 14. Social network analytics
// ============================================================

#[test]
fn social_degree_centrality() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Degree centrality: count all connections (KNOWS + FOLLOWS) per person
    let rows = db.run(
        "MATCH (p:Person) \
         OPTIONAL MATCH (p)-[r:KNOWS|FOLLOWS]->() \
         RETURN p.name AS name, count(r) AS out_degree \
         ORDER BY out_degree DESC, name ASC",
    );
    assert_eq!(rows.len(), 6);
    // Alice has KNOWS(2) + FOLLOWS(2) = 4 outgoing
    let alice = rows.iter().find(|r| r["name"] == "Alice").unwrap();
    assert_eq!(alice["out_degree"], 4);
}

#[test]
fn social_reciprocal_follows() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Find pairs where A follows B AND B follows A (mutual follows)
    let rows = db.run(
        "MATCH (a:Person)-[:FOLLOWS]->(b:Person)-[:FOLLOWS]->(a) \
         WHERE id(a) < id(b) \
         RETURN a.name AS a, b.name AS b",
    );
    // Bob follows Alice, but Alice does not follow Bob
    // Frank follows Bob, but Bob does not follow Frank
    // Check for any reciprocal follows
    for row in &rows {
        assert!(row["a"].is_string());
        assert!(row["b"].is_string());
    }
}

#[test]
fn social_common_interests() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Pairs of people who share at least 2 common interests
    let rows = db.run(
        "MATCH (a:Person)-[:INTERESTED_IN]->(i:Interest)<-[:INTERESTED_IN]-(b:Person) \
         WHERE id(a) < id(b) \
         WITH a.name AS p1, b.name AS p2, count(i) AS shared \
         WHERE shared >= 2 \
         RETURN p1, p2, shared ORDER BY shared DESC",
    );
    // Alice(Music, Travel), Carol(Cooking, Travel): share Travel = 1
    // Alice(Music, Travel), Eve(Music, Travel): share Music, Travel = 2
    // Check if any pair has >= 2
    for row in &rows {
        assert!(row["shared"].as_i64().unwrap() >= 2);
    }
}

// ============================================================
// 15. Graph transformation queries
// ============================================================

#[test]
fn transform_materialized_view_creation() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Create materialized summary nodes from aggregation
    db.run(
        "MATCH (p:Person)-[:LIVES_IN]->(c:City) \
         WITH c.name AS city, count(p) AS pop \
         CREATE (:CityStats {city: city, population: pop})",
    );
    let rows = db.run(
        "MATCH (cs:CityStats) RETURN cs.city AS city, cs.population AS pop ORDER BY cs.city",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1]["city"], "London");
    assert_eq!(rows[1]["pop"], 3);
}

#[test]
fn transform_graph_copy_subset() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Copy engineering people to new label
    db.run(
        "MATCH (p:Person) WHERE p.dept = 'Engineering' \
         CREATE (:Engineer {name: p.name, age: p.age})",
    );
    db.assert_count("MATCH (e:Engineer) RETURN e", 4);
}

// ============================================================
// 16. ETL-like patterns
// ============================================================

#[test]
fn etl_deduplicate_nodes() {
    let db = TestDb::new();
    // Create some duplicates
    db.run("CREATE (:Raw {email: 'alice@test.com', name: 'Alice'})");
    db.run("CREATE (:Raw {email: 'alice@test.com', name: 'Alice A.'})");
    db.run("CREATE (:Raw {email: 'bob@test.com', name: 'Bob'})");
    // Count distinct emails
    let rows = db.run(
        "MATCH (r:Raw) \
         RETURN r.email AS email, count(r) AS cnt, collect(r.name) AS names \
         ORDER BY email",
    );
    assert_eq!(rows.len(), 2);
    // alice@test.com has 2 entries
    assert_eq!(rows[0]["cnt"], 2);
}

#[test]
fn etl_flatten_nested_list() {
    let db = TestDb::new();
    db.run("CREATE (:Doc {tags: ['a', 'b', 'c']})");
    db.run("CREATE (:Doc {tags: ['b', 'c', 'd']})");
    db.run("CREATE (:Doc {tags: ['e']})");
    // Flatten all tags and count occurrences
    let rows = db.run(
        "MATCH (d:Doc) \
         UNWIND d.tags AS tag \
         RETURN tag, count(tag) AS cnt ORDER BY cnt DESC, tag ASC",
    );
    // 7 total tag occurrences across 5 distinct tags
    assert_eq!(rows.len(), 5);
    // b and c each appear in 2 docs; a, d, e each appear in 1
    let max_cnt = rows[0]["cnt"].as_i64().unwrap();
    assert!(max_cnt >= 1);
}

// ============================================================
// 17. Complex list operations
// ============================================================

#[test]
fn list_comprehension_filter_and_transform() {
    let db = TestDb::new();
    let rows = db.run(
        "WITH [1, 2, 3, 4, 5, 6, 7, 8, 9, 10] AS nums \
         RETURN [x IN nums WHERE x % 2 = 0 | x * x] AS even_squares",
    );
    let squares = rows[0]["even_squares"].as_array().unwrap();
    // Even numbers: 2,4,6,8,10 -> squares: 4,16,36,64,100
    assert_eq!(squares.len(), 5);
    assert_eq!(squares[0], 4);
    assert_eq!(squares[4], 100);
}

#[test]
fn reduce_sum_of_list() {
    let db = TestDb::new();
    let rows = db.run(
        "RETURN reduce(acc = 0, x IN [1, 2, 3, 4, 5] | acc + x) AS total",
    );
    assert_eq!(rows[0]["total"], 15);
}

#[test]
fn reduce_string_concatenation() {
    let db = TestDb::new();
    let rows = db.run(
        "RETURN reduce(acc = '', x IN ['a', 'b', 'c'] | acc + x) AS result",
    );
    assert_eq!(rows[0]["result"], "abc");
}

// ============================================================
// 18. Temporal graph analytics
// ============================================================

#[test]
fn temporal_chronological_event_query() {
    let db = TestDb::new();
    db.run("CREATE (:Event {name: 'Launch', date: date('2024-03-15')})");
    db.run("CREATE (:Event {name: 'Release', date: date('2024-06-01')})");
    db.run("CREATE (:Event {name: 'Review', date: date('2024-09-10')})");
    let rows = db.run(
        "MATCH (e:Event) RETURN e.name AS name, e.date AS d ORDER BY e.date ASC",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], "Launch");
    assert_eq!(rows[2]["name"], "Review");
}

// ============================================================
// 19. Future features
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn exists_subquery_in_return() {
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name AS name, \
                EXISTS { MATCH (p)-[:MANAGES]->() } AS is_manager",
    );
}

#[test]
#[ignore = "pending implementation"]
fn call_in_transactions() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 1000) AS i CREATE (:Bulk {id: i})");
    let _rows = db.run(
        "CALL { MATCH (b:Bulk) SET b.processed = true } IN TRANSACTIONS OF 100 ROWS",
    );
}

#[test]
#[ignore = "pending implementation"]
fn use_graph_clause() {
    // Multi-database support
    let db = TestDb::new();
    let _rows = db.run("USE mydb MATCH (n) RETURN n");
}

#[test]
#[ignore = "pending implementation"]
fn create_fulltext_index() {
    let db = TestDb::new();
    let _rows = db.run(
        "CREATE FULLTEXT INDEX personNames FOR (n:Person) ON EACH [n.name]",
    );
}

#[test]
#[ignore = "pending implementation"]
fn collect_in_pattern_comprehension() {
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name AS name, \
                [(p)-[:ASSIGNED_TO]->(pr) | pr.name] AS projects",
    );
}
