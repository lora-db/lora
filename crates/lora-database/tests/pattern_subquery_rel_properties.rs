//! A pattern comprehension or EXISTS subquery honours the property map on a
//! relationship: `[(c)<-[:IN {w: 1}]-(m) | m]` only follows relationships with
//! `w = 1`. The subquery matcher used to ignore the map and follow every
//! relationship of the type.

mod test_helpers;
use test_helpers::TestDb;

fn hub() -> TestDb {
    let db = TestDb::new();
    db.run(
        "CREATE (c:C {key: 'c'}) WITH c UNWIND range(1, 4) AS i \
         CREATE (:M {i: i})-[:IN {w: i % 2}]->(c)",
    );
    db
}

fn sorted(v: &serde_json::Value) -> Vec<i64> {
    let mut out: Vec<i64> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_i64().unwrap())
        .collect();
    out.sort();
    out
}

#[test]
fn comprehension_filters_on_relationship_properties() {
    let db = hub();
    let rows = db.run(
        "MATCH (c:C) WITH c, 0 AS k \
         RETURN [(c)<-[:IN {w: 1}]-(m:M) | m.i] AS odd, \
                [(c)<-[r:IN {w: k}]-(m:M) | m.i] AS even, \
                [(c)<-[:IN {w: 7}]-(m:M) | m.i] AS none, \
                [(c)<-[:IN]-(m:M) | m.i] AS all",
    );
    assert_eq!(sorted(&rows[0]["odd"]), [1, 3]);
    assert_eq!(sorted(&rows[0]["even"]), [2, 4]);
    assert_eq!(sorted(&rows[0]["none"]), Vec::<i64>::new());
    assert_eq!(sorted(&rows[0]["all"]), [1, 2, 3, 4]);
}

#[test]
fn exists_filters_on_relationship_properties() {
    let db = hub();
    let rows = db.run(
        "MATCH (m:M) \
         RETURN m.i AS i, EXISTS { (m)-[:IN {w: 1}]->(:C) } AS odd \
         ORDER BY i",
    );
    let odd: Vec<bool> = rows.iter().map(|r| r["odd"].as_bool().unwrap()).collect();
    assert_eq!(odd, [true, false, true, false]);
    let rows = db.run("MATCH (c:C) RETURN EXISTS { (c)<-[:IN {w: 9}]-(:M) } AS any");
    assert_eq!(rows[0]["any"], false);
}
