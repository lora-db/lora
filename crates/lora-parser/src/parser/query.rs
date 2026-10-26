use super::clauses::{
    lower_create, lower_delete, lower_foreach, lower_in_query_call, lower_match, lower_merge,
    lower_remove, lower_return_clause, lower_set, lower_unwind, lower_with_clause,
};
use super::limits::DepthGuard;
use super::util::{pair_span, single_inner, unexpected_rule};
use super::Rule;
use crate::errors::ParseError;
use lora_ast::*;
use pest::iterators::Pair;

pub(super) fn lower_regular_query(pair: Pair<Rule>) -> Result<RegularQuery, ParseError> {
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

        let union_start = union_pair.as_span().start();
        let all = union_pair.into_inner().any(|p| p.as_rule() == Rule::ALL);
        let union_span = Span::new(union_start, uq.as_span().end());

        unions.push(UnionPart {
            all,
            query: lower_single_query(uq)?,
            span: union_span,
        });
    }

    Ok(RegularQuery { head, unions, span })
}

pub(super) fn lower_single_query(pair: Pair<Rule>) -> Result<SingleQuery, ParseError> {
    if pair.as_rule() != Rule::single_query {
        return Err(unexpected_rule("single_query", pair));
    }
    let span = pair_span(&pair);
    // Each clause of a query runs nested inside the one after it, so a
    // branch's clauses (with those of the CALL bodies inside it) are
    // bounded together.
    let clauses = pair.clone().into_inner().count();
    let _clauses = DepthGuard::clauses(clauses, span.start, span.end)?;

    let mut parts = Vec::new();
    let mut reading_clauses = Vec::new();
    let mut updating_clauses = Vec::new();
    let mut return_clause = None;
    // Start of the first clause of the part being collected.
    let mut part_start = None;

    for p in pair.into_inner() {
        let p_span = pair_span(&p);
        if return_clause.is_some() {
            return Err(ParseError::new(
                "RETURN must be the last clause of a query; use WITH to continue the query",
                p_span.start,
                p_span.end,
            ));
        }
        part_start.get_or_insert(p_span.start);
        match p.as_rule() {
            Rule::reading_clause => {
                if !updating_clauses.is_empty() {
                    return Err(ParseError::new(
                        "a reading clause (MATCH, UNWIND, CALL) cannot follow an updating \
                         clause; separate them with WITH",
                        p_span.start,
                        p_span.end,
                    ));
                }
                reading_clauses.push(lower_reading_clause(p)?);
            }
            Rule::updating_clause => updating_clauses.push(lower_updating_clause(p)?),
            Rule::with_clause => {
                let with_clause = lower_with_clause(p)?;
                parts.push(QueryPart {
                    reading_clauses: std::mem::take(&mut reading_clauses),
                    updating_clauses: std::mem::take(&mut updating_clauses),
                    with_clause,
                    span: Span::new(part_start.take().unwrap_or(p_span.start), p_span.end),
                });
            }
            Rule::return_clause => return_clause = Some(lower_return_clause(p)?),
            _ => return Err(unexpected_rule("clause", p)),
        }
    }

    // The last part ends in RETURN, in updating clauses, or in a unit
    // `CALL { ... }` subquery (one whose body ends in an update, not a
    // RETURN; the analyzer checks that).
    let ends_in_unit_call = matches!(reading_clauses.last(), Some(ReadingClause::CallSubquery(_)));
    if return_clause.is_none() && updating_clauses.is_empty() && !ends_in_unit_call {
        return Err(ParseError::new(
            "a query must end with RETURN, an updating clause, or CALL { ... }",
            span.end,
            span.end,
        ));
    }
    let tail = SinglePartQuery {
        reading_clauses,
        updating_clauses,
        return_clause,
        span: Span::new(part_start.unwrap_or(span.end), span.end),
    };

    if parts.is_empty() {
        Ok(SingleQuery::SinglePart(SinglePartQuery { span, ..tail }))
    } else {
        Ok(SingleQuery::MultiPart(MultiPartQuery {
            parts,
            tail: Box::new(tail),
            span,
        }))
    }
}

pub(super) fn lower_reading_clause(pair: Pair<Rule>) -> Result<ReadingClause, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::match_clause => Ok(ReadingClause::Match(lower_match(inner)?)),
        Rule::unwind_clause => Ok(ReadingClause::Unwind(lower_unwind(inner)?)),
        Rule::in_query_call => Ok(ReadingClause::InQueryCall(lower_in_query_call(inner)?)),
        Rule::call_subquery => Ok(ReadingClause::CallSubquery(lower_call_subquery_pair(
            inner,
        )?)),
        _ => Err(unexpected_rule("reading_clause", inner)),
    }
}

fn lower_call_subquery_pair(pair: Pair<Rule>) -> Result<CallSubquery, ParseError> {
    let span = pair_span(&pair);
    let body_pair = pair
        .into_inner()
        .find(|p| p.as_rule() == Rule::regular_query)
        .ok_or_else(|| {
            ParseError::new("expected query body in CALL { ... }", span.start, span.end)
        })?;
    let body = lower_regular_query(body_pair)?;
    Ok(CallSubquery {
        body: Box::new(body),
        span,
    })
}

pub(super) fn lower_updating_clause(pair: Pair<Rule>) -> Result<UpdatingClause, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::create_clause => Ok(UpdatingClause::Create(lower_create(inner)?)),
        Rule::merge_clause => Ok(UpdatingClause::Merge(lower_merge(inner)?)),
        Rule::delete_clause => Ok(UpdatingClause::Delete(lower_delete(inner)?)),
        Rule::set_clause => Ok(UpdatingClause::Set(lower_set(inner)?)),
        Rule::remove_clause => Ok(UpdatingClause::Remove(lower_remove(inner)?)),
        Rule::foreach_clause => Ok(UpdatingClause::Foreach(lower_foreach(inner)?)),
        _ => Err(unexpected_rule("updating_clause", inner)),
    }
}
