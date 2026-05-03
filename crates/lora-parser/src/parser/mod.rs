//! Pest-driven Cypher parser: lower a raw query string into a [`lora_ast::Document`].
//!
//! Layout:
//! - `query` — top-of-tree query-shape lowerings (`regular_query`,
//!   `single_query`, `multi_part_query`, `query_part`, `single_part_query`,
//!   `reading_clause`, `updating_clause`).
//! - `clauses` — clause-level lowerings (MATCH, UNWIND, CREATE, MERGE,
//!   DELETE, SET, REMOVE, CALL, YIELD, WITH, WHERE, RETURN, projection
//!   body / items, ORDER BY, sort items).
//! - `patterns` — graph-pattern lowerings (pattern, pattern part,
//!   shortest-path pattern, pattern element + chain, node / relationship
//!   patterns, range literals, properties).
//! - `expressions` — operator-precedence chain from `expression` down
//!   through comparison / add / mul / pow / unary / postfix and CASE.
//! - `literals` — leaves of the expression tree: literals, strings,
//!   maps, lists, comprehensions, parameters, list predicates, REDUCE,
//!   EXISTS subqueries, function calls, name-part / variable / symbolic
//!   / schema-name / integer-literal lowerings.
//! - `util` — small helpers shared across the lowerings (`single_inner`,
//!   `pair_span`, `merge_spans`, `unexpected_rule`).
//! - `tests` — unit tests covering the parser surface.

use crate::errors::ParseError;
use lora_ast::*;
use pest::iterators::Pair;
use pest::Parser;

mod clauses;
mod expressions;
mod literals;
mod patterns;
mod query;
mod util;

#[cfg(test)]
mod tests;

use clauses::lower_standalone_call;
use query::lower_regular_query;
use util::{pair_span, unexpected_rule};

#[derive(pest_derive::Parser)]
#[grammar = "cypher.pest"]
pub(super) struct LoraParser;

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
    let inner = util::single_inner(pair)?;
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
