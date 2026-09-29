//! `null` in a property map means "no property" (E19), and existence
//! constraints are checked when the statement finishes, not at `CREATE`
//! (E20).
//!
//! Before: `CREATE (n $props)` and `SET n += $map` stored a `null` value as
//! a property (`keys(n)` listed it and `IS NOT NULL` constraints saw a
//! key), `SET n.a = null` stored `null` as well, and
//! `CREATE (n:User) SET n.name = $name` failed an existence constraint on
//! `name` although the statement supplies it.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::{LoraErrorCode, LoraValue, TransactionMode};
use serde_json::json;
use test_helpers::TestDb;

fn props(pairs: &[(&str, LoraValue)]) -> BTreeMap<String, LoraValue> {
    let map = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect::<BTreeMap<_, _>>();
    BTreeMap::from([("p".to_string(), LoraValue::Map(map))])
}

fn keys(db: &TestDb, q: &str) -> JsonKeys {
    JsonKeys(db.run(q)[0]["k"].clone())
}

#[derive(Debug, PartialEq)]
struct JsonKeys(serde_json::Value);

#[test]
fn null_in_a_create_map_is_not_stored() {
    let db = TestDb::new();
    db.run_with_params(
        "CREATE (:N {id: 1} ) WITH 1 AS x MATCH (n:N {id: 1}) SET n += $p",
        props(&[("a", LoraValue::Int(1)), ("b", LoraValue::Null)]),
    );
    db.run_with_params(
        "CREATE (n:M $p)",
        props(&[("a", LoraValue::Int(1)), ("b", LoraValue::Null)]),
    );
    db.run("CREATE (:L {a: 1, b: null})");
    db.run("CREATE (:S)-[:R {a: 1, b: null}]->(:S)");
    for q in [
        "MATCH (n:M) RETURN keys(n) AS k",
        "MATCH (n:L) RETURN keys(n) AS k",
        "MATCH ()-[r:R]->() RETURN keys(r) AS k",
    ] {
        assert_eq!(keys(&db, q), JsonKeys(json!(["a"])), "{q}");
    }
    assert_eq!(
        keys(&db, "MATCH (n:N) RETURN keys(n) AS k ORDER BY k"),
        JsonKeys(json!(["a", "id"]))
    );
}

#[test]
fn null_in_set_removes_the_property() {
    let db = TestDb::new();
    db.run("CREATE (:N {a: 1, b: 2, c: 3})");
    db.run("MATCH (n:N) SET n += {a: null, d: 4}");
    assert_eq!(
        keys(&db, "MATCH (n:N) RETURN keys(n) AS k"),
        JsonKeys(json!(["b", "c", "d"]))
    );
    db.run("MATCH (n:N) SET n.b = null");
    assert_eq!(
        keys(&db, "MATCH (n:N) RETURN keys(n) AS k"),
        JsonKeys(json!(["c", "d"]))
    );
    db.run("MATCH (n:N) SET n = {c: null, e: 5}");
    assert_eq!(
        keys(&db, "MATCH (n:N) RETURN keys(n) AS k"),
        JsonKeys(json!(["e"]))
    );
    // Removing an absent key is a no-op.
    db.run("MATCH (n:N) SET n += {zzz: null}");
    assert_eq!(
        keys(&db, "MATCH (n:N) RETURN keys(n) AS k"),
        JsonKeys(json!(["e"]))
    );

    db.run("CREATE (:S)-[:R {a: 1, b: 2}]->(:S)");
    db.run("MATCH ()-[r:R]->() SET r += {a: null}");
    assert_eq!(
        keys(&db, "MATCH ()-[r:R]->() RETURN keys(r) AS k"),
        JsonKeys(json!(["b"]))
    );
}

#[test]
fn null_map_value_counts_as_missing_for_existence_constraints() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT un FOR (u:User) REQUIRE u.name IS NOT NULL");
    let err = db
        .exec_with_params(
            "CREATE (u:User $p)",
            props(&[("name", LoraValue::Null), ("x", LoraValue::Int(1))]),
        )
        .expect_err("null name is a missing name");
    assert_eq!(err.code(), LoraErrorCode::NotNullConstraint, "{err}");

    db.run("CREATE (:User {name: 'a'})");
    let err = db
        .exec("MATCH (u:User) SET u += {name: null}")
        .expect_err("removing a required property");
    assert_eq!(err.code(), LoraErrorCode::NotNullConstraint, "{err}");
    db.assert_count("MATCH (u:User) WHERE u.name = 'a' RETURN u", 1);
}

#[test]
fn existence_is_checked_when_the_statement_finishes() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT un FOR (u:User) REQUIRE u.name IS NOT NULL");
    db.run("CREATE CONSTRAINT rs FOR ()-[r:RATED]-() REQUIRE r.stars IS NOT NULL");

    db.run("CREATE (u:User) SET u.name = 'ada'");
    db.run_with_params(
        "CREATE (u:User) SET u += $p",
        props(&[("name", LoraValue::String("bob".into()))]),
    );
    db.run("MERGE (u:User {id: 7}) ON CREATE SET u.name = 'cy'");
    db.run("UNWIND ['d', 'e'] AS n CREATE (u:User) SET u.name = n");
    db.run("MATCH (a:User {name: 'ada'}), (b:User {name: 'bob'}) CREATE (a)-[r:RATED]->(b) SET r.stars = 5");
    db.run("MATCH (u:User {name: 'ada'}) CALL { WITH u CREATE (u)-[:OWNS]->(x:User) SET x.name = 'f' }");
    db.assert_count("MATCH (u:User) RETURN u", 6);

    // Still rejected when the statement ends without the property, and
    // nothing it wrote is kept.
    for q in [
        "CREATE (u:User) SET u.other = 1",
        "CREATE (u:User)",
        "UNWIND [1, 2] AS i CREATE (u:User {name: 'g'}) CREATE (:User)",
        "MERGE (u:User {id: 8}) ON CREATE SET u.other = 1",
        "CREATE (u:User) SET u.name = 'h' REMOVE u.name",
        "MATCH (a:User {name: 'ada'}) CREATE (a)-[r:RATED]->(a) SET r.note = 'x'",
    ] {
        let err = db.exec(q).expect_err(q);
        assert_eq!(err.code(), LoraErrorCode::NotNullConstraint, "{q}: {err}");
    }
    db.assert_count("MATCH (u:User) RETURN u", 6);
    db.assert_count("MATCH ()-[r:RATED]->() RETURN r", 1);
}

#[test]
fn deferred_existence_in_a_transaction_and_a_stream() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT un FOR (u:User) REQUIRE u.name IS NOT NULL");
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadWrite)
        .unwrap();
    tx.execute_rows("CREATE (u:User) SET u.name = 'tx'")
        .unwrap();
    let err = tx
        .execute_rows("CREATE (u:User) SET u.other = 1")
        .expect_err("missing name");
    assert!(err.to_string().contains("22N77"), "{err}");
    tx.commit().unwrap();
    db.assert_count("MATCH (u:User) RETURN u", 1);

    // The streaming write cursor checks each row as it is written.
    let rows: Vec<_> = db
        .service
        .stream("MERGE (u:User {id: 1}) ON CREATE SET u.name = 's' RETURN u.name AS n")
        .unwrap()
        .collect();
    assert_eq!(rows.len(), 1);
    db.assert_count("MATCH (u:User) RETURN u", 2);
}
