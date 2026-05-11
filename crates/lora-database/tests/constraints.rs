//! CREATE / DROP / SHOW CONSTRAINT integration tests.
//!
//! These cover catalog round-trips, DDL-time data validation, and
//! mutation-time enforcement end-to-end through the Database façade.

mod test_helpers;
use test_helpers::TestDb;

use std::collections::BTreeMap;

use lora_database::LoraValue;
use serde_json::Value as JsonValue;

fn rows_for_constraint_named<'a>(rows: &'a [JsonValue], name: &str) -> Option<&'a JsonValue> {
    rows.iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some(name))
}

#[test]
fn create_unique_node_constraint_round_trip() {
    let db = TestDb::new();
    let rows = db.run("CREATE CONSTRAINT book_isbn FOR (book:Book) REQUIRE book.isbn IS UNIQUE");
    assert!(rows.is_empty(), "CREATE CONSTRAINT returns no rows");

    let listed = db.run("SHOW CONSTRAINTS");
    let entry = rows_for_constraint_named(&listed, "book_isbn").expect("constraint listed");
    assert_eq!(
        entry["type"],
        JsonValue::String("NODE_PROPERTY_UNIQUENESS".into())
    );
    assert_eq!(entry["entityType"], JsonValue::String("NODE".into()));
    assert_eq!(
        entry["labelsOrTypes"],
        JsonValue::Array(vec![JsonValue::String("Book".into())])
    );
    assert_eq!(
        entry["properties"],
        JsonValue::Array(vec![JsonValue::String("isbn".into())])
    );
    // Uniqueness is backed by a RANGE index of the same name.
    assert_eq!(entry["ownedIndex"], JsonValue::String("book_isbn".into()));

    // The backing index is visible through SHOW INDEXES too.
    let idx_listed = db.run("SHOW INDEXES");
    let idx_entry = idx_listed
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("book_isbn"))
        .expect("backing index listed");
    assert_eq!(idx_entry["type"], JsonValue::String("RANGE".into()));
}

#[test]
fn create_composite_unique_constraint_relationship() {
    let db = TestDb::new();
    db.run(
        "CREATE CONSTRAINT prequels FOR ()-[seq:SEQUEL_OF]-() \
         REQUIRE (seq.order, seq.author) IS UNIQUE",
    );
    let listed = db.run("SHOW CONSTRAINTS");
    let entry = rows_for_constraint_named(&listed, "prequels").unwrap();
    assert_eq!(
        entry["type"],
        JsonValue::String("RELATIONSHIP_PROPERTY_UNIQUENESS".into())
    );
    assert_eq!(
        entry["properties"],
        JsonValue::Array(vec![
            JsonValue::String("order".into()),
            JsonValue::String("author".into()),
        ])
    );
}

#[test]
fn create_existence_constraint() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT author_name FOR (a:Author) REQUIRE a.name IS NOT NULL");
    let listed = db.run("SHOW CONSTRAINTS");
    let entry = rows_for_constraint_named(&listed, "author_name").unwrap();
    assert_eq!(
        entry["type"],
        JsonValue::String("NODE_PROPERTY_EXISTENCE".into())
    );
    // Existence constraints have no backing index.
    assert_eq!(entry["ownedIndex"], JsonValue::Null);
}

#[test]
fn create_node_key_constraint_composite() {
    let db = TestDb::new();
    db.run(
        "CREATE CONSTRAINT actor_fullname FOR (a:Actor) \
         REQUIRE (a.firstname, a.surname) IS NODE KEY",
    );
    let listed = db.run("SHOW CONSTRAINTS");
    let entry = rows_for_constraint_named(&listed, "actor_fullname").unwrap();
    assert_eq!(entry["type"], JsonValue::String("NODE_KEY".into()));
    assert_eq!(
        entry["ownedIndex"],
        JsonValue::String("actor_fullname".into())
    );
}

#[test]
fn create_property_type_constraint_string() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT movie_title FOR (m:Movie) REQUIRE m.title IS :: STRING");
    let listed = db.run("SHOW CONSTRAINTS");
    let entry = rows_for_constraint_named(&listed, "movie_title").unwrap();
    assert_eq!(
        entry["type"],
        JsonValue::String("NODE_PROPERTY_TYPE".into())
    );
    assert_eq!(entry["propertyType"], JsonValue::String("STRING".into()));
}

#[test]
fn create_property_type_constraint_union() {
    let db = TestDb::new();
    db.run(
        "CREATE CONSTRAINT tagline FOR (m:Movie) \
         REQUIRE m.tagline IS :: STRING | LIST<STRING NOT NULL>",
    );
    let listed = db.run("SHOW CONSTRAINTS");
    let entry = rows_for_constraint_named(&listed, "tagline").unwrap();
    assert_eq!(
        entry["propertyType"],
        JsonValue::String("STRING | LIST<STRING NOT NULL>".into())
    );
}

#[test]
fn duplicate_constraint_name_errors() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT c FOR (n:A) REQUIRE n.x IS UNIQUE");
    let err = db.run_err("CREATE CONSTRAINT c FOR (n:B) REQUIRE n.y IS UNIQUE");
    assert!(err.contains("22N67"), "expected 22N67, got: {err}");
}

#[test]
fn equivalent_constraint_errors() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT c1 FOR (n:Book) REQUIRE n.isbn IS UNIQUE");
    let err = db.run_err("CREATE CONSTRAINT c2 FOR (n:Book) REQUIRE n.isbn IS UNIQUE");
    assert!(err.contains("22N65"), "expected 22N65, got: {err}");
}

#[test]
fn conflicting_unique_and_node_key_errors() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT u FOR (n:Book) REQUIRE n.isbn IS UNIQUE");
    let err = db.run_err("CREATE CONSTRAINT nk FOR (n:Book) REQUIRE n.isbn IS NODE KEY");
    assert!(err.contains("22N66"), "expected 22N66, got: {err}");
}

#[test]
fn if_not_exists_is_idempotent() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT c FOR (n:Book) REQUIRE n.isbn IS UNIQUE");
    let rows = db.run("CREATE CONSTRAINT c IF NOT EXISTS FOR (n:Book) REQUIRE n.isbn IS UNIQUE");
    assert!(rows.is_empty(), "no-op CREATE returns no rows");
}

#[test]
fn drop_constraint() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT c FOR (n:Book) REQUIRE n.isbn IS UNIQUE");
    db.run("DROP CONSTRAINT c");
    let listed = db.run("SHOW CONSTRAINTS");
    assert!(rows_for_constraint_named(&listed, "c").is_none());
    // Backing index is cascaded away.
    let idx_listed = db.run("SHOW INDEXES");
    assert!(idx_listed
        .iter()
        .all(|r| r.get("name").and_then(|v| v.as_str()) != Some("c")));
}

#[test]
fn drop_backing_index_directly_is_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT c FOR (n:Book) REQUIRE n.isbn IS UNIQUE");
    let err = db.run_err("DROP INDEX c");
    assert!(
        err.contains("owned by constraint") && err.contains("DROP CONSTRAINT"),
        "expected owned-index error, got: {err}"
    );

    let constraints = db.run("SHOW CONSTRAINTS");
    assert!(rows_for_constraint_named(&constraints, "c").is_some());
    let indexes = db.run("SHOW INDEXES");
    assert!(indexes
        .iter()
        .any(|r| r.get("name").and_then(|v| v.as_str()) == Some("c")));
}

#[test]
fn drop_constraint_missing_errors() {
    let db = TestDb::new();
    let err = db.run_err("DROP CONSTRAINT missing");
    assert!(err.contains("42N51"), "expected 42N51, got: {err}");
}

#[test]
fn drop_constraint_if_exists_is_idempotent() {
    let db = TestDb::new();
    let rows = db.run("DROP CONSTRAINT missing IF EXISTS");
    assert!(rows.is_empty());
}

#[test]
fn constraint_collides_with_existing_index_name() {
    let db = TestDb::new();
    db.run("CREATE INDEX directors FOR (d:Director) ON (d.name)");
    let err = db.run_err("CREATE CONSTRAINT directors FOR (m:Movie) REQUIRE m.title IS UNIQUE");
    assert!(err.contains("22N71"), "expected 22N71, got: {err}");
}

#[test]
fn constraint_collides_with_existing_same_shape_index_name() {
    let db = TestDb::new();
    db.run("CREATE INDEX book_isbn FOR (b:Book) ON (b.isbn)");
    let err = db.run_err("CREATE CONSTRAINT book_isbn FOR (b:Book) REQUIRE b.isbn IS UNIQUE");
    assert!(err.contains("22N71"), "expected 22N71, got: {err}");
}

#[test]
fn unique_constraint_conflicts_with_existing_index_on_same_schema() {
    let db = TestDb::new();
    db.run("CREATE INDEX book_isbn_idx FOR (b:Book) ON (b.isbn)");
    let err = db.run_err("CREATE CONSTRAINT book_isbn FOR (b:Book) REQUIRE b.isbn IS UNIQUE");
    assert!(err.contains("22N73"), "expected 22N73, got: {err}");
}

#[test]
fn property_type_constraint_rejects_map() {
    let db = TestDb::new();
    let err = db.run_err("CREATE CONSTRAINT score FOR (m:Movie) REQUIRE m.imdbScore IS :: MAP");
    assert!(err.contains("22N90"), "expected 22N90, got: {err}");
}

// ---------- DDL-time data validation ----------

#[test]
fn create_unique_constraint_rejects_existing_duplicates() {
    let db = TestDb::new();
    db.run("CREATE (:Book {isbn: 'X', title: 'Moby Dick'})");
    db.run("CREATE (:Book {isbn: 'X', title: 'Other'})");
    let err = db.run_err("CREATE CONSTRAINT book_isbn FOR (b:Book) REQUIRE b.isbn IS UNIQUE");
    assert!(
        err.contains("50N11") && err.contains("22N79"),
        "expected 50N11 wrapping 22N79, got: {err}"
    );
}

#[test]
fn create_unique_constraint_accepts_when_data_is_clean() {
    let db = TestDb::new();
    db.run("CREATE (:Book {isbn: 'A'})");
    db.run("CREATE (:Book {isbn: 'B'})");
    db.run("CREATE CONSTRAINT book_isbn FOR (b:Book) REQUIRE b.isbn IS UNIQUE");
}

#[test]
fn create_unique_constraint_does_not_collide_distinct_list_values() {
    let db = TestDb::new();
    db.run("CREATE (:Thing {code: ['a', 'b']})");
    db.run("CREATE (:Thing {code: ['a,Sb']})");
    db.run("CREATE CONSTRAINT thing_code FOR (t:Thing) REQUIRE t.code IS UNIQUE");
}

#[test]
fn create_existence_constraint_rejects_when_property_missing() {
    let db = TestDb::new();
    db.run("CREATE (:Author {name: 'Virginia'})");
    db.run("CREATE (:Author {surname: 'Austen'})");
    let err = db.run_err("CREATE CONSTRAINT author_name FOR (a:Author) REQUIRE a.name IS NOT NULL");
    assert!(
        err.contains("50N11") && err.contains("22N77"),
        "expected 50N11 wrapping 22N77, got: {err}"
    );
}

#[test]
fn create_type_constraint_rejects_existing_wrong_type() {
    let db = TestDb::new();
    db.run("CREATE (:Movie {title: 'OK'})");
    db.run("CREATE (:Movie {title: 13})");
    let err =
        db.run_err("CREATE CONSTRAINT movie_title FOR (m:Movie) REQUIRE m.title IS :: STRING");
    assert!(
        err.contains("50N11") && err.contains("22N78"),
        "expected 50N11 wrapping 22N78, got: {err}"
    );
}

#[test]
fn create_node_key_rejects_missing_property() {
    let db = TestDb::new();
    db.run("CREATE (:Actor {first: 'Keanu', last: 'Reeves'})");
    db.run("CREATE (:Actor {last: 'Brontë'})");
    let err = db.run_err(
        "CREATE CONSTRAINT actor_fullname FOR (a:Actor) \
         REQUIRE (a.first, a.last) IS NODE KEY",
    );
    assert!(
        err.contains("50N11") && err.contains("22N77"),
        "expected 50N11 wrapping 22N77, got: {err}"
    );
}

#[test]
fn create_relationship_existence_constraint_rejects_missing_property() {
    let db = TestDb::new();
    db.run("CREATE (a:Author {name: 'A'}), (b:Book {t: 'B'}), (a)-[:WROTE {year: 1900}]->(b)");
    db.run("CREATE (a:Author {name: 'C'}), (b:Book {t: 'D'}), (a)-[:WROTE]->(b)");
    let err =
        db.run_err("CREATE CONSTRAINT wrote_year FOR ()-[w:WROTE]-() REQUIRE w.year IS NOT NULL");
    assert!(
        err.contains("50N11") && err.contains("22N77"),
        "expected 50N11 wrapping 22N77, got: {err}"
    );
}

// ---------- Mutation-time enforcement ----------

#[test]
fn create_node_duplicate_unique_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT book_isbn FOR (b:Book) REQUIRE b.isbn IS UNIQUE");
    db.run("CREATE (:Book {isbn: 'A'})");
    let err = db.run_err("CREATE (:Book {isbn: 'A'})");
    assert!(err.contains("22N79"), "expected 22N79, got: {err}");
}

#[test]
fn create_node_missing_existence_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT author_name FOR (a:Author) REQUIRE a.name IS NOT NULL");
    let err = db.run_err("CREATE (:Author {surname: 'Austen'})");
    assert!(err.contains("22N77"), "expected 22N77, got: {err}");
}

#[test]
fn create_node_wrong_type_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT movie_title FOR (m:Movie) REQUIRE m.title IS :: STRING");
    let err = db.run_err("CREATE (:Movie {title: 13})");
    assert!(err.contains("22N78"), "expected 22N78, got: {err}");
}

#[test]
fn create_node_composite_node_key_rejects_missing_property() {
    let db = TestDb::new();
    db.run(
        "CREATE CONSTRAINT actor_full FOR (a:Actor) \
         REQUIRE (a.first, a.last) IS NODE KEY",
    );
    let err = db.run_err("CREATE (:Actor {last: 'Brontë'})");
    assert!(err.contains("22N77"), "expected 22N77, got: {err}");
}

#[test]
fn set_property_violating_uniqueness_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT book_isbn FOR (b:Book) REQUIRE b.isbn IS UNIQUE");
    db.run("CREATE (:Book {isbn: 'A'})");
    db.run("CREATE (:Book {isbn: 'B'})");
    let err = db.run_err("MATCH (b:Book {isbn: 'B'}) SET b.isbn = 'A'");
    assert!(err.contains("22N79"), "expected 22N79, got: {err}");
}

#[test]
fn set_property_wrong_type_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT movie_title FOR (m:Movie) REQUIRE m.title IS :: STRING");
    db.run("CREATE (:Movie {title: 'Iron Man'})");
    let err = db.run_err("MATCH (m:Movie) SET m.title = 13");
    assert!(err.contains("22N78"), "expected 22N78, got: {err}");
}

#[test]
fn remove_existence_constrained_property_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT author_name FOR (a:Author) REQUIRE a.name IS NOT NULL");
    db.run("CREATE (:Author {name: 'V'})");
    let err = db.run_err("MATCH (a:Author) REMOVE a.name");
    assert!(err.contains("22N77"), "expected 22N77, got: {err}");
}

#[test]
fn replace_node_properties_missing_existence_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT author_name FOR (a:Author) REQUIRE a.name IS NOT NULL");
    db.run("CREATE (:Author {name: 'V'})");
    let err = db.run_err("MATCH (a:Author) SET a = {surname: 'Woolf'}");
    assert!(err.contains("22N77"), "expected 22N77, got: {err}");
}

#[test]
fn replace_node_properties_composite_unique_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT pair FOR (n:N) REQUIRE (n.a, n.b) IS UNIQUE");
    db.run("CREATE (:N {id: 1, a: 1, b: 2})");
    db.run("CREATE (:N {id: 2, a: 2, b: 3})");
    let err = db.run_err("MATCH (n:N {id: 1}) SET n = {id: 1, a: 2, b: 3}");
    assert!(err.contains("22N79"), "expected 22N79, got: {err}");
}

#[test]
fn replace_node_properties_keeps_own_unique_value_valid() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT book_isbn FOR (b:Book) REQUIRE b.isbn IS UNIQUE");
    db.run("CREATE (:Book {isbn: 'A', title: 'Old'})");
    db.run("MATCH (b:Book {isbn: 'A'}) SET b = {isbn: 'A', title: 'New'}");
    let rows = db.run("MATCH (b:Book) RETURN b.title AS title");
    assert_eq!(rows[0]["title"], JsonValue::String("New".into()));
}

#[test]
fn create_relationship_violating_constraint_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT wrote_year FOR ()-[w:WROTE]-() REQUIRE w.year IS NOT NULL");
    db.run("CREATE (:Author {name: 'A'}), (:Book {t: 'B'})");
    let err = db.run_err("MATCH (a:Author), (b:Book) CREATE (a)-[:WROTE]->(b)");
    assert!(err.contains("22N77"), "expected 22N77, got: {err}");
}

#[test]
fn replace_relationship_properties_missing_existence_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT wrote_year FOR ()-[w:WROTE]-() REQUIRE w.year IS NOT NULL");
    db.run("CREATE (:Author {name: 'A'}), (:Book {t: 'B'})");
    db.run("MATCH (a:Author), (b:Book) CREATE (a)-[:WROTE {year: 1900}]->(b)");
    let err = db.run_err("MATCH ()-[w:WROTE]->() SET w = {role: 'author'}");
    assert!(err.contains("22N77"), "expected 22N77, got: {err}");
}

#[test]
fn add_label_activating_constraint_violation_rejected() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT author_name FOR (a:Author) REQUIRE a.name IS NOT NULL");
    db.run("CREATE (:Person {surname: 'X'})");
    let err = db.run_err("MATCH (p:Person) SET p:Author");
    assert!(err.contains("22N77"), "expected 22N77, got: {err}");
}

#[test]
fn unrelated_writes_pay_no_constraint_overhead() {
    // Sanity: with no constraints registered, ordinary writes succeed.
    let db = TestDb::new();
    db.run("CREATE (:N {x: 1})");
    db.run("CREATE (:N {x: 1})"); // would conflict if there were a unique constraint
    db.assert_count("MATCH (n:N) RETURN n", 2);
}

#[test]
fn snapshot_round_trip_preserves_constraints() {
    use std::env;

    // Build a graph with one of every constraint kind.
    let donor = TestDb::new();
    donor.run("CREATE CONSTRAINT u FOR (n:Book) REQUIRE n.isbn IS UNIQUE");
    donor.run("CREATE CONSTRAINT e FOR (a:Author) REQUIRE a.name IS NOT NULL");
    donor.run("CREATE CONSTRAINT k FOR (a:Actor) REQUIRE (a.first, a.last) IS NODE KEY");
    donor.run("CREATE CONSTRAINT t FOR (m:Movie) REQUIRE m.title IS :: STRING");

    let pre_rows = donor.run("SHOW CONSTRAINTS");
    assert_eq!(pre_rows.len(), 4, "donor should have 4 constraints");

    let mut path = env::temp_dir();
    path.push(format!(
        "lora_constraint_snapshot_{}.bin",
        std::process::id()
    ));
    donor.service.save_snapshot_to(&path).unwrap();

    let reloaded = TestDb::new();
    reloaded.service.load_snapshot_from(&path).unwrap();
    let rows = reloaded.run("SHOW CONSTRAINTS");
    let names: std::collections::BTreeSet<String> = rows
        .iter()
        .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(String::from))
        .collect();
    assert!(names.contains("u"), "missing u; got {names:?}");
    assert!(names.contains("e"), "missing e; got {names:?}");
    assert!(names.contains("k"), "missing k; got {names:?}");
    assert!(names.contains("t"), "missing t; got {names:?}");

    let _ = std::fs::remove_file(&path);
}

// ---------- Parameterized constraint name ----------

#[test]
fn create_constraint_with_parameter_name() {
    let db = TestDb::new();
    let mut params = BTreeMap::new();
    params.insert(
        "name".into(),
        LoraValue::String("node_uniqueness_param".into()),
    );
    let rows = db.run_with_params(
        "CREATE CONSTRAINT $name FOR (book:Book) REQUIRE book.prop1 IS UNIQUE",
        params,
    );
    assert!(rows.is_empty(), "CREATE returns no rows");

    let listed = db.run("SHOW CONSTRAINTS");
    let entry = rows_for_constraint_named(&listed, "node_uniqueness_param")
        .expect("constraint listed under substituted name");
    assert_eq!(
        entry["type"],
        JsonValue::String("NODE_PROPERTY_UNIQUENESS".into())
    );
    assert_eq!(
        entry["ownedIndex"],
        JsonValue::String("node_uniqueness_param".into())
    );
}

#[test]
fn drop_constraint_with_parameter_name() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT c FOR (b:Book) REQUIRE b.isbn IS UNIQUE");

    let mut params = BTreeMap::new();
    params.insert("name".into(), LoraValue::String("c".into()));
    db.run_with_params("DROP CONSTRAINT $name", params);

    let listed = db.run("SHOW CONSTRAINTS");
    assert!(rows_for_constraint_named(&listed, "c").is_none());
}

// ---------- VECTOR property type constraint ----------

#[test]
fn create_vector_property_type_constraint_round_trip() {
    let db = TestDb::new();
    db.run(
        "CREATE CONSTRAINT node_vec FOR (n:Movie) \
         REQUIRE n.embedding IS :: VECTOR<INT32>(42)",
    );
    let listed = db.run("SHOW CONSTRAINTS");
    let entry = rows_for_constraint_named(&listed, "node_vec").unwrap();
    assert_eq!(
        entry["type"],
        JsonValue::String("NODE_PROPERTY_TYPE".into())
    );
    assert_eq!(
        entry["propertyType"],
        JsonValue::String("VECTOR<INT32>(42)".into())
    );
}

#[test]
fn create_relationship_vector_property_type_constraint() {
    let db = TestDb::new();
    db.run(
        "CREATE CONSTRAINT rel_vec FOR ()-[r:CONTAINS]-() \
         REQUIRE r.embedding IS :: VECTOR<FLOAT32>(1536)",
    );
    let listed = db.run("SHOW CONSTRAINTS");
    let entry = rows_for_constraint_named(&listed, "rel_vec").unwrap();
    assert_eq!(
        entry["type"],
        JsonValue::String("RELATIONSHIP_PROPERTY_TYPE".into())
    );
    assert_eq!(
        entry["propertyType"],
        JsonValue::String("VECTOR<FLOAT32>(1536)".into())
    );
}

#[test]
fn vector_constraint_dimension_zero_rejected_at_parse() {
    let db = TestDb::new();
    let err = db.run_err("CREATE CONSTRAINT bad FOR (n:Movie) REQUIRE n.v IS :: VECTOR<INT32>(0)");
    assert!(
        err.contains("1..=4096") || err.contains("dimension"),
        "expected dimension bound error, got: {err}"
    );
}

#[test]
fn vector_constraint_dimension_too_large_rejected_at_parse() {
    let db = TestDb::new();
    let err =
        db.run_err("CREATE CONSTRAINT bad FOR (n:Movie) REQUIRE n.v IS :: VECTOR<INT32>(4097)");
    assert!(
        err.contains("1..=4096") || err.contains("dimension"),
        "expected dimension bound error, got: {err}"
    );
}

#[test]
fn vector_constraint_different_dimension_conflicts() {
    let db = TestDb::new();
    db.run(
        "CREATE CONSTRAINT v1 FOR (n:Movie) \
         REQUIRE n.embedding IS :: VECTOR<INT32>(42)",
    );
    let err = db.run_err(
        "CREATE CONSTRAINT v2 FOR (n:Movie) \
         REQUIRE n.embedding IS :: VECTOR<INT32>(64)",
    );
    assert!(err.contains("22N66"), "expected 22N66, got: {err}");
}

#[test]
fn vector_constraint_different_coord_conflicts() {
    let db = TestDb::new();
    db.run(
        "CREATE CONSTRAINT v1 FOR (n:Movie) \
         REQUIRE n.embedding IS :: VECTOR<INT32>(42)",
    );
    let err = db.run_err(
        "CREATE CONSTRAINT v2 FOR (n:Movie) \
         REQUIRE n.embedding IS :: VECTOR<FLOAT32>(42)",
    );
    assert!(err.contains("22N66"), "expected 22N66, got: {err}");
}

// ---------- Single-property RELATIONSHIP KEY ----------

#[test]
fn create_single_property_relationship_key_round_trip() {
    let db = TestDb::new();
    db.run(
        "CREATE CONSTRAINT ownershipId FOR ()-[o:OWNS]-() \
         REQUIRE o.ownershipId IS RELATIONSHIP KEY",
    );
    let listed = db.run("SHOW CONSTRAINTS");
    let entry = rows_for_constraint_named(&listed, "ownershipId").unwrap();
    assert_eq!(entry["type"], JsonValue::String("RELATIONSHIP_KEY".into()));
    assert_eq!(
        entry["entityType"],
        JsonValue::String("RELATIONSHIP".into())
    );
    assert_eq!(
        entry["labelsOrTypes"],
        JsonValue::Array(vec![JsonValue::String("OWNS".into())])
    );
    assert_eq!(entry["ownedIndex"], JsonValue::String("ownershipId".into()));

    // The backing range index exists on the relationship type.
    let idx_listed = db.run("SHOW INDEXES");
    let idx_entry = idx_listed
        .iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some("ownershipId"))
        .expect("backing index listed");
    assert_eq!(idx_entry["type"], JsonValue::String("RANGE".into()));
    assert_eq!(
        idx_entry["entityType"],
        JsonValue::String("RELATIONSHIP".into())
    );
}

#[test]
fn single_property_relationship_key_enforces_existence_at_create() {
    let db = TestDb::new();
    db.run(
        "CREATE CONSTRAINT ownershipId FOR ()-[o:OWNS]-() \
         REQUIRE o.ownershipId IS RELATIONSHIP KEY",
    );
    db.run("CREATE (:Person {n: 'a'}), (:Thing {n: 'b'})");
    let err = db.run_err("MATCH (p:Person), (t:Thing) CREATE (p)-[:OWNS]->(t)");
    assert!(err.contains("22N77"), "expected 22N77, got: {err}");
}

#[test]
fn single_property_relationship_key_enforces_uniqueness() {
    let db = TestDb::new();
    db.run(
        "CREATE CONSTRAINT ownershipId FOR ()-[o:OWNS]-() \
         REQUIRE o.ownershipId IS RELATIONSHIP KEY",
    );
    db.run("CREATE (:Person {n: 'a'}), (:Thing {n: 'b'})");
    db.run("MATCH (p:Person), (t:Thing) CREATE (p)-[:OWNS {ownershipId: 1}]->(t)");
    let err = db.run_err("MATCH (p:Person), (t:Thing) CREATE (p)-[:OWNS {ownershipId: 1}]->(t)");
    assert!(err.contains("22N79"), "expected 22N79, got: {err}");
}
