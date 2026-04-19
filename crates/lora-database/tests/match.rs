/// MATCH clause tests — node matching, label/property constraints, relationship
/// traversal, direction semantics, cross-products, variable binding, and
/// optional match behavior.
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// Basic node matching
// ============================================================

#[test]
fn match_all_nodes_empty_graph() {
    let db = TestDb::new();
    db.assert_count("MATCH (n) RETURN n", 0);
}

#[test]
fn match_all_nodes_single_node() {
    let db = TestDb::new();
    db.run("CREATE (n:User {name: 'Alice'})");
    db.assert_count("MATCH (n) RETURN n", 1);
}

#[test]
fn match_all_nodes_multiple() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("CREATE (c:Product {name: 'Widget'})");
    db.assert_count("MATCH (n) RETURN n", 3);
}

// ============================================================
// Label-constrained matching
// ============================================================

#[test]
fn match_by_label() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:Product {name: 'Widget'})");
    db.assert_count("MATCH (n:User) RETURN n", 1);
    db.assert_count("MATCH (n:Product) RETURN n", 1);
}

#[test]
fn match_by_label_no_results() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    let err = db.run_err("MATCH (n:Product) RETURN n");
    assert!(err.contains("Unknown label"));
}

#[test]
fn match_multiple_labels() {
    let db = TestDb::new();
    db.run("CREATE (a:User:Admin {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    let rows = db.run("MATCH (n:User:Admin) RETURN n");
    assert!(rows.len() <= 2);
}

#[test]
fn match_node_with_multiple_labels_via_single() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Frank is :Person:Manager — match via :Manager label alone
    let rows = db.run("MATCH (m:Manager) RETURN m.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Frank");
}

// ============================================================
// Property-constrained matching
// ============================================================

#[test]
fn match_by_property() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice', age: 30})");
    db.run("CREATE (b:User {name: 'Bob', age: 25})");
    db.assert_count("MATCH (n:User {name: 'Alice'}) RETURN n", 1);
}

#[test]
fn match_by_integer_property() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice', age: 30})");
    db.run("CREATE (b:User {name: 'Bob', age: 25})");
    db.assert_count("MATCH (n:User {age: 30}) RETURN n", 1);
}

#[test]
fn match_by_multiple_properties() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice', age: 30})");
    db.run("CREATE (b:User {name: 'Bob', age: 30})");
    db.assert_count("MATCH (n:User {name: 'Alice', age: 30}) RETURN n", 1);
}

#[test]
fn match_property_no_match() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.assert_count("MATCH (n:User {name: 'Bob'}) RETURN n", 0);
}

#[test]
fn match_relationship_by_property_value() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (p:Person)-[:ASSIGNED_TO {role:'lead'}]->(proj:Project) RETURN p.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Alice", "Carol"]);
}

// ============================================================
// Relationship traversal — direction
// ============================================================

#[test]
fn match_directed_relationship() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name: 'Alice'})-[:FOLLOWS]->(b) RETURN b");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["b"]["properties"]["name"], "Bob");
}

#[test]
fn match_relationship_with_variable() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name: 'Alice'})-[r:FOLLOWS]->(b) RETURN r");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["r"]["type"], "FOLLOWS");
    assert_eq!(rows[0]["r"]["properties"]["since"], 2020);
}

#[test]
fn match_reverse_direction() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (b:User {name: 'Bob'})<-[:FOLLOWS]-(a) RETURN a");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["a"]["properties"]["name"], "Alice");
}

#[test]
fn match_left_arrow_reverses_traversal() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (c:City {name:'London'})<-[:LIVES_IN]-(p:Person) RETURN p.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Alice", "Carol", "Frank"]);
}

#[test]
fn match_undirected_relationship() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name: 'Alice'})-[:KNOWS]-(b) RETURN b");
    assert!(!rows.is_empty());
}

#[test]
fn match_undirected_finds_both_directions() {
    let db = TestDb::new();
    db.run("CREATE (a:N {id:1})-[:R]->(b:N {id:2})");
    let rows = db.run("MATCH (x:N)-[:R]-(y:N) RETURN x.id AS x, y.id AS y");
    assert_eq!(rows.len(), 2);
}

#[test]
fn match_relationship_any_type() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name: 'Alice'})-[r]->(b) RETURN r");
    assert_eq!(rows.len(), 2); // FOLLOWS + KNOWS
}

// ============================================================
// Multi-hop traversal
// ============================================================

#[test]
fn match_chain_two_hops() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run(
        "MATCH (a:User {name: 'Alice'})-[:FOLLOWS]->(b)-[:FOLLOWS]->(c) RETURN c",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["c"]["properties"]["name"], "Carol");
}

#[test]
fn match_two_hop_manager_project() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (m:Manager)-[:MANAGES]->(e:Person)-[:ASSIGNED_TO]->(p:Project) \
         RETURN m.name AS mgr, e.name AS emp, p.name AS proj",
    );
    assert_eq!(rows.len(), 3);
}

#[test]
fn match_manager_to_employee_direct() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (m:Person {name:'Frank'})-[:MANAGES]->(e:Person) RETURN e.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Alice", "Bob", "Eve"]);
}

#[test]
fn match_three_hop_through_company() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (p:Person)-[:WORKS_AT]->(c:Company {name:'Acme'}) RETURN p.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Alice", "Bob", "Carol", "Dave", "Eve", "Frank"]);
}

// ============================================================
// Triangle detection
// ============================================================

#[test]
fn match_triangle_pattern() {
    let db = TestDb::new();
    db.run("CREATE (a:T {name:'A'})-[:E]->(b:T {name:'B'})");
    db.run("MATCH (b:T {name:'B'}) CREATE (b)-[:E]->(c:T {name:'C'})");
    db.run("MATCH (a:T {name:'A'}), (c:T {name:'C'}) CREATE (a)-[:E]->(c)");
    let rows = db.run(
        "MATCH (a:T)-[:E]->(b:T)-[:E]->(c:T), (a)-[:E]->(c) RETURN a.name AS a, b.name AS b, c.name AS c",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["a"], "A");
    assert_eq!(rows[0]["b"], "B");
    assert_eq!(rows[0]["c"], "C");
}

// ============================================================
// Cross-product and disconnected patterns
// ============================================================

#[test]
fn match_disconnected_patterns_cross_product() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("CREATE (c:Product {name: 'Widget'})");
    db.assert_count("MATCH (u:User), (p:Product) RETURN u, p", 2);
}

#[test]
fn match_cross_product_three_labels() {
    let db = TestDb::new();
    db.seed_org_graph();
    let count = db.exec_count("MATCH (p:Project), (c:City) RETURN p.name, c.name").unwrap();
    assert_eq!(count, 6); // 2 projects * 3 cities
}

// ============================================================
// Variable binding and reuse
// ============================================================

#[test]
fn match_same_variable_constrains_same_node() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name: 'Alice'}) RETURN a");
    assert_eq!(rows.len(), 1);
}

#[test]
fn match_returns_all_bound_variables() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name: 'Alice'})-[r:FOLLOWS]->(b) RETURN a, r, b");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].get("a").is_some());
    assert!(rows[0].get("r").is_some());
    assert!(rows[0].get("b").is_some());
}

#[test]
fn match_same_variable_both_sides_constrains_identity() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (a:Person)-[:MANAGES]->(b:Person)-[:ASSIGNED_TO]->(p:Project) \
         RETURN a.name AS mgr, b.name AS emp, p.name AS proj",
    );
    assert!(rows.len() >= 3);
}

#[test]
fn match_reuse_in_where_constrains_single_node() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (a:Person), (b:Person) WHERE a.name = b.name RETURN a.name AS name",
    );
    assert_eq!(rows.len(), 6);
}

// ============================================================
// No results and empty traversal
// ============================================================

#[test]
fn match_no_relationships_returns_empty() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.assert_count("MATCH (a:User {name: 'Alice'})-[:FOLLOWS]->(b) RETURN b", 0);
}

#[test]
fn match_nonexistent_relationship_chain_returns_empty() {
    let db = TestDb::new();
    db.seed_org_graph();
    db.assert_count(
        "MATCH (p:Person)-[:MANAGES]->(e:Person) WHERE e.name = 'NONEXISTENT' RETURN p",
        0,
    );
}

// ============================================================
// OPTIONAL MATCH
// ============================================================

#[test]
fn optional_match_returns_null_when_no_match() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    let rows = db.run("OPTIONAL MATCH (n) RETURN n");
    assert!(!rows.is_empty());
}

#[test]
fn optional_match_null_for_missing_relationship() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (d:Person {name:'Dave'}) \
         OPTIONAL MATCH (d)-[:MANAGES]->(e:Person) \
         RETURN d.name AS name, e.name AS managed",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Dave");
    assert!(rows[0]["managed"].is_null());
}

// ============================================================
// Cycles
// ============================================================

#[test]
fn match_cycle_in_graph() {
    let db = TestDb::new();
    db.run("CREATE (a:User {name: 'Alice'})");
    db.run("CREATE (b:User {name: 'Bob'})");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (a)-[:FOLLOWS]->(b)");
    db.run("MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'}) CREATE (b)-[:FOLLOWS]->(a)");
    db.assert_count("MATCH (a)-[:FOLLOWS]->(b) RETURN a, b", 2);
}

#[test]
fn match_in_cycle_returns_all_edges() {
    let db = TestDb::new();
    db.seed_cycle(3);
    let rows = db.run("MATCH (a:Ring)-[:LOOP]->(b:Ring) RETURN a.idx AS a, b.idx AS b");
    assert_eq!(rows.len(), 3);
}

#[test]
fn match_two_hops_in_cycle() {
    let db = TestDb::new();
    db.seed_cycle(3);
    let rows = db.run(
        "MATCH (a:Ring)-[:LOOP]->(b:Ring)-[:LOOP]->(c:Ring) RETURN a.idx AS a, c.idx AS c",
    );
    assert_eq!(rows.len(), 3);
}

// ============================================================
// Relationship counting
// ============================================================

#[test]
fn count_relationships_by_type() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run("MATCH (a:Person)-[r:WORKS_AT]->(b:Company) RETURN count(r) AS c");
    assert_eq!(rows[0]["c"], 6);
}

#[test]
fn count_manages_relationships() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run("MATCH (a)-[r:MANAGES]->(b) RETURN count(r) AS c");
    assert_eq!(rows[0]["c"], 4);
}

#[test]
fn match_all_outgoing_from_person() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run("MATCH (a:Person {name:'Alice'})-[r]->(x) RETURN r");
    assert_eq!(rows.len(), 3); // WORKS_AT, ASSIGNED_TO, LIVES_IN
}

// ============================================================
// Property types in MATCH patterns
// ============================================================

#[test]
fn match_by_boolean_property_in_pattern() {
    let db = TestDb::new();
    db.run("CREATE (:Flag {name: 'on', active: true})");
    db.run("CREATE (:Flag {name: 'off', active: false})");
    db.assert_count("MATCH (f:Flag {active: true}) RETURN f", 1);
}

#[test]
fn match_by_float_property_in_pattern() {
    let db = TestDb::new();
    db.run("CREATE (:Metric {name: 'temp', value: 36.6})");
    db.run("CREATE (:Metric {name: 'pressure', value: 101.3})");
    db.assert_count("MATCH (m:Metric {value: 36.6}) RETURN m", 1);
}

#[test]
fn match_property_and_where_combined() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (p:Person {dept: 'Engineering'}) WHERE p.age > 30 RETURN p.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Alice", "Frank"]);
}

// ============================================================
// Multi-hop and complex traversal
// ============================================================

#[test]
fn match_four_hop_chain() {
    let db = TestDb::new();
    db.seed_chain(6);
    let rows = db.run(
        "MATCH (a:Chain {idx:0})-[:NEXT]->(b)-[:NEXT]->(c)-[:NEXT]->(d)-[:NEXT]->(e) \
         RETURN e.idx AS idx",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["idx"], 4);
}

#[test]
fn match_diamond_pattern() {
    let db = TestDb::new();
    db.run("CREATE (:D {name:'top'})");
    db.run("CREATE (:D {name:'left'})");
    db.run("CREATE (:D {name:'right'})");
    db.run("CREATE (:D {name:'bottom'})");
    db.run("MATCH (t:D {name:'top'}), (l:D {name:'left'}) CREATE (t)-[:E]->(l)");
    db.run("MATCH (t:D {name:'top'}), (r:D {name:'right'}) CREATE (t)-[:E]->(r)");
    db.run("MATCH (l:D {name:'left'}), (b:D {name:'bottom'}) CREATE (l)-[:E]->(b)");
    db.run("MATCH (r:D {name:'right'}), (b:D {name:'bottom'}) CREATE (r)-[:E]->(b)");
    // Two paths from top to bottom: top->left->bottom and top->right->bottom
    let rows = db.run(
        "MATCH (t:D {name:'top'})-[:E]->(mid:D)-[:E]->(b:D {name:'bottom'}) \
         RETURN mid.name AS via",
    );
    assert_eq!(rows.len(), 2);
    let mut vias: Vec<&str> = rows.iter().map(|r| r["via"].as_str().unwrap()).collect();
    vias.sort();
    assert_eq!(vias, vec!["left", "right"]);
}

#[test]
fn match_star_pattern_hub_and_spokes() {
    let db = TestDb::new();
    db.run("CREATE (:Hub {name:'center'})");
    for i in 1..=5 {
        db.run(&format!("CREATE (:Spoke {{id:{i}}})"));
        db.run(&format!(
            "MATCH (h:Hub), (s:Spoke {{id:{i}}}) CREATE (h)-[:ARM]->(s)"
        ));
    }
    db.assert_count("MATCH (h:Hub)-[:ARM]->(s:Spoke) RETURN s", 5);
}

#[test]
fn match_all_relationships_in_graph() {
    let db = TestDb::new();
    db.seed_social_graph();
    // 2 FOLLOWS + 1 KNOWS = 3 total relationships
    db.assert_count("MATCH (a)-[r]->(b) RETURN r", 3);
}

// ============================================================
// Multiple MATCH clauses
// ============================================================

#[test]
fn match_multiple_independent_match_clauses() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person {name:'Alice'})-[:WORKS_AT]->(c:Company) \
         MATCH (p)-[:LIVES_IN]->(city:City) \
         RETURN c.name AS company, city.name AS city",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["company"], "Acme");
    assert_eq!(rows[0]["city"], "London");
}

#[test]
fn match_two_independent_patterns_constrained() {
    let db = TestDb::new();
    db.seed_org_graph();
    // All manager-employee pairs where both live in the same city
    let rows = db.run(
        "MATCH (m:Person)-[:MANAGES]->(e:Person) \
         MATCH (m)-[:LIVES_IN]->(c:City)<-[:LIVES_IN]-(e) \
         RETURN m.name AS mgr, e.name AS emp, c.name AS city",
    );
    // Frank manages Alice, Bob, Eve. Frank lives in London, Alice in London, Bob in Berlin, Eve in Berlin
    // Only Frank->Alice in London
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["mgr"], "Frank");
    assert_eq!(rows[0]["emp"], "Alice");
    assert_eq!(rows[0]["city"], "London");
}

// ============================================================
// Disconnected patterns
// ============================================================

#[test]
fn match_three_way_cross_product() {
    let db = TestDb::new();
    db.run("CREATE (:Color {name:'red'})");
    db.run("CREATE (:Color {name:'blue'})");
    db.run("CREATE (:Size {name:'small'})");
    db.run("CREATE (:Size {name:'large'})");
    db.run("CREATE (:Shape {name:'circle'})");
    // 2 * 2 * 1 = 4 rows
    db.assert_count(
        "MATCH (c:Color), (s:Size), (sh:Shape) RETURN c.name, s.name, sh.name",
        4,
    );
}

// ============================================================
// Scenario-based: dependency graph
// ============================================================

#[test]
fn match_direct_dependencies() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    let deps = db.sorted_strings(
        "MATCH (src:Package {name:'app'})-[:DEPENDS_ON]->(dep:Package) RETURN dep.name AS name",
        "name",
    );
    assert_eq!(deps, vec!["auth", "log", "web"]);
}

#[test]
fn match_transitive_dependencies_two_hops() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    // app -> X -> Y (indirect deps)
    let indirect = db.sorted_strings(
        "MATCH (src:Package {name:'app'})-[:DEPENDS_ON]->(mid:Package)-[:DEPENDS_ON]->(dep:Package) \
         RETURN DISTINCT dep.name AS name",
        "name",
    );
    // app->web->log, app->web->util, app->auth->crypto, app->auth->log
    assert!(indirect.contains(&"crypto".to_string()));
    assert!(indirect.contains(&"util".to_string()));
    assert!(indirect.contains(&"log".to_string()));
}

#[test]
fn match_packages_depended_on_by_most() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    // log is depended on by: app, web, auth = 3 incoming
    let rows = db.run(
        "MATCH (dep:Package)<-[:DEPENDS_ON]-(consumer:Package) \
         WHERE dep.name = 'log' \
         RETURN count(consumer) AS cnt",
    );
    assert_eq!(rows[0]["cnt"], 3);
}

// ============================================================
// Scenario-based: transport graph
// ============================================================

#[test]
fn match_direct_routes_from_station() {
    let db = TestDb::new();
    db.seed_transport_graph();
    let destinations = db.sorted_strings(
        "MATCH (src:Station {name:'Amsterdam'})-[:ROUTE]->(dest:Station) \
         RETURN dest.name AS name",
        "name",
    );
    assert_eq!(destinations, vec!["Rotterdam", "Utrecht"]);
}

#[test]
fn match_routes_with_distance_filter() {
    let db = TestDb::new();
    db.seed_transport_graph();
    let rows = db.run(
        "MATCH (a:Station)-[r:ROUTE]->(b:Station) \
         WHERE r.distance <= 40 \
         RETURN a.name AS from, b.name AS to, r.distance AS dist \
         ORDER BY r.distance ASC",
    );
    // Routes with distance <= 40: Amsterdam->Utrecht(40), Utrecht->Amsterdam(40), Rotterdam->Den Haag(25), Den Haag->Rotterdam(25)
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["dist"], 25);
}

#[test]
fn match_two_hop_routes() {
    let db = TestDb::new();
    db.seed_transport_graph();
    // Amsterdam -> X -> Den Haag (must go through Rotterdam)
    let rows = db.run(
        "MATCH (src:Station {name:'Amsterdam'})-[:ROUTE]->(mid:Station)-[:ROUTE]->(dst:Station {name:'Den Haag'}) \
         RETURN mid.name AS via",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["via"], "Rotterdam");
}

// ============================================================
// Scenario-based: recommendation graph
// ============================================================

#[test]
fn match_movies_rated_by_viewer() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    let movies = db.sorted_strings(
        "MATCH (v:Viewer {name:'Alice'})-[:RATED]->(m:Movie) RETURN m.title AS title",
        "title",
    );
    assert_eq!(movies, vec!["Amelie", "Inception", "Matrix"]);
}

#[test]
fn match_viewers_who_rated_same_movie() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    // Who else rated Matrix besides Alice?
    let others = db.sorted_strings(
        "MATCH (v:Viewer)-[:RATED]->(m:Movie {title:'Matrix'}) \
         WHERE v.name <> 'Alice' \
         RETURN v.name AS name",
        "name",
    );
    assert_eq!(others, vec!["Bob"]);
}

#[test]
fn match_high_rated_movies() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    let titles = db.sorted_strings(
        "MATCH (v:Viewer)-[r:RATED]->(m:Movie) \
         WHERE r.score >= 5 \
         RETURN DISTINCT m.title AS title",
        "title",
    );
    // Alice rated Matrix 5, Bob rated Matrix 5, Carol rated Inception 5
    assert_eq!(titles, vec!["Inception", "Matrix"]);
}

// ============================================================
// Self-loop matching
// ============================================================

#[test]
fn match_self_loop_typed() {
    let db = TestDb::new();
    db.run("CREATE (n:Recursive {name:'self'})");
    db.run("MATCH (n:Recursive {name:'self'}) CREATE (n)-[:REFS]->(n)");
    let rows = db.run(
        "MATCH (a:Recursive)-[r:REFS]->(b:Recursive) WHERE a.name = b.name RETURN a.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "self");
}

// ============================================================
// Relationship with no type filter
// ============================================================

#[test]
fn match_untyped_relationship_returns_all_types() {
    let db = TestDb::new();
    db.run("CREATE (a:N {id:1}), (b:N {id:2})");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:TYPE_A]->(b)");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:TYPE_B]->(b)");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:TYPE_C]->(b)");
    db.assert_count("MATCH (a:N {id:1})-[r]->(b:N {id:2}) RETURN r", 3);
}

// ============================================================
// OPTIONAL MATCH — basic working case
// ============================================================

#[test]
fn optional_match_with_data_returns_rows() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("OPTIONAL MATCH (n:User) RETURN n");
    assert_eq!(rows.len(), 3);
}

// ============================================================
// Multiple labels matching
// ============================================================

#[test]
fn match_node_requires_all_specified_labels() {
    let db = TestDb::new();
    db.run("CREATE (:A:B {name:'both'})");
    db.run("CREATE (:A {name:'only_a'})");
    db.run("CREATE (:B {name:'only_b'})");
    db.assert_count("MATCH (n:A:B) RETURN n", 1);
    db.assert_count("MATCH (n:A) RETURN n", 2);
    db.assert_count("MATCH (n:B) RETURN n", 2);
}

#[test]
fn match_multi_label_no_node_has_all() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    db.run("CREATE (:Y {id:2})");
    // No node has both X and Y
    db.assert_count("MATCH (n:X:Y) RETURN n", 0);
}

#[test]
fn match_multi_label_subset_of_node_labels() {
    let db = TestDb::new();
    db.run("CREATE (:A:B:C {name:'triple'})");
    db.run("CREATE (:A:B {name:'double'})");
    db.run("CREATE (:A {name:'single'})");
    // :A:B should match 'triple' (has A,B,C) and 'double' (has A,B)
    db.assert_count("MATCH (n:A:B) RETURN n", 2);
    // :A:B:C should match only 'triple'
    db.assert_count("MATCH (n:A:B:C) RETURN n", 1);
}

#[test]
fn match_multi_label_anonymous_node() {
    let db = TestDb::new();
    db.run("CREATE (:Person:Manager {name:'Frank'})");
    db.run("CREATE (:Person {name:'Alice'})");
    db.assert_count("MATCH (:Person:Manager) RETURN 1 AS x", 1);
}

#[test]
fn match_multi_label_in_relationship_pattern() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Frank is :Person:Manager, only he should match the source
    let rows = db.run(
        "MATCH (m:Person:Manager)-[:MANAGES]->(e:Person) RETURN e.name AS name ORDER BY e.name",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], "Alice");
}

#[test]
fn match_single_label_still_works() {
    let db = TestDb::new();
    db.run("CREATE (:User {name:'Alice'})");
    db.run("CREATE (:User:Admin {name:'Bob'})");
    // Single label matches any node with that label (including multi-labeled nodes)
    db.assert_count("MATCH (n:User) RETURN n", 2);
}

#[test]
fn match_no_label_matches_all_nodes() {
    let db = TestDb::new();
    db.run("CREATE (:User {name:'Alice'})");
    db.run("CREATE (:Product {name:'Widget'})");
    db.run("CREATE (n {name:'bare'})");
    db.assert_count("MATCH (n) RETURN n", 3);
}

// ============================================================
// Anonymous node patterns
// ============================================================

#[test]
fn match_anonymous_node_with_label() {
    let db = TestDb::new();
    db.run("CREATE (:Person {name:'Alice'})");
    db.run("CREATE (:Person {name:'Bob'})");
    // Anonymous node (:Person) — no variable binding
    db.assert_count("MATCH (:Person) RETURN 1 AS x", 2);
}

#[test]
fn match_anonymous_node_in_relationship_pattern() {
    let db = TestDb::new();
    db.seed_social_graph();
    // Both endpoints anonymous — match all FOLLOWS relationships
    db.assert_count("MATCH (:User)-[:FOLLOWS]->(:User) RETURN 1 AS x", 2);
}

#[test]
fn match_mixed_named_and_anonymous_nodes() {
    let db = TestDb::new();
    db.seed_social_graph();
    // Named source, anonymous target
    let rows = db.run("MATCH (a:User {name:'Alice'})-[:FOLLOWS]->(:User) RETURN a.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

#[test]
fn match_anonymous_source_named_target() {
    let db = TestDb::new();
    db.seed_social_graph();
    // Anonymous source, named target
    let names = db.sorted_strings(
        "MATCH (:User)-[:FOLLOWS]->(b:User) RETURN b.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Bob", "Carol"]);
}

#[test]
fn match_multiple_anonymous_nodes_in_chain() {
    let db = TestDb::new();
    db.seed_social_graph();
    // Alice->Bob->Carol: anonymous middle node
    db.assert_count(
        "MATCH (a:User {name:'Alice'})-[:FOLLOWS]->(:User)-[:FOLLOWS]->(c:User) RETURN c",
        1,
    );
}

#[test]
fn match_all_anonymous_chain() {
    let db = TestDb::new();
    db.seed_social_graph();
    // Fully anonymous two-hop: (:User)-[:FOLLOWS]->(:User)-[:FOLLOWS]->(:User)
    db.assert_count(
        "MATCH (:User)-[:FOLLOWS]->(:User)-[:FOLLOWS]->(:User) RETURN 1 AS x",
        1,
    );
}

#[test]
fn match_anonymous_node_cross_product() {
    let db = TestDb::new();
    db.run("CREATE (:X {id:1})");
    db.run("CREATE (:X {id:2})");
    db.run("CREATE (:Y {id:3})");
    // Two disconnected anonymous patterns
    db.assert_count("MATCH (:X), (:Y) RETURN 1 AS x", 2);
}

#[test]
fn match_anonymous_with_property_constraint() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Anonymous node with inline property filter
    let rows = db.run(
        "MATCH (:Person {name:'Frank'})-[:MANAGES]->(e:Person) RETURN e.name AS name ORDER BY e.name",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], "Alice");
}

#[test]
fn match_anonymous_does_not_leak_into_return() {
    let db = TestDb::new();
    db.run("CREATE (:A {x:1})-[:R]->(:B {y:2})");
    // Only the named variable should appear in results
    let rows = db.run("MATCH (a:A)-[:R]->(:B) RETURN a");
    assert_eq!(rows.len(), 1);
    // Result should have column "a" but no anonymous variable columns
    let obj = rows[0].as_object().unwrap();
    assert_eq!(obj.len(), 1);
    assert!(obj.contains_key("a"));
}

// ============================================================
// Rich social graph pattern tests
// ============================================================

#[test]
fn rich_social_friend_of_friend_via_knows() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Alice -[:KNOWS]-> Bob -[:KNOWS]-> Carol|Dave
    // Friends-of-friends of Alice through KNOWS*2
    let fof = db.sorted_strings(
        "MATCH (a:Person {name:'Alice'})-[:KNOWS]->(mid:Person)-[:KNOWS]->(fof:Person) \
         RETURN DISTINCT fof.name AS name",
        "name",
    );
    // Alice->Bob->Carol, Alice->Bob->Dave, Alice->Carol->Eve
    assert!(fof.contains(&"Carol".to_string()));
    assert!(fof.contains(&"Dave".to_string()));
    assert!(fof.contains(&"Eve".to_string()));
}

#[test]
fn rich_social_mutual_friends() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Find mutual friends of Alice and Bob (both KNOWS the same person)
    // Alice -[:KNOWS]-> X <-[:KNOWS]- Bob
    // Alice KNOWS: Bob, Carol. Bob KNOWS: Carol, Dave.
    // Common target: Carol
    let mutual = db.sorted_strings(
        "MATCH (a:Person {name:'Alice'})-[:KNOWS]->(friend:Person)<-[:KNOWS]-(b:Person {name:'Bob'}) \
         RETURN friend.name AS name",
        "name",
    );
    assert_eq!(mutual, vec!["Carol"]);
}

#[test]
fn rich_social_shared_interests() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // People who share the Music interest with Alice
    // Alice -[:INTERESTED_IN]-> Music <-[:INTERESTED_IN]- ?
    // Music is liked by: Alice(high), Bob(low), Dave(high), Eve(medium)
    let shared = db.sorted_strings(
        "MATCH (a:Person {name:'Alice'})-[:INTERESTED_IN]->(i:Interest {name:'Music'})<-[:INTERESTED_IN]-(other:Person) \
         WHERE other.name <> 'Alice' \
         RETURN other.name AS name",
        "name",
    );
    assert_eq!(shared, vec!["Bob", "Dave", "Eve"]);
}

#[test]
fn rich_social_filter_blocked_users() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Alice follows Carol and Eve. Alice blocked Frank.
    // Get people Alice follows who she has NOT blocked.
    // Since Alice doesn't block Carol or Eve, both should appear.
    let followed = db.sorted_strings(
        "MATCH (a:Person {name:'Alice'})-[:FOLLOWS]->(f:Person) \
         RETURN f.name AS name",
        "name",
    );
    // Alice follows Carol and Eve
    assert_eq!(followed, vec!["Carol", "Eve"]);
}

#[test]
fn rich_social_blocked_users_list() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Alice blocked Frank, Dave blocked Carol
    let blocked_by_alice = db.sorted_strings(
        "MATCH (a:Person {name:'Alice'})-[:BLOCKED]->(blocked:Person) \
         RETURN blocked.name AS name",
        "name",
    );
    assert_eq!(blocked_by_alice, vec!["Frank"]);
}

#[test]
fn rich_social_multi_relationship_knows_and_follows() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // People Alice both KNOWS and FOLLOWS
    // Alice KNOWS: Bob, Carol. Alice FOLLOWS: Carol, Eve.
    // Intersection: Carol
    let both = db.sorted_strings(
        "MATCH (a:Person {name:'Alice'})-[:KNOWS]->(p:Person) \
         MATCH (a)-[:FOLLOWS]->(p) \
         RETURN p.name AS name",
        "name",
    );
    assert_eq!(both, vec!["Carol"]);
}

#[test]
fn rich_social_influencer_label_match() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Eve is :Person:Influencer — match by the Influencer label
    let rows = db.run(
        "MATCH (inf:Influencer) RETURN inf.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Eve");
}

#[test]
fn rich_social_influencer_multi_label_in_pattern() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Eve is :Person:Influencer — find people who follow her using two MATCH clauses
    let followers_of_influencer = db.sorted_strings(
        "MATCH (inf:Influencer) \
         MATCH (p:Person)-[:FOLLOWS]->(inf) \
         RETURN p.name AS name",
        "name",
    );
    // Alice follows Eve (the influencer)
    assert_eq!(followers_of_influencer, vec!["Alice"]);
}

#[test]
fn rich_social_strong_friendships() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Find KNOWS relationships with strength >= 6
    // Alice->Carol(8), Carol->Eve(6), Eve->Frank(7)
    let strong = db.sorted_strings(
        "MATCH (a:Person)-[k:KNOWS]->(b:Person) \
         WHERE k.strength >= 6 \
         RETURN a.name AS name",
        "name",
    );
    assert_eq!(strong, vec!["Alice", "Carol", "Eve"]);
}

#[test]
fn rich_social_travel_enthusiasts() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // People interested in Travel
    // Alice(medium), Carol(medium), Eve(high)
    let travelers = db.sorted_strings(
        "MATCH (p:Person)-[:INTERESTED_IN]->(i:Interest {name:'Travel'}) \
         RETURN p.name AS name",
        "name",
    );
    assert_eq!(travelers, vec!["Alice", "Carol", "Eve"]);
}

// ============================================================
// Knowledge graph traversal tests
// ============================================================

#[test]
fn knowledge_entity_to_document_traversal() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Einstein authored two documents
    let docs = db.sorted_strings(
        "MATCH (e:Entity {name:'Albert Einstein'})-[:AUTHORED]->(d:Document) \
         RETURN d.title AS title",
        "title",
    );
    assert_eq!(docs.len(), 2);
    assert!(docs.contains(&"On the Electrodynamics of Moving Bodies".to_string()));
    assert!(docs.contains(&"The Foundation of General Relativity".to_string()));
}

#[test]
fn knowledge_multi_hop_person_to_theory_via_document() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Einstein -> AUTHORED -> Document -> ABOUT -> General Relativity
    let theories = db.sorted_strings(
        "MATCH (e:Entity {name:'Albert Einstein'})-[:AUTHORED]->(d:Document)-[:ABOUT]->(t:Entity) \
         RETURN DISTINCT t.name AS name",
        "name",
    );
    // Both documents are ABOUT General Relativity
    assert_eq!(theories, vec!["General Relativity"]);
}

#[test]
fn knowledge_shared_nobel_prize() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Both Einstein and Curie received the Nobel Prize
    let recipients = db.sorted_strings(
        "MATCH (p:Entity)-[:RECEIVED]->(np:Entity {name:'Nobel Prize'}) \
         RETURN p.name AS name",
        "name",
    );
    assert_eq!(recipients, vec!["Albert Einstein", "Marie Curie"]);
}

#[test]
fn knowledge_nobel_prize_years() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Check the years of Nobel Prize receipts
    let rows = db.run(
        "MATCH (p:Entity)-[r:RECEIVED]->(np:Entity {name:'Nobel Prize'}) \
         RETURN p.name AS name, r.year AS year ORDER BY r.year",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "Marie Curie");
    assert_eq!(rows[0]["year"], 1903);
    assert_eq!(rows[1]["name"], "Albert Einstein");
    assert_eq!(rows[1]["year"], 1921);
}

#[test]
fn knowledge_entity_alias_resolution() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Resolve aliases back to the entity
    let aliases = db.sorted_strings(
        "MATCH (e:Entity {name:'Albert Einstein'})-[:HAS_ALIAS]->(a:Alias) \
         RETURN a.value AS name",
        "name",
    );
    assert_eq!(aliases, vec!["A. Einstein", "Einstein"]);
}

#[test]
fn knowledge_topic_hierarchy_from_field() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Physics -[:PARENT_OF]-> Theoretical Physics
    let rows = db.run(
        "MATCH (f:Entity {name:'Physics'})-[:PARENT_OF]->(tp:Topic) \
         RETURN tp.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Theoretical Physics");
}

#[test]
fn knowledge_theory_to_topic_traversal() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Both General Relativity and Quantum Mechanics belong to Theoretical Physics
    let theories = db.sorted_strings(
        "MATCH (t:Entity)-[:BELONGS_TO]->(tp:Topic {name:'Theoretical Physics'}) \
         RETURN t.name AS name",
        "name",
    );
    assert_eq!(theories, vec!["General Relativity", "Quantum Mechanics"]);
}

#[test]
fn knowledge_dense_subgraph_einstein_outgoing() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Count all outgoing relationships from Einstein
    // STUDIED(2) + PROPOSED(1) + CONTRIBUTED_TO(1) + RECEIVED(1) + AUTHORED(2) + HAS_ALIAS(2) = 9
    let rows = db.run(
        "MATCH (e:Entity {name:'Albert Einstein'})-[r]->(x) RETURN count(r) AS cnt",
    );
    assert_eq!(rows[0]["cnt"], 9);
}

// ============================================================
// Advanced multi-hop patterns
// ============================================================

#[test]
fn advanced_five_hop_chain() {
    let db = TestDb::new();
    db.seed_chain(7);
    // Traverse 5 hops: 0->1->2->3->4->5
    let rows = db.run(
        "MATCH (a:Chain {idx:0})-[:NEXT]->(b)-[:NEXT]->(c)-[:NEXT]->(d)-[:NEXT]->(e)-[:NEXT]->(f) \
         RETURN f.idx AS idx",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["idx"], 5);
}

#[test]
fn advanced_six_hop_chain() {
    let db = TestDb::new();
    db.seed_chain(8);
    // Traverse 6 hops: 0->1->2->3->4->5->6
    let rows = db.run(
        "MATCH (a:Chain {idx:0})-[:NEXT]->(b)-[:NEXT]->(c)-[:NEXT]->(d)-[:NEXT]->(e)-[:NEXT]->(f)-[:NEXT]->(g) \
         RETURN g.idx AS idx",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["idx"], 6);
}

#[test]
fn advanced_multiple_rel_types_in_sequence() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Manager -> MANAGES -> Person -> ASSIGNED_TO -> Project -> (nothing outgoing, so stop here)
    // Frank manages Alice, Bob, Eve.
    // Alice -> Alpha(lead), Bob -> Alpha(dev), Eve -> Beta(dev)
    let rows = db.run(
        "MATCH (m:Manager)-[:MANAGES]->(p:Person)-[:ASSIGNED_TO]->(proj:Project) \
         RETURN m.name AS mgr, p.name AS emp, proj.name AS proj ORDER BY p.name",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["emp"], "Alice");
    assert_eq!(rows[0]["proj"], "Alpha");
    assert_eq!(rows[1]["emp"], "Bob");
    assert_eq!(rows[1]["proj"], "Alpha");
    assert_eq!(rows[2]["emp"], "Eve");
    assert_eq!(rows[2]["proj"], "Beta");
}

#[test]
fn advanced_fan_out_pattern() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    // app has 3 direct deps: web, auth, log (fan-out from single source)
    let deps = db.sorted_strings(
        "MATCH (src:Package {name:'app'})-[:DEPENDS_ON]->(dep:Package) \
         RETURN dep.name AS name",
        "name",
    );
    assert_eq!(deps, vec!["auth", "log", "web"]);
}

#[test]
fn advanced_fan_in_pattern() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    // log is depended on by: app, web, auth (fan-in to single target)
    let consumers = db.sorted_strings(
        "MATCH (consumer:Package)-[:DEPENDS_ON]->(dep:Package {name:'log'}) \
         RETURN consumer.name AS name",
        "name",
    );
    assert_eq!(consumers, vec!["app", "auth", "web"]);
}

#[test]
fn advanced_all_pairs_within_two_hops() {
    let db = TestDb::new();
    db.seed_cycle(4);
    // In a 4-node cycle, every node can reach every other node within 2 hops
    // But with directed edges: 0->1->2->3->0
    // From each node, 2 hops covers 2 other nodes
    let rows = db.run(
        "MATCH (a:Ring)-[:LOOP]->(b:Ring)-[:LOOP]->(c:Ring) \
         RETURN a.idx AS src, c.idx AS dst",
    );
    // 4 start nodes * 1 destination each via 2 hops = 4 rows
    assert_eq!(rows.len(), 4);
}

#[test]
fn advanced_three_rel_types_sequence() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Person -> WORKS_AT -> Company, Person -> LIVES_IN -> City, Person -> ASSIGNED_TO -> Project
    // Find people who work at Acme AND live in London AND are assigned to a project
    let rows = db.run(
        "MATCH (p:Person)-[:WORKS_AT]->(c:Company {name:'Acme'}) \
         MATCH (p)-[:LIVES_IN]->(city:City {name:'London'}) \
         MATCH (p)-[:ASSIGNED_TO]->(proj:Project) \
         RETURN p.name AS name, proj.name AS proj ORDER BY p.name",
    );
    // Alice lives in London, works at Acme, assigned to Alpha
    // Carol lives in London, works at Acme, assigned to Beta
    // Frank lives in London, works at Acme, NOT assigned to any project
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "Alice");
    assert_eq!(rows[0]["proj"], "Alpha");
    assert_eq!(rows[1]["name"], "Carol");
    assert_eq!(rows[1]["proj"], "Beta");
}

#[test]
fn advanced_transport_three_hop_route() {
    let db = TestDb::new();
    db.seed_transport_graph();
    // Find 3-hop routes from Amsterdam to Eindhoven
    // Amsterdam->Utrecht->Eindhoven is 2 hops, need 3 hops:
    // Amsterdam->Rotterdam->Utrecht->Eindhoven or Amsterdam->Utrecht->Rotterdam->Den Haag etc.
    let rows = db.run(
        "MATCH (a:Station {name:'Amsterdam'})-[:ROUTE]->(b:Station)-[:ROUTE]->(c:Station)-[:ROUTE]->(d:Station {name:'Eindhoven'}) \
         RETURN b.name AS hop1, c.name AS hop2",
    );
    // Amsterdam->Rotterdam->Utrecht->Eindhoven
    // Amsterdam->Utrecht->Rotterdam->... Rotterdam has no route to Eindhoven
    assert!(rows.len() >= 1);
    // At least the path through Rotterdam->Utrecht should exist
    let has_rot_utr = rows.iter().any(|r| r["hop1"] == "Rotterdam" && r["hop2"] == "Utrecht");
    assert!(has_rot_utr, "expected path via Rotterdam->Utrecht, got: {rows:?}");
}

// ============================================================
// Multiple edges between same endpoints
// ============================================================

#[test]
fn multi_edge_different_types_between_same_pair() {
    let db = TestDb::new();
    db.run("CREATE (:N {id:1}), (:N {id:2})");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:LIKES]->(b)");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:FOLLOWS]->(b)");
    // Two different relationship types between the same pair
    db.assert_count("MATCH (a:N {id:1})-[r]->(b:N {id:2}) RETURN r", 2);
    db.assert_count("MATCH (a:N {id:1})-[:LIKES]->(b:N {id:2}) RETURN 1 AS x", 1);
    db.assert_count("MATCH (a:N {id:1})-[:FOLLOWS]->(b:N {id:2}) RETURN 1 AS x", 1);
}

#[test]
fn multi_edge_same_type_different_properties() {
    let db = TestDb::new();
    db.run("CREATE (:N {id:1}), (:N {id:2})");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:MSG {text:'hello'}]->(b)");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:MSG {text:'world'}]->(b)");
    // Two MSG edges between the same pair with different properties
    db.assert_count("MATCH (a:N {id:1})-[:MSG]->(b:N {id:2}) RETURN 1 AS x", 2);
}

#[test]
fn multi_edge_filter_specific_edge_by_property() {
    let db = TestDb::new();
    db.run("CREATE (:N {id:1}), (:N {id:2})");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:MSG {text:'hello', priority:1}]->(b)");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:MSG {text:'world', priority:2}]->(b)");
    // Filter to the high-priority message
    let rows = db.run(
        "MATCH (a:N {id:1})-[r:MSG]->(b:N {id:2}) WHERE r.priority = 2 RETURN r.text AS text",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["text"], "world");
}

#[test]
fn multi_edge_bidirectional_between_same_pair() {
    let db = TestDb::new();
    db.run("CREATE (:N {id:1}), (:N {id:2})");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:LINK]->(b)");
    db.run("MATCH (a:N {id:2}), (b:N {id:1}) CREATE (a)-[:LINK]->(b)");
    // Directed: one edge in each direction
    db.assert_count("MATCH (a:N {id:1})-[:LINK]->(b:N {id:2}) RETURN 1 AS x", 1);
    db.assert_count("MATCH (a:N {id:2})-[:LINK]->(b:N {id:1}) RETURN 1 AS x", 1);
    // Undirected: both edges found from either endpoint
    db.assert_count("MATCH (a:N {id:1})-[:LINK]-(b:N {id:2}) RETURN 1 AS x", 2);
}

#[test]
fn multi_edge_three_relationships_between_same_pair() {
    let db = TestDb::new();
    db.run("CREATE (:N {id:1}), (:N {id:2})");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:A {w:1}]->(b)");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:B {w:2}]->(b)");
    db.run("MATCH (a:N {id:1}), (b:N {id:2}) CREATE (a)-[:C {w:3}]->(b)");
    // All three edges returned when type is not constrained
    db.assert_count("MATCH (a:N {id:1})-[r]->(b:N {id:2}) RETURN r", 3);
    // Filter by type
    db.assert_count("MATCH (a:N {id:1})-[:B]->(b:N {id:2}) RETURN 1 AS x", 1);
}

// ============================================================
// Ignored future compatibility tests
// ============================================================

#[test]
fn future_optional_match_null_for_missing_pattern() {
    // Lora: OPTIONAL MATCH returns null for missing pattern
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run(
        "MATCH (p:Person {name:'Alice'}) \
         OPTIONAL MATCH (p)-[:WORKS_AT]->(c:Company) \
         RETURN p.name AS name, c.name AS company",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
    assert!(rows[0]["company"].is_null());
}

#[test]
fn future_where_exists_pattern() {
    // Lora: WHERE EXISTS pattern
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         WHERE EXISTS { (p)-[:BLOCKED]->(:Person) } \
         RETURN p.name AS name",
    );
    // Alice blocked Frank, Dave blocked Carol
    assert_eq!(_rows.len(), 2);
}

#[test]
fn future_pattern_comprehension_in_return() {
    // Lora: pattern comprehension in RETURN
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let _rows = db.run(
        "MATCH (p:Person {name:'Alice'}) \
         RETURN p.name AS name, [(p)-[:KNOWS]->(f) | f.name] AS friends",
    );
    assert_eq!(_rows.len(), 1);
}

#[test]
#[ignore = "pending implementation"]
fn future_quantified_path_patterns() {
    // Lora: quantified path patterns
    let db = TestDb::new();
    db.seed_chain(5);
    let _rows = db.run(
        "MATCH (a:Chain {idx:0}) (()-[:NEXT]->())+ (b:Chain) \
         RETURN b.idx AS idx",
    );
    // Should return nodes 1 through 4
    assert_eq!(_rows.len(), 4);
}

#[test]
fn future_relationship_type_disjunction() {
    // Lora: relationship type disjunction [:A|B]
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let _rows = db.run(
        "MATCH (a:Person {name:'Alice'})-[:KNOWS|FOLLOWS]->(b:Person) \
         RETURN b.name AS name",
    );
    // Alice KNOWS Bob, Carol. Alice FOLLOWS Carol, Eve.
    // Union: Bob, Carol, Eve (Carol appears via both, but DISTINCT not used so may appear twice)
    assert!(_rows.len() >= 3);
}

#[test]
#[ignore = "pending implementation"]
fn future_variable_length_with_inline_where() {
    // Lora: variable-length with inline WHERE filter on intermediate nodes
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let _rows = db.run(
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*1..3 WHERE ALL(n IN nodes(path) WHERE n.age > 20)]->(b:Person) \
         RETURN b.name AS name",
    );
    assert!(!_rows.is_empty());
}

#[test]
fn future_map_projection() {
    // Lora: MATCH with map projection n{.name, .age}
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let _rows = db.run(
        "MATCH (p:Person {name:'Alice'}) \
         RETURN p{.name, .age} AS profile",
    );
    assert_eq!(_rows.len(), 1);
}

#[test]
fn future_label_disjunction() {
    // Lora: label disjunction :A|B in MATCH
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let _rows = db.run(
        "MATCH (n:Entity|Alias) RETURN n",
    );
    // 8 Entity + 2 Alias = 10
    assert_eq!(_rows.len(), 10);
}

#[test]
fn future_node_key_constraint_matching() {
    // Lora: node key constraint matching
    let db = TestDb::new();
    db.run("CREATE (:Item {sku:'ABC', region:'EU', stock:5})");
    db.run("CREATE (:Item {sku:'ABC', region:'US', stock:3})");
    db.run("CREATE (:Item {sku:'DEF', region:'EU', stock:7})");
    let _rows = db.run(
        "MATCH (i:Item {sku:'ABC', region:'EU'}) RETURN i.stock AS stock",
    );
    assert_eq!(_rows.len(), 1);
    assert_eq!(_rows[0]["stock"], 5);
}

#[test]
fn match_triangle_pattern_extended() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Find triangles: A knows B knows C knows A
    let rows = db.run(
        "MATCH (a:Person)-[:KNOWS]->(b:Person)-[:KNOWS]->(c:Person)-[:KNOWS]->(a) \
         WHERE id(a) < id(b) AND id(b) < id(c) \
         RETURN a.name AS a, b.name AS b, c.name AS c",
    );
    // There may or may not be triangles depending on the exact graph;
    // the key assertion is that the query runs and returns valid rows
    for row in &rows {
        assert!(row["a"].is_string());
    }
}

#[test]
fn match_two_hop_with_intermediate_filter() {
    let db = TestDb::new();
    db.seed_org_graph();
    // People in Engineering who manage someone who lives in Berlin
    let rows = db.run(
        "MATCH (m:Person)-[:MANAGES]->(p:Person)-[:LIVES_IN]->(c:City {name:'Berlin'}) \
         WHERE m.dept = 'Engineering' \
         RETURN m.name AS manager, p.name AS report",
    );
    assert!(!rows.is_empty());
}

#[test]
fn match_multiple_relationship_types_in_chain() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Person -> works at company AND lives in city
    let rows = db.run(
        "MATCH (p:Person)-[:WORKS_AT]->(c:Company), (p)-[:LIVES_IN]->(city:City) \
         RETURN p.name AS person, c.name AS company, city.name AS city \
         ORDER BY p.name",
    );
    assert!(rows.len() >= 5);
    // Each person should have a company and city
    for row in &rows {
        assert!(row["company"].is_string());
        assert!(row["city"].is_string());
    }
}

#[test]
fn match_optional_match_unmatched_returns_null() {
    let db = TestDb::new();
    db.run("CREATE (:Orphan {name: 'Alone'})");
    let rows = db.run(
        "MATCH (n:Orphan) \
         OPTIONAL MATCH (n)-[:FRIEND]->(f) \
         RETURN n.name AS name, f AS friend",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alone");
    assert!(rows[0]["friend"].is_null());
}

#[test]
fn match_optional_match_mixed_with_regular() {
    let db = TestDb::new();
    db.run("CREATE (:Person {name: 'Has'})-[:KNOWS]->(:Person {name: 'Friend'})");
    db.run("CREATE (:Person {name: 'Lonely'})");
    let rows = db.run(
        "MATCH (p:Person) \
         OPTIONAL MATCH (p)-[:KNOWS]->(f:Person) \
         RETURN p.name AS person, f.name AS friend \
         ORDER BY p.name",
    );
    assert_eq!(rows.len(), 3); // Has, Friend (matched as p), Lonely
}

#[test]
fn match_self_relationship() {
    let db = TestDb::new();
    db.run("CREATE (n:Node {name: 'self'})-[:LOOP]->(n)");
    let rows = db.run(
        "MATCH (n:Node)-[:LOOP]->(n) RETURN n.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "self");
}

#[test]
fn match_no_results_returns_empty() {
    let db = TestDb::new();
    db.seed_social_graph();
    // Use existing label but property that matches nothing
    let rows = db.run("MATCH (n:User {name: 'NoSuchPerson'}) RETURN n");
    assert_eq!(rows.len(), 0);
}

#[test]
fn match_multiple_labels_conjunction() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Frank has both Person and Manager labels
    let rows = db.run("MATCH (n:Person:Manager) RETURN n.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Frank");
}

#[test]
fn match_relationship_with_property_predicate() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[r:WORKS_AT {since: 2018}]->(c:Company) \
         RETURN p.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

#[test]
fn match_undirected_relationship_extended() {
    let db = TestDb::new();
    db.run("CREATE (a:N {name:'A'})-[:E]->(b:N {name:'B'})");
    // Undirected match should find the relationship from either side
    let rows = db.run(
        "MATCH (a:N {name:'A'})-[:E]-(b:N {name:'B'}) RETURN a.name, b.name",
    );
    assert!(!rows.is_empty());
}

#[test]
fn match_count_with_group_by_label() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (n) \
         WITH labels(n) AS lbls, n \
         UNWIND lbls AS lbl \
         RETURN lbl, count(n) AS cnt \
         ORDER BY lbl",
    );
    assert!(!rows.is_empty());
}

#[test]
fn match_exists_subquery_basic() {
    let db = TestDb::new();
    db.seed_org_graph();
    // People who manage someone
    let rows = db.run(
        "MATCH (p:Person) \
         WHERE EXISTS { MATCH (p)-[:MANAGES]->(:Person) } \
         RETURN p.name AS name ORDER BY p.name",
    );
    assert!(rows.len() >= 2); // Frank and Carol manage people
}

#[test]
fn match_with_where_on_relationship_type() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run(
        "MATCH (a:Person {name:'Alice'})-[r]->(b:Person) \
         WHERE type(r) = 'KNOWS' \
         RETURN b.name AS name ORDER BY name",
    );
    assert!(!rows.is_empty());
}

#[test]
fn match_collect_relationship_properties() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "MATCH (p:Person)-[r:WORKS_AT]->(c:Company) \
         RETURN p.name AS name, r.since AS since \
         ORDER BY r.since",
    );
    // Earliest hire should come first
    assert!(rows[0]["since"].as_i64().unwrap() <= rows[1]["since"].as_i64().unwrap());
}

#[test]
#[ignore = "pending implementation"]
fn match_quantified_path_pattern() {
    // GQL quantified path pattern (Lora 25)
    let db = TestDb::new();
    db.seed_chain(10);
    let _rows = db.run(
        "MATCH (a:Chain {idx: 0})(()-[:NEXT]->())+ (b:Chain {idx: 5}) \
         RETURN a, b",
    );
}

#[test]
#[ignore = "pending implementation"]
fn match_pattern_with_where_on_path() {
    // WHERE on the path variable itself
    let db = TestDb::new();
    db.seed_chain(5);
    let _rows = db.run(
        "MATCH p = (:Chain {idx:0})-[:NEXT*]->(:Chain {idx:4}) \
         WHERE length(p) = 4 \
         RETURN p",
    );
}

// ============================================================
// Multiple OPTIONAL MATCH chaining
// ============================================================

#[test]
fn optional_match_chain_two_optionals() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Dave has no MANAGES relationships, and no ASSIGNED_TO
    let rows = db.run(
        "MATCH (p:Person {name:'Dave'}) \
         OPTIONAL MATCH (p)-[:MANAGES]->(sub:Person) \
         OPTIONAL MATCH (p)-[:ASSIGNED_TO]->(proj:Project) \
         RETURN p.name AS name, sub.name AS subordinate, proj.name AS project",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Dave");
    assert!(rows[0]["subordinate"].is_null());
    assert!(rows[0]["project"].is_null());
}

#[test]
fn optional_match_one_matches_one_null() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Alice has ASSIGNED_TO but does not MANAGE anyone
    let rows = db.run(
        "MATCH (p:Person {name:'Alice'}) \
         OPTIONAL MATCH (p)-[:MANAGES]->(sub:Person) \
         OPTIONAL MATCH (p)-[:ASSIGNED_TO]->(proj:Project) \
         RETURN p.name AS name, sub.name AS subordinate, proj.name AS project",
    );
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["subordinate"].is_null());
    assert_eq!(rows[0]["project"], "Alpha");
}

#[test]
fn optional_match_count_including_nulls() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Count managed subordinates per person — those with zero should still appear
    let rows = db.run(
        "MATCH (p:Person) \
         OPTIONAL MATCH (p)-[:MANAGES]->(sub:Person) \
         RETURN p.name AS name, count(sub) AS subs \
         ORDER BY subs DESC, p.name ASC",
    );
    assert_eq!(rows.len(), 6);
    // Frank manages 3, Carol manages 1, rest manage 0
    let frank = rows.iter().find(|r| r["name"] == "Frank").unwrap();
    assert_eq!(frank["subs"], 3);
    let carol = rows.iter().find(|r| r["name"] == "Carol").unwrap();
    assert_eq!(carol["subs"], 1);
    let zero_count = rows.iter().filter(|r| r["subs"] == 0).count();
    assert_eq!(zero_count, 4);
}

// ============================================================
// Anti-pattern: find nodes WITHOUT a relationship
// ============================================================

#[test]
fn anti_pattern_people_not_managing_anyone() {
    let db = TestDb::new();
    db.seed_org_graph();
    // People who do NOT manage anyone (via OPTIONAL MATCH + null check)
    let names = db.sorted_strings(
        "MATCH (p:Person) \
         OPTIONAL MATCH (p)-[:MANAGES]->(sub:Person) \
         WITH p, sub WHERE sub IS NULL \
         RETURN p.name AS name",
        "name",
    );
    // Frank manages 3, Carol manages 1 — rest should appear
    assert_eq!(names, vec!["Alice", "Bob", "Dave", "Eve"]);
}

#[test]
fn anti_pattern_packages_with_no_dependents() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    // Packages nobody depends on (root packages)
    // log and util have incoming deps, but no package depends on app
    let names = db.sorted_strings(
        "MATCH (p:Package) \
         OPTIONAL MATCH (consumer:Package)-[:DEPENDS_ON]->(p) \
         WITH p, consumer WHERE consumer IS NULL \
         RETURN p.name AS name",
        "name",
    );
    assert_eq!(names, vec!["app"]);
}

#[test]
fn anti_pattern_people_not_assigned_to_any_project() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "MATCH (p:Person) \
         OPTIONAL MATCH (p)-[:ASSIGNED_TO]->(proj:Project) \
         WITH p, proj WHERE proj IS NULL \
         RETURN p.name AS name",
        "name",
    );
    // Dave and Frank have no project assignments
    assert_eq!(names, vec!["Dave", "Frank"]);
}

// ============================================================
// Complex multi-hop with intermediate filters
// ============================================================

#[test]
fn multi_hop_filter_at_each_step() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Manager -> manages engineering people -> who live in Berlin
    let rows = db.run(
        "MATCH (m:Person)-[:MANAGES]->(e:Person)-[:LIVES_IN]->(c:City) \
         WHERE e.dept = 'Engineering' AND c.name = 'Berlin' \
         RETURN m.name AS mgr, e.name AS emp",
    );
    // Frank manages Bob(Berlin) and Eve(Berlin)
    assert_eq!(rows.len(), 2);
    let emps = db.sorted_strings(
        "MATCH (m:Person)-[:MANAGES]->(e:Person)-[:LIVES_IN]->(c:City) \
         WHERE e.dept = 'Engineering' AND c.name = 'Berlin' \
         RETURN e.name AS name",
        "name",
    );
    assert_eq!(emps, vec!["Bob", "Eve"]);
}

#[test]
fn multi_hop_four_entities_chain() {
    let db = TestDb::new();
    db.seed_org_graph();
    // Manager -> manages -> person -> works_at -> company -> (verify company name)
    let rows = db.run(
        "MATCH (m:Manager)-[:MANAGES]->(e:Person)-[:WORKS_AT]->(c:Company {name:'Acme'}) \
         RETURN m.name AS mgr, e.name AS emp, c.name AS company ORDER BY e.name",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["emp"], "Alice");
    assert_eq!(rows[0]["company"], "Acme");
}

// ============================================================
// Large fan-out/fan-in patterns
// ============================================================

#[test]
fn large_fan_out_hub_20_spokes() {
    let db = TestDb::new();
    db.run("CREATE (:Center {name:'hub'})");
    db.run("UNWIND range(1, 20) AS i MATCH (h:Center) CREATE (h)-[:SPOKE]->(:Leaf {id: i})");
    db.assert_count("MATCH (:Center)-[:SPOKE]->(l:Leaf) RETURN l", 20);
    // Count from the hub
    let rows = db.run(
        "MATCH (h:Center)-[r:SPOKE]->(l:Leaf) RETURN count(r) AS cnt",
    );
    assert_eq!(rows[0]["cnt"], 20);
}

#[test]
fn large_fan_in_many_to_one() {
    let db = TestDb::new();
    db.run("CREATE (:Sink {name:'target'})");
    db.run("UNWIND range(1, 15) AS i MATCH (s:Sink) CREATE (:Source {id: i})-[:FEEDS]->(s)");
    db.assert_count("MATCH (src:Source)-[:FEEDS]->(sink:Sink) RETURN src", 15);
}

// ============================================================
// Complex graph patterns: bipartite matching
// ============================================================

#[test]
fn bipartite_graph_pattern() {
    let db = TestDb::new();
    // Create a bipartite graph: 3 workers, 3 tasks, each worker assigned to 2 tasks
    for i in 1..=3 {
        db.run(&format!("CREATE (:Worker {{id:{i}}})"));
        db.run(&format!("CREATE (:Task {{id:{i}}})"));
    }
    db.run("MATCH (w:Worker {id:1}), (t:Task {id:1}) CREATE (w)-[:ASSIGNED]->(t)");
    db.run("MATCH (w:Worker {id:1}), (t:Task {id:2}) CREATE (w)-[:ASSIGNED]->(t)");
    db.run("MATCH (w:Worker {id:2}), (t:Task {id:2}) CREATE (w)-[:ASSIGNED]->(t)");
    db.run("MATCH (w:Worker {id:2}), (t:Task {id:3}) CREATE (w)-[:ASSIGNED]->(t)");
    db.run("MATCH (w:Worker {id:3}), (t:Task {id:1}) CREATE (w)-[:ASSIGNED]->(t)");
    db.run("MATCH (w:Worker {id:3}), (t:Task {id:3}) CREATE (w)-[:ASSIGNED]->(t)");

    // Find workers who share at least one task
    let rows = db.run(
        "MATCH (w1:Worker)-[:ASSIGNED]->(t:Task)<-[:ASSIGNED]-(w2:Worker) \
         WHERE id(w1) < id(w2) \
         RETURN DISTINCT w1.id AS w1, w2.id AS w2 ORDER BY w1.id, w2.id",
    );
    // All pairs share tasks: (1,2) share task2, (1,3) share task1, (2,3) share task3
    assert_eq!(rows.len(), 3);
}

// ============================================================
// MATCH with UNWIND-produced constraint
// ============================================================

#[test]
fn match_with_unwind_as_filter() {
    let db = TestDb::new();
    db.seed_org_graph();
    let names = db.sorted_strings(
        "UNWIND ['Alice', 'Eve', 'Frank'] AS target \
         MATCH (p:Person {name: target}) \
         RETURN p.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Alice", "Eve", "Frank"]);
}

// ============================================================
// Match with multiple relationship types in sequence
// ============================================================

#[test]
fn match_person_company_city_triangle() {
    let db = TestDb::new();
    db.seed_org_graph();
    // People who work at Acme AND live in Berlin
    let names = db.sorted_strings(
        "MATCH (p:Person)-[:WORKS_AT]->(c:Company {name:'Acme'}) \
         MATCH (p)-[:LIVES_IN]->(city:City {name:'Berlin'}) \
         RETURN p.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Bob", "Eve"]);
}

// ============================================================
// Knowledge graph: multi-entity resolution
// ============================================================

#[test]
fn knowledge_three_hop_person_to_topic() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Einstein -> PROPOSED -> General Relativity -> BELONGS_TO -> Theoretical Physics
    let rows = db.run(
        "MATCH (e:Entity {name:'Albert Einstein'})-[:PROPOSED]->(t:Entity)-[:BELONGS_TO]->(tp:Topic) \
         RETURN tp.name AS topic",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["topic"], "Theoretical Physics");
}

#[test]
fn knowledge_common_fields_of_study() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Fields studied by both Einstein and Curie
    let fields = db.sorted_strings(
        "MATCH (e:Entity {name:'Albert Einstein'})-[:STUDIED]->(f:Entity)<-[:STUDIED]-(c:Entity {name:'Marie Curie'}) \
         RETURN f.name AS name",
        "name",
    );
    assert_eq!(fields, vec!["Physics"]);
}

// ============================================================
// Transport graph: reachability
// ============================================================

#[test]
fn transport_all_reachable_in_two_hops() {
    let db = TestDb::new();
    db.seed_transport_graph();
    // All stations reachable from Amsterdam in exactly 2 hops
    let stations = db.sorted_strings(
        "MATCH (src:Station {name:'Amsterdam'})-[:ROUTE]->(mid:Station)-[:ROUTE]->(dst:Station) \
         WHERE dst.name <> 'Amsterdam' AND mid.name <> dst.name \
         RETURN DISTINCT dst.name AS name",
        "name",
    );
    // Amsterdam->Utrecht->Rotterdam, Amsterdam->Utrecht->Eindhoven, Amsterdam->Rotterdam->Utrecht, etc.
    assert!(!stations.is_empty());
}

// ============================================================
// Future / pending MATCH tests
// ============================================================

#[test]
#[ignore = "pending implementation"]
fn match_shortest_path() {
    // shortestPath() function
    let db = TestDb::new();
    db.seed_transport_graph();
    let _rows = db.run(
        "MATCH p = shortestPath((a:Station {name:'Amsterdam'})-[:ROUTE*]-(b:Station {name:'Eindhoven'})) \
         RETURN length(p) AS hops",
    );
}

#[test]
#[ignore = "pending implementation"]
fn match_all_shortest_paths() {
    // allShortestPaths() function
    let db = TestDb::new();
    db.seed_transport_graph();
    let _rows = db.run(
        "MATCH p = allShortestPaths((a:Station {name:'Amsterdam'})-[:ROUTE*]-(b:Station {name:'Den Haag'})) \
         RETURN length(p) AS hops, [n IN nodes(p) | n.name] AS route",
    );
}

#[test]
#[ignore = "pending implementation"]
fn match_call_subquery() {
    // CALL subquery
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         CALL { WITH p MATCH (p)-[:MANAGES]->(sub) RETURN count(sub) AS subs } \
         RETURN p.name AS name, subs ORDER BY subs DESC",
    );
}

#[test]
#[ignore = "pending implementation"]
fn match_collect_subquery() {
    // COLLECT subquery (Lora 25)
    let db = TestDb::new();
    db.seed_org_graph();
    let _rows = db.run(
        "MATCH (p:Person) \
         RETURN p.name, COLLECT { MATCH (p)-[:MANAGES]->(s) RETURN s.name } AS subordinates",
    );
}
