//! CREATE INDEX and SHOW INDEXES integration tests.

use std::collections::BTreeMap;

mod test_helpers;
use test_helpers::TestDb;

use lora_database::LoraValue;
use serde_json::Value as JsonValue;

fn rows_for_index_named<'a>(rows: &'a [JsonValue], name: &str) -> Option<&'a JsonValue> {
    rows.iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some(name))
}

#[test]
fn create_named_range_node_index() {
    let db = TestDb::new();
    let rows = db.run("CREATE INDEX node_range_index_name FOR (n:Person) ON (n.surname)");
    assert!(rows.is_empty(), "CREATE INDEX returns no rows on success");

    let listed = db.run("SHOW INDEXES");
    let entry = rows_for_index_named(&listed, "node_range_index_name")
        .expect("expected index in SHOW INDEXES");
    assert_eq!(entry["type"], JsonValue::String("RANGE".into()));
    assert_eq!(entry["entityType"], JsonValue::String("NODE".into()));
    assert_eq!(
        entry["labelsOrTypes"],
        JsonValue::Array(vec![JsonValue::String("Person".into())])
    );
    assert_eq!(
        entry["properties"],
        JsonValue::Array(vec![JsonValue::String("surname".into())])
    );
    assert_eq!(entry["state"], JsonValue::String("ONLINE".into()));
}

#[test]
fn schema_command_routing_ignores_long_leading_whitespace() {
    let db = TestDb::new();
    let query = format!(
        "{}CREATE INDEX spaced_index FOR (n:Person) ON (n.surname)",
        " ".repeat(128)
    );
    db.run(&query);

    let listed = db.run("SHOW INDEXES");
    assert!(rows_for_index_named(&listed, "spaced_index").is_some());
}

#[test]
fn create_default_kind_is_range() {
    let db = TestDb::new();
    db.run("CREATE INDEX default_kind FOR (n:Person) ON (n.surname)");
    let listed = db.run("SHOW INDEXES");
    let entry = rows_for_index_named(&listed, "default_kind").unwrap();
    assert_eq!(entry["type"], JsonValue::String("RANGE".into()));
}

#[test]
fn create_relationship_range_index() {
    let db = TestDb::new();
    db.run("CREATE INDEX rel_range_index_name FOR ()-[r:KNOWS]-() ON (r.since)");
    let listed = db.run("SHOW INDEXES");
    let entry = rows_for_index_named(&listed, "rel_range_index_name").unwrap();
    assert_eq!(entry["type"], JsonValue::String("RANGE".into()));
    assert_eq!(
        entry["entityType"],
        JsonValue::String("RELATIONSHIP".into())
    );
    assert_eq!(
        entry["labelsOrTypes"],
        JsonValue::Array(vec![JsonValue::String("KNOWS".into())])
    );
}

#[test]
fn create_composite_node_range_index() {
    let db = TestDb::new();
    db.run("CREATE INDEX composite_range_node_index_name FOR (n:Person) ON (n.age, n.country)");
    let listed = db.run("SHOW INDEXES");
    let entry = rows_for_index_named(&listed, "composite_range_node_index_name").unwrap();
    assert_eq!(
        entry["properties"],
        JsonValue::Array(vec![
            JsonValue::String("age".into()),
            JsonValue::String("country".into())
        ])
    );
}

#[test]
fn create_composite_relationship_range_index() {
    let db = TestDb::new();
    db.run(
        "CREATE INDEX composite_range_rel_index_name FOR ()-[r:PURCHASED]-() ON (r.date, r.amount)",
    );
    let listed = db.run("SHOW INDEXES");
    let entry = rows_for_index_named(&listed, "composite_range_rel_index_name").unwrap();
    assert_eq!(
        entry["properties"],
        JsonValue::Array(vec![
            JsonValue::String("date".into()),
            JsonValue::String("amount".into())
        ])
    );
}

#[test]
fn create_text_index() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX node_text_index_nickname FOR (n:Person) ON (n.nickname)");
    let listed = db.run("SHOW INDEXES");
    let entry = rows_for_index_named(&listed, "node_text_index_nickname").unwrap();
    assert_eq!(entry["type"], JsonValue::String("TEXT".into()));
}

#[test]
fn create_relationship_text_index() {
    let db = TestDb::new();
    db.run("CREATE TEXT INDEX rel_text_index_name FOR ()-[r:KNOWS]-() ON (r.interest)");
    let listed = db.run("SHOW INDEXES");
    let entry = rows_for_index_named(&listed, "rel_text_index_name").unwrap();
    assert_eq!(entry["type"], JsonValue::String("TEXT".into()));
    assert_eq!(
        entry["entityType"],
        JsonValue::String("RELATIONSHIP".into())
    );
}

#[test]
fn create_point_index() {
    let db = TestDb::new();
    db.run("CREATE POINT INDEX node_point_index_name FOR (n:Person) ON (n.sublocation)");
    let listed = db.run("SHOW INDEXES");
    let entry = rows_for_index_named(&listed, "node_point_index_name").unwrap();
    assert_eq!(entry["type"], JsonValue::String("POINT".into()));
}

#[test]
fn create_point_index_with_options() {
    let db = TestDb::new();
    db.run(
        "CREATE POINT INDEX point_index_with_config FOR (n:Label) ON (n.prop2) \
         OPTIONS { indexConfig: { `spatial.cartesian.min`: [-100.0, -100.0], \
         `spatial.cartesian.max`: [100.0, 100.0] } }",
    );
    let listed = db.run("SHOW INDEXES");
    assert!(rows_for_index_named(&listed, "point_index_with_config").is_some());
}

#[test]
fn create_lookup_index_node() {
    let db = TestDb::new();
    db.run("CREATE LOOKUP INDEX node_label_lookup_index FOR (n) ON EACH labels(n)");
    let listed = db.run("SHOW INDEXES");
    let entry = rows_for_index_named(&listed, "node_label_lookup_index").unwrap();
    assert_eq!(entry["type"], JsonValue::String("LOOKUP".into()));
    assert_eq!(entry["entityType"], JsonValue::String("NODE".into()));
    assert_eq!(entry["properties"], JsonValue::Array(Vec::new()));
}

#[test]
fn create_lookup_index_relationship() {
    let db = TestDb::new();
    db.run("CREATE LOOKUP INDEX rel_type_lookup_index FOR ()-[r]-() ON EACH type(r)");
    let listed = db.run("SHOW INDEXES");
    let entry = rows_for_index_named(&listed, "rel_type_lookup_index").unwrap();
    assert_eq!(entry["type"], JsonValue::String("LOOKUP".into()));
    assert_eq!(
        entry["entityType"],
        JsonValue::String("RELATIONSHIP".into())
    );
}

#[test]
fn create_index_using_parameter_for_name() {
    let db = TestDb::new();
    let mut params: BTreeMap<String, LoraValue> = BTreeMap::new();
    params.insert("name".into(), LoraValue::String("range_index_param".into()));
    let _ = db.run_with_params("CREATE INDEX $name FOR (n:Person) ON (n.firstname)", params);
    let listed = db.run("SHOW INDEXES");
    assert!(rows_for_index_named(&listed, "range_index_param").is_some());
}

#[test]
fn duplicate_index_name_errors() {
    let db = TestDb::new();
    db.run("CREATE INDEX dupe FOR (n:Person) ON (n.surname)");
    let err = db
        .exec("CREATE INDEX dupe FOR (n:Movie) ON (n.title)")
        .expect_err("duplicate name should fail");
    let msg = err.to_string();
    assert!(
        msg.contains("same name") || msg.contains("dupe"),
        "expected duplicated name error, got: {msg}"
    );
}

#[test]
fn equivalent_index_errors() {
    let db = TestDb::new();
    db.run("CREATE INDEX original FOR (n:Person) ON (n.surname)");
    let err = db
        .exec("CREATE INDEX another_name FOR (n:Person) ON (n.surname)")
        .expect_err("equivalent index should fail");
    let msg = err.to_string();
    assert!(
        msg.contains("equivalent") || msg.contains("Person"),
        "expected equivalent index error, got: {msg}"
    );
}

#[test]
fn if_not_exists_is_idempotent_on_name_match() {
    let db = TestDb::new();
    db.run("CREATE INDEX once FOR (n:Person) ON (n.surname)");
    let rows = db.run("CREATE INDEX once IF NOT EXISTS FOR (n:Movie) ON (n.title)");
    assert!(rows.is_empty(), "IF NOT EXISTS should not return rows");
    let listed = db.run("SHOW INDEXES");
    assert_eq!(listed.len(), 1, "no second index should have been created");
}

#[test]
fn if_not_exists_is_idempotent_on_schema_match() {
    let db = TestDb::new();
    db.run("CREATE INDEX one FOR (n:Person) ON (n.surname)");
    let rows = db.run("CREATE INDEX two IF NOT EXISTS FOR (n:Person) ON (n.surname)");
    assert!(rows.is_empty());
    let listed = db.run("SHOW INDEXES");
    assert_eq!(
        listed.len(),
        1,
        "schema-equivalent re-create should be a no-op"
    );
}

#[test]
fn auto_generated_name_when_omitted() {
    let db = TestDb::new();
    db.run("CREATE INDEX FOR (n:Person) ON (n.surname)");
    let listed = db.run("SHOW INDEXES");
    assert_eq!(listed.len(), 1);
    let name = listed[0]["name"].as_str().unwrap();
    assert!(
        name.starts_with("index_"),
        "expected auto-generated name to start with 'index_', got '{name}'"
    );
}

#[test]
fn show_indexes_empty_when_none_created() {
    let db = TestDb::new();
    let listed = db.run("SHOW INDEXES");
    assert!(listed.is_empty());
}

#[test]
fn drop_index_removes_named_entry() {
    let db = TestDb::new();
    db.run("CREATE INDEX gone FOR (n:Person) ON (n.surname)");
    let listed = db.run("SHOW INDEXES");
    assert_eq!(listed.len(), 1);
    let _ = db.run("DROP INDEX gone");
    let listed = db.run("SHOW INDEXES");
    assert!(listed.is_empty());
}

#[test]
fn drop_missing_index_errors() {
    let db = TestDb::new();
    let err = db
        .exec("DROP INDEX nope")
        .expect_err("dropping a missing index should fail");
    let msg = err.to_string();
    assert!(msg.contains("42N51"), "expected 42N51 status, got: {msg}");
    assert!(msg.contains("nope"));
}

#[test]
fn drop_missing_index_if_exists_is_noop() {
    let db = TestDb::new();
    let rows = db.run("DROP INDEX nope IF EXISTS");
    assert!(rows.is_empty());
}

#[test]
fn create_index_error_carries_gql_status() {
    let db = TestDb::new();
    db.run("CREATE INDEX dupe FOR (n:Person) ON (n.surname)");
    let err = db
        .exec("CREATE INDEX dupe FOR (n:Movie) ON (n.title)")
        .expect_err("duplicate name should fail");
    assert!(err.to_string().contains("22N71"));
}
