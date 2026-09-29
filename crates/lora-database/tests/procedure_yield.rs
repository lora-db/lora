//! `CALL db.index.* (...) YIELD ...` composes with the rest of a query,
//! and standalone calls return full nodes (labels + properties).

mod test_helpers;
use serde_json::json;
use test_helpers::TestDb;

fn festivals() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX ft IF NOT EXISTS FOR (n:Festival) ON EACH [n.name]");
    db.run("CREATE (:Festival {key: 'a', name: 'Tomorrowland Winter'})-[:IN_GENRE]->(:Genre {key: 'edm'})");
    db.run(
        "CREATE (:Festival {key: 'b', name: 'Tomorrowland'})-[:IN_GENRE]->(:Genre {key: 'house'})",
    );
    db.run("CREATE (:Festival {key: 'c', name: 'Sonar'})");
    db
}

#[test]
fn standalone_call_returns_hydrated_nodes() {
    let db = festivals();
    let rows = db.run("CALL db.index.fulltext.queryNodes('ft', 'Winter')");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["node"]["labels"], json!(["Festival"]));
    assert_eq!(rows[0]["node"]["properties"]["name"], "Tomorrowland Winter");
}

#[test]
fn yield_then_return() {
    let db = festivals();
    let rows = db.run(
        "CALL db.index.fulltext.queryNodes('ft', 'Winter') YIELD node, score \
         RETURN node.name AS n, score",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["n"], "Tomorrowland Winter");
    assert!(rows[0]["score"].as_f64().unwrap() > 0.0);
}

#[test]
fn yield_composes_with_match_order_and_limit() {
    let db = festivals();
    let mut params = std::collections::BTreeMap::new();
    params.insert(
        "q".to_string(),
        lora_database::LoraValue::String("Tomorrowland".into()),
    );
    let rows = db.run_with_params(
        "CALL db.index.fulltext.queryNodes('ft', $q) YIELD node, score \
         MATCH (node)-[:IN_GENRE]->(g) \
         RETURN node.key AS f, g.key AS g ORDER BY score DESC, f LIMIT 10",
        params,
    );
    let got: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            (
                r["f"].as_str().unwrap().into(),
                r["g"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(got.len(), 2);
    assert!(got.contains(&("a".into(), "edm".into())));
    assert!(got.contains(&("b".into(), "house".into())));
}

#[test]
fn yield_alias_and_where() {
    let db = festivals();
    let rows = db.run(
        "CALL db.index.fulltext.queryNodes('ft', 'Tomorrowland') YIELD node AS f, score AS s \
         WHERE f.key = 'a' RETURN f.key AS k",
    );
    assert_eq!(rows, vec![json!({"k": "a"})]);
}

#[test]
fn call_after_match_runs_per_row() {
    let db = festivals();
    let rows = db.run(
        "UNWIND ['Winter', 'Sonar'] AS q \
         CALL db.index.fulltext.queryNodes('ft', q) YIELD node \
         RETURN q, node.key AS k ORDER BY q",
    );
    assert_eq!(
        rows,
        vec![
            json!({"q": "Sonar", "k": "c"}),
            json!({"q": "Winter", "k": "a"})
        ]
    );
}

#[test]
fn unknown_yield_field_and_procedure_are_errors() {
    let db = festivals();
    let err = db.run_err("CALL db.index.fulltext.queryNodes('ft', 'x') YIELD nope RETURN nope");
    assert!(err.contains("nope"), "{err}");
    let err = db.run_err("CALL db.nothing.here() YIELD x RETURN x");
    assert!(err.contains("unknown procedure"), "{err}");
    let err =
        db.run_err("CALL db.index.fulltext.queryNodes('missing', 'x') YIELD node RETURN node");
    assert!(err.contains("missing"), "{err}");
}

#[test]
fn vector_yield_composes() {
    let db = TestDb::new();
    db.run(
        "CREATE VECTOR INDEX emb FOR (m:Movie) ON (m.embedding) \
         OPTIONS {indexConfig: {`vector.dimensions`: 3, `vector.similarity_function`: 'cosine'}}",
    );
    db.run("CREATE (:Movie {title: 'A', embedding: [1.0, 0.0, 0.0]::VECTOR<FLOAT32>(3)})");
    db.run("CREATE (:Movie {title: 'C', embedding: [0.0, 1.0, 0.0]::VECTOR<FLOAT32>(3)})");
    let rows = db.run(
        "CALL db.index.vector.queryNodes('emb', 1, [1.0, 0.0, 0.0]) YIELD node, score \
         RETURN node.title AS t",
    );
    assert_eq!(rows, vec![json!({"t": "A"})]);
}

#[test]
fn fulltext_folds_diacritics_and_supports_prefix_queries() {
    let db = TestDb::new();
    db.run("CREATE FULLTEXT INDEX ft FOR (n:Festival) ON EACH [n.name]");
    db.run("CREATE (:Festival {key: 's', name: 'Sónar'}), (:Festival {key: 'o', name: 'Øyafestivalen'})");
    let hit = |q: &str| {
        db.run(&format!(
            "CALL db.index.fulltext.queryNodes('ft', '{q}') YIELD node RETURN node.key AS k"
        ))
        .iter()
        .map(|r| r["k"].as_str().unwrap().to_string())
        .collect::<Vec<_>>()
    };
    assert_eq!(hit("Sonar"), vec!["s"]);
    assert_eq!(hit("sónar"), vec!["s"]);
    assert_eq!(hit("Oya*"), vec!["o"]);
    assert_eq!(hit("øyafestivalen"), vec!["o"]);
    assert!(hit("Oya").is_empty(), "without * a term must match whole");
}
