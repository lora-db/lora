//! A pattern comprehension or EXISTS subquery honours variable-length ranges:
//! `[(a)-[:NEXT*2..2]->(x) | x]` follows paths of exactly two hops. The
//! subquery matcher used to ignore the range and follow a single hop.

mod test_helpers;
use test_helpers::TestDb;

/// a -> b -> c -> d, with `ok: false` on the last relationship.
fn chain() -> TestDb {
    let db = TestDb::new();
    db.run(
        "CREATE (a:L {n: 'a'})-[:NEXT {ok: true}]->(b:L {n: 'b'})\
                -[:NEXT {ok: true}]->(c:L {n: 'c'})-[:NEXT {ok: false}]->(d:L {n: 'd'})",
    );
    db
}

fn sorted(v: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.to_string().trim_matches('"').to_string())
        .collect();
    out.sort();
    out
}

#[test]
fn comprehension_follows_the_hop_range() {
    let db = chain();
    let rows = db.run(
        "MATCH (a:L {n: 'a'}), (b:L {n: 'b'}), (d:L {n: 'd'}) \
         RETURN [(a)-[:NEXT*2..2]->(x) | x.n] AS two, \
                [(a)-[:NEXT*1..3]->(x) | x.n] AS upto3, \
                [(a)-[:NEXT*0..1]->(x) | x.n] AS zero, \
                [(a)-[:NEXT*]->(x:L {n: 'd'}) | x.n] AS labelled, \
                [(a)-[rs:NEXT*3..3]->(x) | size(rs)] AS rels, \
                [(a)-[:NEXT*1..3 {ok: true}]->(x) | x.n] AS ok, \
                [(b)-[:NEXT*2..2]-(x) | x.n] AS undirected, \
                size([(a)-[:NEXT*]->(d) | 1]) AS bound, \
                size([(d)-[:NEXT*1..2]->(x) | 1]) AS nowhere",
    );
    let r = &rows[0];
    assert_eq!(sorted(&r["two"]), ["c"]);
    assert_eq!(sorted(&r["upto3"]), ["b", "c", "d"]);
    assert_eq!(sorted(&r["zero"]), ["a", "b"]);
    assert_eq!(sorted(&r["labelled"]), ["d"]);
    assert_eq!(r["rels"], serde_json::json!([3]));
    assert_eq!(sorted(&r["ok"]), ["b", "c"]);
    assert_eq!(sorted(&r["undirected"]), ["d"]);
    assert_eq!(r["bound"], 1);
    assert_eq!(r["nowhere"], 0);
}

#[test]
fn exists_follows_the_hop_range() {
    let db = chain();
    let rows = db.run(
        "MATCH (a:L {n: 'a'}) \
         RETURN EXISTS { (a)-[:NEXT*3..3]->(:L {n: 'd'}) } AS three, \
                EXISTS { (a)-[:NEXT*2..2]->(:L {n: 'd'}) } AS two, \
                EXISTS { (a)-[:NEXT*1..3 {ok: true}]->(:L {n: 'd'}) } AS okonly",
    );
    assert_eq!(rows[0]["three"], true);
    assert_eq!(rows[0]["two"], false);
    assert_eq!(rows[0]["okonly"], false);
}

#[test]
fn a_bound_relationship_list_restricts_the_path() {
    let db = chain();
    let rows = db.run(
        "MATCH (a:L {n: 'a'})-[rs:NEXT*2..2]->(c:L) \
         RETURN size([(a)-[rs*]->(x) | x.n]) AS same, [(a)-[rs*]->(x) | x.n] AS ends",
    );
    assert_eq!(rows[0]["same"], 1);
    assert_eq!(rows[0]["ends"], serde_json::json!(["c"]));
}
