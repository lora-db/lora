use super::expressions::lower_expression;
use super::patterns::{lower_pattern, lower_pattern_element};
use super::util::{pair_span, single_inner, unexpected_rule};
use super::Rule;
use crate::errors::ParseError;
use lora_ast::*;
use pest::iterators::Pair;

pub(super) fn lower_literal(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    match pair.as_rule() {
        Rule::number_literal => lower_literal(single_inner(pair)?),
        Rule::integer_literal => {
            let v = lower_integer_literal(pair.clone())?;
            Ok(Expr::Integer(v, pair_span(&pair)))
        }
        Rule::double_literal => {
            let s = pair.as_str();
            let v: f64 = s.parse().map_err(|_| {
                ParseError::new(
                    "invalid float",
                    pair.as_span().start(),
                    pair.as_span().end(),
                )
            })?;
            Ok(Expr::Float(v, pair_span(&pair)))
        }
        Rule::string_literal => lower_string_literal(pair),
        Rule::boolean_literal => {
            let s = pair.as_str();
            Ok(Expr::Bool(s.eq_ignore_ascii_case("true"), pair_span(&pair)))
        }
        Rule::null_literal => Ok(Expr::Null(pair_span(&pair))),
        Rule::map_literal => lower_map_literal(pair),
        Rule::list_literal => lower_list_literal(pair),
        _ => Err(unexpected_rule("literal", pair)),
    }
}

pub(super) fn lower_string_literal(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let inner_pair = single_inner(pair)?;
    let raw = inner_pair.as_str();

    let inner = &raw[1..raw.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let esc = chars
                .next()
                .ok_or_else(|| ParseError::new("invalid escape", span.start, span.end))?;
            match esc {
                '\\' => out.push('\\'),
                '\'' => out.push('\''),
                '"' => out.push('"'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                other => out.push(other),
            }
        } else {
            out.push(ch);
        }
    }

    Ok(Expr::String(out, span))
}

pub(super) fn lower_map_literal(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut items = Vec::new();
    let mut key: Option<String> = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::property_key_name => key = Some(lower_schema_name(p)?),
            Rule::expression => {
                let k = key
                    .take()
                    .ok_or_else(|| ParseError::new("expected map key", span.start, span.end))?;
                items.push((k, lower_expression(p)?));
            }
            _ => {}
        }
    }

    Ok(Expr::Map(items, span))
}

pub(super) fn lower_list_literal(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let inner: Vec<_> = pair.into_inner().collect();

    if inner.len() == 1 && inner[0].as_rule() == Rule::list_comprehension {
        return lower_list_comprehension(inner.into_iter().next().unwrap(), span);
    }

    if inner.len() == 1 && inner[0].as_rule() == Rule::pattern_comprehension {
        return lower_pattern_comprehension(inner.into_iter().next().unwrap(), span);
    }

    let mut items = Vec::new();
    for p in inner {
        if p.as_rule() == Rule::expression {
            items.push(lower_expression(p)?);
        }
    }

    Ok(Expr::List(items, span))
}

pub(super) fn lower_pattern_comprehension(
    pair: Pair<Rule>,
    outer_span: Span,
) -> Result<Expr, ParseError> {
    let mut pattern = None;
    let mut where_ = None;
    let mut map_expr = None;

    let mut inner = pair.into_inner().peekable();
    while let Some(p) = inner.next() {
        match p.as_rule() {
            Rule::pattern_element => {
                pattern = Some(lower_pattern_element(p)?);
            }
            Rule::WHERE => {
                let expr_pair = inner.next().ok_or_else(|| {
                    ParseError::new(
                        "expected WHERE expression",
                        outer_span.start,
                        outer_span.end,
                    )
                })?;
                where_ = Some(Box::new(lower_expression(expr_pair)?));
            }
            Rule::expression => {
                map_expr = Some(Box::new(lower_expression(p)?));
            }
            _ => {}
        }
    }

    Ok(Expr::PatternComprehension {
        pattern: Box::new(pattern.ok_or_else(|| {
            ParseError::new(
                "expected pattern in comprehension",
                outer_span.start,
                outer_span.end,
            )
        })?),
        where_,
        map_expr: map_expr.ok_or_else(|| {
            ParseError::new(
                "expected map expression after |",
                outer_span.start,
                outer_span.end,
            )
        })?,
        span: outer_span,
    })
}

pub(super) fn lower_list_comprehension(
    pair: Pair<Rule>,
    outer_span: Span,
) -> Result<Expr, ParseError> {
    let mut inner = pair.into_inner();

    let var_pair = inner
        .next()
        .ok_or_else(|| ParseError::new("expected variable", outer_span.start, outer_span.end))?;
    let variable = lower_variable(var_pair)?;

    // skip IN
    let _in_kw = inner.next();

    let list_pair = inner.next().ok_or_else(|| {
        ParseError::new("expected list expression", outer_span.start, outer_span.end)
    })?;
    let list = lower_expression(list_pair)?;

    let mut filter = None;
    let mut map_expr = None;

    // Remaining tokens: optional WHERE expr, optional | expr
    while let Some(p) = inner.next() {
        match p.as_rule() {
            Rule::WHERE => {
                let filter_pair = inner.next().ok_or_else(|| {
                    ParseError::new(
                        "expected filter expression",
                        outer_span.start,
                        outer_span.end,
                    )
                })?;
                filter = Some(Box::new(lower_expression(filter_pair)?));
            }
            Rule::pipe => {
                let map_pair = inner.next().ok_or_else(|| {
                    ParseError::new("expected map expression", outer_span.start, outer_span.end)
                })?;
                map_expr = Some(Box::new(lower_expression(map_pair)?));
            }
            _ if p.as_rule() == Rule::expression => {
                // This is the map expression after pipe was consumed silently
                map_expr = Some(Box::new(lower_expression(p)?));
            }
            _ => {}
        }
    }

    Ok(Expr::ListComprehension {
        variable,
        list: Box::new(list),
        filter,
        map_expr,
        span: outer_span,
    })
}

pub(super) fn lower_parameter(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let raw = pair.as_str();
    Ok(Expr::Parameter(raw[1..].to_string(), span))
}

pub(super) fn lower_list_predicate(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();

    let kind_pair = inner
        .next()
        .ok_or_else(|| ParseError::new("expected list predicate kind", span.start, span.end))?;
    let kind = match kind_pair.as_rule() {
        Rule::ANY_ => ListPredicateKind::Any,
        Rule::ALL => ListPredicateKind::All,
        Rule::NONE => ListPredicateKind::None,
        Rule::SINGLE => ListPredicateKind::Single,
        _ => {
            return Err(ParseError::new(
                "expected ANY/ALL/NONE/SINGLE",
                span.start,
                span.end,
            ))
        }
    };

    let var_pair = inner
        .next()
        .ok_or_else(|| ParseError::new("expected variable", span.start, span.end))?;
    let variable = lower_variable(var_pair)?;

    // skip IN keyword
    let _in_kw = inner.next();

    let list_pair = inner
        .next()
        .ok_or_else(|| ParseError::new("expected list expression", span.start, span.end))?;
    let list = lower_expression(list_pair)?;

    // skip WHERE keyword
    let _where_kw = inner.next();

    let pred_pair = inner
        .next()
        .ok_or_else(|| ParseError::new("expected predicate expression", span.start, span.end))?;
    let predicate = lower_expression(pred_pair)?;

    Ok(Expr::ListPredicate {
        kind,
        variable,
        list: Box::new(list),
        predicate: Box::new(predicate),
        span,
    })
}

pub(super) fn lower_reduce_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();

    // skip REDUCE keyword
    let _reduce_kw = inner.next();

    let acc_pair = inner
        .next()
        .ok_or_else(|| ParseError::new("expected accumulator variable", span.start, span.end))?;
    let accumulator = lower_variable(acc_pair)?;

    // skip = (eq)
    let _eq = inner.next();

    let init_pair = inner
        .next()
        .ok_or_else(|| ParseError::new("expected init expression", span.start, span.end))?;
    let init = lower_expression(init_pair)?;

    // skip comma (already consumed by pest)

    let var_pair = inner
        .next()
        .ok_or_else(|| ParseError::new("expected variable", span.start, span.end))?;
    let variable = lower_variable(var_pair)?;

    // skip IN
    let _in_kw = inner.next();

    let list_pair = inner
        .next()
        .ok_or_else(|| ParseError::new("expected list expression", span.start, span.end))?;
    let list = lower_expression(list_pair)?;

    // skip pipe (already consumed by pest)

    let expr_pair = inner
        .next()
        .ok_or_else(|| ParseError::new("expected reduce expression", span.start, span.end))?;
    let expr = lower_expression(expr_pair)?;

    Ok(Expr::Reduce {
        accumulator,
        init: Box::new(init),
        variable,
        list: Box::new(list),
        expr: Box::new(expr),
        span,
    })
}

pub(super) fn lower_exists_subquery(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut pattern = None;
    let mut where_ = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::pattern => pattern = Some(lower_pattern(p)?),
            Rule::where_clause => {
                let inner = p
                    .into_inner()
                    .find(|q| q.as_rule() == Rule::expression)
                    .ok_or_else(|| ParseError::new("expected expression", span.start, span.end))?;
                where_ = Some(Box::new(lower_expression(inner)?));
            }
            _ => {}
        }
    }

    Ok(Expr::ExistsSubquery {
        pattern: pattern
            .ok_or_else(|| ParseError::new("expected pattern in EXISTS", span.start, span.end))?,
        where_,
        span,
    })
}

pub(super) fn lower_function_invocation(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut name = Vec::new();
    let mut distinct = false;
    let mut args = Vec::new();
    let mut star = false;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::function_name => name = lower_name_parts(p)?,
            Rule::DISTINCT => distinct = true,
            Rule::STAR => star = true,
            Rule::expression => args.push(lower_expression(p)?),
            _ => {}
        }
    }

    // count(*) is represented as a FunctionCall with empty args.
    // The executor already handles count with no args as count-star
    // (counting all rows regardless of null values).
    // Reject * for non-count functions.
    if star && !name.iter().any(|n| n.eq_ignore_ascii_case("count")) {
        return Err(ParseError::new(
            format!(
                "* is only valid as an argument to count(), not {}()",
                name.join(".")
            ),
            span.start,
            span.end,
        ));
    }

    Ok(Expr::FunctionCall {
        name,
        distinct,
        args,
        span,
    })
}

pub(super) fn lower_name_parts(pair: Pair<Rule>) -> Result<Vec<String>, ParseError> {
    let mut parts = Vec::new();
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::namespace => {
                for q in p.into_inner() {
                    if q.as_rule() == Rule::symbolic_name {
                        parts.push(lower_symbolic_name(q)?);
                    }
                }
            }
            Rule::symbolic_name => parts.push(lower_symbolic_name(p)?),
            _ => {}
        }
    }
    Ok(parts)
}

pub(super) fn lower_variable(pair: Pair<Rule>) -> Result<Variable, ParseError> {
    let span = pair_span(&pair);
    let name_pair = single_inner(pair)?;
    Ok(Variable {
        name: lower_symbolic_name(name_pair)?,
        span,
    })
}

pub(super) fn lower_symbolic_name(pair: Pair<Rule>) -> Result<String, ParseError> {
    match pair.as_rule() {
        Rule::symbolic_name => lower_symbolic_name(single_inner(pair)?),
        Rule::unescaped_symbolic_name => Ok(pair.as_str().to_string()),
        Rule::escaped_symbolic_name => {
            let s = pair.as_str();
            Ok(s[1..s.len() - 1].to_string())
        }
        Rule::COUNT | Rule::ANY_ | Rule::NONE | Rule::SINGLE => Ok(pair.as_str().to_string()),
        _ => Err(unexpected_rule("symbolic_name", pair)),
    }
}

pub(super) fn lower_schema_name(pair: Pair<Rule>) -> Result<String, ParseError> {
    match pair.as_rule() {
        Rule::schema_name => lower_schema_name(single_inner(pair)?),
        Rule::symbolic_name => lower_symbolic_name(pair),
        Rule::reserved_word => Ok(pair.as_str().to_string()),
        Rule::label_name | Rule::rel_type_name | Rule::property_key_name => {
            lower_schema_name(single_inner(pair)?)
        }
        _ => Err(unexpected_rule("schema_name", pair)),
    }
}

pub(super) fn lower_integer_literal(pair: Pair<Rule>) -> Result<i64, ParseError> {
    match pair.as_rule() {
        Rule::integer_literal => lower_integer_literal(single_inner(pair)?),
        Rule::decimal_integer => pair.as_str().parse::<i64>().map_err(|_| {
            ParseError::new(
                "invalid decimal integer",
                pair.as_span().start(),
                pair.as_span().end(),
            )
        }),
        Rule::hex_integer => i64::from_str_radix(&pair.as_str()[2..], 16).map_err(|_| {
            ParseError::new(
                "invalid hex integer",
                pair.as_span().start(),
                pair.as_span().end(),
            )
        }),
        Rule::octal_integer => i64::from_str_radix(&pair.as_str()[1..], 8).map_err(|_| {
            ParseError::new(
                "invalid octal integer",
                pair.as_span().start(),
                pair.as_span().end(),
            )
        }),
        _ => Err(unexpected_rule("integer_literal", pair)),
    }
}
