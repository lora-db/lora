//! Segmented binary/blob value tests.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::LoraValue;
use lora_store::LoraBinary;
use serde_json::json;
use test_helpers::TestDb;

fn blob_value() -> LoraValue {
    LoraValue::Binary(LoraBinary::from_segments(vec![
        vec![0, 1, 2, 3],
        vec![250, 251, 252, 253, 254, 255],
    ]))
}

#[test]
fn binary_parameter_returns_tagged_shape() {
    let mut params = BTreeMap::new();
    params.insert("b".into(), blob_value());

    let rows = TestDb::new().run_with_params("RETURN $b AS b, valueType($b) AS t", params);
    let b = &rows[0]["b"];
    assert_eq!(b["kind"], "binary");
    assert_eq!(b["length"], 10);
    assert_eq!(
        b["segments"],
        json!([[0, 1, 2, 3], [250, 251, 252, 253, 254, 255]])
    );
    assert_eq!(rows[0]["t"], "BINARY");
}

#[test]
fn binary_parameter_stores_and_matches_as_node_property() {
    let db = TestDb::new();
    let mut params = BTreeMap::new();
    params.insert("b".into(), blob_value());
    db.run_with_params("CREATE (:Doc {id: 1, payload: $b})", params);

    let stored = db.run("MATCH (d:Doc {id: 1}) RETURN d.payload AS payload");
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0]["payload"]["segments"],
        json!([[0, 1, 2, 3], [250, 251, 252, 253, 254, 255]])
    );

    let mut lookup = BTreeMap::new();
    lookup.insert("b".into(), blob_value());
    let where_rows = db.run_with_params(
        "MATCH (d:Doc) WHERE d.payload = $b RETURN d.payload AS payload",
        lookup,
    );
    assert_eq!(where_rows.len(), 1);
    assert_eq!(
        where_rows[0]["payload"]["segments"],
        json!([[0, 1, 2, 3], [250, 251, 252, 253, 254, 255]])
    );

    let mut pattern_lookup = BTreeMap::new();
    pattern_lookup.insert("b".into(), blob_value());
    let rows = db.run_with_params(
        "MATCH (d:Doc {payload: $b}) RETURN d.payload AS payload",
        pattern_lookup,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["payload"]["segments"],
        json!([[0, 1, 2, 3], [250, 251, 252, 253, 254, 255]])
    );
}
