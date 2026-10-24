//! G-69: a bounding box on a point-indexed property, next to indexed
//! equalities, seeks the point index when it is the narrower probe. A box
//! from the row or from parameters is taken as narrow; a literal box
//! covers its share of the globe; a unique key still wins.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::{LoraValue, PlanTreeNode};
use test_helpers::TestDb;

fn leaf_scan(node: &PlanTreeNode) -> &PlanTreeNode {
    if node.operator.starts_with("NodeBy") {
        return node;
    }
    leaf_scan(node.children.first().expect("a scan below"))
}

fn seed(db: &TestDb) {
    db.run("CREATE CONSTRAINT ek FOR (e:Entity) REQUIRE e.key IS UNIQUE");
    for p in ["country", "kind", "state"] {
        db.run(&format!("CREATE INDEX e_{p} FOR (e:Entity) ON (e.{p})"));
    }
    db.run("CREATE POINT INDEX e_point FOR (e:Entity) ON (e.point)");
    // 31 countries of 400 places, each country a 2x2 degree square.
    db.run(
        "UNWIND range(0, 30) AS c UNWIND range(0, 399) AS i
         CREATE (:Entity {key: 'p' + toString(c) + '_' + toString(i), kind: 'place',
           state: 'ACTIVE', country: 'C' + toString(c),
           point: point({longitude: -10 + (c % 6) * 3 + (i % 20) * 0.1,
                         latitude: 35 + (c / 6) * 3 + (i / 20) * 0.1})})",
    );
}

fn box_param(iso: &str, ll: (f64, f64), ur: (f64, f64)) -> LoraValue {
    let point = |(lon, lat): (f64, f64)| {
        LoraValue::Map(BTreeMap::from([
            ("longitude".to_string(), LoraValue::Float(lon)),
            ("latitude".to_string(), LoraValue::Float(lat)),
        ]))
    };
    LoraValue::Map(BTreeMap::from([
        ("iso".to_string(), LoraValue::String(iso.to_string())),
        ("ll".to_string(), point(ll)),
        ("ur".to_string(), point(ur)),
    ]))
}

const NEAR: &str = "UNWIND $boxes AS b MATCH (p:Entity)
    WHERE geo.within_bbox(p.point, point(b.ll), point(b.ur))
      AND p.kind = 'place' AND p.state = 'ACTIVE' AND p.country = b.iso
    RETURN p.key AS k ORDER BY k";

#[test]
fn a_box_from_the_row_seeks_the_point_index() {
    let db = TestDb::new();
    seed(&db);
    let params = BTreeMap::from([(
        "boxes".to_string(),
        LoraValue::List(vec![box_param("C0", (-10.01, 34.99), (-9.89, 35.01))]),
    )]);
    let plan = db
        .service
        .explain(NEAR, Some(params.clone()))
        .unwrap()
        .tree
        .root;
    assert_eq!(leaf_scan(&plan).operator, "NodeByPointScan", "{plan:#?}");
    let rows = db.run_with_params(NEAR, params);
    let keys: Vec<_> = rows.iter().map(|r| r["k"].as_str().unwrap()).collect();
    assert_eq!(keys, vec!["p0_0", "p0_1"]);
    // The reference: the box alone in the MATCH, the rest after a WITH.
    let reference = db.run_with_params(
        "UNWIND $boxes AS b MATCH (p:Entity) WHERE geo.within_bbox(p.point, point(b.ll), point(b.ur))
         WITH b, p WHERE p.kind = 'place' AND p.state = 'ACTIVE' AND p.country = b.iso
         RETURN p.key AS k ORDER BY k",
        BTreeMap::from([(
            "boxes".to_string(),
            LoraValue::List(vec![box_param("C0", (-10.01, 34.99), (-9.89, 35.01))]),
        )]),
    );
    assert_eq!(rows, reference);
}

#[test]
fn a_literal_box_covering_a_continent_loses_to_the_equality() {
    let db = TestDb::new();
    seed(&db);
    let wide = "MATCH (p:Entity)
        WHERE geo.within_bbox(p.point, point({longitude: -30, latitude: 0}), point({longitude: 60, latitude: 75}))
          AND p.country = 'C3' RETURN count(p) AS c";
    let plan = db.service.explain(wide, None).unwrap().tree.root;
    let scan = leaf_scan(&plan);
    assert_eq!(scan.operator, "NodeByPropertyScan", "{plan:#?}");
    assert_eq!(db.run(wide)[0]["c"], 400);

    let narrow = "MATCH (p:Entity)
        WHERE geo.within_bbox(p.point, point({longitude: -10.01, latitude: 34.99}), point({longitude: -9.89, latitude: 35.01}))
          AND p.country = 'C0' RETURN count(p) AS c";
    let plan = db.service.explain(narrow, None).unwrap().tree.root;
    assert_eq!(leaf_scan(&plan).operator, "NodeByPointScan", "{plan:#?}");
    assert_eq!(db.run(narrow)[0]["c"], 2);
}

#[test]
fn a_unique_key_beats_the_box() {
    let db = TestDb::new();
    seed(&db);
    let q = "MATCH (p:Entity) WHERE geo.within_bbox(p.point, $ll, $ur) AND p.key = 'p0_0' RETURN p.key AS k";
    let point = |lon: f64, lat: f64| {
        LoraValue::Map(BTreeMap::from([
            ("longitude".to_string(), LoraValue::Float(lon)),
            ("latitude".to_string(), LoraValue::Float(lat)),
        ]))
    };
    let params = BTreeMap::from([
        ("ll".to_string(), point(-11.0, 34.0)),
        ("ur".to_string(), point(-9.0, 36.0)),
    ]);
    let plan = db.service.explain(q, Some(params)).unwrap().tree.root;
    let scan = leaf_scan(&plan);
    assert_eq!(scan.operator, "NodeByPropertyScan", "{plan:#?}");
    assert_eq!(scan.details.get("key").map(String::as_str), Some("key"));
}
