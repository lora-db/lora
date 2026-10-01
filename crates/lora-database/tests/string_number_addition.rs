//! `+` follows Cypher across types: a string and a number concatenate, a
//! list and an element append or prepend, null stays null, and any other
//! mix is an error instead of a silent null.

mod test_helpers;
use test_helpers::TestDb;

#[test]
fn a_string_and_a_number_concatenate() {
    let db = TestDb::new();
    assert_eq!(db.scalar("RETURN 'p' + 1 AS v"), "p1");
    assert_eq!(db.scalar("RETURN 1 + 'p' AS v"), "1p");
    assert_eq!(db.scalar("RETURN 'v' + 2.5 AS v"), "v2.5");
    assert_eq!(
        db.column("UNWIND range(1, 3) AS i RETURN 'p' + i AS k", "k"),
        vec!["p1", "p2", "p3"]
    );
}

#[test]
fn a_list_and_an_element_append_or_prepend() {
    let db = TestDb::new();
    assert_eq!(
        db.scalar("RETURN [1, 2] + 3 AS v"),
        serde_json::json!([1, 2, 3])
    );
    assert_eq!(
        db.scalar("RETURN 0 + [1, 2] AS v"),
        serde_json::json!([0, 1, 2])
    );
    assert_eq!(
        db.scalar("RETURN [1] + [2] AS v"),
        serde_json::json!([1, 2])
    );
}

#[test]
fn null_stays_null_and_other_mixes_are_errors() {
    let db = TestDb::new();
    assert_eq!(db.scalar("RETURN 'p' + null AS v"), serde_json::Value::Null);
    assert_eq!(db.scalar("RETURN null + [1] AS v"), serde_json::Value::Null);
    let err = db.run_err("RETURN 'p' + true AS v");
    assert!(err.contains("Cannot add"), "{err}");
    let err = db.run_err("RETURN {a: 1} + 1 AS v");
    assert!(err.contains("Cannot add"), "{err}");
}
