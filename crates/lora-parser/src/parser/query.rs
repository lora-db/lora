use super::clauses::{
    lower_create, lower_delete, lower_in_query_call, lower_match, lower_merge, lower_remove,
    lower_return_clause, lower_set, lower_unwind, lower_with_clause,
};
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
    match pair.as_rule() {
        Rule::single_query => lower_single_query(single_inner(pair)?),
        Rule::single_part_query => Ok(SingleQuery::SinglePart(lower_single_part_query(pair)?)),
        Rule::multi_part_query => Ok(SingleQuery::MultiPart(lower_multi_part_query(pair)?)),
        _ => Err(unexpected_rule("single_query", pair)),
    }
}

pub(super) fn lower_multi_part_query(pair: Pair<Rule>) -> Result<MultiPartQuery, ParseError> {
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

pub(super) fn lower_query_part(pair: Pair<Rule>) -> Result<QueryPart, ParseError> {
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

pub(super) fn lower_single_part_query(pair: Pair<Rule>) -> Result<SinglePartQuery, ParseError> {
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
        _ => Err(unexpected_rule("updating_clause", inner)),
    }
}
