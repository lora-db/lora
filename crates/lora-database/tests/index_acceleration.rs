//! Optimizer-integration tests: ensure RANGE and TEXT catalog entries
//! cause the planner to rewrite `Filter(NodeScan, ...)` patterns into
//! the dedicated `NodeByPropertyRangeScan` / `NodeByTextScan`
//! operators, and that those operators produce the same row-set as the
//! pre-rewrite scan + filter.

mod test_helpers;
use test_helpers::TestDb;

use lora_database::PlanTreeNode;

fn contains_operator(node: &PlanTreeNode, name: &str) -> bool {
    if node.operator == name {
        return true;
    }
    node.children.iter().any(|c| contains_operator(c, name))
}

fn pluck_node_id(rows: &[serde_json::Value], path: &str) -> Vec<i64> {
    rows.iter()
        .map(|r| {
            // Hydrated nodes serialise as `{ id, labels, properties: { ... } }`.
            r["n"]["properties"][path]
                .as_i64()
                .unwrap_or_else(|| panic!("missing properties.{path} in row {r:?}"))
        })
        .collect()
}

// ---------- Range index ----------

#[test]
fn explain_rewrites_greater_than_to_range_scan() {
    let db = TestDb::new();
    db.run("CREATE INDEX age_idx FOR (n:Person) ON (n.age)");
    let plan = db
        .service
        .explain("MATCH (n:Person) WHERE n.age > 30 RETURN n", None)
        .unwrap();
    assert!(
        contains_operator(&plan.tree.root, "NodeByPropertyRangeScan"),
        "expected NodeByPropertyRangeScan in plan, got {:?}",
        plan.tree
    );
}

#[test]
fn explain_combines_two_bounds_into_one_range_scan() {
    let db = TestDb::new();
    db.run("CREATE INDEX age_idx FOR (n:Person) ON (n.age)");
    let plan = db
        .service
        .explain(
            "MATCH (n:Person) WHERE n.age >= 30 AND n.age < 50 RETURN n",
            None,
        )
        .unwrap();
    assert!(contains_operator(
        &plan.tree.root,
        "NodeByPropertyRangeScan"
    ));
}

#[test]
fn range_scan_returns_correct_rows() {
    let db = TestDb::new();
    db.run("CREATE INDEX age_idx FOR (n:Person) ON (n.age)");
    db.run("CREATE (:Person {id: 1, age: 20})");
    db.run("CREATE (:Person {id: 2, age: 30})");
    db.run("CREATE (:Person {id: 3, age: 40})");
    db.run("CREATE (:Person {id: 4, age: 50})");

    let rows = db.run("MATCH (n:Person) WHERE n.age >= 30 AND n.age < 50 RETURN n ORDER BY n.id");
    let ids = pluck_node_id(&rows, "id");
    assert_eq!(ids, vec![2, 3]);
}

#[test]
fn range_scan_preserves_already_bound_node_variable() {
    let db = TestDb::new();
    db.run("CREATE INDEX age_idx FOR (n:Person) ON (n.age)");
    db.run("CREATE (:Person {id: 1, age: 20})");
    db.run("CREATE (:Person {id: 2, age: 40})");

    let rows = db.run(
        "MATCH (n:Person {id: 1}) \
         MATCH (n:Person) \
         WHERE n.age > 30 \
         RETURN n",
    );

    assert!(
        rows.is_empty(),
        "indexed scan must filter the existing binding instead of rebinding n"
    );
}

#[test]
fn range_scan_inclusive_upper_bound() {
    let db = TestDb::new();
    db.run("CREATE INDEX age_idx FOR (n:Person) ON (n.age)");
    db.run("CREATE (:Person {id: 1, age: 30})");
    db.run("CREATE (:Person {id: 2, age: 40})");

    let rows = db.run("MATCH (n:Person) WHERE n.age <= 40 RETURN n ORDER BY n.id");
    let ids = pluck_node_id(&rows, "id");
    assert_eq!(ids, vec![1, 2]);
}

#[test]
fn range_scan_falls_back_when_no_index() {
    // No index -> the optimizer keeps the ordinary scan/filter plan.
    // The query result must still be correct.
    let db = TestDb::new();
    db.run("CREATE (:Person {id: 1, age: 25})");
    db.run("CREATE (:Person {id: 2, age: 35})");

    let plan = db
        .service
        .explain("MATCH (n:Person) WHERE n.age > 30 RETURN n", None)
        .unwrap();
    assert!(!contains_operator(
        &plan.tree.root,
        "NodeByPropertyRangeScan"
    ));

    let rows = db.run("MATCH (n:Person) WHERE n.age > 30 RETURN n ORDER BY n.id");
    let ids = pluck_node_id(&rows, "id");
    assert_eq!(ids, vec![2]);
}

#[test]
fn cached_plan_recompiles_after_index_catalog_changes() {
    let db = TestDb::new();
    db.run("CREATE (:Person {id: 1, age: 25})");
    db.run("CREATE (:Person {id: 2, age: 35})");

    let query = "MATCH (n:Person) WHERE n.age > 30 RETURN n";
    let before = db.service.explain(query, None).unwrap();
    assert!(!contains_operator(
        &before.tree.root,
        "NodeByPropertyRangeScan"
    ));

    db.run("CREATE INDEX age_idx FOR (n:Person) ON (n.age)");

    let after = db.service.explain(query, None).unwrap();
    assert!(
        contains_operator(&after.tree.root, "NodeByPropertyRangeScan"),
        "expected recompile to pick up the new catalog index, got {:?}",
        after.tree
    );
}

// ---------- Text index ----------

#[test]
fn explain_rewrites_starts_with_to_text_scan() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX name_idx FOR (n:Person) ON (n.name)");
    let plan = db
        .service
        .explain(
            "MATCH (n:Person) WHERE n.name STARTS WITH 'Alex' RETURN n",
            None,
        )
        .unwrap();
    assert!(
        contains_operator(&plan.tree.root, "NodeByTextScan"),
        "expected NodeByTextScan in plan, got {:?}",
        plan.tree
    );
}

#[test]
fn explain_rewrites_contains_to_text_scan() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX name_idx FOR (n:Person) ON (n.name)");
    let plan = db
        .service
        .explain(
            "MATCH (n:Person) WHERE n.name CONTAINS 'lex' RETURN n",
            None,
        )
        .unwrap();
    assert!(contains_operator(&plan.tree.root, "NodeByTextScan"));
}

#[test]
fn explain_rewrites_ends_with_to_text_scan() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX name_idx FOR (n:Person) ON (n.name)");
    let plan = db
        .service
        .explain(
            "MATCH (n:Person) WHERE n.name ENDS WITH 'ander' RETURN n",
            None,
        )
        .unwrap();
    assert!(contains_operator(&plan.tree.root, "NodeByTextScan"));
}

#[test]
fn text_scan_returns_correct_starts_with_rows() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX name_idx FOR (n:Person) ON (n.name)");
    db.run("CREATE (:Person {id: 1, name: 'Alexander'})");
    db.run("CREATE (:Person {id: 2, name: 'Alexandra'})");
    db.run("CREATE (:Person {id: 3, name: 'Bob'})");

    let rows = db.run("MATCH (n:Person) WHERE n.name STARTS WITH 'Alex' RETURN n ORDER BY n.id");
    let ids = pluck_node_id(&rows, "id");
    assert_eq!(ids, vec![1, 2]);
}

#[test]
fn text_scan_preserves_already_bound_node_variable() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX name_idx FOR (n:Person) ON (n.name)");
    db.run("CREATE (:Person {id: 1, name: 'Alice'})");
    db.run("CREATE (:Person {id: 2, name: 'Bob'})");

    let rows = db.run(
        "MATCH (n:Person {id: 1}) \
         MATCH (n:Person) \
         WHERE n.name STARTS WITH 'Bo' \
         RETURN n",
    );

    assert!(
        rows.is_empty(),
        "indexed scan must filter the existing binding instead of rebinding n"
    );
}

#[test]
fn text_scan_returns_correct_contains_rows() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX name_idx FOR (n:Person) ON (n.name)");
    db.run("CREATE (:Person {id: 1, name: 'Alexander'})");
    db.run("CREATE (:Person {id: 2, name: 'Alexandra'})");
    db.run("CREATE (:Person {id: 3, name: 'Bob'})");

    let rows = db.run("MATCH (n:Person) WHERE n.name CONTAINS 'lex' RETURN n ORDER BY n.id");
    let ids = pluck_node_id(&rows, "id");
    assert_eq!(ids, vec![1, 2]);
}

#[test]
fn text_scan_returns_correct_ends_with_rows() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX name_idx FOR (n:Person) ON (n.name)");
    db.run("CREATE (:Person {id: 1, name: 'Alexander'})");
    db.run("CREATE (:Person {id: 2, name: 'Alexandra'})");

    let rows = db.run("MATCH (n:Person) WHERE n.name ENDS WITH 'ander' RETURN n ORDER BY n.id");
    let ids = pluck_node_id(&rows, "id");
    assert_eq!(ids, vec![1]);
}

#[test]
fn text_scan_falls_back_when_no_index() {
    let db = TestDb::new();
    db.run("CREATE (:Person {id: 1, name: 'Alice'})");
    db.run("CREATE (:Person {id: 2, name: 'Bob'})");

    let plan = db
        .service
        .explain(
            "MATCH (n:Person) WHERE n.name STARTS WITH 'Al' RETURN n",
            None,
        )
        .unwrap();
    assert!(!contains_operator(&plan.tree.root, "NodeByTextScan"));

    let rows = db.run("MATCH (n:Person) WHERE n.name STARTS WITH 'Al' RETURN n ORDER BY n.id");
    let ids = pluck_node_id(&rows, "id");
    assert_eq!(ids, vec![1]);
}

#[test]
fn text_index_tracks_node_label_add_and_remove() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX name_idx FOR (n:Person) ON (n.name)");
    db.run("CREATE (:Other {id: 1, name: 'Alice'})");
    db.run("CREATE (:Person {id: 99, name: 'Zed'})");

    db.run("MATCH (n:Other {id: 1}) SET n:Person");
    let rows = db.run("MATCH (n:Person) WHERE n.name STARTS WITH 'Ali' RETURN n");
    assert_eq!(pluck_node_id(&rows, "id"), vec![1]);

    db.run("MATCH (n:Person {id: 1}) REMOVE n:Person");
    let rows = db.run("MATCH (n:Person) WHERE n.name STARTS WITH 'Ali' RETURN n");
    assert!(rows.is_empty());
}

// ---------- Point index ----------

#[test]
fn explain_rewrites_within_bbox_to_point_scan() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX loc_idx FOR (n:Place) ON (n.loc)");
    let plan = db
        .service
        .explain(
            "MATCH (n:Place) WHERE point.withinBBox(n.loc, point({x: 0, y: 0}), point({x: 100, y: 100})) RETURN n",
            None,
        )
        .unwrap();
    assert!(
        contains_operator(&plan.tree.root, "NodeByPointScan"),
        "expected NodeByPointScan, got {:?}",
        plan.tree
    );
}

#[test]
fn explain_rewrites_distance_le_to_point_scan() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX loc_idx FOR (n:Place) ON (n.loc)");
    let plan = db
        .service
        .explain(
            "MATCH (n:Place) WHERE point.distance(n.loc, point({x: 0, y: 0})) <= 100 RETURN n",
            None,
        )
        .unwrap();
    assert!(contains_operator(&plan.tree.root, "NodeByPointScan"));
}

#[test]
fn point_scan_returns_within_bbox_correctly() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX loc_idx FOR (n:Place) ON (n.loc)");
    db.run("CREATE (:Place {id: 1, loc: point({x: 50, y: 50})})");
    db.run("CREATE (:Place {id: 2, loc: point({x: 150, y: 150})})");
    db.run("CREATE (:Place {id: 3, loc: point({x: 1000, y: 1000})})");

    let rows = db.run(
        "MATCH (n:Place) WHERE point.withinBBox(n.loc, point({x: 0, y: 0}), point({x: 200, y: 200})) RETURN n ORDER BY n.id",
    );
    let ids = pluck_node_id(&rows, "id");
    assert_eq!(ids, vec![1, 2]);
}

#[test]
fn point_scan_returns_within_distance_correctly() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX loc_idx FOR (n:Place) ON (n.loc)");
    db.run("CREATE (:Place {id: 1, loc: point({x: 0, y: 0})})");
    db.run("CREATE (:Place {id: 2, loc: point({x: 30, y: 40})})"); // distance 50
    db.run("CREATE (:Place {id: 3, loc: point({x: 300, y: 400})})"); // distance 500

    let rows = db.run(
        "MATCH (n:Place) WHERE point.distance(n.loc, point({x: 0, y: 0})) <= 60 RETURN n ORDER BY n.id",
    );
    let ids = pluck_node_id(&rows, "id");
    assert_eq!(ids, vec![1, 2]);
}

#[test]
fn point_scan_falls_back_when_no_index() {
    let db = TestDb::new();
    db.run("CREATE (:Place {id: 1, loc: point({x: 50, y: 50})})");
    db.run("CREATE (:Place {id: 2, loc: point({x: 1000, y: 1000})})");

    let plan = db
        .service
        .explain(
            "MATCH (n:Place) WHERE point.withinBBox(n.loc, point({x: 0, y: 0}), point({x: 100, y: 100})) RETURN n",
            None,
        )
        .unwrap();
    assert!(!contains_operator(&plan.tree.root, "NodeByPointScan"));

    let rows = db.run(
        "MATCH (n:Place) WHERE point.withinBBox(n.loc, point({x: 0, y: 0}), point({x: 100, y: 100})) RETURN n ORDER BY n.id",
    );
    let ids = pluck_node_id(&rows, "id");
    assert_eq!(ids, vec![1]);
}

#[test]
fn point_scan_correctly_filters_after_property_update() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX loc_idx FOR (n:Place) ON (n.loc)");
    db.run("CREATE (:Place {id: 1, loc: point({x: 50, y: 50})})");

    let inside = db.run(
        "MATCH (n:Place) WHERE point.withinBBox(n.loc, point({x: 0, y: 0}), point({x: 100, y: 100})) RETURN n",
    );
    assert_eq!(inside.len(), 1);

    db.run("MATCH (n:Place {id: 1}) SET n.loc = point({x: 1000, y: 1000})");

    let still_inside = db.run(
        "MATCH (n:Place) WHERE point.withinBBox(n.loc, point({x: 0, y: 0}), point({x: 100, y: 100})) RETURN n",
    );
    assert!(
        still_inside.is_empty(),
        "spatial index must reflect the SET"
    );

    let now_outside = db.run(
        "MATCH (n:Place) WHERE point.withinBBox(n.loc, point({x: 500, y: 500}), point({x: 1500, y: 1500})) RETURN n",
    );
    assert_eq!(now_outside.len(), 1);
}

#[test]
fn text_scan_correctly_filters_after_property_update() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX name_idx FOR (n:Person) ON (n.name)");
    db.run("CREATE (:Person {id: 1, name: 'Alice'})");
    let rows = db.run("MATCH (n:Person) WHERE n.name STARTS WITH 'Alic' RETURN n ORDER BY n.id");
    assert_eq!(pluck_node_id(&rows, "id"), vec![1]);

    // Update the property: trigram index must reflect the new value.
    db.run("MATCH (n:Person {id: 1}) SET n.name = 'Bob'");
    let rows_after = db.run("MATCH (n:Person) WHERE n.name STARTS WITH 'Alic' RETURN n");
    assert!(rows_after.is_empty(), "old trigrams must be evicted");

    let rows_new = db.run("MATCH (n:Person) WHERE n.name STARTS WITH 'Bo' RETURN n ORDER BY n.id");
    assert_eq!(pluck_node_id(&rows_new, "id"), vec![1]);
}

// ---------- Cost-based selection ----------
//
// These tests assert the optimizer's behaviour when several index
// rewrites apply to the same `Filter(NodeScan)` site, or when the
// applicable rewrite would select every row. The contract: a
// tautological predicate must leave the label scan in place; among
// non-tautological candidates the cheapest by `score_logical_op`
// wins.

#[test]
fn cost_model_keeps_label_scan_for_starts_with_empty_string() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX name_idx FOR (n:Person) ON (n.name)");
    db.run("CREATE (:Person {id: 1, name: 'Alice'})");

    let plan = db
        .service
        .explain(
            "MATCH (n:Person) WHERE n.name STARTS WITH '' RETURN n",
            None,
        )
        .unwrap();

    assert!(
        !contains_operator(&plan.tree.root, "NodeByTextScan"),
        "STARTS WITH '' is tautological; expected label scan, got {:?}",
        plan.tree
    );
    assert!(contains_operator(&plan.tree.root, "NodeByLabelScan"));
}

#[test]
fn cost_model_keeps_label_scan_for_world_bbox() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX loc_idx FOR (n:Place) ON (n.loc)");
    db.run("CREATE (:Place {id: 1, loc: point({x: 50, y: 50})})");

    let plan = db
        .service
        .explain(
            "MATCH (n:Place) \
             WHERE point.withinBBox(n.loc, \
                point({longitude: -180, latitude: -90}), \
                point({longitude: 180, latitude: 90})) \
             RETURN n",
            None,
        )
        .unwrap();

    assert!(
        !contains_operator(&plan.tree.root, "NodeByPointScan"),
        "world bbox covers every point; expected label scan, got {:?}",
        plan.tree
    );
    assert!(contains_operator(&plan.tree.root, "NodeByLabelScan"));
}

#[test]
fn cost_model_picks_cheaper_among_competing_candidates() {
    // Both a range bound and a text prefix apply to the same scan.
    // `score_logical_op` ranks `STARTS WITH` (denominator 4) below the
    // one-sided range (denominator 3), so the optimizer must pick text.
    let db = TestDb::new();
    db.run("CREATE INDEX age_idx FOR (n:Person) ON (n.age)");
    db.run("CREATE TEXT INDEX name_idx FOR (n:Person) ON (n.name)");
    for i in 0..16 {
        db.run(&format!(
            "CREATE (:Person {{id: {i}, age: {age}, name: 'A{i}'}})",
            age = 30 + i
        ));
    }

    let plan = db
        .service
        .explain(
            "MATCH (n:Person) WHERE n.name STARTS WITH 'A' AND n.age > 30 RETURN n",
            None,
        )
        .unwrap();

    assert!(
        contains_operator(&plan.tree.root, "NodeByTextScan"),
        "expected the cheaper text scan to win over the range scan, got {:?}",
        plan.tree
    );
    assert!(
        !contains_operator(&plan.tree.root, "NodeByPropertyRangeScan"),
        "range scan should not be picked when text is cheaper, got {:?}",
        plan.tree
    );
}

#[test]
fn cost_model_picks_property_scan_when_distinct_is_high() {
    // 100 Person rows, each with a unique `id` (distinct = 100).
    // Per-value selectivity is 1/100, far below the full label scan.
    let db = TestDb::new();
    db.run("CREATE INDEX id_idx FOR (n:Person) ON (n.id)");
    for i in 0..100 {
        db.run(&format!("CREATE (:Person {{id: {i}}})"));
    }

    let plan = db
        .service
        .explain("MATCH (n:Person) WHERE n.id = 7 RETURN n", None)
        .unwrap();

    assert!(
        contains_operator(&plan.tree.root, "NodeByPropertyScan"),
        "high-distinct equality should beat label scan, got {:?}",
        plan.tree
    );
}

// ---------- Rel-targeted index scans ----------
//
// Mirror of the node-side acceleration tests for the
// `RelByPropertyRangeScan` / `RelByTextScan` / `RelByPointScan`
// rewrites: anonymous-endpoint patterns of the form
// `MATCH ()-[r:TYPE]-() WHERE r.prop CMP value` should hit the
// rel-typed index registry instead of running a NodeScan + Expand.

fn pluck_rel_property(rows: &[serde_json::Value], var: &str, prop: &str) -> Vec<i64> {
    rows.iter()
        .map(|r| {
            r[var]["properties"][prop]
                .as_i64()
                .unwrap_or_else(|| panic!("missing {var}.properties.{prop} in row {r:?}"))
        })
        .collect()
}

fn pluck_var_node_property(rows: &[serde_json::Value], var: &str, prop: &str) -> Vec<i64> {
    rows.iter()
        .map(|r| {
            r[var]["properties"][prop]
                .as_i64()
                .unwrap_or_else(|| panic!("missing {var}.properties.{prop} in row {r:?}"))
        })
        .collect()
}

#[test]
fn explain_rewrites_rel_range_filter_to_rel_range_scan() {
    let db = TestDb::new();
    db.run("CREATE INDEX rel_since_idx FOR ()-[r:KNOWS]-() ON (r.since)");
    let plan = db
        .service
        .explain("MATCH ()-[r:KNOWS]->() WHERE r.since > 2020 RETURN r", None)
        .unwrap();
    assert!(
        contains_operator(&plan.tree.root, "RelByPropertyRangeScan"),
        "expected RelByPropertyRangeScan in plan, got {:?}",
        plan.tree
    );
}

#[test]
fn rel_range_scan_returns_correct_rows_directed() {
    let db = TestDb::new();
    db.run("CREATE INDEX rel_since_idx FOR ()-[r:KNOWS]-() ON (r.since)");
    db.run("CREATE (:Person {id: 1})");
    db.run("CREATE (:Person {id: 2})");
    db.run("CREATE (:Person {id: 3})");
    db.run(
        "MATCH (a:Person {id: 1}), (b:Person {id: 2}) \
         CREATE (a)-[:KNOWS {since: 2018}]->(b)",
    );
    db.run(
        "MATCH (a:Person {id: 2}), (b:Person {id: 3}) \
         CREATE (a)-[:KNOWS {since: 2022}]->(b)",
    );
    db.run(
        "MATCH (a:Person {id: 1}), (b:Person {id: 3}) \
         CREATE (a)-[:KNOWS {since: 2024}]->(b)",
    );

    let rows = db.run("MATCH ()-[r:KNOWS]->() WHERE r.since > 2020 RETURN r ORDER BY r.since");
    let years = pluck_rel_property(&rows, "r", "since");
    assert_eq!(years, vec![2022, 2024]);
}

#[test]
fn rel_range_scan_respects_bound_source_from_upstream() {
    let db = TestDb::new();
    db.run("CREATE INDEX rel_since_idx FOR ()-[r:KNOWS]-() ON (r.since)");
    db.run("CREATE (:Person {id: 1})");
    db.run("CREATE (:Person {id: 2})");
    db.run("CREATE (:Person {id: 3})");
    db.run(
        "MATCH (a:Person {id: 1}), (b:Person {id: 2}) \
         CREATE (a)-[:KNOWS {since: 2018}]->(b)",
    );
    db.run(
        "MATCH (a:Person {id: 2}), (b:Person {id: 3}) \
         CREATE (a)-[:KNOWS {since: 2022}]->(b)",
    );
    db.run(
        "MATCH (a:Person {id: 1}), (b:Person {id: 3}) \
         CREATE (a)-[:KNOWS {since: 2024}]->(b)",
    );

    let rows = db.run(
        "MATCH (a:Person {id: 1}) MATCH (a)-[r:KNOWS]->(b) \
         WHERE r.since > 2020 RETURN b ORDER BY b.id",
    );
    assert_eq!(pluck_var_node_property(&rows, "b", "id"), vec![3]);
}

#[test]
fn rel_range_scan_undirected_emits_both_orientations() {
    // Undirected expansion would emit each rel twice (once per
    // endpoint orientation); the rel-scan rewrite must preserve
    // that semantics.
    let db = TestDb::new();
    db.run("CREATE INDEX rel_since_idx FOR ()-[r:KNOWS]-() ON (r.since)");
    db.run("CREATE (:Person {id: 1})");
    db.run("CREATE (:Person {id: 2})");
    db.run(
        "MATCH (a:Person {id: 1}), (b:Person {id: 2}) \
         CREATE (a)-[:KNOWS {since: 2024}]->(b)",
    );

    let rows = db.run("MATCH (a)-[r:KNOWS]-(b) WHERE r.since > 2020 RETURN a, r, b");
    assert_eq!(rows.len(), 2, "undirected scan must emit both orientations");
}

#[test]
fn rel_range_scan_falls_back_when_no_index() {
    // No rel-targeted index -> the optimizer keeps the ordinary expand/filter
    // plan. Result must remain correct.
    let db = TestDb::new();
    db.run("CREATE (:P {id: 1})");
    db.run("CREATE (:P {id: 2})");
    db.run(
        "MATCH (a:P {id: 1}), (b:P {id: 2}) \
         CREATE (a)-[:LINKS {weight: 5}]->(b)",
    );
    db.run(
        "MATCH (a:P {id: 2}), (b:P {id: 1}) \
         CREATE (a)-[:LINKS {weight: 50}]->(b)",
    );

    let plan = db
        .service
        .explain("MATCH ()-[r:LINKS]->() WHERE r.weight > 10 RETURN r", None)
        .unwrap();
    assert!(!contains_operator(
        &plan.tree.root,
        "RelByPropertyRangeScan"
    ));

    let rows = db.run("MATCH ()-[r:LINKS]->() WHERE r.weight > 10 RETURN r");
    assert_eq!(rows.len(), 1);
}

#[test]
fn rel_range_scan_preserves_residual_predicates() {
    let db = TestDb::new();
    db.run("CREATE INDEX rel_weight_idx FOR ()-[r:LINKS]-() ON (r.weight)");
    db.run("CREATE (:P {id: 1})");
    db.run("CREATE (:P {id: 2})");
    db.run(
        "MATCH (a:P {id: 1}), (b:P {id: 2}) \
         CREATE (a)-[:LINKS {weight: 20, keep: true}]->(b)",
    );
    db.run(
        "MATCH (a:P {id: 2}), (b:P {id: 1}) \
         CREATE (a)-[:LINKS {weight: 30, keep: false}]->(b)",
    );

    let plan = db
        .service
        .explain(
            "MATCH ()-[r:LINKS]->() WHERE r.weight > 10 AND r.keep = true RETURN r",
            None,
        )
        .unwrap();
    assert!(
        contains_operator(&plan.tree.root, "RelByPropertyRangeScan"),
        "expected the index prefilter to remain in the plan, got {:?}",
        plan.tree
    );

    let rows = db.run(
        "MATCH ()-[r:LINKS]->() WHERE r.weight > 10 AND r.keep = true RETURN r ORDER BY r.weight",
    );
    assert_eq!(pluck_rel_property(&rows, "r", "weight"), vec![20]);
}

#[test]
fn rel_range_scan_preserves_cross_type_comparison_semantics() {
    let db = TestDb::new();
    db.run("CREATE INDEX rel_weight_idx FOR ()-[r:LINKS]-() ON (r.weight)");
    db.run("CREATE (:P {id: 1})");
    db.run("CREATE (:P {id: 2})");
    db.run(
        "MATCH (a:P {id: 1}), (b:P {id: 2}) \
         CREATE (a)-[:LINKS {weight: 'heavy'}]->(b)",
    );

    let rows = db.run("MATCH ()-[r:LINKS]->() WHERE r.weight > 10 RETURN r");
    assert!(
        rows.is_empty(),
        "range-index rewrite must match scan+filter semantics for non-comparable values"
    );
}

#[test]
fn explain_rewrites_rel_text_filter_to_rel_text_scan() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX rel_label_idx FOR ()-[r:KNOWS]-() ON (r.note)");
    let plan = db
        .service
        .explain(
            "MATCH ()-[r:KNOWS]->() WHERE r.note STARTS WITH 'Co' RETURN r",
            None,
        )
        .unwrap();
    assert!(
        contains_operator(&plan.tree.root, "RelByTextScan"),
        "expected RelByTextScan in plan, got {:?}",
        plan.tree
    );
}

#[test]
fn rel_text_scan_returns_correct_rows() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX rel_note_idx FOR ()-[r:KNOWS]-() ON (r.note)");
    db.run("CREATE (:P {id: 1})");
    db.run("CREATE (:P {id: 2})");
    db.run("CREATE (:P {id: 3})");
    db.run(
        "MATCH (a:P {id: 1}), (b:P {id: 2}) \
         CREATE (a)-[:KNOWS {note: 'Colleague'}]->(b)",
    );
    db.run(
        "MATCH (a:P {id: 2}), (b:P {id: 3}) \
         CREATE (a)-[:KNOWS {note: 'Friend'}]->(b)",
    );
    db.run(
        "MATCH (a:P {id: 1}), (b:P {id: 3}) \
         CREATE (a)-[:KNOWS {note: 'Confidant'}]->(b)",
    );

    let rows =
        db.run("MATCH ()-[r:KNOWS]->() WHERE r.note STARTS WITH 'Co' RETURN r ORDER BY r.note");
    assert_eq!(rows.len(), 2);
}

#[test]
fn rel_text_scan_keeps_label_scan_for_empty_query() {
    // STARTS WITH '' is tautological — the rewrite must NOT fire,
    // matching the node-side guard.
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX rel_note_idx FOR ()-[r:KNOWS]-() ON (r.note)");
    db.run("CREATE (:P {id: 1})");
    db.run("CREATE (:P {id: 2})");
    db.run(
        "MATCH (a:P {id: 1}), (b:P {id: 2}) \
         CREATE (a)-[:KNOWS {note: 'X'}]->(b)",
    );

    let plan = db
        .service
        .explain(
            "MATCH ()-[r:KNOWS]->() WHERE r.note STARTS WITH '' RETURN r",
            None,
        )
        .unwrap();
    assert!(
        !contains_operator(&plan.tree.root, "RelByTextScan"),
        "tautological STARTS WITH '' must not trigger the index rewrite, got {:?}",
        plan.tree
    );
}

#[test]
fn explain_rewrites_rel_bbox_to_rel_point_scan() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX rel_loc_idx FOR ()-[r:DELIVERED]-() ON (r.loc)");
    let plan = db
        .service
        .explain(
            "MATCH ()-[r:DELIVERED]->() \
             WHERE point.withinBBox(r.loc, point({x: 0, y: 0}), point({x: 100, y: 100})) \
             RETURN r",
            None,
        )
        .unwrap();
    assert!(
        contains_operator(&plan.tree.root, "RelByPointScan"),
        "expected RelByPointScan in plan, got {:?}",
        plan.tree
    );
}

#[test]
fn rel_point_scan_returns_within_bbox_correctly() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX rel_loc_idx FOR ()-[r:DELIVERED]-() ON (r.loc)");
    db.run("CREATE (:P {id: 1})");
    db.run("CREATE (:P {id: 2})");
    db.run(
        "MATCH (a:P {id: 1}), (b:P {id: 2}) \
         CREATE (a)-[:DELIVERED {loc: point({x: 50, y: 50})}]->(b)",
    );
    db.run(
        "MATCH (a:P {id: 1}), (b:P {id: 2}) \
         CREATE (a)-[:DELIVERED {loc: point({x: 150, y: 150})}]->(b)",
    );

    let rows = db.run(
        "MATCH ()-[r:DELIVERED]->() \
         WHERE point.withinBBox(r.loc, point({x: 0, y: 0}), point({x: 100, y: 100})) \
         RETURN r",
    );
    assert_eq!(rows.len(), 1);
}

#[test]
fn rel_scan_skipped_when_src_has_label_constraint() {
    // Source constraint must keep the original Expand-based plan —
    // the rel-scan op doesn't refilter src by label.
    let db = TestDb::new();
    db.run("CREATE INDEX rel_since_idx FOR ()-[r:KNOWS]-() ON (r.since)");
    let plan = db
        .service
        .explain(
            "MATCH (a:Person)-[r:KNOWS]->() WHERE r.since > 2020 RETURN r",
            None,
        )
        .unwrap();
    assert!(
        !contains_operator(&plan.tree.root, "RelByPropertyRangeScan"),
        "rewrite must not fire when src has label constraint, got {:?}",
        plan.tree
    );
}

#[test]
fn rel_scan_with_residual_conjunct_keeps_filter_semantics() {
    // Rel-targeted scans replace the Expand under the Filter. The scan
    // may prefilter on the indexed conjunct, but the parent Filter must
    // still apply the residual weight predicate.
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX rel_note_idx FOR ()-[r:KNOWS]-() ON (r.note)");
    db.run("CREATE (:P {id: 1})");
    db.run("CREATE (:P {id: 2})");
    db.run(
        "MATCH (a:P {id: 1}), (b:P {id: 2}) \
         CREATE (a)-[:KNOWS {note: 'Colleague', weight: 1}]->(b)",
    );
    db.run(
        "MATCH (a:P {id: 2}), (b:P {id: 1}) \
         CREATE (a)-[:KNOWS {note: 'Colleague', weight: 10}]->(b)",
    );

    let plan = db
        .service
        .explain(
            "MATCH ()-[r:KNOWS]->() \
             WHERE r.note STARTS WITH 'Co' AND r.weight > 5 \
             RETURN r",
            None,
        )
        .unwrap();
    assert!(
        contains_operator(&plan.tree.root, "Filter"),
        "residual weight predicate must keep a parent Filter, got {:?}",
        plan.tree
    );
    assert!(
        contains_operator(&plan.tree.root, "RelByTextScan")
            || contains_operator(&plan.tree.root, "RelByPropertyRangeScan"),
        "one indexed conjunct should still prefilter below Filter, got {:?}",
        plan.tree
    );

    let rows = db.run(
        "MATCH ()-[r:KNOWS]->() \
         WHERE r.note STARTS WITH 'Co' AND r.weight > 5 \
         RETURN r",
    );
    assert_eq!(rows.len(), 1);
}

#[test]
fn rel_scan_with_duplicate_same_side_range_bounds_keeps_filter_semantics() {
    // Duplicate lower bounds can still prefilter through the strongest
    // lower bound, but the parent Filter must remain responsible for
    // the complete expression.
    let db = TestDb::new();
    db.run("CREATE INDEX rel_weight_idx FOR ()-[r:LINKS]-() ON (r.weight)");
    db.run("CREATE (:P {id: 1})");
    db.run("CREATE (:P {id: 2})");
    db.run(
        "MATCH (a:P {id: 1}), (b:P {id: 2}) \
         CREATE (a)-[:LINKS {weight: 20}]->(b)",
    );
    db.run(
        "MATCH (a:P {id: 2}), (b:P {id: 1}) \
         CREATE (a)-[:LINKS {weight: 40}]->(b)",
    );

    let query = "MATCH ()-[r:LINKS]->() \
                 WHERE r.weight > 10 AND r.weight > 30 \
                 RETURN r ORDER BY r.weight";
    let plan = db.service.explain(query, None).unwrap();
    assert!(
        contains_operator(&plan.tree.root, "Filter"),
        "duplicate lower bounds must keep a parent Filter, got {:?}",
        plan.tree
    );
    assert!(
        contains_operator(&plan.tree.root, "RelByPropertyRangeScan"),
        "strongest indexed lower bound should still prefilter, got {:?}",
        plan.tree
    );

    let rows = db.run(query);
    assert_eq!(pluck_rel_property(&rows, "r", "weight"), vec![40]);
}

#[test]
fn rel_scan_skipped_when_src_may_be_prebound() {
    let db = TestDb::new();
    db.run("CREATE INDEX rel_since_idx FOR ()-[r:KNOWS]-() ON (r.since)");

    let plan = db
        .service
        .explain(
            "MATCH (a:Person {id: 1}) \
             MATCH (a)-[r:KNOWS]->() \
             WHERE r.since > 2020 \
             RETURN r",
            None,
        )
        .unwrap();

    assert!(
        !contains_operator(&plan.tree.root, "RelByPropertyRangeScan"),
        "rewrite must not bypass an upstream source binding, got {:?}",
        plan.tree
    );
}
