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
fn vector_type_cast_lowers_to_type_literal() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN [1, 2, 3]::VECTOR<INTEGER>(3) AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer
        .analyze(&doc)
        .expect("VECTOR type should be formatted as a type literal");
    let Some(ResolvedClause::Return(ret)) = resolved.clauses.last() else {
        panic!("expected RETURN clause");
    };
    let ResolvedExpr::Function { args, .. } = &ret.items[0].expr else {
        panic!("expected function call");
    };
    assert!(matches!(
        args.get(1),
        Some(ResolvedExpr::Literal(LiteralValue::TypeName(s))) if s == "VECTOR<INTEGER>(3)"
    ));
}

#[test]
fn vector_distance_rewrites_bare_metric_identifier() {
    let graph = InMemoryGraph::new();
    let doc = parse_query(
        "RETURN vector.distance([1,2]::VECTOR<INTEGER>(2), [3,4]::VECTOR<INTEGER>(2), EUCLIDEAN) AS d",
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
fn cast_rewrites_bare_target_to_type_literal() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN cast.to('42', INTEGER) AS n").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer
        .analyze(&doc)
        .expect("INTEGER should be rewritten as a type literal");
    let Some(ResolvedClause::Return(ret)) = resolved.clauses.last() else {
        panic!("expected RETURN clause");
    };
    let ResolvedExpr::Function { args, .. } = &ret.items[0].expr else {
        panic!("expected function call");
    };
    assert!(matches!(
        args.get(1),
        Some(ResolvedExpr::Literal(LiteralValue::TypeName(s))) if s == "INTEGER"
    ));
}

#[test]
fn parenthesized_scalar_type_cast_lowers_to_strict_cast() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN ('42' AS INTEGER) AS n").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer
        .analyze(&doc)
        .expect("parenthesized scalar cast should analyze");
    let ResolvedExpr::Function { function, args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };

    assert_eq!(function.name(), "cast.to");
    assert_eq!(args.len(), 2);
    assert!(matches!(
        args.get(1),
        Some(ResolvedExpr::Literal(LiteralValue::TypeName(s))) if s == "INTEGER"
    ));
}

#[test]
fn duckdb_cast_call_lowers_to_strict_cast() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN CAST('42' AS INTEGER) AS n").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer.analyze(&doc).expect("CAST should analyze");
    let ResolvedExpr::Function { function, args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };

    assert_eq!(function.name(), "cast.to");
    assert!(matches!(
        args.get(1),
        Some(ResolvedExpr::Literal(LiteralValue::TypeName(s))) if s == "INTEGER"
    ));
}

#[test]
fn duckdb_try_cast_call_lowers_to_nullable_cast() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN TRY_CAST('bad' AS DATE) AS d").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer.analyze(&doc).expect("TRY_CAST should analyze");
    let ResolvedExpr::Function { function, args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };

    assert_eq!(function.name(), "cast.try");
    assert!(matches!(
        args.get(1),
        Some(ResolvedExpr::Literal(LiteralValue::TypeName(s))) if s == "DATE"
    ));
}

#[test]
fn duckdb_postfix_cast_lowers_to_strict_cast() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN '42'::INTEGER AS n").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer.analyze(&doc).expect("postfix cast should analyze");
    let ResolvedExpr::Function { function, args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };

    assert_eq!(function.name(), "cast.to");
    assert!(matches!(
        args.get(1),
        Some(ResolvedExpr::Literal(LiteralValue::TypeName(s))) if s == "INTEGER"
    ));
}

#[test]
fn parenthesized_temporal_type_cast_lowers_to_strict_cast() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN ('2024-01-15' AS DATE) AS d").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer
        .analyze(&doc)
        .expect("DATE cast should analyze as strict cast");
    let ResolvedExpr::Function { function, args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };

    assert_eq!(function.name(), "cast.to");
    assert_eq!(args.len(), 2);
    assert!(matches!(
        args.get(1),
        Some(ResolvedExpr::Literal(LiteralValue::TypeName(s))) if s == "DATE"
    ));
}

#[test]
fn parenthesized_local_datetime_type_cast_lowers_to_strict_cast() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN ('2024-01-15T10:30:00' AS LOCAL DATETIME) AS dt").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer
        .analyze(&doc)
        .expect("LOCAL DATETIME cast should analyze as strict cast");
    let ResolvedExpr::Function { function, args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };

    assert_eq!(function.name(), "cast.to");
    assert_eq!(args.len(), 2);
    assert!(matches!(
        args.get(1),
        Some(ResolvedExpr::Literal(LiteralValue::TypeName(s))) if s == "LOCAL_DATETIME"
    ));
}

#[test]
fn parenthesized_vector_type_cast_lowers_to_strict_cast() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN ([1, 2] AS VECTOR<FLOAT>(2)) AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer
        .analyze(&doc)
        .expect("VECTOR cast should analyze as strict cast");
    let ResolvedExpr::Function { function, args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };

    assert_eq!(function.name(), "cast.to");
    assert_eq!(args.len(), 2);
    assert!(matches!(
        args.get(1),
        Some(ResolvedExpr::Literal(LiteralValue::TypeName(s))) if s == "VECTOR<FLOAT64>(2)"
    ));
}

#[test]
fn parenthesized_list_type_cast_lowers_to_formatted_type_literal() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN ([[1], [2]] AS LIST<LIST<INTEGER>>) AS xs").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer
        .analyze(&doc)
        .expect("LIST cast should analyze as a formatted type literal");
    let ResolvedExpr::Function { function, args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };

    assert_eq!(function.name(), "cast.to");
    assert!(matches!(
        args.get(1),
        Some(ResolvedExpr::Literal(LiteralValue::TypeName(s))) if s == "LIST<LIST<INTEGER>>"
    ));
}

#[test]
fn vector_norm_rewrites_bare_metric_identifier() {
    let graph = InMemoryGraph::new();
    let doc =
        parse_query("RETURN vector.norm([1,2,3]::VECTOR<FLOAT32>(3), MANHATTAN) AS n").unwrap();
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
    // INTEGER in the value slot should still attempt to resolve as a
    // variable; the target type itself is parser syntax.
    let graph = InMemoryGraph::new();

    let bad_first = parse_query("RETURN INTEGER::VECTOR<INTEGER>(3) AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&bad_first),
        Err(SemanticError::UnknownVariable(name)) if name == "INTEGER"
    ));
}

#[test]
fn vector_distance_does_not_rewrite_first_or_second_argument() {
    let graph = InMemoryGraph::new();
    let doc =
        parse_query("RETURN vector.distance(EUCLIDEAN, [1,2]::VECTOR<INTEGER>(2), EUCLIDEAN) AS d")
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
    let doc = parse_query("RETURN vector.norm(MANHATTAN, EUCLIDEAN) AS n").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    assert!(matches!(
        analyzer.analyze(&doc),
        Err(SemanticError::UnknownVariable(name)) if name == "MANHATTAN"
    ));
}

#[test]
fn parameter_in_type_slot_is_preserved_as_parameter() {
    // A $param in a type slot must NOT be rewritten — callers need
    // to pass target types dynamically.
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN cast.to('42', $type) AS v").unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer.analyze(&doc).expect("parameter should be kept");
    let ResolvedExpr::Function { args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };
    assert!(matches!(args.get(1), Some(ResolvedExpr::Parameter(p)) if p == "type"));
}

#[test]
fn parameter_in_vector_norm_metric_slot_is_preserved() {
    let graph = InMemoryGraph::new();
    let doc = parse_query("RETURN vector.norm([1,2,3]::VECTOR<FLOAT32>(3), $metric) AS n").unwrap();
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
    let doc = parse_query(
        "RETURN vector.distance([1]::VECTOR<INTEGER>(1), [2]::VECTOR<INTEGER>(1), 'euclidean') AS d",
    )
    .unwrap();
    let mut analyzer = Analyzer::new(&graph);
    let resolved = analyzer
        .analyze(&doc)
        .expect("string literal must be passed through");
    let ResolvedExpr::Function { args, .. } = return_expr(&resolved.clauses) else {
        panic!("expected function");
    };
    assert!(matches!(
        args.get(2),
        Some(ResolvedExpr::Literal(LiteralValue::String(s))) if s == "euclidean"
    ));
}

// --- Arity coverage for every vector function ------------------------

#[test]
fn every_vector_function_has_arity_guard() {
    // (function name, offending argument count, min–max hint)
    let cases = &[
        ("RETURN vector.similarity([1]) AS s", 1, "vector.similarity"),
        (
            "RETURN vector.similarity([1],[2],[3],[4]) AS s",
            4,
            "vector.similarity",
        ),
        (
            "RETURN vector.distance([1]::VECTOR<INTEGER>(1)) AS d",
            1,
            "vector.distance",
        ),
        (
            "RETURN vector.norm([1]::VECTOR<INTEGER>(1)) AS n",
            1,
            "vector.norm",
        ),
        ("RETURN vector.dimension() AS n", 0, "vector.dimension"),
        ("RETURN vector.coordinates() AS l", 0, "vector.coordinates"),
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
