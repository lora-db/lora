use super::literals::{
    lower_exists_subquery, lower_function_invocation, lower_list_predicate, lower_literal,
    lower_parameter, lower_reduce_expression, lower_schema_name, lower_variable,
};
use super::util::{merge_spans, pair_span, single_inner, unexpected_rule};
use super::Rule;
use crate::errors::ParseError;
use lora_ast::*;
use pest::iterators::Pair;

pub(super) fn lower_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    match pair.as_rule() {
        Rule::expression => lower_expression(single_inner(pair)?),
        Rule::case_expression => lower_expression(single_inner(pair)?),
        Rule::simple_case_expression => lower_simple_case_expression(pair),
        Rule::generic_case_expression => lower_generic_case_expression(pair),
        Rule::or_expression => {
            lower_left_assoc(pair, lower_expression, &[(Rule::OR, BinaryOp::Or)])
        }
        Rule::xor_expression => {
            lower_left_assoc(pair, lower_expression, &[(Rule::XOR, BinaryOp::Xor)])
        }
        Rule::and_expression => {
            lower_left_assoc(pair, lower_expression, &[(Rule::AND, BinaryOp::And)])
        }
        Rule::not_expression => lower_not_expression(pair),
        Rule::comparison_expression => lower_comparison_expression(pair),
        Rule::add_expression => lower_add_expression(pair),
        Rule::mul_expression => lower_mul_expression(pair),
        Rule::pow_expression => lower_pow_expression(pair),
        Rule::unary_expression => lower_unary_expression(pair),
        Rule::postfix_expression => lower_postfix_expression(pair),
        Rule::atom => lower_expression(single_inner(pair)?),
        Rule::list_predicate => lower_list_predicate(pair),
        Rule::reduce_expression => lower_reduce_expression(pair),
        Rule::variable => Ok(Expr::Variable(lower_variable(pair)?)),
        Rule::literal => lower_literal(single_inner(pair)?),
        Rule::parameter => Ok(lower_parameter(pair)),
        Rule::function_invocation => lower_function_invocation(pair),
        Rule::parenthesized_expression => {
            let span = pair_span(&pair);
            let expr = pair
                .into_inner()
                .find(|p| p.as_rule() == Rule::expression)
                .ok_or_else(|| ParseError::new("expected expression", span.start, span.end))?;
            lower_expression(expr)
        }
        Rule::exists_subquery => lower_exists_subquery(pair),
        _ => Err(unexpected_rule("expression", pair)),
    }
}

pub(super) fn lower_left_assoc(
    pair: Pair<Rule>,
    recurse: fn(Pair<Rule>) -> Result<Expr, ParseError>,
    ops: &[(Rule, BinaryOp)],
) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let first = inner
        .next()
        .ok_or_else(|| ParseError::new("expected lhs", span.start, span.end))?;
    let mut expr = recurse(first)?;

    while let Some(op_pair) = inner.next() {
        let rhs_pair = inner
            .next()
            .ok_or_else(|| ParseError::new("expected rhs", span.start, span.end))?;
        let op = ops
            .iter()
            .find_map(|(r, op)| {
                if *r == op_pair.as_rule() {
                    Some(*op)
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                ParseError::new(
                    "unknown operator",
                    op_pair.as_span().start(),
                    op_pair.as_span().end(),
                )
            })?;

        let rhs = recurse(rhs_pair)?;
        let merged = merge_spans(expr.span(), rhs.span());
        expr = Expr::Binary {
            lhs: Box::new(expr),
            op,
            rhs: Box::new(rhs),
            span: merged,
        };
    }

    Ok(expr)
}

pub(super) fn lower_not_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut not_count = 0usize;
    let mut tail = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::NOT => not_count += 1,
            _ => tail = Some(p),
        }
    }

    let mut expr = lower_expression(
        tail.ok_or_else(|| ParseError::new("expected expression", span.start, span.end))?,
    )?;

    for _ in 0..not_count {
        expr = Expr::Unary {
            op: UnaryOp::Not,
            expr: Box::new(expr),
            span,
        };
    }

    Ok(expr)
}

pub(super) fn lower_comparison_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();

    let first = inner
        .next()
        .ok_or_else(|| ParseError::new("expected lhs", span.start, span.end))?;
    let mut expr = lower_expression(first)?;

    for tail in inner {
        if tail.as_rule() != Rule::comparison_tail {
            continue;
        }

        let tail_span = pair_span(&tail);
        let (op, rhs) = lower_comparison_tail(tail, tail_span)?;

        match rhs {
            Some(rhs) => {
                let merged = merge_spans(expr.span(), rhs.span());
                expr = Expr::Binary {
                    lhs: Box::new(expr),
                    op,
                    rhs: Box::new(rhs),
                    span: merged,
                };
            }
            None => {
                let rhs = Expr::Null(tail_span);
                let merged = merge_spans(expr.span(), rhs.span());
                expr = Expr::Binary {
                    lhs: Box::new(expr),
                    op,
                    rhs: Box::new(rhs),
                    span: merged,
                };
            }
        }
    }

    Ok(expr)
}

fn lower_comparison_tail(
    tail: Pair<Rule>,
    tail_span: Span,
) -> Result<(BinaryOp, Option<Expr>), ParseError> {
    let mut parts = tail.into_inner();
    let Some(first) = parts.next() else {
        return Err(ParseError::new(
            "invalid comparison tail",
            tail_span.start,
            tail_span.end,
        ));
    };

    let (op, rhs) = match first.as_rule() {
        Rule::comparison_op => {
            let op = match first.as_str() {
                "=" => BinaryOp::Eq,
                "<>" => BinaryOp::Ne,
                "<" => BinaryOp::Lt,
                ">" => BinaryOp::Gt,
                "<=" => BinaryOp::Le,
                ">=" => BinaryOp::Ge,
                _ => {
                    return Err(ParseError::new(
                        "unknown comparison operator",
                        tail_span.start,
                        tail_span.end,
                    ));
                }
            };
            (op, Some(lower_required_tail_expr(&mut parts, tail_span)?))
        }
        Rule::IN => (
            BinaryOp::In,
            Some(lower_required_tail_expr(&mut parts, tail_span)?),
        ),
        Rule::STARTS => {
            expect_tail_rule(&mut parts, Rule::WITH, tail_span)?;
            (
                BinaryOp::StartsWith,
                Some(lower_required_tail_expr(&mut parts, tail_span)?),
            )
        }
        Rule::ENDS => {
            expect_tail_rule(&mut parts, Rule::WITH, tail_span)?;
            (
                BinaryOp::EndsWith,
                Some(lower_required_tail_expr(&mut parts, tail_span)?),
            )
        }
        Rule::CONTAINS => (
            BinaryOp::Contains,
            Some(lower_required_tail_expr(&mut parts, tail_span)?),
        ),
        Rule::IS => match parts.next().map(|p| p.as_rule()) {
            Some(Rule::NULL) => (BinaryOp::IsNull, None),
            Some(Rule::NOT) => {
                expect_tail_rule(&mut parts, Rule::NULL, tail_span)?;
                (BinaryOp::IsNotNull, None)
            }
            _ => {
                return Err(ParseError::new(
                    "invalid comparison tail",
                    tail_span.start,
                    tail_span.end,
                ));
            }
        },
        Rule::regex_match => (
            BinaryOp::RegexMatch,
            Some(lower_required_tail_expr(&mut parts, tail_span)?),
        ),
        _ => {
            return Err(ParseError::new(
                "invalid comparison tail",
                tail_span.start,
                tail_span.end,
            ));
        }
    };

    if parts.next().is_some() {
        return Err(ParseError::new(
            "invalid comparison tail",
            tail_span.start,
            tail_span.end,
        ));
    }

    Ok((op, rhs))
}

fn lower_required_tail_expr(
    parts: &mut pest::iterators::Pairs<'_, Rule>,
    tail_span: Span,
) -> Result<Expr, ParseError> {
    let expr = parts
        .next()
        .ok_or_else(|| ParseError::new("expected expression", tail_span.start, tail_span.end))?;
    lower_expression(expr)
}

fn expect_tail_rule(
    parts: &mut pest::iterators::Pairs<'_, Rule>,
    expected: Rule,
    tail_span: Span,
) -> Result<(), ParseError> {
    let Some(next) = parts.next() else {
        return Err(ParseError::new(
            "invalid comparison tail",
            tail_span.start,
            tail_span.end,
        ));
    };
    if next.as_rule() == expected {
        Ok(())
    } else {
        Err(ParseError::new(
            "invalid comparison tail",
            tail_span.start,
            tail_span.end,
        ))
    }
}

pub(super) fn lower_add_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let first = inner
        .next()
        .ok_or_else(|| ParseError::new("expected lhs", span.start, span.end))?;
    let mut expr = lower_expression(first)?;

    while let Some(op_pair) = inner.next() {
        let rhs_pair = inner
            .next()
            .ok_or_else(|| ParseError::new("expected rhs", span.start, span.end))?;
        let op = match op_pair.as_rule() {
            Rule::add => BinaryOp::Add,
            Rule::sub => BinaryOp::Sub,
            _ => return Err(unexpected_rule("add/sub op", op_pair)),
        };

        let rhs = lower_expression(rhs_pair)?;
        let merged = merge_spans(expr.span(), rhs.span());
        expr = Expr::Binary {
            lhs: Box::new(expr),
            op,
            rhs: Box::new(rhs),
            span: merged,
        };
    }

    Ok(expr)
}

pub(super) fn lower_mul_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let first = inner
        .next()
        .ok_or_else(|| ParseError::new("expected lhs", span.start, span.end))?;
    let mut expr = lower_expression(first)?;

    while let Some(op_pair) = inner.next() {
        let rhs_pair = inner
            .next()
            .ok_or_else(|| ParseError::new("expected rhs", span.start, span.end))?;
        let op = match op_pair.as_rule() {
            Rule::mul => BinaryOp::Mul,
            Rule::div => BinaryOp::Div,
            Rule::modulo => BinaryOp::Mod,
            _ => return Err(unexpected_rule("mul/div/mod op", op_pair)),
        };

        let rhs = lower_expression(rhs_pair)?;
        let merged = merge_spans(expr.span(), rhs.span());
        expr = Expr::Binary {
            lhs: Box::new(expr),
            op,
            rhs: Box::new(rhs),
            span: merged,
        };
    }

    Ok(expr)
}

pub(super) fn lower_pow_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let first = inner
        .next()
        .ok_or_else(|| ParseError::new("expected lhs", span.start, span.end))?;
    let mut expr = lower_expression(first)?;

    while let Some(_pow) = inner.next() {
        let rhs_pair = inner
            .next()
            .ok_or_else(|| ParseError::new("expected rhs", span.start, span.end))?;
        let rhs = lower_expression(rhs_pair)?;
        let merged = merge_spans(expr.span(), rhs.span());
        expr = Expr::Binary {
            lhs: Box::new(expr),
            op: BinaryOp::Pow,
            rhs: Box::new(rhs),
            span: merged,
        };
    }

    Ok(expr)
}

pub(super) fn lower_unary_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut ops = Vec::new();
    let mut tail = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::add => ops.push(UnaryOp::Pos),
            Rule::sub => ops.push(UnaryOp::Neg),
            _ => tail = Some(p),
        }
    }

    let mut expr = lower_expression(
        tail.ok_or_else(|| ParseError::new("expected expression", span.start, span.end))?,
    )?;

    for op in ops.into_iter().rev() {
        expr = Expr::Unary {
            op,
            expr: Box::new(expr),
            span,
        };
    }

    Ok(expr)
}

pub(super) fn lower_postfix_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();

    let atom = inner
        .next()
        .ok_or_else(|| ParseError::new("expected atom", span.start, span.end))?;

    let mut expr = lower_expression(atom)?;

    for p in inner {
        match p.as_rule() {
            Rule::postfix_op => {
                let inner_pair = single_inner(p)?;
                match inner_pair.as_rule() {
                    Rule::property_lookup => {
                        let p_span = pair_span(&inner_pair);
                        let key_pair = inner_pair
                            .into_inner()
                            .find(|q| q.as_rule() == Rule::property_key_name)
                            .ok_or_else(|| {
                                ParseError::new("expected property key", p_span.start, p_span.end)
                            })?;
                        let key = lower_schema_name(key_pair)?;
                        let merged = merge_spans(expr.span(), p_span);
                        expr = Expr::Property {
                            expr: Box::new(expr),
                            key,
                            span: merged,
                        };
                    }
                    Rule::map_projection_postfix => {
                        let mp_span = pair_span(&inner_pair);
                        let mut selectors = Vec::new();
                        for sel_pair in inner_pair.into_inner() {
                            if sel_pair.as_rule() == Rule::map_projection_selector {
                                selectors.push(lower_map_projection_selector(sel_pair)?);
                            }
                        }
                        let merged = merge_spans(expr.span(), mp_span);
                        expr = Expr::MapProjection {
                            base: Box::new(expr),
                            selectors,
                            span: merged,
                        };
                    }
                    Rule::index_or_slice => {
                        let is_span = pair_span(&inner_pair);
                        let inner_op = single_inner(inner_pair)?;
                        match inner_op.as_rule() {
                            Rule::slice_op => {
                                let mut from_expr = None;
                                let mut to_expr = None;
                                let mut seen_dots = false;
                                for p in inner_op.into_inner() {
                                    if p.as_rule() == Rule::slice_dots {
                                        seen_dots = true;
                                    } else if p.as_rule() == Rule::expression {
                                        if !seen_dots {
                                            from_expr = Some(Box::new(lower_expression(p)?));
                                        } else {
                                            to_expr = Some(Box::new(lower_expression(p)?));
                                        }
                                    }
                                }
                                let merged = merge_spans(expr.span(), is_span);
                                expr = Expr::Slice {
                                    expr: Box::new(expr),
                                    from: from_expr,
                                    to: to_expr,
                                    span: merged,
                                };
                            }
                            Rule::index_op => {
                                let idx_pair = inner_op
                                    .into_inner()
                                    .find(|p| p.as_rule() == Rule::expression);
                                if let Some(idx) = idx_pair {
                                    let merged = merge_spans(expr.span(), is_span);
                                    expr = Expr::Index {
                                        expr: Box::new(expr),
                                        index: Box::new(lower_expression(idx)?),
                                        span: merged,
                                    };
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Rule::property_lookup => {
                let p_span = pair_span(&p);
                let key_pair = p
                    .into_inner()
                    .find(|q| q.as_rule() == Rule::property_key_name)
                    .ok_or_else(|| {
                        ParseError::new("expected property key", p_span.start, p_span.end)
                    })?;
                let key = lower_schema_name(key_pair)?;
                let merged = merge_spans(expr.span(), p_span);
                expr = Expr::Property {
                    expr: Box::new(expr),
                    key,
                    span: merged,
                };
            }
            _ => {}
        }
    }

    Ok(expr)
}

pub(super) fn lower_map_projection_selector(
    pair: Pair<Rule>,
) -> Result<MapProjectionSelector, ParseError> {
    let span = pair_span(&pair);
    let mut parts = pair.into_inner();
    let Some(first) = parts.next() else {
        return Err(ParseError::new(
            "invalid map projection selector",
            span.start,
            span.end,
        ));
    };

    match first.as_rule() {
        Rule::STAR if parts.next().is_none() => Ok(MapProjectionSelector::AllProperties),
        Rule::property_key_name => {
            let key = lower_schema_name(first)?;
            match parts.next() {
                None => Ok(MapProjectionSelector::Property(key)),
                Some(expr) if expr.as_rule() == Rule::expression && parts.next().is_none() => {
                    Ok(MapProjectionSelector::Literal(key, lower_expression(expr)?))
                }
                _ => Err(ParseError::new(
                    "invalid map projection selector",
                    span.start,
                    span.end,
                )),
            }
        }
        _ => Err(ParseError::new(
            "invalid map projection selector",
            span.start,
            span.end,
        )),
    }
}

pub(super) fn lower_simple_case_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();

    let first = inner
        .next()
        .ok_or_else(|| ParseError::new("expected CASE", span.start, span.end))?;
    if first.as_rule() != Rule::CASE {
        return Err(unexpected_rule("CASE", first));
    }

    let input_pair = inner
        .next()
        .ok_or_else(|| ParseError::new("expected CASE input expression", span.start, span.end))?;
    let input = lower_expression(input_pair)?;

    let mut alternatives = Vec::new();
    let mut else_expr = None;

    while let Some(p) = inner.next() {
        match p.as_rule() {
            Rule::WHEN => {
                let when_expr = inner
                    .next()
                    .ok_or_else(|| {
                        ParseError::new("expected WHEN expression", span.start, span.end)
                    })
                    .and_then(lower_expression)?;

                let then_kw = inner
                    .next()
                    .ok_or_else(|| ParseError::new("expected THEN", span.start, span.end))?;
                if then_kw.as_rule() != Rule::THEN {
                    return Err(unexpected_rule("THEN", then_kw));
                }

                let then_expr = inner
                    .next()
                    .ok_or_else(|| {
                        ParseError::new("expected THEN expression", span.start, span.end)
                    })
                    .and_then(lower_expression)?;

                alternatives.push((when_expr, then_expr));
            }
            Rule::ELSE => {
                let expr = inner
                    .next()
                    .ok_or_else(|| {
                        ParseError::new("expected ELSE expression", span.start, span.end)
                    })
                    .and_then(lower_expression)?;
                else_expr = Some(Box::new(expr));
            }
            Rule::END => {}
            _ => {}
        }
    }

    Ok(Expr::Case {
        input: Some(Box::new(input)),
        alternatives,
        else_expr,
        span,
    })
}

pub(super) fn lower_generic_case_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();

    let mut alternatives = Vec::new();
    let mut else_expr = None;

    while let Some(p) = inner.next() {
        match p.as_rule() {
            Rule::WHEN => {
                let when_expr = inner
                    .next()
                    .ok_or_else(|| {
                        ParseError::new("expected WHEN expression", span.start, span.end)
                    })
                    .and_then(lower_expression)?;

                let then_kw = inner
                    .next()
                    .ok_or_else(|| ParseError::new("expected THEN", span.start, span.end))?;
                if then_kw.as_rule() != Rule::THEN {
                    return Err(unexpected_rule("THEN", then_kw));
                }

                let then_expr = inner
                    .next()
                    .ok_or_else(|| {
                        ParseError::new("expected THEN expression", span.start, span.end)
                    })
                    .and_then(lower_expression)?;

                alternatives.push((when_expr, then_expr));
            }
            Rule::ELSE => {
                let expr = inner
                    .next()
                    .ok_or_else(|| {
                        ParseError::new("expected ELSE expression", span.start, span.end)
                    })
                    .and_then(lower_expression)?;
                else_expr = Some(Box::new(expr));
            }
            Rule::END => {}
            _ => {}
        }
    }

    Ok(Expr::Case {
        input: None,
        alternatives,
        else_expr,
        span,
    })
}
