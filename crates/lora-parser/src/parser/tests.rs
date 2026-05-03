use super::*;

fn as_regular_single_part(doc: Document) -> SinglePartQuery {
    let Statement::Query(Query::Regular(rq)) = doc.statement else {
        panic!("expected regular query");
    };
    let SingleQuery::SinglePart(sp) = rq.head else {
        panic!("expected single-part query");
    };
    sp
}

fn as_regular_multi_part(doc: Document) -> MultiPartQuery {
    let Statement::Query(Query::Regular(rq)) = doc.statement else {
        panic!("expected regular query");
    };
    let SingleQuery::MultiPart(mp) = rq.head else {
        panic!("expected multi-part query");
    };
    mp
}

fn as_standalone_call(doc: Document) -> StandaloneCall {
    let Statement::Query(Query::StandaloneCall(call)) = doc.statement else {
        panic!("expected standalone CALL");
    };
    call
}

fn first_match_clause(sp: &SinglePartQuery) -> &Match {
    let Some(first) = sp.reading_clauses.first() else {
        panic!("expected at least one reading clause");
    };
    let ReadingClause::Match(m) = first else {
        panic!("expected first reading clause to be MATCH");
    };
    m
}

fn first_return_expr(sp: &SinglePartQuery) -> &Expr {
    let ret = sp.return_clause.as_ref().expect("expected RETURN clause");
    let Some(first) = ret.body.items.first() else {
        panic!("expected at least one projection item");
    };
    let ProjectionItem::Expr { expr, .. } = first else {
        panic!("expected first projection item to be an expression");
    };
    expr
}

#[test]
fn parse_basic_match_return() {
    let doc = parse_query("MATCH (n) RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    assert_eq!(sp.reading_clauses.len(), 1);
    assert!(sp.updating_clauses.is_empty());
    assert!(sp.return_clause.is_some());

    let m = first_match_clause(&sp);
    assert!(!m.optional);
    assert_eq!(m.pattern.parts.len(), 1);
    assert!(m.where_.is_none());

    match first_return_expr(&sp) {
        Expr::Variable(v) => assert_eq!(v.name, "n"),
        other => panic!("expected RETURN variable n, got {other:?}"),
    }
}

#[test]
fn parse_match_with_label() {
    let doc = parse_query("MATCH (n:User) RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let PatternElement::NodeChain { head, chain, .. } = &m.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    assert!(chain.is_empty());
    assert_eq!(head.variable.as_ref().map(|v| v.name.as_str()), Some("n"));
    assert_eq!(head.labels.len(), 1);
    assert_eq!(head.labels[0][0], "User");
    assert!(head.properties.is_none());
}

#[test]
fn parse_match_multiple_labels() {
    let doc = parse_query("MATCH (n:User:Admin) RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let PatternElement::NodeChain { head, .. } = &m.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    assert_eq!(head.labels.len(), 2);
    assert_eq!(head.labels[0][0], "User");
    assert_eq!(head.labels[1][0], "Admin");
}

#[test]
fn parse_optional_match() {
    let doc = parse_query("OPTIONAL MATCH (n) RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    assert!(m.optional);
    assert!(m.where_.is_none());
}

#[test]
fn parse_match_relationship() {
    let doc = parse_query("MATCH (a)-[:FOLLOWS]->(b) RETURN a, b").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let PatternElement::NodeChain { head, chain, .. } = &m.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    assert_eq!(head.variable.as_ref().map(|v| v.name.as_str()), Some("a"));
    assert_eq!(chain.len(), 1);

    let rel = chain[0]
        .relationship
        .detail
        .as_ref()
        .expect("expected relationship detail");

    assert_eq!(rel.types.len(), 1);
    assert_eq!(rel.types[0], "FOLLOWS");
    assert!(matches!(chain[0].relationship.direction, Direction::Right));
    assert_eq!(
        chain[0].node.variable.as_ref().map(|v| v.name.as_str()),
        Some("b")
    );
}

#[test]
fn parse_match_left_relationship() {
    let doc = parse_query("MATCH (a)<-[:FOLLOWS]-(b) RETURN a, b").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let PatternElement::NodeChain { chain, .. } = &m.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    assert_eq!(chain.len(), 1);
    assert!(matches!(chain[0].relationship.direction, Direction::Left));
}

#[test]
fn parse_match_undirected_relationship() {
    let doc = parse_query("MATCH (a)-[:KNOWS]-(b) RETURN a, b").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let PatternElement::NodeChain { chain, .. } = &m.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    assert_eq!(chain.len(), 1);
    assert!(matches!(
        chain[0].relationship.direction,
        Direction::Undirected
    ));
}

#[test]
fn parse_relationship_with_variable_and_range() {
    let doc = parse_query("MATCH (a)-[r:FOLLOWS*1..3]->(b) RETURN r").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let PatternElement::NodeChain { chain, .. } = &m.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    let rel = chain[0]
        .relationship
        .detail
        .as_ref()
        .expect("expected relationship detail");

    assert_eq!(rel.variable.as_ref().map(|v| v.name.as_str()), Some("r"));
    assert_eq!(rel.types.len(), 1);
    assert_eq!(rel.types[0], "FOLLOWS");

    let range = rel.range.as_ref().expect("expected range");
    assert_eq!(range.start, Some(1));
    assert_eq!(range.end, Some(3));
}

#[test]
fn parse_relationship_range_upper_only() {
    let doc = parse_query("MATCH (a)-[:FOLLOWS*..3]->(b) RETURN a").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let PatternElement::NodeChain { chain, .. } = &m.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    let range = chain[0]
        .relationship
        .detail
        .as_ref()
        .and_then(|d| d.range.as_ref())
        .expect("expected range");

    assert_eq!(range.start, None);
    assert_eq!(range.end, Some(3));
}

#[test]
fn parse_relationship_range_lower_only() {
    let doc = parse_query("MATCH (a)-[:FOLLOWS*3..]->(b) RETURN a").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let PatternElement::NodeChain { chain, .. } = &m.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    let range = chain[0]
        .relationship
        .detail
        .as_ref()
        .and_then(|d| d.range.as_ref())
        .expect("expected range");

    assert_eq!(range.start, Some(3));
    assert_eq!(range.end, None);
}

#[test]
fn parse_relationship_range_unbounded() {
    let doc = parse_query("MATCH (a)-[:FOLLOWS*]->(b) RETURN a").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let PatternElement::NodeChain { chain, .. } = &m.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    let range = chain[0]
        .relationship
        .detail
        .as_ref()
        .and_then(|d| d.range.as_ref())
        .expect("expected range");

    assert_eq!(range.start, None);
    assert_eq!(range.end, None);
}

#[test]
fn parse_pattern_binding() {
    let doc = parse_query("MATCH p = (a)-[:FOLLOWS]->(b) RETURN p").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    assert_eq!(m.pattern.parts.len(), 1);
    assert_eq!(
        m.pattern.parts[0].binding.as_ref().map(|v| v.name.as_str()),
        Some("p")
    );

    match first_return_expr(&sp) {
        Expr::Variable(v) => assert_eq!(v.name, "p"),
        other => panic!("expected RETURN variable p, got {other:?}"),
    }
}

#[test]
fn parse_parenthesized_pattern_element() {
    let doc = parse_query("MATCH ((n)) RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    match &m.pattern.parts[0].element {
        PatternElement::Parenthesized(inner, _) => match inner.as_ref() {
            PatternElement::NodeChain { head, chain, .. } => {
                assert!(chain.is_empty());
                assert_eq!(head.variable.as_ref().map(|v| v.name.as_str()), Some("n"));
            }
            other => panic!("expected node chain inside parentheses, got {other:?}"),
        },
        other => panic!("expected parenthesized pattern element, got {other:?}"),
    }
}

#[test]
fn parse_where_expression() {
    let doc = parse_query("MATCH (n) WHERE 1 + 2 > 2 RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let where_ = m.where_.as_ref().expect("expected WHERE clause");

    match where_ {
        Expr::Binary { lhs, op, rhs, .. } => {
            assert!(matches!(op, BinaryOp::Gt));

            match lhs.as_ref() {
                Expr::Binary {
                    lhs: add_lhs,
                    op: add_op,
                    rhs: add_rhs,
                    ..
                } => {
                    assert!(matches!(add_op, BinaryOp::Add));
                    assert!(matches!(add_lhs.as_ref(), Expr::Integer(1, _)));
                    assert!(matches!(add_rhs.as_ref(), Expr::Integer(2, _)));
                }
                other => panic!("expected lhs to be addition expression, got {other:?}"),
            }

            assert!(matches!(rhs.as_ref(), Expr::Integer(2, _)));
        }
        other => panic!("expected binary WHERE expression, got {other:?}"),
    }
}

#[test]
fn parse_where_boolean_precedence() {
    let doc = parse_query("MATCH (n) WHERE NOT n.active AND n.age >= 18 RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let where_ = m.where_.as_ref().expect("expected WHERE clause");

    match where_ {
        Expr::Binary { lhs, op, rhs, .. } => {
            assert!(matches!(op, BinaryOp::And));

            match lhs.as_ref() {
                Expr::Unary {
                    op: UnaryOp::Not,
                    expr,
                    ..
                } => match expr.as_ref() {
                    Expr::Property { key, .. } => assert_eq!(key, "active"),
                    other => panic!("expected property under NOT, got {other:?}"),
                },
                other => panic!("expected NOT expression on lhs, got {other:?}"),
            }

            match rhs.as_ref() {
                Expr::Binary {
                    lhs: cmp_lhs,
                    op: cmp_op,
                    rhs: cmp_rhs,
                    ..
                } => {
                    assert!(matches!(cmp_op, BinaryOp::Ge));
                    assert!(matches!(cmp_rhs.as_ref(), Expr::Integer(18, _)));
                    match cmp_lhs.as_ref() {
                        Expr::Property { key, .. } => assert_eq!(key, "age"),
                        other => panic!("expected property on comparison lhs, got {other:?}"),
                    }
                }
                other => panic!("expected comparison expression on rhs, got {other:?}"),
            }
        }
        other => panic!("expected binary WHERE expression, got {other:?}"),
    }
}

#[test]
fn parse_where_parenthesized_expression() {
    let doc = parse_query("MATCH (n) WHERE (1 + 2) * 3 > 5 RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let where_ = m.where_.as_ref().expect("expected WHERE clause");

    match where_ {
        Expr::Binary { lhs, op, rhs, .. } => {
            assert!(matches!(op, BinaryOp::Gt));
            assert!(matches!(rhs.as_ref(), Expr::Integer(5, _)));

            match lhs.as_ref() {
                Expr::Binary {
                    lhs: mul_lhs,
                    op: mul_op,
                    rhs: mul_rhs,
                    ..
                } => {
                    assert!(matches!(mul_op, BinaryOp::Mul));
                    assert!(matches!(mul_rhs.as_ref(), Expr::Integer(3, _)));

                    match mul_lhs.as_ref() {
                        Expr::Binary {
                            lhs: add_lhs,
                            op: add_op,
                            rhs: add_rhs,
                            ..
                        } => {
                            assert!(matches!(add_op, BinaryOp::Add));
                            assert!(matches!(add_lhs.as_ref(), Expr::Integer(1, _)));
                            assert!(matches!(add_rhs.as_ref(), Expr::Integer(2, _)));
                        }
                        other => {
                            panic!("expected addition under multiplication, got {other:?}")
                        }
                    }
                }
                other => panic!("expected multiplication lhs, got {other:?}"),
            }
        }
        other => panic!("expected binary WHERE expression, got {other:?}"),
    }
}

#[test]
fn parse_where_in_operator() {
    let doc = parse_query("MATCH (n) WHERE n.age IN [1, 2, 3] RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let where_ = first_match_clause(&sp).where_.as_ref().unwrap();
    match where_ {
        Expr::Binary { lhs, op, rhs, .. } => {
            assert!(matches!(op, BinaryOp::In));
            assert!(matches!(lhs.as_ref(), Expr::Property { key, .. } if key == "age"));
            assert!(matches!(rhs.as_ref(), Expr::List(_, _)));
        }
        other => panic!("expected IN expression, got {other:?}"),
    }
}

#[test]
fn parse_where_contains_operator() {
    let doc = parse_query("MATCH (n) WHERE n.name CONTAINS 'al' RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let where_ = first_match_clause(&sp).where_.as_ref().unwrap();
    match where_ {
        Expr::Binary { op, rhs, .. } => {
            assert!(matches!(op, BinaryOp::Contains));
            assert!(matches!(rhs.as_ref(), Expr::String(s, _) if s == "al"));
        }
        other => panic!("expected CONTAINS expression, got {other:?}"),
    }
}

#[test]
fn parse_where_starts_with_operator() {
    let doc = parse_query("MATCH (n) WHERE n.name STARTS WITH 'a' RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let where_ = first_match_clause(&sp).where_.as_ref().unwrap();
    match where_ {
        Expr::Binary { op, rhs, .. } => {
            assert!(matches!(op, BinaryOp::StartsWith));
            assert!(matches!(rhs.as_ref(), Expr::String(s, _) if s == "a"));
        }
        other => panic!("expected STARTS WITH expression, got {other:?}"),
    }
}

#[test]
fn parse_where_ends_with_operator() {
    let doc = parse_query("MATCH (n) WHERE n.name ENDS WITH 'z' RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let where_ = first_match_clause(&sp).where_.as_ref().unwrap();
    match where_ {
        Expr::Binary { op, rhs, .. } => {
            assert!(matches!(op, BinaryOp::EndsWith));
            assert!(matches!(rhs.as_ref(), Expr::String(s, _) if s == "z"));
        }
        other => panic!("expected ENDS WITH expression, got {other:?}"),
    }
}

#[test]
fn parse_where_is_null() {
    let doc = parse_query("MATCH (n) WHERE n.name IS NULL RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let where_ = first_match_clause(&sp).where_.as_ref().unwrap();
    match where_ {
        Expr::Binary { lhs, op, rhs, .. } => {
            assert!(matches!(op, BinaryOp::IsNull));
            assert!(matches!(lhs.as_ref(), Expr::Property { key, .. } if key == "name"));
            assert!(matches!(rhs.as_ref(), Expr::Null(_)));
        }
        other => panic!("expected IS NULL expression, got {other:?}"),
    }
}

#[test]
fn parse_where_is_not_null() {
    let doc = parse_query("MATCH (n) WHERE n.name IS NOT NULL RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let where_ = first_match_clause(&sp).where_.as_ref().unwrap();
    match where_ {
        Expr::Binary { lhs, op, rhs, .. } => {
            assert!(matches!(op, BinaryOp::IsNotNull));
            assert!(matches!(lhs.as_ref(), Expr::Property { key, .. } if key == "name"));
            assert!(matches!(rhs.as_ref(), Expr::Null(_)));
        }
        other => panic!("expected IS NOT NULL expression, got {other:?}"),
    }
}

#[test]
fn parse_limit() {
    let doc = parse_query("MATCH (n) RETURN n LIMIT 10").unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    assert!(matches!(ret.body.limit, Some(Expr::Integer(10, _))));
    assert!(ret.body.skip.is_none());
}

#[test]
fn parse_distinct_return() {
    let doc = parse_query("MATCH (n) RETURN DISTINCT n").unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    assert!(ret.body.distinct);
    assert_eq!(ret.body.items.len(), 1);
}

#[test]
fn parse_return_star() {
    let doc = parse_query("MATCH (n) RETURN *").unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    assert_eq!(ret.body.items.len(), 1);
    assert!(matches!(ret.body.items[0], ProjectionItem::Star { .. }));
}

#[test]
fn parse_return_star_and_expr() {
    let doc = parse_query("MATCH (n) RETURN *, n.name").unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    assert_eq!(ret.body.items.len(), 2);
    assert!(matches!(ret.body.items[0], ProjectionItem::Star { .. }));
    assert!(matches!(
        &ret.body.items[1],
        ProjectionItem::Expr {
            expr: Expr::Property { key, .. },
            ..
        } if key == "name"
    ));
}

#[test]
fn parse_multiple_projection_items() {
    let doc = parse_query("MATCH (n) RETURN n, n.name AS name").unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    assert_eq!(ret.body.items.len(), 2);

    match &ret.body.items[0] {
        ProjectionItem::Expr { expr, alias, .. } => {
            assert!(alias.is_none());
            assert!(matches!(expr, Expr::Variable(_)));
        }
        other => panic!("expected projection expr, got {other:?}"),
    }

    match &ret.body.items[1] {
        ProjectionItem::Expr { expr, alias, .. } => {
            assert_eq!(alias.as_ref().map(|v| v.name.as_str()), Some("name"));
            match expr {
                Expr::Property { key, .. } => assert_eq!(key, "name"),
                other => panic!("expected property projection, got {other:?}"),
            }
        }
        other => panic!("expected projection expr, got {other:?}"),
    }
}

#[test]
fn parse_order_skip_limit() {
    let doc = parse_query("MATCH (n) RETURN n ORDER BY n.name DESC SKIP 5 LIMIT 10").unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    assert_eq!(ret.body.order.len(), 1);
    assert!(matches!(ret.body.order[0].direction, SortDirection::Desc));
    assert!(matches!(ret.body.skip, Some(Expr::Integer(5, _))));
    assert!(matches!(ret.body.limit, Some(Expr::Integer(10, _))));

    match &ret.body.order[0].expr {
        Expr::Property { key, .. } => assert_eq!(key, "name"),
        other => panic!("expected ORDER BY property lookup, got {other:?}"),
    }
}

#[test]
fn parse_order_multiple_sort_items() {
    let doc = parse_query("MATCH (n) RETURN n ORDER BY n.last ASC, n.first DESC").unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    assert_eq!(ret.body.order.len(), 2);
    assert!(matches!(ret.body.order[0].direction, SortDirection::Asc));
    assert!(matches!(ret.body.order[1].direction, SortDirection::Desc));
}

#[test]
fn parse_create_clause() {
    let doc = parse_query("CREATE (n:User {name: 'alice'}) RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    assert_eq!(sp.updating_clauses.len(), 1);

    let UpdatingClause::Create(create) = &sp.updating_clauses[0] else {
        panic!("expected CREATE clause");
    };
    let PatternElement::NodeChain { head, chain, .. } = &create.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    assert!(chain.is_empty());
    assert_eq!(head.labels[0][0], "User");
    assert!(head.properties.is_some());
}

#[test]
fn parse_create_without_return() {
    let doc = parse_query("CREATE (n:User)").unwrap();
    let sp = as_regular_single_part(doc);

    assert_eq!(sp.updating_clauses.len(), 1);
    assert!(sp.return_clause.is_none());
}

#[test]
fn parse_merge_clause() {
    let doc = parse_query("MERGE (n:User {id: 1}) RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    assert_eq!(sp.updating_clauses.len(), 1);
    let UpdatingClause::Merge(merge) = &sp.updating_clauses[0] else {
        panic!("expected MERGE clause");
    };

    let PatternElement::NodeChain { head, chain, .. } = &merge.pattern_part.element else {
        panic!("expected node chain");
    };
    assert!(chain.is_empty());
    assert_eq!(head.labels[0][0], "User");
    assert!(head.properties.is_some());
}

#[test]
fn parse_merge_with_actions() {
    let doc = parse_query(
        "MERGE (n:User {id: 1}) ON MATCH SET n.name = 'alice' ON CREATE SET n:New RETURN n",
    )
    .unwrap();
    let sp = as_regular_single_part(doc);

    let UpdatingClause::Merge(merge) = &sp.updating_clauses[0] else {
        panic!("expected MERGE clause");
    };

    assert_eq!(merge.actions.len(), 2);
    assert!(merge.actions[0].on_match);
    assert!(!merge.actions[1].on_match);
    assert_eq!(merge.actions[0].set.items.len(), 1);
    assert_eq!(merge.actions[1].set.items.len(), 1);
}

#[test]
fn parse_delete_clause() {
    let doc = parse_query("MATCH (n) DELETE n").unwrap();
    let sp = as_regular_single_part(doc);

    let UpdatingClause::Delete(delete) = &sp.updating_clauses[0] else {
        panic!("expected DELETE clause");
    };

    assert!(!delete.detach);
    assert_eq!(delete.expressions.len(), 1);
    assert!(matches!(delete.expressions[0], Expr::Variable(_)));
}

#[test]
fn parse_detach_delete_clause() {
    let doc = parse_query("MATCH (n) DETACH DELETE n").unwrap();
    let sp = as_regular_single_part(doc);

    let UpdatingClause::Delete(delete) = &sp.updating_clauses[0] else {
        panic!("expected DELETE clause");
    };

    assert!(delete.detach);
    assert_eq!(delete.expressions.len(), 1);
}

#[test]
fn parse_set_variable_clause() {
    let doc = parse_query("MATCH (n) SET n = {name: 'alice'} RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let UpdatingClause::Set(set) = &sp.updating_clauses[0] else {
        panic!("expected SET clause");
    };

    assert_eq!(set.items.len(), 1);
    match &set.items[0] {
        SetItem::SetVariable {
            variable, value, ..
        } => {
            assert_eq!(variable.name, "n");
            assert!(matches!(value, Expr::Map(_, _)));
        }
        other => panic!("expected SetVariable, got {other:?}"),
    }
}

#[test]
fn parse_set_property_clause() {
    let doc = parse_query("MATCH (n) SET n.name = 'alice' RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let UpdatingClause::Set(set) = &sp.updating_clauses[0] else {
        panic!("expected SET clause");
    };

    assert_eq!(set.items.len(), 1);
    match &set.items[0] {
        SetItem::SetProperty { target, value, .. } => {
            assert!(matches!(target, Expr::Property { key, .. } if key == "name"));
            assert!(matches!(value, Expr::String(s, _) if s == "alice"));
        }
        other => panic!("expected SetProperty, got {other:?}"),
    }
}

#[test]
fn parse_set_mutate_variable_clause() {
    let doc = parse_query("MATCH (n) SET n += {age: 42} RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let UpdatingClause::Set(set) = &sp.updating_clauses[0] else {
        panic!("expected SET clause");
    };

    assert_eq!(set.items.len(), 1);
    match &set.items[0] {
        SetItem::MutateVariable {
            variable, value, ..
        } => {
            assert_eq!(variable.name, "n");
            assert!(matches!(value, Expr::Map(_, _)));
        }
        other => panic!("expected MutateVariable, got {other:?}"),
    }
}

#[test]
fn parse_set_labels_clause() {
    let doc = parse_query("MATCH (n) SET n:User:Admin RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let UpdatingClause::Set(set) = &sp.updating_clauses[0] else {
        panic!("expected SET clause");
    };

    assert_eq!(set.items.len(), 1);
    match &set.items[0] {
        SetItem::SetLabels {
            variable, labels, ..
        } => {
            assert_eq!(variable.name, "n");
            assert_eq!(labels, &vec!["User".to_string(), "Admin".to_string()]);
        }
        other => panic!("expected SetLabels, got {other:?}"),
    }
}

#[test]
fn parse_remove_labels_clause() {
    let doc = parse_query("MATCH (n) REMOVE n:User:Admin RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let UpdatingClause::Remove(remove) = &sp.updating_clauses[0] else {
        panic!("expected REMOVE clause");
    };

    assert_eq!(remove.items.len(), 1);
    match &remove.items[0] {
        RemoveItem::Labels {
            variable, labels, ..
        } => {
            assert_eq!(variable.name, "n");
            assert_eq!(labels, &vec!["User".to_string(), "Admin".to_string()]);
        }
        other => panic!("expected RemoveItem::Labels, got {other:?}"),
    }
}

#[test]
fn parse_remove_property_clause() {
    let doc = parse_query("MATCH (n) REMOVE n.name RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let UpdatingClause::Remove(remove) = &sp.updating_clauses[0] else {
        panic!("expected REMOVE clause");
    };

    assert_eq!(remove.items.len(), 1);
    match &remove.items[0] {
        RemoveItem::Property { expr, .. } => {
            assert!(matches!(expr, Expr::Property { key, .. } if key == "name"));
        }
        other => panic!("expected RemoveItem::Property, got {other:?}"),
    }
}

#[test]
fn parse_node_properties_map() {
    let doc = parse_query("MATCH (n:User {name: 'alice', age: 42}) RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let PatternElement::NodeChain { head, .. } = &m.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    match head.properties.as_ref().expect("expected node properties") {
        Expr::Map(items, _) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].0, "name");
            assert!(matches!(items[0].1, Expr::String(_, _)));
            assert_eq!(items[1].0, "age");
            assert!(matches!(items[1].1, Expr::Integer(42, _)));
        }
        other => panic!("expected map literal properties, got {other:?}"),
    }
}

#[test]
fn parse_relationship_properties_map() {
    let doc = parse_query("MATCH (a)-[:FOLLOWS {since: 2020}]->(b) RETURN a").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let PatternElement::NodeChain { chain, .. } = &m.pattern.parts[0].element else {
        panic!("expected node chain");
    };

    let rel = chain[0]
        .relationship
        .detail
        .as_ref()
        .expect("expected relationship detail");

    match rel
        .properties
        .as_ref()
        .expect("expected relationship properties")
    {
        Expr::Map(items, _) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].0, "since");
            assert!(matches!(items[0].1, Expr::Integer(2020, _)));
        }
        other => panic!("expected relationship map properties, got {other:?}"),
    }
}

#[test]
fn parse_unwind_clause() {
    let doc = parse_query("UNWIND [1, 2, 3] AS n RETURN n").unwrap();
    let sp = as_regular_single_part(doc);

    assert_eq!(sp.reading_clauses.len(), 1);

    let ReadingClause::Unwind(unwind) = &sp.reading_clauses[0] else {
        panic!("expected UNWIND clause");
    };

    assert_eq!(unwind.alias.name, "n");

    match &unwind.expr {
        Expr::List(items, _) => {
            assert_eq!(items.len(), 3);
            assert!(matches!(items[0], Expr::Integer(1, _)));
            assert!(matches!(items[1], Expr::Integer(2, _)));
            assert!(matches!(items[2], Expr::Integer(3, _)));
        }
        other => panic!("expected list expression, got {other:?}"),
    }
}

#[test]
fn parse_unary_operators() {
    let doc = parse_query("RETURN -1, +2").unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    assert_eq!(ret.body.items.len(), 2);

    match &ret.body.items[0] {
        ProjectionItem::Expr { expr, .. } => match expr {
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
                ..
            } => assert!(matches!(expr.as_ref(), Expr::Integer(1, _))),
            other => panic!("expected unary negation, got {other:?}"),
        },
        other => panic!("expected projection expr, got {other:?}"),
    }

    match &ret.body.items[1] {
        ProjectionItem::Expr { expr, .. } => match expr {
            Expr::Unary {
                op: UnaryOp::Pos,
                expr,
                ..
            } => assert!(matches!(expr.as_ref(), Expr::Integer(2, _))),
            other => panic!("expected unary positive, got {other:?}"),
        },
        other => panic!("expected projection expr, got {other:?}"),
    }
}

#[test]
fn parse_power_operator() {
    let doc = parse_query("RETURN 2 ^ 3 ^ 4").unwrap();
    let sp = as_regular_single_part(doc);

    match first_return_expr(&sp) {
        Expr::Binary { lhs, op, rhs, .. } => {
            assert!(matches!(op, BinaryOp::Pow));
            assert!(matches!(rhs.as_ref(), Expr::Integer(4, _)));
            assert!(matches!(
                lhs.as_ref(),
                Expr::Binary {
                    op: BinaryOp::Pow,
                    ..
                }
            ));
        }
        other => panic!("expected power expression, got {other:?}"),
    }
}

#[test]
fn parse_function_call_and_alias() {
    let doc = parse_query("MATCH (n) RETURN count(n) AS c").unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    assert_eq!(ret.body.items.len(), 1);

    let ProjectionItem::Expr { expr, alias, .. } = &ret.body.items[0] else {
        panic!("expected projection expr");
    };

    assert_eq!(alias.as_ref().map(|v| v.name.as_str()), Some("c"));

    match expr {
        Expr::FunctionCall {
            name,
            distinct,
            args,
            ..
        } => {
            assert_eq!(name, &vec!["count".to_string()]);
            assert!(!distinct);
            assert_eq!(args.len(), 1);
            assert!(matches!(args[0], Expr::Variable(_)));
        }
        other => panic!("expected function call, got {other:?}"),
    }
}

#[test]
fn parse_distinct_function_call() {
    let doc = parse_query("MATCH (n) RETURN count(DISTINCT n) AS c").unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    let ProjectionItem::Expr { expr, .. } = &ret.body.items[0] else {
        panic!("expected projection expr");
    };

    match expr {
        Expr::FunctionCall {
            name,
            distinct,
            args,
            ..
        } => {
            assert_eq!(name, &vec!["count".to_string()]);
            assert!(*distinct);
            assert_eq!(args.len(), 1);
        }
        other => panic!("expected function call, got {other:?}"),
    }
}

#[test]
fn parse_namespaced_function_call() {
    let doc = parse_query("RETURN my.ns.func(1, 2)").unwrap();
    let sp = as_regular_single_part(doc);

    match first_return_expr(&sp) {
        Expr::FunctionCall { name, args, .. } => {
            assert_eq!(
                name,
                &vec!["my".to_string(), "ns".to_string(), "func".to_string()]
            );
            assert_eq!(args.len(), 2);
            assert!(matches!(args[0], Expr::Integer(1, _)));
            assert!(matches!(args[1], Expr::Integer(2, _)));
        }
        other => panic!("expected namespaced function call, got {other:?}"),
    }
}

#[test]
fn parse_parameter_and_property_lookup() {
    let doc = parse_query("MATCH (n) WHERE n.age >= $minAge RETURN n.name").unwrap();
    let sp = as_regular_single_part(doc);

    let m = first_match_clause(&sp);
    let where_ = m.where_.as_ref().expect("expected WHERE clause");

    match where_ {
        Expr::Binary { lhs, op, rhs, .. } => {
            assert!(matches!(op, BinaryOp::Ge));

            match lhs.as_ref() {
                Expr::Property { key, .. } => assert_eq!(key, "age"),
                other => panic!("expected property lookup on lhs, got {other:?}"),
            }

            match rhs.as_ref() {
                Expr::Parameter(name, _) => assert_eq!(name, "minAge"),
                other => panic!("expected parameter on rhs, got {other:?}"),
            }
        }
        other => panic!("expected binary WHERE expression, got {other:?}"),
    }

    let ret = sp.return_clause.expect("expected RETURN clause");
    let ProjectionItem::Expr { expr, .. } = &ret.body.items[0] else {
        panic!("expected projection expr");
    };

    match expr {
        Expr::Property { key, .. } => assert_eq!(key, "name"),
        other => panic!("expected property lookup in RETURN, got {other:?}"),
    }
}

#[test]
fn parse_numeric_parameter() {
    let doc = parse_query("RETURN $1").unwrap();
    let sp = as_regular_single_part(doc);

    match first_return_expr(&sp) {
        Expr::Parameter(name, _) => assert_eq!(name, "1"),
        other => panic!("expected numeric parameter, got {other:?}"),
    }
}

#[test]
fn parse_map_and_list_literals() {
    let doc = parse_query("RETURN {name: 'alice', nums: [1, 2, 3]}").unwrap();
    let sp = as_regular_single_part(doc);

    match first_return_expr(&sp) {
        Expr::Map(items, _) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].0, "name");
            assert!(matches!(items[0].1, Expr::String(_, _)));

            assert_eq!(items[1].0, "nums");
            match &items[1].1 {
                Expr::List(values, _) => {
                    assert_eq!(values.len(), 3);
                    assert!(matches!(values[0], Expr::Integer(1, _)));
                    assert!(matches!(values[1], Expr::Integer(2, _)));
                    assert!(matches!(values[2], Expr::Integer(3, _)));
                }
                other => panic!("expected nested list, got {other:?}"),
            }
        }
        other => panic!("expected map literal, got {other:?}"),
    }
}

#[test]
fn parse_case_expression() {
    let doc = parse_query("RETURN CASE WHEN 1 = 1 THEN 'yes' ELSE 'no' END").unwrap();
    let sp = as_regular_single_part(doc);

    match first_return_expr(&sp) {
        Expr::Case {
            input,
            alternatives,
            else_expr,
            ..
        } => {
            assert!(input.is_none());
            assert_eq!(alternatives.len(), 1);
            assert!(else_expr.is_some());
        }
        other => panic!("expected CASE expression, got {other:?}"),
    }
}

#[test]
fn parse_case_expression_with_input() {
    let doc = parse_query("RETURN CASE n.age WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END")
        .unwrap();
    let sp = as_regular_single_part(doc);

    match first_return_expr(&sp) {
        Expr::Case {
            input,
            alternatives,
            else_expr,
            ..
        } => {
            assert!(input.is_some());
            assert_eq!(alternatives.len(), 2);
            assert!(else_expr.is_some());
        }
        other => panic!("expected CASE expression, got {other:?}"),
    }
}

#[test]
fn parse_literals() {
    let doc = parse_query("RETURN 42, 3.14, true, false, null, 'x'").unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    assert_eq!(ret.body.items.len(), 6);

    match &ret.body.items[0] {
        ProjectionItem::Expr { expr, .. } => assert!(matches!(expr, Expr::Integer(42, _))),
        other => panic!("expected projection expr, got {other:?}"),
    }
    match &ret.body.items[1] {
        ProjectionItem::Expr { expr, .. } => assert!(matches!(expr, Expr::Float(_, _))),
        other => panic!("expected projection expr, got {other:?}"),
    }
    match &ret.body.items[2] {
        ProjectionItem::Expr { expr, .. } => assert!(matches!(expr, Expr::Bool(true, _))),
        other => panic!("expected projection expr, got {other:?}"),
    }
    match &ret.body.items[3] {
        ProjectionItem::Expr { expr, .. } => assert!(matches!(expr, Expr::Bool(false, _))),
        other => panic!("expected projection expr, got {other:?}"),
    }
    match &ret.body.items[4] {
        ProjectionItem::Expr { expr, .. } => assert!(matches!(expr, Expr::Null(_))),
        other => panic!("expected projection expr, got {other:?}"),
    }
    match &ret.body.items[5] {
        ProjectionItem::Expr { expr, .. } => match expr {
            Expr::String(s, _) => assert_eq!(s, "x"),
            other => panic!("expected string literal, got {other:?}"),
        },
        other => panic!("expected projection expr, got {other:?}"),
    }
}

#[test]
fn parse_string_escapes() {
    let doc = parse_query(r#"RETURN "a\nb", 'it\'s', "\\""#).unwrap();
    let sp = as_regular_single_part(doc);

    let ret = sp.return_clause.expect("expected RETURN clause");
    assert_eq!(ret.body.items.len(), 3);

    match &ret.body.items[0] {
        ProjectionItem::Expr {
            expr: Expr::String(s, _),
            ..
        } => assert_eq!(s, "a\nb"),
        other => panic!("expected escaped string, got {other:?}"),
    }
    match &ret.body.items[1] {
        ProjectionItem::Expr {
            expr: Expr::String(s, _),
            ..
        } => assert_eq!(s, "it's"),
        other => panic!("expected escaped string, got {other:?}"),
    }
    match &ret.body.items[2] {
        ProjectionItem::Expr {
            expr: Expr::String(s, _),
            ..
        } => assert_eq!(s, "\\"),
        other => panic!("expected escaped string, got {other:?}"),
    }
}

#[test]
fn parse_union_query() {
    let doc = parse_query("MATCH (a) RETURN a UNION MATCH (b) RETURN b").unwrap();

    let Statement::Query(Query::Regular(rq)) = doc.statement else {
        panic!("expected regular query");
    };

    assert_eq!(rq.unions.len(), 1);
    assert!(!rq.unions[0].all);

    let SingleQuery::SinglePart(head) = rq.head else {
        panic!("expected single-part head");
    };
    let head_ret = head.return_clause.expect("expected head return");
    assert_eq!(head_ret.body.items.len(), 1);

    let SingleQuery::SinglePart(union_q) = &rq.unions[0].query else {
        panic!("expected single-part union");
    };
    let union_ret = union_q
        .return_clause
        .as_ref()
        .expect("expected union return");
    assert_eq!(union_ret.body.items.len(), 1);
}

#[test]
fn parse_union_all_query() {
    let doc = parse_query("MATCH (a) RETURN a UNION ALL MATCH (b) RETURN b").unwrap();

    let Statement::Query(Query::Regular(rq)) = doc.statement else {
        panic!("expected regular query");
    };

    assert_eq!(rq.unions.len(), 1);
    assert!(rq.unions[0].all);
}

#[test]
fn parse_with_clause_in_multi_part_query() {
    let doc = parse_query("MATCH (n) WITH n RETURN n").unwrap();
    let mp = as_regular_multi_part(doc);

    assert_eq!(mp.parts.len(), 1);
    assert_eq!(mp.parts[0].reading_clauses.len(), 1);
    assert!(mp.parts[0].updating_clauses.is_empty());
    assert_eq!(mp.parts[0].with_clause.body.items.len(), 1);
    assert!(mp.parts[0].with_clause.where_.is_none());
    assert!(mp.tail.return_clause.is_some());
}

#[test]
fn parse_with_where_clause_in_multi_part_query() {
    let doc = parse_query("MATCH (n) WITH n WHERE n.age >= 18 RETURN n").unwrap();
    let mp = as_regular_multi_part(doc);

    let where_ = mp.parts[0]
        .with_clause
        .where_
        .as_ref()
        .expect("expected WITH WHERE clause");
    match where_ {
        Expr::Binary { op, .. } => assert!(matches!(op, BinaryOp::Ge)),
        other => panic!("expected binary expression, got {other:?}"),
    }
}

#[test]
fn parse_multi_part_with_update() {
    let doc = parse_query("MATCH (n) SET n:Seen WITH n RETURN n").unwrap();
    let mp = as_regular_multi_part(doc);

    assert_eq!(mp.parts.len(), 1);
    assert_eq!(mp.parts[0].reading_clauses.len(), 1);
    assert_eq!(mp.parts[0].updating_clauses.len(), 1);
    assert!(mp.tail.return_clause.is_some());
}

#[test]
fn parse_standalone_call_explicit() {
    let doc = parse_query("CALL db.labels()").unwrap();
    let call = as_standalone_call(doc);

    match call.procedure {
        ProcedureInvocationKind::Explicit(proc_) => {
            assert_eq!(
                proc_.name.parts,
                vec!["db".to_string(), "labels".to_string()]
            );
            assert!(proc_.args.is_empty());
        }
        other => panic!("expected explicit procedure invocation, got {other:?}"),
    }

    assert!(call.yield_items.is_empty());
    assert!(!call.yield_all);
}

#[test]
fn parse_standalone_call_implicit() {
    let doc = parse_query("CALL db.labels").unwrap();
    let call = as_standalone_call(doc);

    match call.procedure {
        ProcedureInvocationKind::Implicit(name) => {
            assert_eq!(name.parts, vec!["db".to_string(), "labels".to_string()]);
        }
        other => panic!("expected implicit procedure name, got {other:?}"),
    }
}

#[test]
fn parse_standalone_call_yield_all() {
    let doc = parse_query("CALL db.labels() YIELD *").unwrap();
    let call = as_standalone_call(doc);

    assert!(call.yield_all);
    assert!(call.yield_items.is_empty());
}

#[test]
fn parse_standalone_call_yield_items() {
    let doc = parse_query("CALL db.labels() YIELD label, value AS v").unwrap();
    let call = as_standalone_call(doc);

    assert!(!call.yield_all);
    assert_eq!(call.yield_items.len(), 2);

    assert_eq!(call.yield_items[0].field, None);
    assert_eq!(call.yield_items[0].alias.name, "label");

    assert_eq!(call.yield_items[1].field.as_deref(), Some("value"));
    assert_eq!(call.yield_items[1].alias.name, "v");
}

#[test]
fn parse_in_query_call() {
    let doc = parse_query("CALL db.labels() YIELD label RETURN label").unwrap();
    let sp = as_regular_single_part(doc);

    assert_eq!(sp.reading_clauses.len(), 1);
    let ReadingClause::InQueryCall(call) = &sp.reading_clauses[0] else {
        panic!("expected in-query CALL");
    };

    assert_eq!(
        call.procedure.name.parts,
        vec!["db".to_string(), "labels".to_string()]
    );
    assert_eq!(call.yield_items.len(), 1);
    assert!(call.where_.is_none());
}

#[test]
fn parse_in_query_call_with_where() {
    let doc =
        parse_query("CALL db.labels() YIELD label WHERE label IS NOT NULL RETURN label").unwrap();
    let sp = as_regular_single_part(doc);

    let ReadingClause::InQueryCall(call) = &sp.reading_clauses[0] else {
        panic!("expected in-query CALL");
    };
    let where_ = call.where_.as_ref().expect("expected WHERE on CALL");

    match where_ {
        Expr::Binary { op, .. } => assert!(matches!(op, BinaryOp::IsNotNull)),
        other => panic!("expected binary WHERE expression, got {other:?}"),
    }
}

#[test]
fn parse_semicolon() {
    let doc = parse_query("MATCH (n) RETURN n;").unwrap();
    let sp = as_regular_single_part(doc);
    assert!(sp.return_clause.is_some());
}
