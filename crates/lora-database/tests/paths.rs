/// Path tests — variable-length traversal, range bounds, cycles, direction,
/// path materialization (pending).
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// Variable-length: fixed range on linear chains
// ============================================================

#[test]
fn varlen_one_hop_same_as_single() {
    let db = TestDb::new();
    db.seed_chain(5);
    db.assert_count("MATCH (a:Chain {idx:0})-[:NEXT*1..1]->(b) RETURN b", 1);
}

#[test]
fn varlen_exact_distance() {
    let db = TestDb::new();
    db.seed_chain(5);
    let rows = db.run("MATCH (a:Chain {idx:0})-[:NEXT*3..3]->(b) RETURN b.idx AS idx");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["idx"], 3);
}

#[test]
fn varlen_range_accumulates() {
    let db = TestDb::new();
    db.seed_chain(5);
    db.assert_count("MATCH (a:Chain {idx:0})-[:NEXT*1..4]->(b) RETURN b", 4);
}

#[test]
fn varlen_from_middle() {
    let db = TestDb::new();
    db.seed_chain(5);
    let idxs = db.sorted_ints(
        "MATCH (a:Chain {idx:2})-[:NEXT*1..2]->(b) RETURN b.idx AS idx",
        "idx",
    );
    assert_eq!(idxs, vec![3, 4]);
}

#[test]
fn varlen_unbounded_reaches_all() {
    let db = TestDb::new();
    db.seed_chain(5);
    db.assert_count("MATCH (a:Chain {idx:0})-[:NEXT*]->(b) RETURN b", 4);
}

// ============================================================
// Variable-length: social graph
// ============================================================

#[test]
fn variable_length_path_fixed_range() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name: 'Alice'})-[:FOLLOWS*1..2]->(b) RETURN b");
    assert_eq!(rows.len(), 2);
}

#[test]
fn variable_length_path_unbounded() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name: 'Alice'})-[:FOLLOWS*]->(b) RETURN b");
    assert_eq!(rows.len(), 2);
}

// ============================================================
// Variable-length: zero-hop
// ============================================================

#[test]
fn varlen_zero_hop_includes_start() {
    let db = TestDb::new();
    db.seed_chain(3);
    let idxs = db.sorted_ints(
        "MATCH (a:Chain {idx:0})-[:NEXT*0..1]->(b) RETURN b.idx AS idx",
        "idx",
    );
    assert_eq!(idxs, vec![0, 1]);
}

#[test]
fn varlen_zero_to_zero_only_start() {
    let db = TestDb::new();
    db.seed_chain(3);
    let idxs = db.sorted_ints(
        "MATCH (a:Chain {idx:1})-[:NEXT*0..0]->(b) RETURN b.idx AS idx",
        "idx",
    );
    assert_eq!(idxs, vec![1]);
}

#[test]
fn variable_length_zero_hops() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH (a:User {name: 'Alice'})-[:FOLLOWS*0..1]->(b) RETURN b");
    assert_eq!(rows.len(), 2); // Alice (0 hops) + Bob (1 hop)
}

// ============================================================
// Variable-length: cycles
// ============================================================

#[test]
fn varlen_in_cycle_terminates() {
    let db = TestDb::new();
    db.seed_cycle(3);
    let count = db
        .exec_count("MATCH (a:Ring {idx:0})-[:LOOP*1..3]->(b) RETURN b")
        .unwrap();
    assert_eq!(count, 3);
}

#[test]
fn varlen_cycle_does_not_revisit_relationships() {
    let db = TestDb::new();
    db.seed_cycle(3);
    let count = db
        .exec_count("MATCH (a:Ring {idx:0})-[:LOOP*1..10]->(b) RETURN b")
        .unwrap();
    assert_eq!(count, 3);
}

// ============================================================
// Variable-length: direction
// ============================================================

#[test]
fn varlen_reverse_direction() {
    let db = TestDb::new();
    db.seed_chain(5);
    let idxs = db.sorted_ints(
        "MATCH (a:Chain {idx:4})<-[:NEXT*1..2]-(b) RETURN b.idx AS idx",
        "idx",
    );
    assert_eq!(idxs, vec![2, 3]);
}

// ============================================================
// Variable-length: no results
// ============================================================

#[test]
fn varlen_no_outgoing_returns_empty() {
    let db = TestDb::new();
    db.seed_chain(5);
    db.assert_count("MATCH (a:Chain {idx:4})-[:NEXT*1..3]->(b) RETURN b", 0);
}

#[test]
fn varlen_wrong_type_returns_error_on_nonempty_graph() {
    let db = TestDb::new();
    db.seed_chain(5);
    let err = db.run_err("MATCH (a:Chain {idx:0})-[:FRIEND*1..3]->(b) RETURN b");
    assert!(err.contains("Unknown relationship type"));
}

// ============================================================
// Variable-length: long chain
// ============================================================

#[test]
fn varlen_long_chain_20_nodes() {
    let db = TestDb::new();
    db.seed_chain(20);
    db.assert_count("MATCH (a:Chain {idx:0})-[:NEXT*1..19]->(b) RETURN b", 19);
}

// ============================================================
// Path materialization (pending)
// ============================================================

#[test]
fn path_variable_returned_as_structured_value() {
    let db = TestDb::new();
    db.seed_chain(3);
    let rows = db.run("MATCH p = (a:Chain {idx:0})-[:NEXT]->(b:Chain) RETURN p");
    assert_eq!(rows.len(), 1);
}

#[test]
fn path_variable_contains_nodes_and_rels() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH p = (a:User {name: 'Alice'})-[:FOLLOWS]->(b) RETURN p");
    assert_eq!(rows.len(), 1);
}

#[test]
fn path_length_function() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run(
        "MATCH p = (a:User {name: 'Alice'})-[:FOLLOWS]->(b)-[:FOLLOWS]->(c) RETURN length(p) AS len",
    );
    assert_eq!(rows[0]["len"], 2);
}

#[test]
fn path_length_on_matched_path() {
    let db = TestDb::new();
    db.seed_chain(4);
    let rows = db.run(
        "MATCH p = (a:Chain {idx:0})-[:NEXT*1..3]->(b:Chain) RETURN length(p) AS len, b.idx AS idx",
    );
    assert!(rows.len() >= 3);
}

#[test]
fn path_nodes_function() {
    let db = TestDb::new();
    db.seed_social_graph();
    let rows = db.run("MATCH p = (a:User {name: 'Alice'})-[:FOLLOWS]->(b) RETURN nodes(p) AS ns");
    let ns = rows[0]["ns"].as_array().unwrap();
    assert_eq!(ns.len(), 2);
}

#[test]
fn path_nodes_extraction() {
    let db = TestDb::new();
    db.seed_chain(3);
    let rows = db.run("MATCH p = (a:Chain {idx:0})-[:NEXT*1..2]->(b) RETURN nodes(p) AS ns");
    assert!(!rows.is_empty());
}

#[test]
fn path_relationships_extraction() {
    let db = TestDb::new();
    db.seed_chain(3);
    let rows =
        db.run("MATCH p = (a:Chain {idx:0})-[:NEXT*1..2]->(b) RETURN relationships(p) AS rels");
    assert!(!rows.is_empty());
}

// ============================================================
// Variable-length: diamond graph
// ============================================================

#[test]
fn varlen_diamond_multiple_paths() {
    let db = TestDb::new();
    db.run("CREATE (:D {name:'top'})");
    db.run("CREATE (:D {name:'left'})");
    db.run("CREATE (:D {name:'right'})");
    db.run("CREATE (:D {name:'bottom'})");
    db.run("MATCH (t:D {name:'top'}), (l:D {name:'left'}) CREATE (t)-[:E]->(l)");
    db.run("MATCH (t:D {name:'top'}), (r:D {name:'right'}) CREATE (t)-[:E]->(r)");
    db.run("MATCH (l:D {name:'left'}), (b:D {name:'bottom'}) CREATE (l)-[:E]->(b)");
    db.run("MATCH (r:D {name:'right'}), (b:D {name:'bottom'}) CREATE (r)-[:E]->(b)");
    // Variable-length 1..2 from top should find: left(1), right(1), bottom(2), bottom(2) = 4 unique paths
    let count = db
        .exec_count("MATCH (src:D {name:'top'})-[:E*1..2]->(d:D) RETURN d.name AS name")
        .unwrap();
    // left, right at dist 1; bottom via left and bottom via right at dist 2 = 4
    assert_eq!(count, 4);
}

// ============================================================
// Variable-length: transport graph
// ============================================================

#[test]
fn varlen_transport_reachable_within_two_hops() {
    let db = TestDb::new();
    db.seed_transport_graph();
    // From Amsterdam, what stations are reachable within 2 hops?
    let stations = db.sorted_strings(
        "MATCH (src:Station {name:'Amsterdam'})-[:ROUTE*1..2]->(dest:Station) \
         RETURN DISTINCT dest.name AS name",
        "name",
    );
    // 1 hop: Utrecht, Rotterdam
    // 2 hops: Utrecht->Rotterdam, Utrecht->Eindhoven, Rotterdam->Den Haag, Rotterdam->Utrecht, etc.
    assert!(stations.contains(&"Den Haag".to_string()));
    assert!(stations.contains(&"Eindhoven".to_string()));
}

// ============================================================
// Variable-length: dependency graph transitive closure
// ============================================================

#[test]
fn varlen_all_transitive_dependencies() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    let deps = db.sorted_strings(
        "MATCH (src:Package {name:'app'})-[:DEPENDS_ON*]->(dep:Package) \
         RETURN DISTINCT dep.name AS name",
        "name",
    );
    // app -> web, auth, log (direct)
    // web -> log, util; auth -> crypto, log; crypto -> util
    // All: web, auth, log, util, crypto = 5
    assert_eq!(deps, vec!["auth", "crypto", "log", "util", "web"]);
}

// ============================================================
// Variable-length: undirected
// ============================================================

#[test]
fn varlen_undirected_traversal() {
    let db = TestDb::new();
    db.seed_chain(4);
    // From node 2, undirected *1..1 should find nodes 1 and 3
    let idxs = db.sorted_ints(
        "MATCH (a:Chain {idx:2})-[:NEXT*1..1]-(b:Chain) RETURN b.idx AS idx",
        "idx",
    );
    assert_eq!(idxs, vec![1, 3]);
}

// ============================================================
// Variable-length: upper-bound only (e.g. *..3)
// ============================================================

#[test]
fn varlen_upper_bound_only() {
    let db = TestDb::new();
    db.seed_chain(5);
    // *..3 means *1..3 (default lower bound is 1)
    let count = db
        .exec_count("MATCH (a:Chain {idx:0})-[:NEXT*..3]->(b:Chain) RETURN b")
        .unwrap();
    assert_eq!(count, 3);
}

// ============================================================
// Variable-length: lower-bound only (e.g. *2..)
// ============================================================

#[test]
fn varlen_lower_bound_only() {
    let db = TestDb::new();
    db.seed_chain(5);
    // *2.. means at least 2 hops (up to max)
    let count = db
        .exec_count("MATCH (a:Chain {idx:0})-[:NEXT*2..]->(b:Chain) RETURN b")
        .unwrap();
    // nodes 2, 3, 4 = 3
    assert_eq!(count, 3);
}

// ============================================================
// Pending: shortest path
// ============================================================

#[test]
fn shortest_path_between_two_nodes() {
    let db = TestDb::new();
    db.seed_transport_graph();
    let _rows = db.run(
        "MATCH p = shortestPath((:Station {name:'Amsterdam'})-[:ROUTE*]->(:Station {name:'Den Haag'})) \
         RETURN length(p) AS hops",
    );
}

#[test]
fn all_shortest_paths_between_two_nodes() {
    let db = TestDb::new();
    db.seed_transport_graph();
    let _rows = db.run(
        "MATCH p = allShortestPaths((:Station {name:'Amsterdam'})-[:ROUTE*]->(:Station {name:'Den Haag'})) \
         RETURN p",
    );
}

// ============================================================
// Variable-length: exact zero
// ============================================================

#[test]
fn varlen_exact_zero_returns_source() {
    let db = TestDb::new();
    db.seed_chain(3);
    let rows = db.run("MATCH (a:Chain {idx:0})-[:NEXT*0..0]->(b) RETURN b.idx AS idx");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["idx"], 0);
}

// ============================================================
// Variable-length: knowledge graph traversal
// ============================================================

#[test]
fn knowledge_graph_einstein_authored_documents_about_theory() {
    // Transitive traversal: Entity -> AUTHORED -> Document -> ABOUT -> Theory
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let theories = db.sorted_strings(
        "MATCH (e:Entity {name:'Albert Einstein'})-[:AUTHORED]->(d:Document)-[:ABOUT]->(t:Entity) \
         RETURN DISTINCT t.name AS name",
        "name",
    );
    // Both documents are ABOUT General Relativity
    assert_eq!(theories, vec!["General Relativity"]);
}

#[test]
fn knowledge_graph_reachable_from_einstein_within_3_hops() {
    // All entities/nodes reachable from Einstein within 3 hops (any rel type)
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let count = db
        .exec_count(
            "MATCH (e:Entity {name:'Albert Einstein'})-[*1..3]->(target) \
         RETURN DISTINCT target",
        )
        .unwrap();
    // 1 hop: Physics, Mathematics, General Relativity, Quantum Mechanics, Nobel Prize,
    //        Doc1905, Doc1916, AliasEinstein, AliasA.Einstein = 9
    // 2 hop: From Physics->Theoretical Physics; From GR->Theoretical Physics, Cosmology;
    //        From QM->Theoretical Physics; From Doc1905->GR; From Doc1916->GR = new: Theoretical Physics, Cosmology
    // 3 hop: From Theoretical Physics (no outgoing?), Cosmology (no outgoing?) ... etc.
    // At least 9 nodes reachable at 1 hop
    assert!(count >= 9);
}

#[test]
fn knowledge_graph_varlen_across_any_relationship_type() {
    // Variable-length with no type filter: [*1..2] from General Relativity
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let names = db.sorted_strings(
        "MATCH (e:Entity {name:'General Relativity'})-[*1..2]->(target) \
         RETURN DISTINCT target.name AS name",
        "name",
    );
    // 1 hop: GR->BELONGS_TO->Theoretical Physics (topic), GR->RELATES_TO->Cosmology (topic)
    // 2 hop: from those topics, no further outgoing = still 2
    // But target.name on Topic nodes = "Theoretical Physics", "Cosmology"
    assert!(names.contains(&"Theoretical Physics".to_string()));
    assert!(names.contains(&"Cosmology".to_string()));
}

#[test]
fn knowledge_graph_count_reachable_at_each_depth() {
    // Count reachable nodes at depth 1 vs depth 2 from Einstein
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let hop1 = db
        .exec_count(
            "MATCH (e:Entity {name:'Albert Einstein'})-[*1..1]->(target) RETURN DISTINCT target",
        )
        .unwrap();
    let hop2 = db
        .exec_count(
            "MATCH (e:Entity {name:'Albert Einstein'})-[*1..2]->(target) RETURN DISTINCT target",
        )
        .unwrap();
    // More nodes reachable at 2 hops than at 1 hop
    assert!(hop2 >= hop1);
    // Einstein has 9 direct neighbors
    assert_eq!(hop1, 9);
}

// ============================================================
// Variable-length: rich social graph
// ============================================================

#[test]
fn rich_social_knows_within_3_hops_from_alice() {
    // KNOWS*1..3 from Alice
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let names = db.sorted_strings(
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*1..3]->(p:Person) \
         RETURN DISTINCT p.name AS name",
        "name",
    );
    // 1 hop: Bob, Carol
    // 2 hop: Bob->Carol (dup), Bob->Dave, Carol->Eve
    // 3 hop: Dave->Eve (dup), Eve->Frank
    // Distinct: Bob, Carol, Dave, Eve, Frank
    assert_eq!(names, vec!["Bob", "Carol", "Dave", "Eve", "Frank"]);
}

#[test]
fn rich_social_friends_of_friends_all_at_distance_2() {
    // Friends-of-friends of Alice at exactly distance 2 (without exclusion filter)
    let db = TestDb::new();
    db.seed_rich_social_graph();
    // Alice KNOWS*2..2: Alice->Bob->Carol, Alice->Bob->Dave, Alice->Carol->Eve
    // Distinct results: Carol, Dave, Eve
    let fof = db.sorted_strings(
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*2..2]->(fof:Person) \
         RETURN DISTINCT fof.name AS name",
        "name",
    );
    assert_eq!(fof, vec!["Carol", "Dave", "Eve"]);
}

#[test]
fn rich_social_count_reachable_at_distance_2() {
    // Count of reachable people at exactly distance 2 via KNOWS
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let count = db
        .exec_count(
            "MATCH (a:Person {name:'Alice'})-[:KNOWS*2..2]->(p:Person) \
         RETURN DISTINCT p.name AS name",
        )
        .unwrap();
    // Alice->Bob->Carol, Alice->Bob->Dave, Alice->Carol->Eve = 3 distinct (Carol, Dave, Eve)
    assert_eq!(count, 3);
}

#[test]
fn rich_social_knows_single_hop_only() {
    // Direct KNOWS from Alice
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let names = db.sorted_strings(
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*1..1]->(p:Person) \
         RETURN p.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Bob", "Carol"]);
}

// ============================================================
// Variable-length: edge cases
// ============================================================

#[test]
fn varlen_large_cycle_with_bounded_range() {
    // Large cycle (10 nodes) with bounded varlen
    let db = TestDb::new();
    db.seed_cycle(10);
    let count = db
        .exec_count("MATCH (a:Ring {idx:0})-[:LOOP*1..5]->(b) RETURN b")
        .unwrap();
    // Can reach nodes at distances 1..5: that's 5 distinct nodes
    assert_eq!(count, 5);
}

#[test]
fn varlen_large_cycle_unbounded_visits_all() {
    // Unbounded varlen on cycle of 10 should visit all other 9 nodes (plus might re-hit start)
    let db = TestDb::new();
    db.seed_cycle(10);
    let count = db
        .exec_count("MATCH (a:Ring {idx:0})-[:LOOP*]->(b) RETURN b")
        .unwrap();
    // With cycle termination, should visit exactly 10 (nodes 1-9 at dist 1-9, and node 0 at dist 10)
    assert_eq!(count, 10);
}

#[test]
fn varlen_disconnected_components_no_cross() {
    // Variable-length on disconnected components should not cross between them
    let db = TestDb::new();
    db.seed_chain(3); // Chain: 0->1->2
                      // Create a separate disconnected chain
    db.run("CREATE (:Island {idx: 10})");
    db.run("CREATE (:Island {idx: 11})");
    db.run("MATCH (a:Island {idx:10}), (b:Island {idx:11}) CREATE (a)-[:LINK]->(b)");
    // Traversing NEXT from Chain should not find Island nodes
    db.assert_count("MATCH (a:Chain {idx:0})-[:NEXT*]->(b) RETURN b", 2);
}

#[test]
fn varlen_one_to_one_same_as_single_hop_on_social() {
    // Variable-length *1..1 is same as single hop
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let single_hop = db.sorted_strings(
        "MATCH (a:Person {name:'Alice'})-[:KNOWS]->(p:Person) RETURN p.name AS name",
        "name",
    );
    let varlen_one = db.sorted_strings(
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*1..1]->(p:Person) RETURN p.name AS name",
        "name",
    );
    assert_eq!(single_hop, varlen_one);
}

#[test]
fn varlen_zero_to_zero_returns_only_start_node_social() {
    // Variable-length *0..0 returns only start node
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run("MATCH (a:Person {name:'Alice'})-[:KNOWS*0..0]->(b) RETURN b.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Alice");
}

// ============================================================
// Path materialization (pending) — additional tests
// ============================================================

#[test]
fn path_variable_as_structured_return_value() {
    // Lora: path variable as structured return value
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run("MATCH p = (a:Person {name:'Alice'})-[:KNOWS]->(b:Person) RETURN p");
    assert_eq!(rows.len(), 2); // Alice->Bob, Alice->Carol
}

#[test]
fn path_length_returns_hop_count() {
    // Lora: length(path) returns hop count
    let db = TestDb::new();
    db.seed_chain(5);
    let rows = db.run(
        "MATCH p = (a:Chain {idx:0})-[:NEXT*1..3]->(b) RETURN length(p) AS hops, b.idx AS idx",
    );
    assert_eq!(rows.len(), 3);
}

#[test]
fn path_nodes_returns_ordered_node_list() {
    // Lora: nodes(path) returns ordered node list
    let db = TestDb::new();
    db.seed_chain(4);
    let rows =
        db.run("MATCH p = (a:Chain {idx:0})-[:NEXT*1..3]->(b:Chain {idx:3}) RETURN nodes(p) AS ns");
    let ns = rows[0]["ns"].as_array().unwrap();
    assert_eq!(ns.len(), 4); // nodes 0, 1, 2, 3
}

#[test]
fn path_relationships_returns_ordered_rel_list() {
    // Lora: relationships(path) returns ordered relationship list
    let db = TestDb::new();
    db.seed_chain(4);
    let rows = db.run(
        "MATCH p = (a:Chain {idx:0})-[:NEXT*1..3]->(b:Chain {idx:3}) RETURN relationships(p) AS rels",
    );
    let rels = rows[0]["rels"].as_array().unwrap();
    assert_eq!(rels.len(), 3); // 3 NEXT relationships
}

#[test]
fn path_binding_with_variable_length() {
    // Lora: path binding with variable-length
    let db = TestDb::new();
    db.seed_rich_social_graph();
    let rows = db.run(
        "MATCH p = (a:Person {name:'Alice'})-[:KNOWS*1..3]->(b:Person {name:'Eve'}) \
         RETURN length(p) AS hops",
    );
    // Alice->Carol->Eve = 2 hops
    assert!(!rows.is_empty());
    assert_eq!(rows[0]["hops"], 2);
}

#[test]
fn comparing_path_lengths() {
    // Lora: comparing path lengths
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let rows = db.run(
        "MATCH p1 = (a:Entity {name:'Albert Einstein'})-[*]->(x:Entity {name:'General Relativity'}), \
               p2 = (a)-[:AUTHORED]->(d:Document)-[:ABOUT]->(x) \
         WHERE length(p1) <= length(p2) \
         RETURN length(p1) AS direct, length(p2) AS indirect",
    );
    assert!(!rows.is_empty());
}

#[test]
fn shortest_path_between_nodes_knowledge_graph() {
    // Lora: shortest path between nodes
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let rows = db.run(
        "MATCH p = shortestPath((a:Entity {name:'Albert Einstein'})-[*]-(b:Entity {name:'Marie Curie'})) \
         RETURN length(p) AS hops",
    );
    assert!(!rows.is_empty());
}

#[test]
fn all_shortest_paths_knowledge_graph() {
    // Lora: all shortest paths
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let rows = db.run(
        "MATCH p = allShortestPaths((a:Entity {name:'Albert Einstein'})-[*]-(b:Entity {name:'Marie Curie'})) \
         RETURN p",
    );
    assert!(!rows.is_empty());
}

// ============================================================
// Extended shortest path tests
// ============================================================

#[test]
fn shortest_path_returns_minimum_hops() {
    let db = TestDb::new();
    db.seed_transport_graph();
    // Amsterdam -> Den Haag has two routes:
    //   Amsterdam -> Rotterdam -> Den Haag (2 hops)
    //   Amsterdam -> Utrecht -> Rotterdam -> Den Haag (3 hops)
    // shortestPath should find 2 hops
    let rows = db.run(
        "MATCH p = shortestPath((:Station {name:'Amsterdam'})-[:ROUTE*]->(:Station {name:'Den Haag'})) \
         RETURN length(p) AS hops",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["hops"], 2);
}

#[test]
fn shortest_path_returns_single_result() {
    let db = TestDb::new();
    db.seed_transport_graph();
    // shortestPath (not allShortestPaths) should return exactly 1 result
    let rows = db.run(
        "MATCH p = shortestPath((:Station {name:'Amsterdam'})-[:ROUTE*]->(:Station {name:'Den Haag'})) \
         RETURN p",
    );
    assert_eq!(rows.len(), 1);
}

#[test]
fn all_shortest_paths_may_return_multiple() {
    let db = TestDb::new();
    // Create a diamond graph: A->B->D and A->C->D (both 2 hops)
    db.run("CREATE (:N {name:'A'})");
    db.run("CREATE (:N {name:'B'})");
    db.run("CREATE (:N {name:'C'})");
    db.run("CREATE (:N {name:'D'})");
    db.run("MATCH (a:N {name:'A'}), (b:N {name:'B'}) CREATE (a)-[:E]->(b)");
    db.run("MATCH (a:N {name:'A'}), (c:N {name:'C'}) CREATE (a)-[:E]->(c)");
    db.run("MATCH (b:N {name:'B'}), (d:N {name:'D'}) CREATE (b)-[:E]->(d)");
    db.run("MATCH (c:N {name:'C'}), (d:N {name:'D'}) CREATE (c)-[:E]->(d)");
    let rows = db.run(
        "MATCH p = allShortestPaths((:N {name:'A'})-[:E*]->(:N {name:'D'})) \
         RETURN length(p) AS hops",
    );
    // Both paths are 2 hops — allShortestPaths should find both
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["hops"], 2);
    assert_eq!(rows[1]["hops"], 2);
}

#[test]
fn shortest_path_no_path_returns_empty() {
    let db = TestDb::new();
    db.run("CREATE (:X {name:'A'})");
    db.run("CREATE (:X {name:'B'})");
    // No relationship between A and B
    let rows = db.run(
        "MATCH p = shortestPath((:X {name:'A'})-[:R*]->(:X {name:'B'})) \
         RETURN p",
    );
    assert_eq!(rows.len(), 0);
}

#[test]
fn shortest_path_undirected() {
    let db = TestDb::new();
    db.run("CREATE (:W {name:'A'})");
    db.run("CREATE (:W {name:'B'})");
    db.run("CREATE (:W {name:'C'})");
    db.run("MATCH (a:W {name:'A'}), (b:W {name:'B'}) CREATE (a)-[:L]->(b)");
    db.run("MATCH (b:W {name:'B'}), (c:W {name:'C'}) CREATE (c)-[:L]->(b)");
    // A->B<-C with undirected traversal: A-B-C = 2 hops
    let rows = db.run(
        "MATCH p = shortestPath((:W {name:'A'})-[:L*]-(:W {name:'C'})) \
         RETURN length(p) AS hops",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["hops"], 2);
}

#[test]
fn shortest_path_with_path_nodes_function() {
    let db = TestDb::new();
    db.run("CREATE (:S {name:'A'})");
    db.run("CREATE (:S {name:'B'})");
    db.run("CREATE (:S {name:'C'})");
    db.run("MATCH (a:S {name:'A'}), (b:S {name:'B'}) CREATE (a)-[:R]->(b)");
    db.run("MATCH (b:S {name:'B'}), (c:S {name:'C'}) CREATE (b)-[:R]->(c)");
    let rows = db.run(
        "MATCH p = shortestPath((:S {name:'A'})-[:R*]->(:S {name:'C'})) \
         RETURN length(p) AS hops, nodes(p) AS ns",
    );
    assert_eq!(rows[0]["hops"], 2);
    let ns = rows[0]["ns"].as_array().unwrap();
    assert_eq!(ns.len(), 3); // A, B, C
}
