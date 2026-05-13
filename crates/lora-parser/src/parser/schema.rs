//! Lowering for schema commands: `CREATE [type] INDEX ...` and
//! `SHOW INDEXES`. Statement-level DDL that bypasses the regular
//! query / call grammar.

use super::clauses::{
    lower_order_clause, lower_projection_items, lower_where_clause, lower_yield_item,
};
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
        Rule::create_constraint_command => Ok(SchemaCommand::CreateConstraint(
            lower_create_constraint(inner)?,
        )),
        Rule::drop_constraint_command => {
            Ok(SchemaCommand::DropConstraint(lower_drop_constraint(inner)?))
        }
        Rule::show_constraints_command => Ok(SchemaCommand::ShowConstraints(
            lower_show_constraints(inner)?,
        )),
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
    let span = pair_span(&pair);
    let mut filter = None;
    let mut pipeline = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::SHOW | Rule::INDEXES | Rule::INDEX => {}
            Rule::show_index_filter => filter = Some(lower_show_index_filter(p)?),
            Rule::show_pipeline => pipeline = Some(lower_show_pipeline(p)?),
            _ => return Err(unexpected_rule("show_indexes_command", p)),
        }
    }
    Ok(ShowIndexes {
        filter,
        pipeline,
        span,
    })
}

fn lower_show_index_filter(pair: Pair<Rule>) -> Result<IndexKindFilter, ParseError> {
    let inner = single_inner(pair)?;
    Ok(match inner.as_rule() {
        Rule::ALL => IndexKindFilter::All,
        Rule::RANGE => IndexKindFilter::Range,
        Rule::TEXT => IndexKindFilter::Text,
        Rule::POINT => IndexKindFilter::Point,
        Rule::LOOKUP => IndexKindFilter::Lookup,
        Rule::FULLTEXT => IndexKindFilter::Fulltext,
        Rule::VECTOR => IndexKindFilter::Vector,
        _ => return Err(unexpected_rule("show_index_filter", inner)),
    })
}

fn lower_show_pipeline(pair: Pair<Rule>) -> Result<ShowPipeline, ParseError> {
    let span = pair_span(&pair);
    let mut yield_part = None;
    let mut where_ = None;
    let mut return_part = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::show_yield => yield_part = Some(lower_show_yield(p)?),
            Rule::where_clause => where_ = Some(lower_where_clause(p)?),
            Rule::show_return => return_part = Some(lower_show_return(p)?),
            _ => return Err(unexpected_rule("show_pipeline", p)),
        }
    }
    Ok(ShowPipeline {
        yield_part: yield_part
            .ok_or_else(|| ParseError::new("SHOW pipeline requires YIELD", span.start, span.end))?,
        where_,
        return_part,
        span,
    })
}

fn lower_show_yield(pair: Pair<Rule>) -> Result<ShowYield, ParseError> {
    let span = pair_span(&pair);
    let mut star = false;
    let mut items = Vec::new();
    let mut order = Vec::new();
    let mut skip = None;
    let mut limit = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::YIELD => {}
            Rule::STAR => star = true,
            Rule::yield_items => {
                for q in p.into_inner() {
                    if q.as_rule() == Rule::yield_item {
                        items.push(lower_yield_item(q)?);
                    }
                }
            }
            Rule::order_clause => order = lower_order_clause(p)?,
            Rule::skip_clause => skip = Some(lower_inner_expression(p, "SKIP")?),
            Rule::limit_clause => limit = Some(lower_inner_expression(p, "LIMIT")?),
            _ => return Err(unexpected_rule("show_yield", p)),
        }
    }
    Ok(ShowYield {
        star,
        items,
        order,
        skip,
        limit,
        span,
    })
}

fn lower_show_return(pair: Pair<Rule>) -> Result<ShowReturn, ParseError> {
    let span = pair_span(&pair);
    let mut items = Vec::new();
    let mut order = Vec::new();
    let mut skip = None;
    let mut limit = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::RETURN => {}
            Rule::projection_items => items = lower_projection_items(p)?,
            Rule::order_clause => order = lower_order_clause(p)?,
            Rule::skip_clause => skip = Some(lower_inner_expression(p, "SKIP")?),
            Rule::limit_clause => limit = Some(lower_inner_expression(p, "LIMIT")?),
            _ => return Err(unexpected_rule("show_return", p)),
        }
    }
    Ok(ShowReturn {
        items,
        order,
        skip,
        limit,
        span,
    })
}

fn lower_inner_expression(pair: Pair<Rule>, label: &str) -> Result<Expr, ParseError> {
    let span = pair_span(&pair);
    let expr = pair
        .into_inner()
        .find(|q| q.as_rule() == Rule::expression)
        .ok_or_else(|| {
            ParseError::new(
                format!("expected expression in {label}"),
                span.start,
                span.end,
            )
        })?;
    lower_expression(expr)
}

fn lower_create_index(pair: Pair<Rule>) -> Result<CreateIndex, ParseError> {
    let span = pair_span(&pair);
    let mut kind = IndexKind::Range;
    let mut name = None;
    let mut if_not_exists = false;
    let mut entity = IndexEntityKind::Node;
    let mut variable = String::new();
    let mut label: Option<String> = None;
    let mut additional_labels: Vec<String> = Vec::new();
    let mut properties: Vec<String> = Vec::new();
    let mut options: Option<IndexOptions> = None;
    let mut explicit_kind = false;
    let mut is_lookup_pattern = false;
    let mut is_each_property_form = false;

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
                additional_labels = parsed.additional_labels;
                is_lookup_pattern = parsed.is_lookup;
            }
            Rule::index_property_spec => {
                let inner = single_inner(p)?;
                match inner.as_rule() {
                    Rule::index_property_list => {
                        properties = lower_index_property_list(inner)?;
                    }
                    Rule::index_property_each => {
                        properties = lower_index_property_list(inner)?;
                        is_each_property_form = true;
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

    // FULLTEXT requires the `ON EACH [n.p, ...]` shape; other kinds use
    // the parenthesized property list.
    match kind {
        IndexKind::Fulltext if !is_each_property_form => {
            return Err(ParseError::new(
                "FULLTEXT indexes require `ON EACH [n.p, ...]`",
                span.start,
                span.end,
            ));
        }
        IndexKind::Fulltext => {}
        _ if is_each_property_form => {
            return Err(ParseError::new(
                "only FULLTEXT indexes accept the `ON EACH [n.p, ...]` form",
                span.start,
                span.end,
            ));
        }
        _ if !additional_labels.is_empty() => {
            return Err(ParseError::new(
                "only FULLTEXT indexes accept multi-label patterns like (n:A|B)",
                span.start,
                span.end,
            ));
        }
        _ => {}
    }

    Ok(CreateIndex {
        kind,
        name,
        if_not_exists,
        entity,
        variable,
        label,
        additional_labels,
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
        Rule::VECTOR => Ok(IndexKind::Vector),
        Rule::FULLTEXT => Ok(IndexKind::Fulltext),
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
    /// Additional labels beyond `label`, captured from `(n:A|B|C)` patterns.
    /// Only fulltext indexes accept this form.
    additional_labels: Vec<String>,
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
                additional_labels: Vec::new(),
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
                additional_labels: Vec::new(),
                is_lookup: false,
            })
        }
        Rule::fulltext_node_pattern => {
            let mut variable = String::new();
            let mut all_labels: Vec<String> = Vec::new();
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::index_var => variable = lower_index_var(p)?,
                    Rule::label_name => all_labels.push(lower_schema_name(p)?),
                    _ => {}
                }
            }
            let mut iter = all_labels.into_iter();
            let label = iter.next();
            Ok(ParsedIndexPattern {
                entity: IndexEntityKind::Node,
                variable,
                label,
                additional_labels: iter.collect(),
                is_lookup: false,
            })
        }
        Rule::fulltext_rel_pattern => {
            let mut variable = String::new();
            let mut all_types: Vec<String> = Vec::new();
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::index_var => variable = lower_index_var(p)?,
                    Rule::rel_type_name => all_types.push(lower_schema_name(p)?),
                    _ => {}
                }
            }
            let mut iter = all_types.into_iter();
            let label = iter.next();
            Ok(ParsedIndexPattern {
                entity: IndexEntityKind::Relationship,
                variable,
                label,
                additional_labels: iter.collect(),
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
                additional_labels: Vec::new(),
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
                additional_labels: Vec::new(),
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

fn lower_show_constraints(pair: Pair<Rule>) -> Result<ShowConstraints, ParseError> {
    let span = pair_span(&pair);
    let mut pipeline = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::SHOW | Rule::CONSTRAINTS | Rule::CONSTRAINT => {}
            Rule::show_pipeline => pipeline = Some(lower_show_pipeline(p)?),
            _ => return Err(unexpected_rule("show_constraints_command", p)),
        }
    }
    Ok(ShowConstraints { pipeline, span })
}

fn lower_drop_constraint(pair: Pair<Rule>) -> Result<DropConstraint, ParseError> {
    let span = pair_span(&pair);
    let mut name: Option<ConstraintNameSpec> = None;
    let mut if_exists = false;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::DROP_KW | Rule::CONSTRAINT => {}
            Rule::constraint_name_spec => name = Some(lower_constraint_name_spec(p)?),
            Rule::if_exists => if_exists = true,
            _ => return Err(unexpected_rule("drop_constraint_command", p)),
        }
    }
    Ok(DropConstraint {
        name: name.ok_or_else(|| {
            ParseError::new(
                "DROP CONSTRAINT requires a constraint name",
                span.start,
                span.end,
            )
        })?,
        if_exists,
        span,
    })
}

fn lower_create_constraint(pair: Pair<Rule>) -> Result<CreateConstraint, ParseError> {
    let span = pair_span(&pair);
    let mut name: Option<ConstraintNameSpec> = None;
    let mut if_not_exists = false;
    let mut entity = IndexEntityKind::Node;
    let mut variable = String::new();
    let mut label: Option<String> = None;
    let mut properties: Vec<String> = Vec::new();
    let mut composite = false;
    let mut requirement: Option<ConstraintKind> = None;

    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::CREATE | Rule::CONSTRAINT | Rule::FOR | Rule::REQUIRE => {}
            Rule::constraint_name_spec => name = Some(lower_constraint_name_spec(p)?),
            Rule::if_not_exists => if_not_exists = true,
            Rule::constraint_pattern => {
                let parsed = lower_constraint_pattern(p)?;
                entity = parsed.entity;
                variable = parsed.variable;
                label = Some(parsed.label);
            }
            Rule::constraint_property_spec => {
                let parsed = lower_constraint_property_spec(p)?;
                properties = parsed.properties;
                composite = parsed.composite;
            }
            Rule::constraint_requirement => {
                requirement = Some(lower_constraint_requirement(p)?);
            }
            _ => return Err(unexpected_rule("create_constraint_command", p)),
        }
    }

    let name = name.ok_or_else(|| {
        ParseError::new(
            "CREATE CONSTRAINT requires a constraint name",
            span.start,
            span.end,
        )
    })?;
    let label = label.ok_or_else(|| {
        ParseError::new(
            "CREATE CONSTRAINT requires a label or relationship type in the FOR clause",
            span.start,
            span.end,
        )
    })?;
    let kind = requirement.ok_or_else(|| {
        ParseError::new(
            "CREATE CONSTRAINT requires a REQUIRE clause",
            span.start,
            span.end,
        )
    })?;

    // Validate kind / entity / arity combinations.
    match (&kind, entity) {
        (ConstraintKind::Existence, _) if properties.len() != 1 => {
            return Err(ParseError::new(
                "property existence constraints must reference exactly one property",
                span.start,
                span.end,
            ));
        }
        (ConstraintKind::PropertyType(_), _) if properties.len() != 1 => {
            return Err(ParseError::new(
                "property type constraints must reference exactly one property",
                span.start,
                span.end,
            ));
        }
        (ConstraintKind::NodeKey, IndexEntityKind::Relationship) => {
            return Err(ParseError::new(
                "IS NODE KEY can only be used on nodes",
                span.start,
                span.end,
            ));
        }
        (ConstraintKind::RelationshipKey, IndexEntityKind::Node) => {
            return Err(ParseError::new(
                "IS RELATIONSHIP KEY can only be used on relationships",
                span.start,
                span.end,
            ));
        }
        _ => {}
    }

    // Composite properties must be parenthesized — single-property requirements
    // may use either form.
    if properties.len() > 1 && !composite {
        return Err(ParseError::new(
            "composite constraints require parentheses around the property list",
            span.start,
            span.end,
        ));
    }
    if properties.is_empty() {
        return Err(ParseError::new(
            "constraint REQUIRE clause must reference at least one property",
            span.start,
            span.end,
        ));
    }

    Ok(CreateConstraint {
        name,
        if_not_exists,
        entity,
        variable,
        label,
        properties,
        kind,
        span,
    })
}

fn lower_constraint_name_spec(pair: Pair<Rule>) -> Result<ConstraintNameSpec, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::parameter => {
            let raw = inner.as_str();
            Ok(ConstraintNameSpec::Parameter(raw[1..].to_string()))
        }
        Rule::symbolic_name => Ok(ConstraintNameSpec::Literal(lower_symbolic_name(inner)?)),
        _ => Err(unexpected_rule("constraint_name_spec", inner)),
    }
}

struct ParsedConstraintPattern {
    entity: IndexEntityKind,
    variable: String,
    label: String,
}

fn lower_constraint_pattern(pair: Pair<Rule>) -> Result<ParsedConstraintPattern, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::constraint_node_pattern => {
            let mut variable = String::new();
            let mut label: Option<String> = None;
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::index_var => variable = lower_index_var(p)?,
                    Rule::label_name => label = Some(lower_schema_name(p)?),
                    _ => {}
                }
            }
            Ok(ParsedConstraintPattern {
                entity: IndexEntityKind::Node,
                variable,
                label: label.ok_or_else(|| ParseError::new("missing label name", 0, 0))?,
            })
        }
        Rule::constraint_rel_pattern => {
            let mut variable = String::new();
            let mut label: Option<String> = None;
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::index_var => variable = lower_index_var(p)?,
                    Rule::rel_type_name => label = Some(lower_schema_name(p)?),
                    _ => {}
                }
            }
            Ok(ParsedConstraintPattern {
                entity: IndexEntityKind::Relationship,
                variable,
                label: label.ok_or_else(|| ParseError::new("missing relationship type", 0, 0))?,
            })
        }
        _ => Err(unexpected_rule("constraint_pattern", inner)),
    }
}

struct ParsedConstraintPropertySpec {
    properties: Vec<String>,
    composite: bool,
}

fn lower_constraint_property_spec(
    pair: Pair<Rule>,
) -> Result<ParsedConstraintPropertySpec, ParseError> {
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::constraint_property_group => {
            let mut out = Vec::new();
            for p in inner.into_inner() {
                if p.as_rule() == Rule::constraint_property {
                    out.push(lower_constraint_property(p)?);
                }
            }
            Ok(ParsedConstraintPropertySpec {
                properties: out,
                composite: true,
            })
        }
        Rule::constraint_property => Ok(ParsedConstraintPropertySpec {
            properties: vec![lower_constraint_property(inner)?],
            composite: false,
        }),
        _ => Err(unexpected_rule("constraint_property_spec", inner)),
    }
}

fn lower_constraint_property(pair: Pair<Rule>) -> Result<String, ParseError> {
    let mut key: Option<String> = None;
    for p in pair.into_inner() {
        if p.as_rule() == Rule::property_key_name {
            key = Some(lower_schema_name(p)?);
        }
    }
    key.ok_or_else(|| ParseError::new("expected property key", 0, 0))
}

fn lower_constraint_requirement(pair: Pair<Rule>) -> Result<ConstraintKind, ParseError> {
    let span = pair_span(&pair);
    let mut tokens = pair.into_inner();
    let Some(first) = tokens.next() else {
        return Err(ParseError::new(
            "unsupported REQUIRE clause shape",
            span.start,
            span.end,
        ));
    };
    let Some(second) = tokens.next() else {
        return Err(ParseError::new(
            "unsupported REQUIRE clause shape",
            span.start,
            span.end,
        ));
    };
    let third = tokens.next();
    if tokens.next().is_some() {
        return Err(ParseError::new(
            "unsupported REQUIRE clause shape",
            span.start,
            span.end,
        ));
    }

    match (first.as_rule(), second.as_rule(), third) {
        (Rule::IS, Rule::UNIQUE, None) => Ok(ConstraintKind::Unique),
        (Rule::IS, Rule::NODE, Some(third)) if third.as_rule() == Rule::KEY => {
            Ok(ConstraintKind::NodeKey)
        }
        (Rule::IS, Rule::RELATIONSHIP, Some(third)) if third.as_rule() == Rule::KEY => {
            Ok(ConstraintKind::RelationshipKey)
        }
        (Rule::IS, Rule::NOT, Some(third)) if third.as_rule() == Rule::NULL => {
            Ok(ConstraintKind::Existence)
        }
        (Rule::IS, Rule::type_predicate, Some(type_expr_pair))
            if type_expr_pair.as_rule() == Rule::constraint_type_expr =>
        {
            let type_expr = lower_constraint_type_expr(type_expr_pair)?;
            Ok(ConstraintKind::PropertyType(type_expr))
        }
        _ => Err(ParseError::new(
            "unsupported REQUIRE clause shape",
            span.start,
            span.end,
        )),
    }
}

fn lower_constraint_type_expr(pair: Pair<Rule>) -> Result<PropertyTypeExpr, ParseError> {
    let mut alternatives = Vec::new();
    for p in pair.into_inner() {
        if p.as_rule() == Rule::constraint_type_term {
            alternatives.push(lower_constraint_type_term(p)?);
        }
    }
    Ok(PropertyTypeExpr { alternatives })
}

fn lower_constraint_type_term(pair: Pair<Rule>) -> Result<PropertyTypeTerm, ParseError> {
    let span = pair_span(&pair);
    let inner = single_inner(pair)?;
    match inner.as_rule() {
        Rule::constraint_scalar_type => Ok(PropertyTypeTerm::Scalar(lower_scalar_type(inner)?)),
        Rule::constraint_list_type => {
            let mut inner_term: Option<PropertyTypeTerm> = None;
            let mut not_null = false;
            let mut saw_not = false;
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::constraint_type_term => {
                        inner_term = Some(lower_constraint_type_term(p)?);
                    }
                    Rule::NOT => saw_not = true,
                    Rule::NULL if saw_not => not_null = true,
                    _ => {}
                }
            }
            Ok(PropertyTypeTerm::List {
                inner: Box::new(inner_term.ok_or_else(|| {
                    ParseError::new("LIST<...> requires an inner type", span.start, span.end)
                })?),
                not_null,
            })
        }
        Rule::constraint_vector_type => {
            let mut coord: Option<VectorCoordType> = None;
            let mut dim: Option<u32> = None;
            for p in inner.into_inner() {
                match p.as_rule() {
                    Rule::vector_coord_type => coord = Some(lower_vector_coord_type(p)?),
                    Rule::integer_literal => {
                        let raw = p.as_str();
                        let parsed: u32 = raw.parse().map_err(|_| {
                            ParseError::new(
                                "VECTOR dimension must be a positive integer",
                                span.start,
                                span.end,
                            )
                        })?;
                        if parsed == 0 || parsed > 4096 {
                            return Err(ParseError::new(
                                "VECTOR dimension must be in 1..=4096",
                                span.start,
                                span.end,
                            ));
                        }
                        dim = Some(parsed);
                    }
                    _ => {}
                }
            }
            Ok(PropertyTypeTerm::Vector {
                coord: coord.ok_or_else(|| {
                    ParseError::new("VECTOR requires a coordinate type", span.start, span.end)
                })?,
                dimension: dim.ok_or_else(|| {
                    ParseError::new("VECTOR requires a dimension", span.start, span.end)
                })?,
            })
        }
        _ => Err(unexpected_rule("constraint_type_term", inner)),
    }
}

fn lower_scalar_type(pair: Pair<Rule>) -> Result<ScalarType, ParseError> {
    let inner = single_inner(pair)?;
    Ok(match inner.as_rule() {
        Rule::BOOLEAN => ScalarType::Boolean,
        Rule::STRING => ScalarType::String,
        Rule::INTEGER => ScalarType::Integer,
        Rule::FLOAT => ScalarType::Float,
        Rule::DATE => ScalarType::Date,
        Rule::LOCAL_TIME => ScalarType::LocalTime,
        Rule::ZONED_TIME => ScalarType::ZonedTime,
        Rule::LOCAL_DATETIME => ScalarType::LocalDateTime,
        Rule::ZONED_DATETIME => ScalarType::ZonedDateTime,
        Rule::DURATION => ScalarType::Duration,
        Rule::POINT => ScalarType::Point,
        Rule::MAP_T => ScalarType::Map,
        Rule::ANY_T => ScalarType::Any,
        _ => return Err(unexpected_rule("constraint_scalar_type", inner)),
    })
}

fn lower_vector_coord_type(pair: Pair<Rule>) -> Result<VectorCoordType, ParseError> {
    let inner = single_inner(pair)?;
    Ok(match inner.as_rule() {
        Rule::INT8 => VectorCoordType::Int8,
        Rule::INT16 => VectorCoordType::Int16,
        Rule::INT32 => VectorCoordType::Int32,
        Rule::INT64 => VectorCoordType::Int64,
        Rule::INTEGER => VectorCoordType::Int64,
        Rule::FLOAT32 => VectorCoordType::Float32,
        Rule::FLOAT64 => VectorCoordType::Float64,
        Rule::FLOAT => VectorCoordType::Float64,
        _ => return Err(unexpected_rule("vector_coord_type", inner)),
    })
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
                if let Some(single) = config.pop() {
                    if let Expr::Map(inner_entries, _) = single.1 {
                        config = inner_entries;
                    } else {
                        config.push(single);
                    }
                }
            }
        }
    }

    Ok(IndexOptions { config, span })
}
