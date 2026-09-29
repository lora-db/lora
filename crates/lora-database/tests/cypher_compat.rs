//! Standard Cypher spellings that queries written from memory use:
//! function names, `WHERE n:Label`, and `COUNT { pattern }`.

mod test_helpers;
use serde_json::json;
use test_helpers::TestDb;

#[test]
fn standard_function_names_resolve() {
    let db = TestDb::new();
    let rows = db.run(
        "RETURN split('a,b', ',') AS s, trim('  a ') AS t, ltrim(' a') AS l, rtrim('a ') AS r, \
         replace('abc', 'b', 'x') AS rep, abs(-2) AS a, sqrt(9) AS q, tail([1, 2, 3]) AS tl, \
         point.distance(point({x: 0, y: 0}), point({x: 3, y: 4})) AS d, toUpper('x') AS u",
    );
    assert_eq!(
        rows[0],
        json!({"s": ["a", "b"], "t": "a", "l": "a", "r": "a", "rep": "axc", "a": 2, "q": 3.0,
               "tl": [2, 3], "d": 5.0, "u": "X"})
    );
}

#[test]
fn path_and_relationship_functions_resolve() {
    let db = TestDb::new();
    db.run("CREATE (:A {k: 1})-[:R]->(:B {k: 2})");
    let rows = db.run(
        "MATCH p = (a)-[r:R]->(b) RETURN size(nodes(p)) AS n, size(relationships(p)) AS e, \
         startNode(r).k AS s, endNode(r).k AS t",
    );
    assert_eq!(rows[0], json!({"n": 2, "e": 1, "s": 1, "t": 2}));
}

#[test]
fn where_label_predicate() {
    let db = TestDb::new();
    db.run("CREATE (:Festival:Big {k: 1}), (:Festival {k: 2}), (:Venue {k: 3})");
    let ks = |q: &str| db.sorted_ints(q, "k");
    assert_eq!(ks("MATCH (n) WHERE n:Festival RETURN n.k AS k"), vec![1, 2]);
    assert_eq!(
        ks("MATCH (n) WHERE n:Festival:Big RETURN n.k AS k"),
        vec![1]
    );
    assert_eq!(
        ks("MATCH (n) WHERE n:Big|Venue RETURN n.k AS k"),
        vec![1, 3]
    );
    assert_eq!(
        ks("MATCH (n) WHERE NOT n:Festival RETURN n.k AS k"),
        vec![3]
    );
    let rows = db.run("MATCH (n {k: 2}) RETURN n:Festival AS f, n:Venue AS v");
    assert_eq!(rows[0], json!({"f": true, "v": false}));
    // `::` type casts still parse after a variable.
    assert_eq!(
        db.run("MATCH (n {k: 1}) RETURN n.k::STRING AS s")[0],
        json!({"s": "1"})
    );
}

#[test]
fn count_subquery() {
    let db = TestDb::new();
    db.run("CREATE (f:Festival {k: 1}), (g:Festival {k: 2}), (:User)-[:FOLLOWS]->(f), (:User)-[:FOLLOWS]->(f)");
    let rows =
        db.run("MATCH (f:Festival) RETURN f.k AS k, COUNT { (f)<-[:FOLLOWS]-() } AS n ORDER BY k");
    assert_eq!(rows, vec![json!({"k": 1, "n": 2}), json!({"k": 2, "n": 0})]);
    let rows = db.run(
        "MATCH (f:Festival) WHERE COUNT { MATCH (f)<-[:FOLLOWS]-(u) WHERE u:User } > 1 RETURN f.k AS k",
    );
    assert_eq!(rows, vec![json!({"k": 1})]);
    // count(...) the aggregate is unaffected.
    assert_eq!(
        db.run("MATCH (f:Festival) RETURN count(*) AS c")[0],
        json!({"c": 2})
    );
}
