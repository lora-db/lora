use crate::error::ParseError;
use lora_ast::*;
use pest::iterators::Pair;
use pest::Parser;
use smallvec::SmallVec;

#[derive(pest_derive::Parser)]
#[grammar = "cypher.pest"]
struct LoraParser;

pub fn parse_query(input: &str) -> Result<Document, ParseError> {
    let mut pairs = LoraParser::parse(Rule::query, input)
        .map_err(|e| ParseError::new(e.to_string(), 0, input.len()))?;

    let pair = pairs
        .next()
        .ok_or_else(|| ParseError::new("expected query", 0, input.len()))?;

    lower_query(pair)
}

fn lower_query(pair: Pair<Rule>) -> Result<Document, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();

    let stmt = inner
        .find(|p| p.as_rule() == Rule::statement)
        .ok_or_else(|| ParseError::new("expected statement", span.start, span.end))?;

    Ok(Document {
        statement: lower_statement(stmt)?,
        span,
    })
}

fn lower_statement(pair: Pair<Rule>) -> Result<Statement, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::regular_query => Ok(Statement::Query(Query::Regular(lower_regular_query(
            inner,
        )?))),
        Rule::standalone_call => Ok(Statement::Query(Query::StandaloneCall(
            lower_standalone_call(inner)?,
        ))),
        _ => Err(unexpected_rule("statement", inner)),
    }
}

fn lower_regular_query(pair: Pair<Rule>) -> Result<RegularQuery, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();

    let first = inner
        .next()
        .ok_or_else(|| ParseError::new("expected single query", span.start, span.end))?;

    let head = lower_single_query(first)?;
    let mut unions = Vec::new();

    while let Some(union_pair) = inner.next() {
        let uq = inner
            .next()
            .ok_or_else(|| ParseError::new("expected query after UNION", span.start, span.end))?;

        let all = union_pair
            .clone()
            .into_inner()
            .any(|p| p.as_rule() == Rule::ALL);
        let union_span = Span::new(union_pair.as_span().start(), uq.as_span().end());

        unions.push(UnionPart {
            all,
            query: lower_single_query(uq)?,
            span: union_span,
        });
    }

    Ok(RegularQuery { head, unions, span })
}

fn lower_single_query(pair: Pair<Rule>) -> Result<SingleQuery, ParseError> {
    match pair.as_rule() {
        Rule::single_query => lower_single_query(single_inner(pair)?),
        Rule::single_part_query => Ok(SingleQuery::SinglePart(lower_single_part_query(pair)?)),
        Rule::multi_part_query => Ok(SingleQuery::MultiPart(lower_multi_part_query(pair)?)),
        _ => Err(unexpected_rule("single_query", pair)),
    }
}

fn lower_multi_part_query(pair: Pair<Rule>) -> Result<MultiPartQuery, ParseError> {
    let span = pair_span(&pair);
    let mut parts = Vec::new();
    let mut tail = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::query_part => parts.push(lower_query_part(p)?),
            Rule::single_part_query => tail = Some(lower_single_part_query(p)?),
            _ => {}
        }
    }

    Ok(MultiPartQuery {
        parts,
        tail: Box::new(tail.ok_or_else(|| {
            ParseError::new("expected single-part tail query", span.start, span.end)
        })?),
        span,
    })
}

fn lower_query_part(pair: Pair<Rule>) -> Result<QueryPart, ParseError> {
    let span = pair_span(&pair);
    let mut reading_clauses = Vec::new();
    let mut updating_clauses = Vec::new();
    let mut with_clause = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::reading_clause => reading_clauses.push(lower_reading_clause(p)?),
            Rule::updating_clause => updating_clauses.push(lower_updating_clause(p)?),
            Rule::with_clause => with_clause = Some(lower_with_clause(p)?),
            _ => {}
        }
    }

    Ok(QueryPart {
        reading_clauses,
        updating_clauses,
        with_clause: with_clause
            .ok_or_else(|| ParseError::new("expected WITH clause", span.start, span.end))?,
        span,
    })
}

fn lower_single_part_query(pair: Pair<Rule>) -> Result<SinglePartQuery, ParseError> {
    let span = pair_span(&pair);
    let mut reading_clauses = Vec::new();
    let mut updating_clauses = Vec::new();
    let mut return_clause = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::reading_clause => reading_clauses.push(lower_reading_clause(p)?),
            Rule::updating_clause => updating_clauses.push(lower_updating_clause(p)?),
            Rule::return_clause => return_clause = Some(lower_return_clause(p)?),
            _ => {}
        }
    }

    Ok(SinglePartQuery {
        reading_clauses,
        updating_clauses,
        return_clause,
        span,
    })
}

fn lower_reading_clause(pair: Pair<Rule>) -> Result<ReadingClause, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::match_clause => Ok(ReadingClause::Match(lower_match(inner)?)),
        Rule::unwind_clause => Ok(ReadingClause::Unwind(lower_unwind(inner)?)),
        Rule::in_query_call => Ok(ReadingClause::InQueryCall(lower_in_query_call(inner)?)),
        _ => Err(unexpected_rule("reading_clause", inner)),
    }
}

fn lower_updating_clause(pair: Pair<Rule>) -> Result<UpdatingClause, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::create_clause => Ok(UpdatingClause::Create(lower_create(inner)?)),
        Rule::merge_clause => Ok(UpdatingClause::Merge(lower_merge(inner)?)),
        Rule::delete_clause => Ok(UpdatingClause::Delete(lower_delete(inner)?)),
        Rule::set_clause => Ok(UpdatingClause::Set(lower_set(inner)?)),
        Rule::remove_clause => Ok(UpdatingClause::Remove(lower_remove(inner)?)),
        _ => Err(unexpected_rule("updating_clause", inner)),
    }
}

fn lower_match(pair: Pair<Rule>) -> Result<Match, ParseError> {
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

fn lower_unwind(pair: Pair<Rule>) -> Result<Unwind, ParseError> {
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

fn lower_create(pair: Pair<Rule>) -> Result<Create, ParseError> {
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

fn lower_merge(pair: Pair<Rule>) -> Result<Merge, ParseError> {
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

fn lower_merge_action(pair: Pair<Rule>) -> Result<MergeAction, ParseError> {
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

fn lower_delete(pair: Pair<Rule>) -> Result<Delete, ParseError> {
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

fn lower_set(pair: Pair<Rule>) -> Result<Set, ParseError> {
    let span = pair_span(&pair);
    let mut items = Vec::new();

    for p in pair.into_inner() {
        if p.as_rule() == Rule::set_item {
            items.push(lower_set_item(p)?);
        }
    }

    Ok(Set { items, span })
}
fn lower_set_item(pair: Pair<Rule>) -> Result<SetItem, ParseError> {
    let span = pair_span(&pair);
    let inner: Vec<_> = pair.into_inner().collect();

    match inner.as_slice() {
        [var, labels]
            if var.as_rule() == Rule::variable && labels.as_rule() == Rule::node_labels =>
        {
            let variable = lower_variable(var.clone())?;
            let labels: Vec<String> = lower_node_labels(labels.clone())?
                .into_iter()
                .flat_map(|g| g.into_iter())
                .collect();
            Ok(SetItem::SetLabels {
                variable,
                labels,
                span,
            })
        }

        [var, op, value]
            if var.as_rule() == Rule::variable
                && op.as_rule() == Rule::plus_eq
                && value.as_rule() == Rule::expression =>
        {
            let variable = lower_variable(var.clone())?;
            let value = lower_expression(value.clone())?;
            Ok(SetItem::MutateVariable {
                variable,
                value,
                span,
            })
        }

        [var, op, value]
            if var.as_rule() == Rule::variable
                && op.as_rule() == Rule::eq
                && value.as_rule() == Rule::expression =>
        {
            let variable = lower_variable(var.clone())?;
            let value = lower_expression(value.clone())?;
            Ok(SetItem::SetVariable {
                variable,
                value,
                span,
            })
        }

        [target, op, value]
            if target.as_rule() == Rule::property_set_target
                && op.as_rule() == Rule::eq
                && value.as_rule() == Rule::expression =>
        {
            let target = lower_property_set_target(target.clone())?;
            let value = lower_expression(value.clone())?;
            Ok(SetItem::SetProperty {
                target,
                value,
                span,
            })
        }

        _ => Err(ParseError::new("invalid SET item", span.start, span.end)),
    }
}

fn lower_property_set_target(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_remove(pair: Pair<Rule>) -> Result<Remove, ParseError> {
    let span = pair_span(&pair);
    let mut items = Vec::new();

    for p in pair.into_inner() {
        if p.as_rule() == Rule::remove_item {
            items.push(lower_remove_item(p)?);
        }
    }

    Ok(Remove { items, span })
}

fn lower_remove_item(pair: Pair<Rule>) -> Result<RemoveItem, ParseError> {
    let span = pair_span(&pair);
    let inner: Vec<_> = pair.into_inner().collect();

    if inner.len() == 2
        && inner[0].as_rule() == Rule::variable
        && inner[1].as_rule() == Rule::node_labels
    {
        let variable = lower_variable(inner[0].clone())?;
        let labels: Vec<String> = lower_node_labels(inner[1].clone())?
            .into_iter()
            .flat_map(|g| g.into_iter())
            .collect();
        return Ok(RemoveItem::Labels {
            variable,
            labels,
            span,
        });
    }

    if inner.len() == 1 {
        return Ok(RemoveItem::Property {
            expr: lower_expression(inner[0].clone())?,
            span,
        });
    }

    Err(ParseError::new("invalid REMOVE item", span.start, span.end))
}

fn lower_in_query_call(pair: Pair<Rule>) -> Result<InQueryCall, ParseError> {
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

fn lower_standalone_call(pair: Pair<Rule>) -> Result<StandaloneCall, ParseError> {
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

fn lower_procedure_invocation(pair: Pair<Rule>) -> Result<ProcedureInvocation, ParseError> {
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

fn lower_procedure_name(pair: Pair<Rule>) -> Result<ProcedureName, ParseError> {
    let span = pair_span(&pair);
    let parts = lower_name_parts(pair)?;
    Ok(ProcedureName { parts, span })
}

fn lower_yield_clause(pair: Pair<Rule>) -> Result<(Vec<YieldItem>, bool), ParseError> {
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

fn lower_yield_item(pair: Pair<Rule>) -> Result<YieldItem, ParseError> {
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

fn lower_with_clause(pair: Pair<Rule>) -> Result<With, ParseError> {
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

fn lower_where_clause(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);

    let expr = pair
        .into_inner()
        .find(|p| p.as_rule() == Rule::expression)
        .ok_or_else(|| ParseError::new("expected expression", span.start, span.end))?;

    lower_expression(expr)
}

fn lower_return_clause(pair: Pair<Rule>) -> Result<Return, ParseError> {
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

fn lower_projection_body(pair: Pair<Rule>) -> Result<ProjectionBody, ParseError> {
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

fn lower_projection_items(pair: Pair<Rule>) -> Result<Vec<ProjectionItem>, ParseError> {
    let mut out = Vec::new();
    for p in pair.into_inner() {
        if p.as_rule() == Rule::projection_item {
            out.push(lower_projection_item(p)?);
        }
    }
    Ok(out)
}

fn lower_projection_item(pair: Pair<Rule>) -> Result<ProjectionItem, ParseError> {
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

fn lower_order_clause(pair: Pair<Rule>) -> Result<Vec<SortItem>, ParseError> {
    let mut out = Vec::new();
    for p in pair.into_inner() {
        if p.as_rule() == Rule::sort_item {
            out.push(lower_sort_item(p)?);
        }
    }
    Ok(out)
}

fn lower_sort_item(pair: Pair<Rule>) -> Result<SortItem, ParseError> {
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

fn lower_pattern(pair: Pair<Rule>) -> Result<Pattern, ParseError> {
    let span = pair_span(&pair);
    let mut parts = Vec::new();

    for p in pair.into_inner() {
        if p.as_rule() == Rule::pattern_part {
            parts.push(lower_pattern_part(p)?);
        }
    }

    Ok(Pattern { parts, span })
}

fn lower_pattern_part(pair: Pair<Rule>) -> Result<PatternPart, ParseError> {
    let span = pair_span(&pair);
    let mut binding = None;
    let mut element = None;
    let mut saw_eq = false;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::variable if !saw_eq && binding.is_none() => binding = Some(lower_variable(p)?),
            Rule::eq => saw_eq = true,
            Rule::anonymous_pattern_part => {
                for inner in p.into_inner() {
                    match inner.as_rule() {
                        Rule::pattern_element => element = Some(lower_pattern_element(inner)?),
                        Rule::shortest_path_pattern => element = Some(lower_shortest_path_pattern(inner)?),
                        _ => {}
                    }
                }
            }
            Rule::pattern_element => element = Some(lower_pattern_element(p)?),
            _ => {}
        }
    }

    Ok(PatternPart {
        binding,
        element: element
            .ok_or_else(|| ParseError::new("expected pattern element", span.start, span.end))?,
        span,
    })
}

fn lower_shortest_path_pattern(pair: Pair<Rule>) -> Result<PatternElement, ParseError> {
    let span = pair_span(&pair);
    let mut all = false;
    let mut inner_element = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::SHORTEST_PATH => all = false,
            Rule::ALL_SHORTEST_PATHS => all = true,
            Rule::pattern_element => inner_element = Some(lower_pattern_element(p)?),
            _ => {}
        }
    }

    Ok(PatternElement::ShortestPath {
        all,
        element: Box::new(inner_element.ok_or_else(|| {
            ParseError::new("expected pattern element in shortestPath", span.start, span.end)
        })?),
        span,
    })
}

fn lower_pattern_element(pair: Pair<Rule>) -> Result<PatternElement, ParseError> {
    let span = pair_span(&pair);
    let mut inners = pair.into_inner().peekable();

    let first = inners
        .next()
        .ok_or_else(|| ParseError::new("expected pattern element", span.start, span.end))?;

    match first.as_rule() {
        Rule::node_pattern => {
            let head = lower_node_pattern(first)?;
            let mut chain = Vec::new();

            for p in inners {
                if p.as_rule() == Rule::pattern_element_chain {
                    chain.push(lower_pattern_element_chain(p)?);
                }
            }

            Ok(PatternElement::NodeChain { head, chain, span })
        }
        Rule::pattern_element => {
            let inner = lower_pattern_element(first)?;
            Ok(PatternElement::Parenthesized(Box::new(inner), span))
        }
        _ => Err(unexpected_rule("pattern_element", first)),
    }
}

fn lower_pattern_element_chain(pair: Pair<Rule>) -> Result<PatternElementChain, ParseError> {
    let span = pair_span(&pair);
    let mut relationship = None;
    let mut node = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::relationship_pattern => relationship = Some(lower_relationship_pattern(p)?),
            Rule::node_pattern => node = Some(lower_node_pattern(p)?),
            _ => {}
        }
    }

    Ok(PatternElementChain {
        relationship: relationship.ok_or_else(|| {
            ParseError::new("expected relationship pattern", span.start, span.end)
        })?,
        node: node.ok_or_else(|| ParseError::new("expected node pattern", span.start, span.end))?,
        span,
    })
}

fn lower_node_pattern(pair: Pair<Rule>) -> Result<NodePattern, ParseError> {
    let span = pair_span(&pair);
    let mut variable = None;
    let mut labels = SmallVec::new();
    let mut properties = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::variable => variable = Some(lower_variable(p)?),
            Rule::node_labels => labels = lower_node_labels(p)?,
            Rule::properties => properties = Some(lower_properties(p)?),
            _ => {}
        }
    }

    Ok(NodePattern {
        variable,
        labels,
        properties,
        span,
    })
}

fn lower_node_labels(pair: Pair<Rule>) -> Result<SmallVec<SmallVec<String, 2>, 2>, ParseError> {
    let mut out: SmallVec<SmallVec<String, 2>, 2> = SmallVec::new();
    for p in pair.into_inner() {
        if p.as_rule() == Rule::node_label_set {
            let mut group = SmallVec::new();
            for q in p.into_inner() {
                if q.as_rule() == Rule::label_name {
                    group.push(lower_schema_name(q)?);
                }
            }
            if !group.is_empty() {
                out.push(group);
            }
        }
    }
    Ok(out)
}

fn lower_relationship_pattern(pair: Pair<Rule>) -> Result<RelationshipPattern, ParseError> {
    let span = pair_span(&pair);
    let mut left = false;
    let mut right = false;
    let mut detail = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::left_arrow => left = true,
            Rule::right_arrow => right = true,
            Rule::relationship_detail => detail = Some(lower_relationship_detail(p)?),
            _ => {}
        }
    }

    let direction = match (left, right) {
        (true, false) => Direction::Left,
        (false, true) => Direction::Right,
        _ => Direction::Undirected,
    };

    Ok(RelationshipPattern {
        direction,
        detail,
        span,
    })
}

fn lower_relationship_detail(pair: Pair<Rule>) -> Result<RelationshipDetail, ParseError> {
    let span = pair_span(&pair);
    let mut variable = None;
    let mut types = SmallVec::new();
    let mut range = None;
    let mut properties = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::variable => variable = Some(lower_variable(p)?),
            Rule::relationship_types => types = lower_relationship_types(p)?,
            Rule::range_literal => range = Some(lower_range_literal(p)?),
            Rule::properties => properties = Some(lower_properties(p)?),
            _ => {}
        }
    }

    Ok(RelationshipDetail {
        variable,
        types,
        range,
        properties,
        span,
    })
}

fn lower_relationship_types(pair: Pair<Rule>) -> Result<SmallVec<String, 2>, ParseError> {
    let mut out = SmallVec::new();
    for p in pair.into_inner() {
        if p.as_rule() == Rule::rel_type_name {
            out.push(lower_schema_name(p)?);
        }
    }
    Ok(out)
}

fn lower_range_literal(pair: Pair<Rule>) -> Result<RangeLiteral, ParseError> {
    let span = pair_span(&pair);
    let raw = pair.as_str().trim();
    let body = raw.strip_prefix('*').unwrap_or(raw);

    let (start, end) = if let Some((lhs, rhs)) = body.split_once("..") {
        let start = if lhs.is_empty() {
            None
        } else {
            Some(
                lhs.parse::<u64>()
                    .map_err(|_| ParseError::new("invalid range start", span.start, span.end))?,
            )
        };
        let end = if rhs.is_empty() {
            None
        } else {
            Some(
                rhs.parse::<u64>()
                    .map_err(|_| ParseError::new("invalid range end", span.start, span.end))?,
            )
        };
        (start, end)
    } else if body.is_empty() {
        (None, None)
    } else {
        (
            Some(
                body.parse::<u64>()
                    .map_err(|_| ParseError::new("invalid range bound", span.start, span.end))?,
            ),
            None,
        )
    };

    Ok(RangeLiteral { start, end, span })
}

fn lower_properties(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::map_literal => lower_map_literal(inner),
        Rule::parameter => lower_parameter(inner),
        _ => Err(unexpected_rule("properties", inner)),
    }
}

fn lower_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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
        Rule::parameter => lower_parameter(pair),
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

fn lower_left_assoc(
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

fn lower_not_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_comparison_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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
        let parts: Vec<_> = tail.into_inner().collect();

        let (op, rhs): (BinaryOp, Option<Expr>) = match parts.as_slice() {
            [p0, p1] if p0.as_rule() == Rule::comparison_op => {
                let op = match p0.as_str() {
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
                        ))
                    }
                };
                (op, Some(lower_expression(p1.clone())?))
            }
            [p0, p1] if p0.as_rule() == Rule::IN => {
                (BinaryOp::In, Some(lower_expression(p1.clone())?))
            }
            [p0, p1, p2] if p0.as_rule() == Rule::STARTS && p1.as_rule() == Rule::WITH => {
                (BinaryOp::StartsWith, Some(lower_expression(p2.clone())?))
            }
            [p0, p1, p2] if p0.as_rule() == Rule::ENDS && p1.as_rule() == Rule::WITH => {
                (BinaryOp::EndsWith, Some(lower_expression(p2.clone())?))
            }
            [p0, p1] if p0.as_rule() == Rule::CONTAINS => {
                (BinaryOp::Contains, Some(lower_expression(p1.clone())?))
            }
            [p0, p1] if p0.as_rule() == Rule::IS && p1.as_rule() == Rule::NULL => {
                (BinaryOp::IsNull, None)
            }
            [p0, p1, p2]
                if p0.as_rule() == Rule::IS
                    && p1.as_rule() == Rule::NOT
                    && p2.as_rule() == Rule::NULL =>
            {
                (BinaryOp::IsNotNull, None)
            }
            [p0, p1] if p0.as_rule() == Rule::regex_match => {
                (BinaryOp::RegexMatch, Some(lower_expression(p1.clone())?))
            }
            _ => {
                return Err(ParseError::new(
                    "invalid comparison tail",
                    tail_span.start,
                    tail_span.end,
                ))
            }
        };

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

fn lower_add_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_mul_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_pow_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_unary_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_postfix_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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
                                let parts: Vec<_> = inner_op.into_inner().collect();
                                let mut from_expr = None;
                                let mut to_expr = None;
                                let mut seen_dots = false;
                                for p in parts {
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
                                let idx_pair = inner_op.into_inner().find(|p| p.as_rule() == Rule::expression);
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

fn lower_map_projection_selector(pair: Pair<Rule>) -> Result<MapProjectionSelector, ParseError> {
    let parts: Vec<_> = pair.into_inner().collect();

    // .* (dot + STAR)
    if parts.len() == 1 && parts[0].as_rule() == Rule::STAR {
        return Ok(MapProjectionSelector::AllProperties);
    }

    // .name (dot is consumed, only property_key_name remains)
    if parts.len() == 1 && parts[0].as_rule() == Rule::property_key_name {
        let name = lower_schema_name(parts[0].clone())?;
        return Ok(MapProjectionSelector::Property(name));
    }

    // key: expr (property_key_name + expression)
    if parts.len() >= 2 && parts[0].as_rule() == Rule::property_key_name {
        let key = lower_schema_name(parts[0].clone())?;
        let expr_pair = parts.into_iter().find(|p| p.as_rule() == Rule::expression);
        if let Some(ep) = expr_pair {
            return Ok(MapProjectionSelector::Literal(key, lower_expression(ep)?));
        }
    }

    Err(ParseError::new("invalid map projection selector", 0, 0))
}

fn lower_simple_case_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_generic_case_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_literal(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_string_literal(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_map_literal(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_list_literal(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_pattern_comprehension(pair: Pair<Rule>, outer_span: Span) -> Result<Expr, ParseError> {
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
                    ParseError::new("expected WHERE expression", outer_span.start, outer_span.end)
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
            ParseError::new("expected pattern in comprehension", outer_span.start, outer_span.end)
        })?),
        where_,
        map_expr: map_expr.ok_or_else(|| {
            ParseError::new("expected map expression after |", outer_span.start, outer_span.end)
        })?,
        span: outer_span,
    })
}

fn lower_list_comprehension(pair: Pair<Rule>, outer_span: Span) -> Result<Expr, ParseError> {
    let mut inner = pair.into_inner();

    let var_pair = inner.next().ok_or_else(|| ParseError::new("expected variable", outer_span.start, outer_span.end))?;
    let variable = lower_variable(var_pair)?;

    // skip IN
    let _in_kw = inner.next();

    let list_pair = inner.next().ok_or_else(|| ParseError::new("expected list expression", outer_span.start, outer_span.end))?;
    let list = lower_expression(list_pair)?;

    let mut filter = None;
    let mut map_expr = None;

    // Remaining tokens: optional WHERE expr, optional | expr
    while let Some(p) = inner.next() {
        match p.as_rule() {
            Rule::WHERE => {
                let filter_pair = inner.next().ok_or_else(|| ParseError::new("expected filter expression", outer_span.start, outer_span.end))?;
                filter = Some(Box::new(lower_expression(filter_pair)?));
            }
            Rule::pipe => {
                let map_pair = inner.next().ok_or_else(|| ParseError::new("expected map expression", outer_span.start, outer_span.end))?;
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

fn lower_parameter(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let raw = pair.as_str();
    Ok(Expr::Parameter(raw[1..].to_string(), span))
}

fn lower_list_predicate(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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
        _ => return Err(ParseError::new("expected ANY/ALL/NONE/SINGLE", span.start, span.end)),
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

fn lower_reduce_expression(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();

    // skip REDUCE keyword
    let _reduce_kw = inner.next();

    let acc_pair = inner.next().ok_or_else(|| ParseError::new("expected accumulator variable", span.start, span.end))?;
    let accumulator = lower_variable(acc_pair)?;

    // skip = (eq)
    let _eq = inner.next();

    let init_pair = inner.next().ok_or_else(|| ParseError::new("expected init expression", span.start, span.end))?;
    let init = lower_expression(init_pair)?;

    // skip comma (already consumed by pest)

    let var_pair = inner.next().ok_or_else(|| ParseError::new("expected variable", span.start, span.end))?;
    let variable = lower_variable(var_pair)?;

    // skip IN
    let _in_kw = inner.next();

    let list_pair = inner.next().ok_or_else(|| ParseError::new("expected list expression", span.start, span.end))?;
    let list = lower_expression(list_pair)?;

    // skip pipe (already consumed by pest)

    let expr_pair = inner.next().ok_or_else(|| ParseError::new("expected reduce expression", span.start, span.end))?;
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

fn lower_exists_subquery(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_function_invocation(pair: Pair<Rule>) -> Result<Expr, ParseError> {
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

fn lower_name_parts(pair: Pair<Rule>) -> Result<Vec<String>, ParseError> {
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

fn lower_variable(pair: Pair<Rule>) -> Result<Variable, ParseError> {
    let span = pair_span(&pair);
    let name_pair = single_inner(pair)?;
    Ok(Variable {
        name: lower_symbolic_name(name_pair)?,
        span,
    })
}

fn lower_symbolic_name(pair: Pair<Rule>) -> Result<String, ParseError> {
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

fn lower_schema_name(pair: Pair<Rule>) -> Result<String, ParseError> {
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

fn lower_integer_literal(pair: Pair<Rule>) -> Result<i64, ParseError> {
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

fn single_inner(pair: Pair<Rule>) -> Result<Pair<Rule>, ParseError> {
    let span = pair.as_span();
    pair.into_inner()
        .next()
        .ok_or_else(|| ParseError::new("expected inner rule", span.start(), span.end()))
}

fn pair_span(pair: &Pair<Rule>) -> Span {
    Span::new(pair.as_span().start(), pair.as_span().end())
}

fn merge_spans(a: Span, b: Span) -> Span {
    Span {
        start: a.start.min(b.start),
        end: a.end.max(b.end),
    }
}

fn unexpected_rule(expected: &str, pair: Pair<Rule>) -> ParseError {
    ParseError::new(
        format!("expected {expected}, got {:?}", pair.as_rule()),
        pair.as_span().start(),
        pair.as_span().end(),
    )
}

#[cfg(test)]
mod tests {
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
        let doc =
            parse_query("RETURN CASE n.age WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END")
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
        let doc = parse_query("CALL db.labels() YIELD label WHERE label IS NOT NULL RETURN label")
            .unwrap();
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
}
