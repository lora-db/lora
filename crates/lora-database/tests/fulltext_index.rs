//! CREATE FULLTEXT INDEX + db.index.fulltext.queryNodes / queryRelationships.

mod test_helpers;
use test_helpers::TestDb;

use serde_json::Value as JsonValue;

fn index_named<'a>(rows: &'a [JsonValue], name: &str) -> Option<&'a JsonValue> {
    rows.iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some(name))
}

fn node_ids(rows: &[JsonValue]) -> Vec<i64> {
    rows.iter()
        .filter_map(|r| {
            r.get("node")
                .and_then(|n| n.get("id"))
                .and_then(|i| i.as_i64())
        })
        .collect()
}

// ---------- DDL ----------

#[test]
fn create_fulltext_index_single_label_single_prop() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    let listed = db.run("SHOW INDEXES");
    let entry = index_named(&listed, "titles").expect("listed");
    assert_eq!(entry["type"], JsonValue::String("FULLTEXT".into()));
    assert_eq!(entry["entityType"], JsonValue::String("NODE".into()));
    assert_eq!(
        entry["labelsOrTypes"],
        JsonValue::Array(vec![JsonValue::String("Article".into())])
    );
    assert_eq!(
        entry["properties"],
        JsonValue::Array(vec![JsonValue::String("title".into())])
    );
}

#[test]
fn create_fulltext_index_multi_label_multi_prop() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX search FOR (n:Article|Note) ON EACH [n.title, n.body]");
    let listed = db.run("SHOW INDEXES");
    let entry = index_named(&listed, "search").unwrap();
    assert_eq!(
        entry["labelsOrTypes"],
        JsonValue::Array(vec![
            JsonValue::String("Article".into()),
            JsonValue::String("Note".into()),
        ])
    );
    assert_eq!(
        entry["properties"],
        JsonValue::Array(vec![
            JsonValue::String("title".into()),
            JsonValue::String("body".into()),
        ])
    );
}

#[test]
fn create_fulltext_index_relationship_scope() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX rel_text FOR ()-[r:WROTE]-() ON EACH [r.summary]");
    let listed = db.run("SHOW INDEXES");
    let entry = index_named(&listed, "rel_text").unwrap();
    assert_eq!(
        entry["entityType"],
        JsonValue::String("RELATIONSHIP".into())
    );
}

#[test]
fn show_fulltext_indexes_filter_returns_entries() {
    let db = TestDb::new();
    db.run("CREATE RANGE INDEX rng FOR (n:N) ON (n.x)");
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    let rows = db.run("SHOW FULLTEXT INDEXES");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], JsonValue::String("titles".into()));
}

#[test]
fn fulltext_index_with_analyzer_option() {
    let db = TestDb::new();
    db.run(
        "CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title] \
         OPTIONS {`fulltext.analyzer`: 'standard'}",
    );
    let listed = db.run("SHOW INDEXES");
    assert!(index_named(&listed, "titles").is_some());
}

#[test]
fn fulltext_index_rejects_unknown_analyzer() {
    let db = TestDb::new();
    let err = db.run_err(
        "CREATE FULLTEXT INDEX bad FOR (n:Article) ON EACH [n.title] \
         OPTIONS {`fulltext.analyzer`: 'english'}",
    );
    assert!(err.contains("english"), "got: {err}");
}

#[test]
fn fulltext_requires_each_property_form() {
    let db = TestDb::new();
    let err = db.run_err("CREATE FULLTEXT INDEX bad FOR (n:Article) ON (n.title)");
    assert!(err.contains("ON EACH"), "got: {err}");
}

#[test]
fn non_fulltext_rejects_each_property_form() {
    let db = TestDb::new();
    let err = db.run_err("CREATE INDEX bad FOR (n:Article) ON EACH [n.title]");
    assert!(
        err.contains("only FULLTEXT") || err.contains("EACH"),
        "got: {err}"
    );
}

#[test]
fn non_fulltext_rejects_multi_label_pattern() {
    let db = TestDb::new();
    let err = db.run_err("CREATE INDEX bad FOR (n:A|B) ON (n.x)");
    assert!(err.contains("multi-label"), "got: {err}");
}

#[test]
fn drop_fulltext_index() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    db.run("DROP INDEX titles");
    let listed = db.run("SHOW INDEXES");
    assert!(index_named(&listed, "titles").is_none());
}

// ---------- Procedure: db.index.fulltext.queryNodes ----------

fn seed_articles(db: &TestDb) {
    db.run("CREATE (:Article {title: 'Graph databases are powerful'})");
    db.run("CREATE (:Article {title: 'Cypher query language'})");
    db.run("CREATE (:Article {title: 'Vector search with graph indexes'})");
    db.run("CREATE (:Article {title: 'Powerful tools, simple queries'})");
}

#[test]
fn fulltext_query_returns_matching_nodes() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    seed_articles(&db);
    let rows = db.run("CALL db.index.fulltext.queryNodes('titles', 'graph') YIELD node, score");
    assert_eq!(
        rows.len(),
        2,
        "expected 2 articles mentioning 'graph': {rows:?}"
    );
}

#[test]
fn fulltext_query_is_case_insensitive() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    seed_articles(&db);
    let rows = db.run("CALL db.index.fulltext.queryNodes('titles', 'POWERFUL') YIELD node, score");
    assert_eq!(rows.len(), 2);
}

#[test]
fn fulltext_query_intersects_terms() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    seed_articles(&db);
    // Only 'Graph databases are powerful' contains both 'graph' and 'powerful'.
    let rows =
        db.run("CALL db.index.fulltext.queryNodes('titles', 'graph powerful') YIELD node, score");
    assert_eq!(rows.len(), 1, "{rows:?}");
}

#[test]
fn fulltext_query_multi_property_index() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX search FOR (n:Article) ON EACH [n.title, n.body]");
    db.run("CREATE (:Article {title: 'Hello', body: 'world of graph'})");
    db.run("CREATE (:Article {title: 'Graph theory', body: 'notes'})");
    let rows = db.run("CALL db.index.fulltext.queryNodes('search', 'graph') YIELD node, score");
    assert_eq!(rows.len(), 2);
}

#[test]
fn fulltext_query_multi_label_index() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX search FOR (n:Article|Note) ON EACH [n.title]");
    db.run("CREATE (:Article {title: 'Graph A'})");
    db.run("CREATE (:Note {title: 'Graph note'})");
    db.run("CREATE (:Other {title: 'Graph elsewhere'})"); // unrelated label
    let rows = db.run("CALL db.index.fulltext.queryNodes('search', 'graph') YIELD node, score");
    assert_eq!(rows.len(), 2, "{rows:?}");
}

#[test]
fn fulltext_query_reflects_set_property_updates() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    db.run("CREATE (:Article {title: 'first'})");
    let before = db.run("CALL db.index.fulltext.queryNodes('titles', 'second') YIELD node, score");
    assert!(before.is_empty());
    db.run("MATCH (n:Article) SET n.title = 'second'");
    let after = db.run("CALL db.index.fulltext.queryNodes('titles', 'second') YIELD node, score");
    assert_eq!(after.len(), 1, "{after:?}");
    let stale = db.run("CALL db.index.fulltext.queryNodes('titles', 'first') YIELD node, score");
    assert!(
        stale.is_empty(),
        "old term should be gone after SET, got {stale:?}"
    );
}

#[test]
fn fulltext_query_reflects_remove_property() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    db.run("CREATE (:Article {title: 'temp value'})");
    db.run("MATCH (n:Article) REMOVE n.title");
    let rows = db.run("CALL db.index.fulltext.queryNodes('titles', 'temp') YIELD node, score");
    assert!(rows.is_empty(), "got {rows:?}");
}

#[test]
fn fulltext_query_returns_empty_for_unknown_term() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    seed_articles(&db);
    let rows = db.run("CALL db.index.fulltext.queryNodes('titles', 'zebra') YIELD node, score");
    assert!(rows.is_empty());
}

#[test]
fn fulltext_query_rejects_unknown_index() {
    let db = TestDb::new();
    let err = db.run_err("CALL db.index.fulltext.queryNodes('nope', 'x') YIELD node, score");
    assert!(err.contains("no fulltext index"), "got: {err}");
}

#[test]
fn fulltext_query_rejects_wrong_index_kind() {
    let db = TestDb::new();
    db.run("CREATE RANGE INDEX rng FOR (n:N) ON (n.x)");
    let err = db.run_err("CALL db.index.fulltext.queryNodes('rng', 'x') YIELD node, score");
    assert!(err.contains("not a FULLTEXT"), "got: {err}");
}

#[test]
fn fulltext_query_relationships() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX wrote_text FOR ()-[r:WROTE]-() ON EACH [r.summary]");
    db.run(
        "CREATE (a:Author), (b:Book), \
         (a)-[:WROTE {summary: 'A thrilling tale of graph theory'}]->(b)",
    );
    let rows = db.run(
        "CALL db.index.fulltext.queryRelationships('wrote_text', 'graph') \
         YIELD relationship, score",
    );
    assert_eq!(rows.len(), 1);
}

#[test]
fn fulltext_score_ranks_by_term_frequency() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    db.run("CREATE (:Article {title: 'one one one'})");
    db.run("CREATE (:Article {title: 'one'})");
    let rows = db.run("CALL db.index.fulltext.queryNodes('titles', 'one') YIELD node, score");
    assert_eq!(rows.len(), 2);
    let s0 = rows[0]["score"].as_f64().unwrap();
    let s1 = rows[1]["score"].as_f64().unwrap();
    assert!(s0 > s1, "high-TF article should rank first: {s0} vs {s1}");
}

#[test]
fn fulltext_punctuation_is_split() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    db.run("CREATE (:Article {title: 'hello, world!'})");
    let rows = db.run("CALL db.index.fulltext.queryNodes('titles', 'world') YIELD node, score");
    assert_eq!(rows.len(), 1, "punctuation should not block the match");
}

#[test]
fn fulltext_create_then_seed_then_query() {
    // Seed FIRST, then create the index — backfill path.
    let db = TestDb::new();
    db.run("CREATE (:Article {title: 'preexisting graph article'})");
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    let rows = db.run("CALL db.index.fulltext.queryNodes('titles', 'graph') YIELD node, score");
    assert_eq!(
        rows.len(),
        1,
        "backfill should have indexed pre-existing entities: {rows:?}"
    );
}

#[test]
fn fulltext_node_ids_returned() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX titles FOR (n:Article) ON EACH [n.title]");
    db.run("CREATE (:Article {title: 'graph'})");
    let rows = db.run("CALL db.index.fulltext.queryNodes('titles', 'graph') YIELD node, score");
    let ids = node_ids(&rows);
    assert_eq!(ids.len(), 1);
}
