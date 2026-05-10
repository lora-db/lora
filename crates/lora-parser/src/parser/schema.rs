//! Lowering for schema commands: `CREATE [type] INDEX ...` and
//! `SHOW INDEXES`. Statement-level DDL that bypasses the regular
//! query / call grammar.

use super::expressions::lower_expression;
use super::literals::{lower_schema_name, lower_symbolic_name};
use super::util::{pair_span, single_inner, unexpected_rule};
use super::Rule;
use crate::errors::ParseError;
use lora_ast::*;
use pest::iterators::Pair;

pub(super) fn lower_schema_command(pair: Pair<Rule>) -> Result<SchemaCommand, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::create_index_command => Ok(SchemaCommand::CreateIndex(lower_create_index(inner)?)),
        Rule::drop_index_command => Ok(SchemaCommand::DropIndex(lower_drop_index(inner)?)),
        Rule::show_indexes_command => Ok(SchemaCommand::ShowIndexes(lower_show_indexes(inner)?)),
        _ => Err(unexpected_rule("schema_command", inner)),
    }
}

fn lower_drop_index(pair: Pair<Rule>) -> Result<DropIndex, ParseError> {
    let span = pair_span(&pair);
    let mut name: Option<IndexNameSpec> = None;
    let mut if_exists = false;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::DROP_KW | Rule::INDEX => {}
            Rule::index_name_spec => name = Some(lower_index_name_spec(p)?),
            Rule::if_exists => if_exists = true,
            _ => return Err(unexpected_rule("drop_index_command", p)),
        }
    }
    Ok(DropIndex {
        name: name.ok_or_else(|| {
            ParseError::new("DROP INDEX requires an index name", span.start, span.end)
        })?,
        if_exists,
        span,
    })
}

fn lower_show_indexes(pair: Pair<Rule>) -> Result<ShowIndexes, ParseError> {
    Ok(ShowIndexes {
        span: pair_span(&pair),
    })
}

fn lower_create_index(pair: Pair<Rule>) -> Result<CreateIndex, ParseError> {
    let span = pair_span(&pair);
    let mut kind = IndexKind::Range;
    let mut name = None;
    let mut if_not_exists = false;
    let mut entity = IndexEntityKind::Node;
    let mut variable = String::new();
    let mut label: Option<String> = None;
    let mut properties: Vec<String> = Vec::new();
    let mut options: Option<IndexOptions> = None;
    let mut explicit_kind = false;
    let mut is_lookup_pattern = false;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::CREATE | Rule::INDEX | Rule::FOR | Rule::ON => {}
            Rule::index_kind => {
                kind = lower_index_kind(p)?;
                explicit_kind = true;
            }
            Rule::index_name_spec => {
                name = Some(lower_index_name_spec(p)?);
            }
            Rule::if_not_exists => {
                if_not_exists = true;
            }
            Rule::index_pattern => {
                let parsed = lower_index_pattern(p)?;
                entity = parsed.entity;
                variable = parsed.variable;
                label = parsed.label;
                is_lookup_pattern = parsed.is_lookup;
            }
            Rule::index_property_spec => {
                let inner = single_inner(p)?;
                match inner.as_rule() {
                    Rule::index_property_list => {
                        properties = lower_index_property_list(inner)?;
                    }
                    Rule::index_token_lookup => {
                        // EACH labels(n) / EACH type(r) — token lookup. The default
                        // `kind` becomes `Lookup` if not explicitly specified.
                        if !explicit_kind {
                            kind = IndexKind::Lookup;
                        }
                        // No properties for token lookup indexes.
                    }
                    _ => return Err(unexpected_rule("index_property_spec inner", inner)),
                }
            }
            Rule::index_options => {
                options = Some(lower_index_options(p)?);
            }
            _ => return Err(unexpected_rule("create_index_command", p)),
        }
    }

    if is_lookup_pattern && kind != IndexKind::Lookup {
        return Err(ParseError::new(
            "the wildcard pattern (n) / ()-[r]-() is only valid for LOOKUP indexes",
            span.start,
            span.end,
        ));
    }
    if matches!(kind, IndexKind::Lookup) && !properties.is_empty() {
        return Err(ParseError::new(
            "LOOKUP indexes are populated via labels(n) / type(r); they take no property list",
            span.start,
            span.end,
        ));
    }
    if !matches!(kind, IndexKind::Lookup) && properties.is_empty() {
        return Err(ParseError::new(
            "non-LOOKUP indexes require at least one property",
            span.start,
            span.end,
        ));
    }

    Ok(CreateIndex {
        kind,
        name,
        if_not_exists,
        entity,
        variable,
        label,
        properties,
        options,
        span,
    })
}

fn lower_index_kind(pair: Pair<Rule>) -> Result<IndexKind, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::TEXT => Ok(IndexKind::Text),
        Rule::POINT => Ok(IndexKind::Point),
        Rule::LOOKUP => Ok(IndexKind::Lookup),
        Rule::RANGE => Ok(IndexKind::Range),
        _ => Err(unexpected_rule("index_kind", inner)),
    }
}

fn lower_index_name_spec(pair: Pair<Rule>) -> Result<IndexNameSpec, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::parameter => {
            // The parameter rule's content is `$<name>`; reuse the existing
            // expression lowering and pull out the name.
            let raw = inner.as_str();
            Ok(IndexNameSpec::Parameter(raw[1..].to_string()))
        }
        Rule::symbolic_name => Ok(IndexNameSpec::Literal(lower_symbolic_name(inner)?)),
        _ => Err(unexpected_rule("index_name_spec", inner)),
    }
}

struct ParsedIndexPattern {
    entity: IndexEntityKind,
    variable: String,
    label: Option<String>,
    is_lookup: bool,
}

fn lower_index_pattern(pair: Pair<Rule>) -> Result<ParsedIndexPattern, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::indexed_node_pattern => {
            let mut variable = String::new();
            let mut label: Option<String> = None;
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::index_var => variable = lower_index_var(p)?,
                    Rule::label_name => label = Some(lower_schema_name(p)?),
                    _ => {}
                }
            }
            Ok(ParsedIndexPattern {
                entity: IndexEntityKind::Node,
                variable,
                label,
                is_lookup: false,
            })
        }
        Rule::indexed_rel_pattern => {
            let mut variable = String::new();
            let mut label: Option<String> = None;
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::index_var => variable = lower_index_var(p)?,
                    Rule::rel_type_name => label = Some(lower_schema_name(p)?),
                    _ => {}
                }
            }
            Ok(ParsedIndexPattern {
                entity: IndexEntityKind::Relationship,
                variable,
                label,
                is_lookup: false,
            })
        }
        Rule::lookup_node_pattern => {
            let mut variable = String::new();
            for p in inner.into_inner() {
                if p.as_rule() == Rule::index_var {
                    variable = lower_index_var(p)?;
                }
            }
            Ok(ParsedIndexPattern {
                entity: IndexEntityKind::Node,
                variable,
                label: None,
                is_lookup: true,
            })
        }
        Rule::lookup_rel_pattern => {
            let mut variable = String::new();
            for p in inner.into_inner() {
                if p.as_rule() == Rule::index_var {
                    variable = lower_index_var(p)?;
                }
            }
            Ok(ParsedIndexPattern {
                entity: IndexEntityKind::Relationship,
                variable,
                label: None,
                is_lookup: true,
            })
        }
        _ => Err(unexpected_rule("index_pattern", inner)),
    }
}

fn lower_index_var(pair: Pair<Rule>) -> Result<String, ParseError> {
    let inner = single_inner(pair)?;
    lower_symbolic_name(inner)
}

fn lower_index_property_list(pair: Pair<Rule>) -> Result<Vec<String>, ParseError> {
    let mut out = Vec::new();
    for p in pair.into_inner() {
        if p.as_rule() == Rule::index_property {
            out.push(lower_index_property(p)?);
        }
    }
    Ok(out)
}

fn lower_index_property(pair: Pair<Rule>) -> Result<String, ParseError> {
    // index_property = { index_var ~ dot ~ property_key_name }
    // We only care about the property key.
    let mut key: Option<String> = None;
    for p in pair.into_inner() {
        if p.as_rule() == Rule::property_key_name {
            key = Some(lower_schema_name(p)?);
        }
    }
    key.ok_or_else(|| ParseError::new("expected property key", 0, 0))
}

fn lower_index_options(pair: Pair<Rule>) -> Result<IndexOptions, ParseError> {
    let span = pair_span(&pair);
    let mut config: Vec<(String, Expr)> = Vec::new();

    for p in pair.into_inner() {
        if p.as_rule() == Rule::map_literal {
            let mut key: Option<String> = None;
            for q in p.into_inner() {
                match q.as_rule() {
                    Rule::property_key_name => key = Some(lower_schema_name(q)?),
                    Rule::expression => {
                        let k = key.take().ok_or_else(|| {
                            ParseError::new(
                                "expected option key before expression",
                                span.start,
                                span.end,
                            )
                        })?;
                        config.push((k, lower_expression(q)?));
                    }
                    _ => {}
                }
            }
            // Recognise the standard `OPTIONS { indexConfig: { ... } }` shape:
            // if there is exactly one entry whose key is `indexConfig` and value is a
            // map literal, hoist the inner map up so config is the literal config map.
            if config.len() == 1 && config[0].0.eq_ignore_ascii_case("indexConfig") {
                if let Expr::Map(inner_entries, _) = config[0].1.clone() {
                    config = inner_entries;
                }
            }
        }
    }

    Ok(IndexOptions { config, span })
}
