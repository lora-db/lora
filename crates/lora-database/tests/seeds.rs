/// Seed graph integrity tests — verify that each seed function creates
/// the expected graph topology (node counts, relationship counts, property
/// correctness, structural invariants).
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// 1. Social graph integrity
// ============================================================

#[test]
fn social_graph_node_count() {
    let db = TestDb::new();
    db.seed_social_graph();
    db.assert_count("MATCH (n:User) RETURN n", 3);
}

#[test]
fn social_graph_relationship_count() {
    let db = TestDb::new();
    db.seed_social_graph();
    db.assert_count("MATCH ()-[r]->() RETURN r", 3);
}

#[test]
fn social_graph_follows_count() {
    let db = TestDb::new();
    db.seed_social_graph();
    db.assert_count("MATCH ()-[r:FOLLOWS]->() RETURN r", 2);
}

#[test]
fn social_graph_names_present() {
    let db = TestDb::new();
    db.seed_social_graph();
    let names = db.sorted_strings("MATCH (n:User) RETURN n.name AS name", "name");
    assert_eq!(names, vec!["Alice", "Bob", "Carol"]);
}

// ============================================================
// 2. Org graph integrity
// ============================================================

#[test]
fn org_graph_total_node_count() {
    let db = TestDb::new();
    db.seed_org_graph();
    db.assert_count("MATCH (n) RETURN n", 12);
}

#[test]
fn org_graph_total_relationship_count() {
    let db = TestDb::new();
    db.seed_org_graph();
    db.assert_count("MATCH ()-[r]->() RETURN r", 20);
}

#[test]
fn org_graph_person_count() {
    let db = TestDb::new();
    db.seed_org_graph();
    db.assert_count("MATCH (p:Person) RETURN p", 6);
}

#[test]
fn org_graph_engineering_dept_count() {
    let db = TestDb::new();
    db.seed_org_graph();
    db.assert_count(
        "MATCH (p:Person) WHERE p.dept = 'Engineering' RETURN p",
        4,
    );
}

#[test]
fn org_graph_marketing_dept_count() {
    let db = TestDb::new();
    db.seed_org_graph();
    db.assert_count(
        "MATCH (p:Person) WHERE p.dept = 'Marketing' RETURN p",
        2,
    );
}

#[test]
fn org_graph_manager_label_count() {
    let db = TestDb::new();
    db.seed_org_graph();
    db.assert_count("MATCH (m:Manager) RETURN m", 1);
}

// ============================================================
// 3. Chain and cycle integrity
// ============================================================

#[test]
fn chain_of_five_node_count() {
    let db = TestDb::new();
    db.seed_chain(5);
    db.assert_count("MATCH (n:Chain) RETURN n", 5);
}

#[test]
fn chain_of_five_relationship_count() {
    let db = TestDb::new();
    db.seed_chain(5);
    db.assert_count("MATCH ()-[r:NEXT]->() RETURN r", 4);
}

#[test]
fn chain_head_is_index_zero() {
    let db = TestDb::new();
    db.seed_chain(5);
    // The first node (idx=0) exists
    db.assert_count("MATCH (n:Chain {idx: 0}) RETURN n", 1);
}

#[test]
fn chain_tail_is_last_index() {
    let db = TestDb::new();
    db.seed_chain(5);
    // The last node (idx=4) exists
    db.assert_count("MATCH (n:Chain {idx: 4}) RETURN n", 1);
    // And idx=4 has no outgoing NEXT (nothing beyond it)
    db.assert_count(
        "MATCH (a:Chain {idx: 4})-[:NEXT]->(b:Chain) RETURN b",
        0,
    );
}

#[test]
fn cycle_of_four_node_count() {
    let db = TestDb::new();
    db.seed_cycle(4);
    db.assert_count("MATCH (n:Ring) RETURN n", 4);
}

#[test]
fn cycle_of_four_relationship_count() {
    let db = TestDb::new();
    db.seed_cycle(4);
    db.assert_count("MATCH ()-[r:LOOP]->() RETURN r", 4);
}

// ============================================================
// 4. Dependency graph integrity
// ============================================================

#[test]
fn dependency_graph_package_count() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    db.assert_count("MATCH (p:Package) RETURN p", 6);
}

#[test]
fn dependency_graph_depends_on_count() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    db.assert_count("MATCH ()-[r:DEPENDS_ON]->() RETURN r", 8);
}

#[test]
fn dependency_graph_leaf_packages() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    // Leaf packages have no outgoing DEPENDS_ON: log and util
    // Verify log has 0 outgoing deps
    db.assert_count(
        "MATCH (p:Package {name:'log'})-[:DEPENDS_ON]->() RETURN p",
        0,
    );
    // Verify util has 0 outgoing deps
    db.assert_count(
        "MATCH (p:Package {name:'util'})-[:DEPENDS_ON]->() RETURN p",
        0,
    );
}

#[test]
fn dependency_graph_root_package() {
    let db = TestDb::new();
    db.seed_dependency_graph();
    // Root package 'app' has no incoming DEPENDS_ON
    db.assert_count(
        "MATCH ()-[:DEPENDS_ON]->(p:Package {name:'app'}) RETURN p",
        0,
    );
    // Non-root packages do have incoming DEPENDS_ON
    db.assert_count(
        "MATCH ()-[:DEPENDS_ON]->(p:Package {name:'web'}) RETURN p",
        1,
    );
}

// ============================================================
// 5. Transport graph integrity
// ============================================================

#[test]
fn transport_graph_station_count() {
    let db = TestDb::new();
    db.seed_transport_graph();
    db.assert_count("MATCH (s:Station) RETURN s", 5);
}

#[test]
fn transport_graph_route_count() {
    let db = TestDb::new();
    db.seed_transport_graph();
    db.assert_count("MATCH ()-[r:ROUTE]->() RETURN r", 10);
}

#[test]
fn transport_graph_zone_distribution() {
    let db = TestDb::new();
    db.seed_transport_graph();
    // Zone 1: Amsterdam, Utrecht
    db.assert_count("MATCH (s:Station) WHERE s.zone = 1 RETURN s", 2);
    // Zone 2: Rotterdam, Den Haag
    db.assert_count("MATCH (s:Station) WHERE s.zone = 2 RETURN s", 2);
    // Zone 3: Eindhoven
    db.assert_count("MATCH (s:Station) WHERE s.zone = 3 RETURN s", 1);
}

#[test]
fn transport_graph_bidirectional_pairs() {
    let db = TestDb::new();
    db.seed_transport_graph();
    // Each pair has routes in both directions
    // Verify Amsterdam<->Utrecht has both directions
    db.assert_count(
        "MATCH (a:Station {name:'Amsterdam'})-[:ROUTE]->(b:Station {name:'Utrecht'}) RETURN a",
        1,
    );
    db.assert_count(
        "MATCH (a:Station {name:'Utrecht'})-[:ROUTE]->(b:Station {name:'Amsterdam'}) RETURN a",
        1,
    );
}

// ============================================================
// 6. Recommendation graph integrity
// ============================================================

#[test]
fn recommendation_graph_viewer_count() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    db.assert_count("MATCH (v:Viewer) RETURN v", 3);
}

#[test]
fn recommendation_graph_movie_count() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    db.assert_count("MATCH (m:Movie) RETURN m", 4);
}

#[test]
fn recommendation_graph_rating_count() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    db.assert_count("MATCH ()-[r:RATED]->() RETURN r", 7);
}

#[test]
fn recommendation_graph_genre_distribution() {
    let db = TestDb::new();
    db.seed_recommendation_graph();
    // 2 sci-fi movies: Matrix, Inception
    db.assert_count("MATCH (m:Movie) WHERE m.genre = 'sci-fi' RETURN m", 2);
    // 1 drama: Amelie
    db.assert_count("MATCH (m:Movie) WHERE m.genre = 'drama' RETURN m", 1);
    // 1 thriller: Jaws
    db.assert_count("MATCH (m:Movie) WHERE m.genre = 'thriller' RETURN m", 1);
}

// ============================================================
// 7. Rich social graph integrity
// ============================================================

#[test]
fn rich_social_person_count() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    db.assert_count("MATCH (p:Person) RETURN p", 6);
}

#[test]
fn rich_social_interest_count() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    db.assert_count("MATCH (i:Interest) RETURN i", 4);
}

#[test]
fn rich_social_knows_count() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    db.assert_count("MATCH ()-[r:KNOWS]->() RETURN r", 7);
}

#[test]
fn rich_social_follows_count() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    db.assert_count("MATCH ()-[r:FOLLOWS]->() RETURN r", 6);
}

#[test]
fn rich_social_blocked_count() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    db.assert_count("MATCH ()-[r:BLOCKED]->() RETURN r", 2);
}

#[test]
fn rich_social_interested_in_count() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    db.assert_count("MATCH ()-[r:INTERESTED_IN]->() RETURN r", 11);
}

#[test]
fn rich_social_influencer_label_on_eve() {
    let db = TestDb::new();
    db.seed_rich_social_graph();
    db.assert_count(
        "MATCH (p:Person:Influencer) RETURN p",
        1,
    );
    let names = db.sorted_strings(
        "MATCH (p:Influencer) RETURN p.name AS name",
        "name",
    );
    assert_eq!(names, vec!["Eve"]);
}

// ============================================================
// 8. Knowledge graph integrity
// ============================================================

#[test]
fn knowledge_graph_entity_count() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    db.assert_count("MATCH (e:Entity) RETURN e", 8);
}

#[test]
fn knowledge_graph_document_count() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    db.assert_count("MATCH (d:Document) RETURN d", 2);
}

#[test]
fn knowledge_graph_topic_count() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    db.assert_count("MATCH (t:Topic) RETURN t", 2);
}

#[test]
fn knowledge_graph_alias_count() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    db.assert_count("MATCH (a:Alias) RETURN a", 2);
}

#[test]
fn knowledge_graph_both_nobel_laureates() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    let laureates = db.sorted_strings(
        "MATCH (e:Entity)-[:RECEIVED]->(n:Entity {name:'Nobel Prize'}) RETURN e.name AS name",
        "name",
    );
    assert_eq!(laureates, vec!["Albert Einstein", "Marie Curie"]);
}

#[test]
fn knowledge_graph_document_theory_links() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // Both documents are ABOUT General Relativity
    db.assert_count(
        "MATCH (d:Document)-[:ABOUT]->(t:Entity {name:'General Relativity'}) RETURN d",
        2,
    );
}

#[test]
fn knowledge_graph_total_relationship_count() {
    let db = TestDb::new();
    db.seed_knowledge_graph();
    // 19 total relationships
    db.assert_count("MATCH ()-[r]->() RETURN r", 19);
}
