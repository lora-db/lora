use super::expressions::lower_expression;
use super::literals::{lower_name_parts, lower_schema_name, lower_symbolic_name, lower_variable};
use super::patterns::{lower_node_labels, lower_pattern, lower_pattern_part};
use super::util::{merge_spans, pair_span};
use super::Rule;
use crate::errors::ParseError;
use lora_ast::*;
use pest::iterators::Pair;

pub(super) fn lower_match(pair: Pair<Rule>) -> Result<Match, ParseError> {
    let span = pair_span(&pair);
    let mut optional = false;
    let mut pattern = None;
    let mut where_ = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::OPTIONAL => optional = true,
            Rule::pattern => pattern = Some(lower_pattern(p)?),
            Rule::where_clause => where_ = Some(lower_where_clause(p)?),
            _ => {}
        }
    }

    Ok(Match {
        optional,
        pattern: pattern
            .ok_or_else(|| ParseError::new("expected pattern", span.start, span.end))?,
        where_,
        span,
    })
}

pub(super) fn lower_unwind(pair: Pair<Rule>) -> Result<Unwind, ParseError> {
    let span = pair_span(&pair);
    let mut expr = None;
    let mut alias = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::expression => expr = Some(lower_expression(p)?),
            Rule::variable => alias = Some(lower_variable(p)?),
            _ => {}
        }
    }

    Ok(Unwind {
        expr: expr.ok_or_else(|| ParseError::new("expected expression", span.start, span.end))?,
        alias: alias.ok_or_else(|| ParseError::new("expected alias", span.start, span.end))?,
        span,
    })
}

pub(super) fn lower_create(pair: Pair<Rule>) -> Result<Create, ParseError> {
    let span = pair_span(&pair);
    let pattern = pair
        .into_inner()
        .find(|p| p.as_rule() == Rule::pattern)
        .ok_or_else(|| ParseError::new("expected pattern", span.start, span.end))?;

    Ok(Create {
        pattern: lower_pattern(pattern)?,
        span,
    })
}

pub(super) fn lower_merge(pair: Pair<Rule>) -> Result<Merge, ParseError> {
    let span = pair_span(&pair);
    let mut pattern_part = None;
    let mut actions = Vec::new();

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::pattern_part => pattern_part = Some(lower_pattern_part(p)?),
            Rule::merge_action => actions.push(lower_merge_action(p)?),
            _ => {}
        }
    }

    Ok(Merge {
        pattern_part: pattern_part
            .ok_or_else(|| ParseError::new("expected pattern part", span.start, span.end))?,
        actions,
        span,
    })
}

pub(super) fn lower_merge_action(pair: Pair<Rule>) -> Result<MergeAction, ParseError> {
    let span = pair_span(&pair);
    let mut on_match = false;
    let mut set = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::MATCH => on_match = true,
            Rule::CREATE => on_match = false,
            Rule::set_clause => set = Some(lower_set(p)?),
            _ => {}
        }
    }

    Ok(MergeAction {
        on_match,
        set: set.ok_or_else(|| ParseError::new("expected SET clause", span.start, span.end))?,
        span,
    })
}

pub(super) fn lower_delete(pair: Pair<Rule>) -> Result<Delete, ParseError> {
    let span = pair_span(&pair);
    let mut detach = false;
    let mut expressions = Vec::new();

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::DETACH => detach = true,
            Rule::expression => expressions.push(lower_expression(p)?),
            _ => {}
        }
    }

    Ok(Delete {
        detach,
        expressions,
        span,
    })
}

pub(super) fn lower_set(pair: Pair<Rule>) -> Result<Set, ParseError> {
    let span = pair_span(&pair);
    let mut items = Vec::new();

    for p in pair.into_inner() {
        if p.as_rule() == Rule::set_item {
            items.push(lower_set_item(p)?);
        }
    }

    Ok(Set { items, span })
}
pub(super) fn lower_set_item(pair: Pair<Rule>) -> Result<SetItem, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let Some(first) = inner.next() else {
        return Err(ParseError::new("invalid SET item", span.start, span.end));
    };
    let second = inner.next();
    let third = inner.next();
    if inner.next().is_some() {
        return Err(ParseError::new("invalid SET item", span.start, span.end));
    }

    match (first, second, third) {
        (var, Some(labels), None)
            if var.as_rule() == Rule::variable && labels.as_rule() == Rule::node_labels =>
        {
            let variable = lower_variable(var)?;
            let labels: Vec<String> = lower_node_labels(labels)?
                .into_iter()
                .flat_map(|g| g.into_iter())
                .collect();
            Ok(SetItem::SetLabels {
                variable,
                labels,
                span,
            })
        }

        (var, Some(op), Some(value))
            if var.as_rule() == Rule::variable
                && op.as_rule() == Rule::plus_eq
                && value.as_rule() == Rule::expression =>
        {
            let variable = lower_variable(var)?;
            let value = lower_expression(value)?;
            Ok(SetItem::MutateVariable {
                variable,
                value,
                span,
            })
        }

        (var, Some(op), Some(value))
            if var.as_rule() == Rule::variable
                && op.as_rule() == Rule::eq
                && value.as_rule() == Rule::expression =>
        {
            let variable = lower_variable(var)?;
            let value = lower_expression(value)?;
            Ok(SetItem::SetVariable {
                variable,
                value,
                span,
            })
        }

        (target, Some(op), Some(value))
            if target.as_rule() == Rule::property_set_target
                && op.as_rule() == Rule::eq
                && value.as_rule() == Rule::expression =>
        {
            let target = lower_property_set_target(target)?;
            let value = lower_expression(value)?;
            Ok(SetItem::SetProperty {
                target,
                value,
                span,
            })
        }

        _ => Err(ParseError::new("invalid SET item", span.start, span.end)),
    }
}

pub(super) fn lower_property_set_target(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();

    let first = inner
        .next()
        .ok_or_else(|| ParseError::new("expected variable", span.start, span.end))?;
    let mut expr = Expr::Variable(lower_variable(first)?);

    for p in inner {
        if p.as_rule() == Rule::property_lookup {
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
    }

    Ok(expr)
}

pub(super) fn lower_remove(pair: Pair<Rule>) -> Result<Remove, ParseError> {
    let span = pair_span(&pair);
    let mut items = Vec::new();

    for p in pair.into_inner() {
        if p.as_rule() == Rule::remove_item {
            items.push(lower_remove_item(p)?);
        }
    }

    Ok(Remove { items, span })
}

pub(super) fn lower_remove_item(pair: Pair<Rule>) -> Result<RemoveItem, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let Some(first) = inner.next() else {
        return Err(ParseError::new("invalid REMOVE item", span.start, span.end));
    };
    let second = inner.next();
    if inner.next().is_some() {
        return Err(ParseError::new("invalid REMOVE item", span.start, span.end));
    }

    match (first, second) {
        (variable, Some(labels))
            if variable.as_rule() == Rule::variable && labels.as_rule() == Rule::node_labels =>
        {
            let variable = lower_variable(variable)?;
            let labels: Vec<String> = lower_node_labels(labels)?
                .into_iter()
                .flat_map(|g| g.into_iter())
                .collect();
            Ok(RemoveItem::Labels {
                variable,
                labels,
                span,
            })
        }
        (expr, None) => Ok(RemoveItem::Property {
            expr: lower_expression(expr)?,
            span,
        }),
        _ => Err(ParseError::new("invalid REMOVE item", span.start, span.end)),
    }
}

pub(super) fn lower_in_query_call(pair: Pair<Rule>) -> Result<InQueryCall, ParseError> {
    let span = pair_span(&pair);
    let mut procedure = None;
    let mut yield_items = Vec::new();
    let mut where_ = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::procedure_invocation => procedure = Some(lower_procedure_invocation(p)?),
            Rule::yield_clause => {
                let (items, _all) = lower_yield_clause(p)?;
                yield_items = items;
            }
            Rule::where_clause => where_ = Some(lower_where_clause(p)?),
            _ => {}
        }
    }

    Ok(InQueryCall {
        procedure: procedure.ok_or_else(|| {
            ParseError::new("expected procedure invocation", span.start, span.end)
        })?,
        yield_items,
        where_,
        span,
    })
}

pub(super) fn lower_standalone_call(pair: Pair<Rule>) -> Result<StandaloneCall, ParseError> {
    let span = pair_span(&pair);
    let mut procedure = None;
    let mut yield_items = Vec::new();
    let mut yield_all = false;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::procedure_invocation => {
                procedure = Some(ProcedureInvocationKind::Explicit(
                    lower_procedure_invocation(p)?,
                ));
            }
            Rule::procedure_name => {
                procedure = Some(ProcedureInvocationKind::Implicit(lower_procedure_name(p)?));
            }
            Rule::yield_clause => {
                let (items, all) = lower_yield_clause(p)?;
                yield_items = items;
                yield_all = all;
            }
            _ => {}
        }
    }

    Ok(StandaloneCall {
        procedure: procedure
            .ok_or_else(|| ParseError::new("expected procedure", span.start, span.end))?,
        yield_items,
        yield_all,
        span,
    })
}

pub(super) fn lower_procedure_invocation(
    pair: Pair<Rule>,
) -> Result<ProcedureInvocation, ParseError> {
    let span = pair_span(&pair);
    let mut name = None;
    let mut args = Vec::new();

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::procedure_name => name = Some(lower_procedure_name(p)?),
            Rule::expression => args.push(lower_expression(p)?),
            _ => {}
        }
    }

    Ok(ProcedureInvocation {
        name: name
            .ok_or_else(|| ParseError::new("expected procedure name", span.start, span.end))?,
        args,
        span,
    })
}

pub(super) fn lower_procedure_name(pair: Pair<Rule>) -> Result<ProcedureName, ParseError> {
    let span = pair_span(&pair);
    let parts = lower_name_parts(pair)?;
    Ok(ProcedureName { parts, span })
}

pub(super) fn lower_yield_clause(pair: Pair<Rule>) -> Result<(Vec<YieldItem>, bool), ParseError> {
    let mut items = Vec::new();
    let mut all = false;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::STAR => all = true,
            Rule::yield_items => {
                for q in p.into_inner() {
                    if q.as_rule() == Rule::yield_item {
                        items.push(lower_yield_item(q)?);
                    }
                }
            }
            _ => {}
        }
    }

    Ok((items, all))
}

pub(super) fn lower_yield_item(pair: Pair<Rule>) -> Result<YieldItem, ParseError> {
    let span = pair_span(&pair);
    let mut symbolic = None;
    let mut alias = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::symbolic_name => symbolic = Some(lower_symbolic_name(p)?),
            Rule::variable => alias = Some(lower_variable(p)?),
            _ => {}
        }
    }

    match (symbolic, alias) {
        (Some(field), Some(alias)) => Ok(YieldItem {
            field: Some(field),
            alias,
            span,
        }),
        (Some(name), None) => Ok(YieldItem {
            field: None,
            alias: Variable { name, span },
            span,
        }),
        _ => Err(ParseError::new("invalid YIELD item", span.start, span.end)),
    }
}

pub(super) fn lower_with_clause(pair: Pair<Rule>) -> Result<With, ParseError> {
    let span = pair_span(&pair);
    let mut body = None;
    let mut where_ = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::projection_body => body = Some(lower_projection_body(p)?),
            Rule::where_clause => where_ = Some(lower_where_clause(p)?),
            _ => {}
        }
    }

    Ok(With {
        body: body
            .ok_or_else(|| ParseError::new("expected projection body", span.start, span.end))?,
        where_,
        span,
    })
}

pub(super) fn lower_where_clause(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);

    let expr = pair
        .into_inner()
        .find(|p| p.as_rule() == Rule::expression)
        .ok_or_else(|| ParseError::new("expected expression", span.start, span.end))?;

    lower_expression(expr)
}

pub(super) fn lower_return_clause(pair: Pair<Rule>) -> Result<Return, ParseError> {
    let span = pair_span(&pair);
    let body = pair
        .into_inner()
        .find(|p| p.as_rule() == Rule::projection_body)
        .ok_or_else(|| ParseError::new("expected projection body", span.start, span.end))?;

    Ok(Return {
        body: lower_projection_body(body)?,
        span,
    })
}

pub(super) fn lower_projection_body(pair: Pair<Rule>) -> Result<ProjectionBody, ParseError> {
    let span = pair_span(&pair);
    let mut distinct = false;
    let mut items = Vec::new();
    let mut order = Vec::new();
    let mut skip = None;
    let mut limit = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::DISTINCT => distinct = true,
            Rule::projection_items => items = lower_projection_items(p)?,
            Rule::order_clause => order = lower_order_clause(p)?,
            Rule::skip_clause => {
                let expr = p
                    .into_inner()
                    .find(|q| q.as_rule() == Rule::expression)
                    .ok_or_else(|| {
                        ParseError::new("expected skip expression", span.start, span.end)
                    })?;
                skip = Some(lower_expression(expr)?);
            }
            Rule::limit_clause => {
                let expr = p
                    .into_inner()
                    .find(|q| q.as_rule() == Rule::expression)
                    .ok_or_else(|| {
                        ParseError::new("expected limit expression", span.start, span.end)
                    })?;
                limit = Some(lower_expression(expr)?);
            }
            _ => {}
        }
    }

    Ok(ProjectionBody {
        distinct,
        items,
        order,
        skip,
        limit,
        span,
    })
}

pub(super) fn lower_projection_items(pair: Pair<Rule>) -> Result<Vec<ProjectionItem>, ParseError> {
    let mut out = Vec::new();
    for p in pair.into_inner() {
        if p.as_rule() == Rule::projection_item {
            out.push(lower_projection_item(p)?);
        }
    }
    Ok(out)
}

pub(super) fn lower_projection_item(pair: Pair<Rule>) -> Result<ProjectionItem, ParseError> {
    let span = pair_span(&pair);
    let mut expr = None;
    let mut alias = None;
    let mut saw_star = false;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::star => saw_star = true,
            Rule::expression => expr = Some(lower_expression(p)?),
            Rule::variable => alias = Some(lower_variable(p)?),
            _ => {}
        }
    }

    if saw_star {
        return Ok(ProjectionItem::Star { span });
    }

    Ok(ProjectionItem::Expr {
        expr: expr.ok_or_else(|| ParseError::new("expected expression", span.start, span.end))?,
        alias,
        span,
    })
}

pub(super) fn lower_order_clause(pair: Pair<Rule>) -> Result<Vec<SortItem>, ParseError> {
    let mut out = Vec::new();
    for p in pair.into_inner() {
        if p.as_rule() == Rule::sort_item {
            out.push(lower_sort_item(p)?);
        }
    }
    Ok(out)
}

pub(super) fn lower_sort_item(pair: Pair<Rule>) -> Result<SortItem, ParseError> {
    let span = pair_span(&pair);
    let mut expr = None;
    let mut direction = SortDirection::Asc;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::expression => expr = Some(lower_expression(p)?),
            Rule::DESC | Rule::DESCENDING => direction = SortDirection::Desc,
            Rule::ASC | Rule::ASCENDING => direction = SortDirection::Asc,
            _ => {}
        }
    }

    Ok(SortItem {
        expr: expr.ok_or_else(|| ParseError::new("expected expression", span.start, span.end))?,
        direction,
        span,
    })
}
