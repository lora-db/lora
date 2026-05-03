use super::literals::{lower_map_literal, lower_parameter, lower_schema_name, lower_variable};
use super::util::{pair_span, single_inner, unexpected_rule};
use super::Rule;
use crate::errors::ParseError;
use lora_ast::*;
use pest::iterators::Pair;
use smallvec::SmallVec;

pub(super) fn lower_pattern(pair: Pair<Rule>) -> Result<Pattern, ParseError> {
    let span = pair_span(&pair);
    let mut parts = Vec::new();

    for p in pair.into_inner() {
        if p.as_rule() == Rule::pattern_part {
            parts.push(lower_pattern_part(p)?);
        }
    }

    Ok(Pattern { parts, span })
}

pub(super) fn lower_pattern_part(pair: Pair<Rule>) -> Result<PatternPart, ParseError> {
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
                        Rule::shortest_path_pattern => {
                            element = Some(lower_shortest_path_pattern(inner)?)
                        }
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

pub(super) fn lower_shortest_path_pattern(pair: Pair<Rule>) -> Result<PatternElement, ParseError> {
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
            ParseError::new(
                "expected pattern element in shortestPath",
                span.start,
                span.end,
            )
        })?),
        span,
    })
}

pub(super) fn lower_pattern_element(pair: Pair<Rule>) -> Result<PatternElement, ParseError> {
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

pub(super) fn lower_pattern_element_chain(
    pair: Pair<Rule>,
) -> Result<PatternElementChain, ParseError> {
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

pub(super) fn lower_node_pattern(pair: Pair<Rule>) -> Result<NodePattern, ParseError> {
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

pub(super) fn lower_node_labels(
    pair: Pair<Rule>,
) -> Result<SmallVec<SmallVec<String, 2>, 2>, ParseError> {
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

pub(super) fn lower_relationship_pattern(
    pair: Pair<Rule>,
) -> Result<RelationshipPattern, ParseError> {
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

pub(super) fn lower_relationship_detail(
    pair: Pair<Rule>,
) -> Result<RelationshipDetail, ParseError> {
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

pub(super) fn lower_relationship_types(
    pair: Pair<Rule>,
) -> Result<SmallVec<String, 2>, ParseError> {
    let mut out = SmallVec::new();
    for p in pair.into_inner() {
        if p.as_rule() == Rule::rel_type_name {
            out.push(lower_schema_name(p)?);
        }
    }
    Ok(out)
}

pub(super) fn lower_range_literal(pair: Pair<Rule>) -> Result<RangeLiteral, ParseError> {
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

pub(super) fn lower_properties(pair: Pair<Rule>) -> Result<Expr, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::map_literal => lower_map_literal(inner),
        Rule::parameter => lower_parameter(inner),
        _ => Err(unexpected_rule("properties", inner)),
    }
}
