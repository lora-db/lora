use super::*;
use crate::errors::SemanticError;
use crate::resolved::{LiteralValue, ResolvedClause, ResolvedExpr};
use lora_parser::parse_query;
use lora_store::{GraphStorageMut, InMemoryGraph, Properties};

#[test]
fn create_allows_new_relationship_type_when_graph_is_not_empty() {
    let mut graph = InMemoryGraph::new();
    let alice = graph.create_node(vec!["User".into()], Properties::new());
    let bob = graph.create_node(vec!["User".into()], Properties::new());
    let _carol = graph.create_node(vec!["User".into()], Properties::new());

    graph
        .create_relationship(alice.id, bob.id, "FOLLOWS", Properties::new())
        .unwrap();

    let doc = parse_query(
        "MATCH (a:User {id: 2}), (b:User {id: 3}) CREATE (a)-[:KNOWS]->(b) RETURN a, b",
    )
    .unwrap();

    let mut analyzer = Analyzer::new(&graph);
    assert!(analyzer.analyze(&doc).is_ok());

    let match_doc = parse_query("MATCH (a)-[:KNOWS]->(b) RETURN a, b").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&match_doc),
        Err(SemanticError::UnknownRelationshipType(rel_type)) if rel_type == "KNOWS"
    ));
}

// --- Vector function analyzer tests ----------------------------------

#[test]
fn vector_rewrites_bare_coordinate_type_to_string_literal() {
    // `vector([1,2,3], 3, INTEGER)` should not try to resolve INTEGER
    // as a variable — the third argument is an enum-like type literal.
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN vector([1, 2, 3], 3, INTEGER) AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer
        .analyze(&doc)
        .expect("INTEGER should be rewritten as a string literal, not a variable");
    // Walk down into the function call's third arg and confirm it came
    // through as a String literal.
    let Some(ResolvedClause::Return(ret)) = resolved.clauses.last() else {
        panic!("expected RETURN clause");
    };
    let ResolvedExpr::Function { args, .. } = &ret.items[0].expr else {
        panic!("expected function call");
    };
    assert!(matches!(
        args.get(2),
        Some(ResolvedExpr::Literal(LiteralValue::String(s))) if s == "INTEGER"
    ));
}

#[test]
fn vector_distance_rewrites_bare_metric_identifier() {
    let graph = InMemoryGraph::new();
    let doc = parse_query(
        "RETURN vector_distance(vector([1,2], 2, INT), vector([3,4], 2, INT), EUCLIDEAN) AS d",
    )
    .unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer
        .analyze(&doc)
        .expect("EUCLIDEAN should be rewritten as a string literal");
    let Some(ResolvedClause::Return(ret)) = resolved.clauses.last() else {
        panic!("expected RETURN clause");
    };
    let ResolvedExpr::Function { args, .. } = &ret.items[0].expr else {
        panic!("expected function call");
    };
    assert!(matches!(
        args.get(2),
        Some(ResolvedExpr::Literal(LiteralValue::String(s))) if s == "EUCLIDEAN"
    ));
}

#[test]
fn vector_norm_rewrites_bare_metric_identifier() {
    let graph = InMemoryGraph::new();
    let doc =
        parse_query("RETURN vector_norm(vector([1,2,3], 3, FLOAT32), MANHATTAN) AS n").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(analyzer.analyze(&doc).is_ok());
}

#[test]
fn bare_identifier_outside_enum_slot_still_resolves_as_variable() {
    // Outside the enum slot, INTEGER should still behave like a
    // variable reference — this guards against the rewrite leaking.
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN INTEGER AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&doc),
        Err(SemanticError::UnknownVariable(name)) if name == "INTEGER"
    ));
}

#[test]
fn vector_function_arity_is_validated() {
    let graph = InMemoryGraph::new();
    // vector() requires exactly 3 arguments.
    let doc = parse_query("RETURN vector([1, 2, 3], 3) AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&doc),
        Err(SemanticError::WrongArity(name, _, 2)) if name == "vector"
    ));
}

#[test]
fn unknown_vector_function_is_rejected() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN vector.bogus([1,2,3], 3, INTEGER) AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&doc),
        Err(SemanticError::UnknownFunction(name, _, _)) if name == "vector.bogus"
    ));
}

// --- Enum-literal rewrite scope --------------------------------------

/// Walk a ResolvedClause list, return the last RETURN's first item.
fn return_expr(clauses: &[ResolvedClause]) -> &ResolvedExpr {
    let Some(ResolvedClause::Return(ret)) = clauses.last() else {
        panic!("expected RETURN clause");
    };
    &ret.items[0].expr
}

#[test]
fn vector_does_not_rewrite_first_or_second_argument() {
    // INTEGER in slot 0 or 1 should still attempt to resolve as a
    // variable — only slot 2 is the enum-type slot.
    let graph = InMemoryGraph::new();

    let bad_first = parse_query("RETURN vector(INTEGER, 3, INTEGER) AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&bad_first),
        Err(SemanticError::UnknownVariable(name)) if name == "INTEGER"
    ));

    let bad_second = parse_query("RETURN vector([1, 2, 3], INTEGER, INTEGER) AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&bad_second),
        Err(SemanticError::UnknownVariable(name)) if name == "INTEGER"
    ));
}

#[test]
fn vector_distance_does_not_rewrite_first_or_second_argument() {
    let graph = InMemoryGraph::new();
    let doc =
        parse_query("RETURN vector_distance(EUCLIDEAN, vector([1,2], 2, INT), EUCLIDEAN) AS d")
            .unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&doc),
        Err(SemanticError::UnknownVariable(name)) if name == "EUCLIDEAN"
    ));
}

#[test]
fn vector_norm_does_not_rewrite_first_argument() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN vector_norm(MANHATTAN, EUCLIDEAN) AS n").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&doc),
        Err(SemanticError::UnknownVariable(name)) if name == "MANHATTAN"
    ));
}

#[test]
fn parameter_in_enum_slot_is_preserved_as_parameter() {
    // A $param in the enum slot must NOT be rewritten — callers need
    // to pass coordinate/metric names dynamically.
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN vector([1, 2, 3], 3, $type) AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer.analyze(&doc).expect("parameter should be kept");
    let ResolvedExpr::Function { args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };
    assert!(matches!(args.get(2), Some(ResolvedExpr::Parameter(p)) if p == "type"));
}

#[test]
fn parameter_in_vector_norm_metric_slot_is_preserved() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN vector_norm(vector([1,2,3], 3, FLOAT32), $metric) AS n").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer.analyze(&doc).expect("parameter should be kept");
    let ResolvedExpr::Function { args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };
    assert!(matches!(args.get(1), Some(ResolvedExpr::Parameter(p)) if p == "metric"));
}

#[test]
fn variable_named_like_metric_outside_enum_slot_is_not_rewritten() {
    // UNWIND exposes a variable literally called COSINE — which is a
    // metric name. Using it outside the enum slot must bind normally.
    let graph = InMemoryGraph::new();
    let doc = parse_query("UNWIND [1.0, 2.0, 3.0] AS COSINE RETURN COSINE AS val").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(analyzer.analyze(&doc).is_ok());
}

#[test]
fn string_literal_in_enum_slot_remains_string_literal() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN vector([1, 2, 3], 3, 'INTEGER32') AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer
        .analyze(&doc)
        .expect("string literal must be passed through");
    let ResolvedExpr::Function { args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };
    assert!(matches!(
        args.get(2),
        Some(ResolvedExpr::Literal(LiteralValue::String(s))) if s == "INTEGER32"
    ));
}

// --- Arity coverage for every vector function ------------------------

#[test]
fn every_vector_function_has_arity_guard() {
    // (function name, offending argument count, min–max hint)
    let cases = &[
        ("RETURN vector([1], 1) AS v", 2, "vector"),
        ("RETURN vector([1], 1, INTEGER, INTEGER) AS v", 4, "vector"),
        (
            "RETURN vector.similarity.cosine([1]) AS s",
            1,
            "vector.similarity.cosine",
        ),
        (
            "RETURN vector.similarity.cosine([1],[2],[3]) AS s",
            3,
            "vector.similarity.cosine",
        ),
        (
            "RETURN vector.similarity.euclidean([1]) AS s",
            1,
            "vector.similarity.euclidean",
        ),
        (
            "RETURN vector_distance(vector([1],1,INT)) AS d",
            1,
            "vector_distance",
        ),
        (
            "RETURN vector_norm(vector([1],1,INT)) AS n",
            1,
            "vector_norm",
        ),
        (
            "RETURN vector_dimension_count() AS n",
            0,
            "vector_dimension_count",
        ),
        ("RETURN toIntegerList() AS l", 0, "toIntegerList"),
        ("RETURN toFloatList() AS l", 0, "toFloatList"),
    ];
    for (query, expected_args, name) in cases {
        let graph = InMemoryGraph::new();
        let doc = parse_query(query).unwrap();
        let mut analyzer = Analyzer::new(&graph);
        let result = analyzer.analyze(&doc);
        match result {
            Err(SemanticError::WrongArity(got_name, _, got_args)) => {
                assert_eq!(got_name.to_ascii_lowercase(), name.to_ascii_lowercase());
                assert_eq!(got_args, *expected_args, "query {query:?}");
            }
            other => panic!("query {query:?} expected WrongArity, got {other:?}"),
        }
    }
}

#[test]
fn dotted_similarity_typo_is_rejected() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN vector.similarity.manhattan([1,2],[3,4]) AS s").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&doc),
        Err(SemanticError::UnknownFunction(name, _, _))
            if name == "vector.similarity.manhattan"
    ));
}

#[test]
fn list_expression_scope_is_popped_after_analysis_error() {
    let graph = InMemoryGraph::new();
    let bad = parse_query("RETURN any(x IN [1] WHERE missing) AS ok").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&bad),
        Err(SemanticError::UnknownVariable(name)) if name == "missing"
    ));

    let leaked = parse_query("RETURN x AS leaked").unwrap();
    assert!(matches!(
        analyzer.analyze(&leaked),
        Err(SemanticError::UnknownVariable(name)) if name == "x"
    ));
}
