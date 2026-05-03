use super::Rule;
use crate::errors::ParseError;
use lora_ast::Span;
use pest::iterators::Pair;

pub(super) fn single_inner(pair: Pair<Rule>) -> Result<Pair<Rule>, ParseError> {
    let span = pair.as_span();
    pair.into_inner()
        .next()
        .ok_or_else(|| ParseError::new("expected inner rule", span.start(), span.end()))
}

pub(super) fn pair_span(pair: &Pair<Rule>) -> Span {
    Span::new(pair.as_span().start(), pair.as_span().end())
}

pub(super) fn merge_spans(a: Span, b: Span) -> Span {
    Span {
        start: a.start.min(b.start),
        end: a.end.max(b.end),
    }
}

pub(super) fn unexpected_rule(expected: &str, pair: Pair<Rule>) -> ParseError {
    ParseError::new(
        format!("expected {expected}, got {:?}", pair.as_rule()),
        pair.as_span().start(),
        pair.as_span().end(),
    )
}
