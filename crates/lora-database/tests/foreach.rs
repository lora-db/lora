//! FOREACH clause tests.
//!
//! Covers the Cypher-style conditional set idiom
//! `FOREACH (_ IN CASE WHEN cond THEN [1] ELSE [] END | SET ...)`,
//! along with the `point({longitude, latitude, crs})` and
//! `datetime("...")` builtins that frequent FOREACH-driven ingestion
//! queries.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::LoraValue;
use serde_json::Value as JsonValue;
use test_helpers::TestDb;

#[test]
fn foreach_runs_body_per_list_element() {
    let db = TestDb::new();
    db.run(
        "CREATE (n:Bag) \
         FOREACH (x IN [1, 2, 3] | \
            CREATE (:Item { value: x }))",
    );

    db.assert_count("MATCH (i:Item) RETURN i", 3);
    let values: Vec<JsonValue> = db.run("MATCH (i:Item) RETURN i.value AS v ORDER BY v");
    assert_eq!(
        values
            .iter()
            .map(|r| r["v"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2, 3],
    );
}

#[test]
fn foreach_with_empty_list_runs_body_zero_times() {
    let db = TestDb::new();
    // Pre-seed an Item so the label is registered in the catalog (the
    // analyzer rejects MATCHes against labels it has never seen once
    // the graph is non-empty). The FOREACH below has an empty list,
    // so it must not create any additional items.
    db.run("CREATE (:Item { value: 999 })");
    db.run(
        "CREATE (n:Bag) \
         FOREACH (_ IN [] | \
            CREATE (:Item { value: 1 }))",
    );

    // Only the pre-seeded Item survives — FOREACH body never ran.
    db.assert_count("MATCH (i:Item) RETURN i", 1);
    db.assert_count("MATCH (b:Bag) RETURN b", 1);
}

#[test]
fn foreach_case_idiom_sets_property_when_condition_holds() {
    let db = TestDb::new();
    db.run("CREATE (n:Thing { name: 'a' })");
    db.run(
        "MATCH (n:Thing { name: 'a' }) \
         FOREACH (_ IN CASE WHEN n.name = 'a' THEN [1] ELSE [] END | \
            SET n.flagged = true)",
    );

    let rows = db.run("MATCH (n:Thing { name: 'a' }) RETURN n.flagged AS flagged");
    assert_eq!(rows[0]["flagged"], JsonValue::Bool(true));
}

#[test]
fn foreach_case_idiom_skips_when_condition_false() {
    let db = TestDb::new();
    // Seed a node with `flagged` set so the property key is in the
    // catalog — then the read-side analyzer accepts `n.flagged` even
    // when the row we want to assert about has not been flagged.
    db.run("CREATE (:Other { flagged: false })");
    db.run("CREATE (n:Thing { name: 'b' })");
    db.run(
        "MATCH (n:Thing { name: 'b' }) \
         FOREACH (_ IN CASE WHEN n.name = 'a' THEN [1] ELSE [] END | \
            SET n.flagged = true)",
    );

    let rows = db.run("MATCH (n:Thing { name: 'b' }) RETURN n.flagged AS flagged");
    assert!(rows[0]["flagged"].is_null());
}

#[test]
fn point_function_builds_geographic_point_from_map() {
    let db = TestDb::new();
    let rows = db.run("RETURN point({ longitude: 4.9, latitude: 52.37, crs: 'wgs-84' }) AS p");
    // Point serialization: shape varies by binding but it should not be null
    // and should carry a longitude/latitude or x/y pair.
    let p = &rows[0]["p"];
    assert!(!p.is_null(), "point() returned null: {p:?}");
}

#[test]
fn datetime_parses_iso_string() {
    let db = TestDb::new();
    let rows = db.run("RETURN datetime('2025-01-02T03:04:05Z') AS t");
    assert!(!rows[0]["t"].is_null(), "datetime() parse returned null");
}

#[test]
fn venue_ingestion_unwind_foreach_pattern_end_to_end() {
    let db = TestDb::new();

    let venues = LoraValue::List(vec![
        venue(
            "Cafe One",
            "1 Foo St",
            "cafe",
            Some(101),
            Some(4.9),
            Some(52.37),
            Some("2025-01-02T03:04:05Z"),
        ),
        venue("Park Two", "2 Bar Ave", "park", None, None, None, None),
        venue(
            "Shop Three",
            "3 Baz Rd",
            "shop",
            Some(303),
            Some(4.5),
            Some(52.0),
            None,
        ),
    ]);

    let mut params = BTreeMap::new();
    params.insert("venues".to_string(), venues);

    db.run_with_params(
        "UNWIND $venues AS v \
         CREATE (n:Venue) \
         SET n.name = v.name, \
             n.address = v.address, \
             n.category = v.category \
         FOREACH (_ IN CASE WHEN v.osm_id IS NOT NULL THEN [1] ELSE [] END | \
            SET n.osm_id = toInteger(v.osm_id)) \
         FOREACH (_ IN CASE WHEN v.created_at IS NOT NULL THEN [1] ELSE [] END | \
            SET n.created_at = datetime(v.created_at)) \
         FOREACH (_ IN CASE WHEN v.lng IS NOT NULL AND v.lat IS NOT NULL THEN [1] ELSE [] END | \
            SET n.location = point({ \
                longitude: toFloat(v.lng), \
                latitude: toFloat(v.lat), \
                crs: 'wgs-84' \
            }))",
        params,
    );

    db.assert_count("MATCH (v:Venue) RETURN v", 3);

    // Cafe One has all optional fields populated.
    let rows =
        db.run("MATCH (v:Venue { name: 'Cafe One' }) RETURN v.osm_id AS osm, v.location AS loc");
    assert_eq!(rows[0]["osm"].as_i64().unwrap(), 101);
    assert!(!rows[0]["loc"].is_null(), "location should be set");

    // Park Two had no optional fields; they must remain null.
    let rows = db.run(
        "MATCH (v:Venue { name: 'Park Two' }) \
         RETURN v.osm_id AS osm, v.location AS loc, v.created_at AS created",
    );
    assert!(rows[0]["osm"].is_null(), "osm_id should be unset");
    assert!(rows[0]["loc"].is_null(), "location should be unset");
    assert!(rows[0]["created"].is_null(), "created_at should be unset");
}

fn venue(
    name: &str,
    address: &str,
    category: &str,
    osm_id: Option<i64>,
    lng: Option<f64>,
    lat: Option<f64>,
    created_at: Option<&str>,
) -> LoraValue {
    let mut m = BTreeMap::new();
    m.insert("name".to_string(), LoraValue::String(name.to_string()));
    m.insert(
        "address".to_string(),
        LoraValue::String(address.to_string()),
    );
    m.insert(
        "category".to_string(),
        LoraValue::String(category.to_string()),
    );
    m.insert(
        "osm_id".to_string(),
        osm_id.map(LoraValue::Int).unwrap_or(LoraValue::Null),
    );
    m.insert(
        "lng".to_string(),
        lng.map(LoraValue::Float).unwrap_or(LoraValue::Null),
    );
    m.insert(
        "lat".to_string(),
        lat.map(LoraValue::Float).unwrap_or(LoraValue::Null),
    );
    m.insert(
        "created_at".to_string(),
        created_at
            .map(|s| LoraValue::String(s.to_string()))
            .unwrap_or(LoraValue::Null),
    );
    LoraValue::Map(m)
}
