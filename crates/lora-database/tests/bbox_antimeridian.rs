//! A geographic `geo.within_bbox` whose lower-left longitude is greater
//! than its upper-right longitude crosses the antimeridian, with and
//! without a point index, on nodes and relationships.

mod test_helpers;
use test_helpers::TestDb;

fn seed_places(db: &TestDb) {
    db.run(
        "CREATE (:F {key:'fiji', at: point({longitude: 178.0, latitude: -18.0})}), \
                (:F {key:'samoa', at: point({longitude: -172.0, latitude: -13.8})}), \
                (:F {key:'paris', at: point({longitude: 2.35, latitude: 48.85})}), \
                (:F {key:'north', at: point({longitude: 179.0, latitude: 70.0})})",
    );
}

const ACROSS: &str = "MATCH (f:F) WHERE geo.within_bbox(f.at, \
    point({longitude: 170, latitude: -60}), point({longitude: -170, latitude: 60})) \
    RETURN f.key AS k ORDER BY k";

#[test]
fn bbox_across_the_antimeridian_without_an_index() {
    let db = TestDb::new();
    seed_places(&db);
    assert_eq!(db.sorted_strings(ACROSS, "k"), vec!["fiji", "samoa"]);
}

#[test]
fn bbox_across_the_antimeridian_seeks_the_point_index() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX fa FOR (f:F) ON (f.at)");
    seed_places(&db);
    assert_eq!(db.sorted_strings(ACROSS, "k"), vec!["fiji", "samoa"]);
    let plan = db.service.explain(ACROSS, None).unwrap();
    let tree = format!("{:?}", plan.tree);
    assert!(tree.contains("NodeByPointScan"), "{tree}");
}

#[test]
fn bbox_across_the_antimeridian_on_relationships() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX ra FOR ()-[r:AT]-() ON (r.at)");
    db.run(
        "CREATE (:X)-[:AT {key:'fiji', at: point({longitude: 178.0, latitude: -18.0})}]->(:Y), \
                (:X)-[:AT {key:'paris', at: point({longitude: 2.35, latitude: 48.85})}]->(:Y)",
    );
    let got = db.sorted_strings(
        "MATCH ()-[r:AT]->() WHERE geo.within_bbox(r.at, \
         point({longitude: 170, latitude: -60}), point({longitude: -170, latitude: 60})) \
         RETURN r.key AS k",
        "k",
    );
    assert_eq!(got, vec!["fiji"]);
}

#[test]
fn ordinary_and_cartesian_boxes_are_unchanged() {
    let db = TestDb::new();
    seed_places(&db);
    let got = db.sorted_strings(
        "MATCH (f:F) WHERE geo.within_bbox(f.at, \
         point({longitude: -10, latitude: 40}), point({longitude: 10, latitude: 60})) \
         RETURN f.key AS k",
        "k",
    );
    assert_eq!(got, vec!["paris"]);
    // Cartesian corners in either order still mean min/max.
    let inside = db.scalar(
        "RETURN geo.within_bbox(point({x: 5, y: 5}), point({x: 10, y: 0}), point({x: 0, y: 10})) AS v",
    );
    assert_eq!(inside, serde_json::json!(true));
}
