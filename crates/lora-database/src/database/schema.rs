//! Schema-command execution: routes `CREATE INDEX` and `SHOW INDEXES`
//! straight to the catalog instead of going through the read/write
//! query pipeline.
//!
//! DDL bypasses the analyzer/compiler entirely. `CREATE INDEX` mutates
//! the in-memory catalog, emits a catalog mutation event for WAL/archive
//! replay, and populates the backing RANGE/TEXT/POINT structures as a
//! side effect; `SHOW INDEXES` is a pure read.
//!
//! ## Why not a physical operator?
//!
//! Index DDL is a catalog mutation, not a row-producing op. Threading
//! it through the full plan tree would force every executor cursor to
//! understand catalog-only side effects. Routing here keeps the
//! existing pipeline focused on row-producing work.

use std::any::Any;
use std::collections::BTreeMap;

use web_time::Instant;

use anyhow::{anyhow, Result};
use lora_ast::{
    ConstraintKind as AstConstraintKind, ConstraintNameSpec, CreateConstraint, CreateIndex,
    DropConstraint, DropIndex, Expr, IndexEntityKind as AstIndexEntityKind,
    IndexKind as AstIndexKind, IndexKindFilter, IndexNameSpec, IndexOptions,
    PropertyTypeExpr as AstPropertyTypeExpr, PropertyTypeTerm as AstPropertyTypeTerm,
    ScalarType as AstScalarType, SchemaCommand, ShowConstraints, ShowIndexes,
    VectorCoordType as AstVectorCoordType,
};
use lora_executor::{LoraValue, Row};
use lora_store::{
    ConstraintDefinition, ConstraintRequest, CreateConstraintError, CreateIndexError,
    CreateIndexOutcome, DropConstraintError, DropIndexError, GraphStorage, GraphStorageMut,
    IndexConfigValue, IndexDefinition, IndexRequest, StoredConstraintKind, StoredIndexEntity,
    StoredIndexKind, StoredPropertyType, StoredPropertyTypeTerm, StoredScalarType,
    StoredVectorCoordType,
};

use crate::database::{
    row_projection::{row_from_columns, NamedColumn},
    Database,
};

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Cheap textual prefix check that lets us route DDL before invoking
    /// the parser-cache + analyzer + compiler pipeline. Kept independent
    /// of the full grammar: false negatives only delay routing (DDL falls
    /// into the regular parser, which then fails); false positives are
    /// impossible because the actual parser accepts these prefixes only
    /// for schema commands.
    pub(crate) fn is_schema_command_text(query: &str) -> bool {
        let mut words = query.split_whitespace();
        let Some(first) = words.next() else {
            return false;
        };
        let Some(second) = words.next() else {
            return false;
        };

        if first.eq_ignore_ascii_case("SHOW") {
            return is_index_keyword(second)
                || is_constraint_keyword(second)
                || (is_show_index_filter(second) && words.next().is_some_and(is_index_keyword));
        }

        if first.eq_ignore_ascii_case("CREATE") {
            return second.eq_ignore_ascii_case("CONSTRAINT")
                || second.eq_ignore_ascii_case("INDEX")
                || (is_create_index_kind(second)
                    && words
                        .next()
                        .is_some_and(|word| word.eq_ignore_ascii_case("INDEX")));
        }

        first.eq_ignore_ascii_case("DROP")
            && (second.eq_ignore_ascii_case("INDEX") || second.eq_ignore_ascii_case("CONSTRAINT"))
    }

    pub(crate) fn execute_schema_command(
        &self,
        command: &SchemaCommand,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
    ) -> Result<Vec<Row>> {
        match command {
            SchemaCommand::CreateIndex(cmd) => self.execute_create_index(cmd, params, deadline),
            SchemaCommand::DropIndex(cmd) => self.execute_drop_index(cmd, params),
            SchemaCommand::ShowIndexes(cmd) => self.execute_show_indexes(cmd, &params),
            SchemaCommand::CreateConstraint(cmd) => {
                self.execute_create_constraint(cmd, params, deadline)
            }
            SchemaCommand::DropConstraint(cmd) => self.execute_drop_constraint(cmd, params),
            SchemaCommand::ShowConstraints(cmd) => self.execute_show_constraints(cmd, &params),
        }
    }

    fn execute_create_index(
        &self,
        cmd: &CreateIndex,
        params: BTreeMap<String, LoraValue>,
        _deadline: Option<Instant>,
    ) -> Result<Vec<Row>> {
        let request = build_index_request(cmd, &params)?;
        let if_not_exists = cmd.if_not_exists;

        // Catalog mutation goes through the canonical write path so the
        // writer lock is held for the duration and the store can emit
        // the durable catalog mutation event used by WAL/archive replay.
        let outcome = self.with_logged_store_mut(|store| {
            store
                .create_index(request, if_not_exists)
                .map_err(map_create_index_error)
        })?;

        match outcome {
            CreateIndexOutcome::Created(_def) => Ok(Vec::new()),
            CreateIndexOutcome::NoOpExists(_def) => {
                // The Cypher reference returns *no rows* and a notification
                // for IF NOT EXISTS no-ops. Notifications are not a
                // first-class result kind in lora yet; surface this via
                // the absence of a row, mirroring the row count.
                Ok(Vec::new())
            }
        }
    }

    fn execute_show_indexes(
        &self,
        cmd: &ShowIndexes,
        params: &BTreeMap<String, LoraValue>,
    ) -> Result<Vec<Row>> {
        let snapshot = self.read_store();
        let indexes = snapshot.list_indexes();
        let rows: Vec<Row> = indexes
            .into_iter()
            .filter(|def| index_matches_filter(def, cmd.filter))
            .map(definition_to_row)
            .collect();
        match &cmd.pipeline {
            Some(pipeline) => super::show_pipeline::apply_pipeline(rows, pipeline, params),
            None => Ok(rows),
        }
    }

    fn execute_drop_index(
        &self,
        cmd: &DropIndex,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Vec<Row>> {
        let name = match &cmd.name {
            IndexNameSpec::Literal(n) => n.clone(),
            IndexNameSpec::Parameter(p) => resolve_string_param(p, &params)?,
        };
        let if_exists = cmd.if_exists;
        let _outcome = self.with_logged_store_mut(|store| {
            store
                .drop_index(&name, if_exists)
                .map_err(map_drop_index_error)
        })?;
        Ok(Vec::new())
    }

    fn execute_create_constraint(
        &self,
        cmd: &CreateConstraint,
        params: BTreeMap<String, LoraValue>,
        _deadline: Option<Instant>,
    ) -> Result<Vec<Row>> {
        let request = build_constraint_request(cmd, &params)?;
        let if_not_exists = cmd.if_not_exists;
        let _outcome = self.with_logged_store_mut(|store| {
            store
                .create_constraint(request, if_not_exists)
                .map_err(map_create_constraint_error)
        })?;
        Ok(Vec::new())
    }

    fn execute_drop_constraint(
        &self,
        cmd: &DropConstraint,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Vec<Row>> {
        let name = match &cmd.name {
            ConstraintNameSpec::Literal(n) => n.clone(),
            ConstraintNameSpec::Parameter(p) => resolve_string_param(p, &params)?,
        };
        let if_exists = cmd.if_exists;
        let _outcome = self.with_logged_store_mut(|store| {
            store
                .drop_constraint(&name, if_exists)
                .map_err(map_drop_constraint_error)
        })?;
        Ok(Vec::new())
    }

    fn execute_show_constraints(
        &self,
        cmd: &ShowConstraints,
        params: &BTreeMap<String, LoraValue>,
    ) -> Result<Vec<Row>> {
        let snapshot = self.read_store();
        let constraints = snapshot.list_constraints();
        let rows: Vec<Row> = constraints.into_iter().map(constraint_to_row).collect();
        match &cmd.pipeline {
            Some(pipeline) => super::show_pipeline::apply_pipeline(rows, pipeline, params),
            None => Ok(rows),
        }
    }
}

fn is_index_keyword(word: &str) -> bool {
    word.eq_ignore_ascii_case("INDEXES") || word.eq_ignore_ascii_case("INDEX")
}

fn is_constraint_keyword(word: &str) -> bool {
    word.eq_ignore_ascii_case("CONSTRAINTS") || word.eq_ignore_ascii_case("CONSTRAINT")
}

fn is_show_index_filter(word: &str) -> bool {
    word.eq_ignore_ascii_case("ALL")
        || word.eq_ignore_ascii_case("RANGE")
        || word.eq_ignore_ascii_case("TEXT")
        || word.eq_ignore_ascii_case("POINT")
        || word.eq_ignore_ascii_case("LOOKUP")
        || word.eq_ignore_ascii_case("FULLTEXT")
        || word.eq_ignore_ascii_case("VECTOR")
}

fn is_create_index_kind(word: &str) -> bool {
    word.eq_ignore_ascii_case("RANGE")
        || word.eq_ignore_ascii_case("TEXT")
        || word.eq_ignore_ascii_case("POINT")
        || word.eq_ignore_ascii_case("LOOKUP")
        || word.eq_ignore_ascii_case("FULLTEXT")
        || word.eq_ignore_ascii_case("VECTOR")
}

fn index_matches_filter(def: &IndexDefinition, filter: Option<IndexKindFilter>) -> bool {
    let Some(filter) = filter else { return true };
    match filter {
        IndexKindFilter::All => true,
        IndexKindFilter::Range => matches!(def.kind, StoredIndexKind::Range),
        IndexKindFilter::Text => matches!(def.kind, StoredIndexKind::Text),
        IndexKindFilter::Point => matches!(def.kind, StoredIndexKind::Point),
        IndexKindFilter::Lookup => matches!(def.kind, StoredIndexKind::Lookup),
        IndexKindFilter::Fulltext => matches!(def.kind, StoredIndexKind::Fulltext),
        IndexKindFilter::Vector => matches!(def.kind, StoredIndexKind::Vector),
    }
}

fn build_index_request(
    cmd: &CreateIndex,
    params: &BTreeMap<String, LoraValue>,
) -> Result<IndexRequest> {
    let explicit_name = match &cmd.name {
        Some(IndexNameSpec::Literal(name)) => Some(name.clone()),
        Some(IndexNameSpec::Parameter(param)) => Some(resolve_string_param(param, params)?),
        None => None,
    };

    let kind = match cmd.kind {
        AstIndexKind::Range => StoredIndexKind::Range,
        AstIndexKind::Text => StoredIndexKind::Text,
        AstIndexKind::Point => StoredIndexKind::Point,
        AstIndexKind::Lookup => StoredIndexKind::Lookup,
        AstIndexKind::Vector => StoredIndexKind::Vector,
        AstIndexKind::Fulltext => StoredIndexKind::Fulltext,
    };

    let entity = match cmd.entity {
        AstIndexEntityKind::Node => StoredIndexEntity::Node,
        AstIndexEntityKind::Relationship => StoredIndexEntity::Relationship,
    };

    let options = match cmd.options.as_ref() {
        Some(opts) => evaluate_options(opts)?,
        None => BTreeMap::new(),
    };

    if kind == StoredIndexKind::Vector {
        if cmd.properties.len() != 1 {
            return Err(anyhow!(
                "VECTOR indexes are single-property; got {} properties",
                cmd.properties.len()
            ));
        }
        validate_vector_options(&options)?;
    }

    if kind == StoredIndexKind::Fulltext {
        validate_fulltext_options(&options)?;
    }

    Ok(IndexRequest {
        explicit_name,
        kind,
        entity,
        label: cmd.label.clone(),
        additional_labels: cmd.additional_labels.clone(),
        properties: cmd.properties.clone(),
        options,
    })
}

fn resolve_string_param(name: &str, params: &BTreeMap<String, LoraValue>) -> Result<String> {
    match params.get(name) {
        Some(LoraValue::String(s)) => Ok(s.clone()),
        Some(other) => Err(anyhow!(
            "parameter `${name}` for an index name must be a string, got {:?}",
            other
        )),
        None => Err(anyhow!(
            "parameter `${name}` was not supplied for index name"
        )),
    }
}

fn evaluate_options(opts: &IndexOptions) -> Result<BTreeMap<String, IndexConfigValue>> {
    let mut out = BTreeMap::new();
    for (key, expr) in &opts.config {
        out.insert(key.clone(), evaluate_literal_expr(expr)?);
    }
    Ok(out)
}

/// Validate the OPTIONS map for `CREATE FULLTEXT INDEX`. Today the
/// engine only ships a single "standard" analyzer (lowercase +
/// non-alphanumeric tokenisation), so we accept that name and reject
/// anything else with a clear error. `fulltext.eventually_consistent`
/// parses but is currently a no-op — we apply maintenance synchronously.
fn validate_fulltext_options(opts: &BTreeMap<String, IndexConfigValue>) -> Result<()> {
    if let Some(analyzer) = opts.get("fulltext.analyzer") {
        let name = match analyzer {
            IndexConfigValue::String(s) => s.as_str(),
            other => {
                return Err(anyhow!(
                    "`fulltext.analyzer` must be a string, got {other:?}"
                ))
            }
        };
        if !(name.eq_ignore_ascii_case("standard") || name.eq_ignore_ascii_case("simple")) {
            return Err(anyhow!(
                "fulltext analyzer `{name}` is not supported; only `standard` and `simple` are currently available"
            ));
        }
    }
    if let Some(ec) = opts.get("fulltext.eventually_consistent") {
        match ec {
            IndexConfigValue::Bool(_) => {}
            other => {
                return Err(anyhow!(
                    "`fulltext.eventually_consistent` must be a boolean, got {other:?}"
                ))
            }
        }
    }
    Ok(())
}

/// Validate the OPTIONS map for `CREATE VECTOR INDEX`. The parser
/// hoists the inner `indexConfig: { ... }` map up so `options` is the
/// keys directly: `vector.dimensions` and `vector.similarity_function`.
fn validate_vector_options(opts: &BTreeMap<String, IndexConfigValue>) -> Result<()> {
    let dim = opts
        .get("vector.dimensions")
        .ok_or_else(|| anyhow!(
            "CREATE VECTOR INDEX requires OPTIONS {{ indexConfig: {{ `vector.dimensions`: N, `vector.similarity_function`: '...' }} }}"
        ))?;
    let dim = match dim {
        IndexConfigValue::Integer(n) => *n,
        other => {
            return Err(anyhow!(
                "`vector.dimensions` must be a positive integer, got {other:?}"
            ))
        }
    };
    if !(1..=4096).contains(&dim) {
        return Err(anyhow!(
            "`vector.dimensions` must be in 1..=4096, got {dim}"
        ));
    }
    let sim = opts
        .get("vector.similarity_function")
        .ok_or_else(|| anyhow!("`vector.similarity_function` is required"))?;
    let sim = match sim {
        IndexConfigValue::String(s) => s.as_str(),
        other => {
            return Err(anyhow!(
                "`vector.similarity_function` must be a string, got {other:?}"
            ))
        }
    };
    let normalized = sim.to_ascii_lowercase();
    let known = matches!(
        normalized.as_str(),
        "cosine" | "euclidean" | "dot" | "dot_product" | "manhattan"
    );
    if !known {
        return Err(anyhow!(
            "`vector.similarity_function` must be one of 'cosine', 'euclidean', 'dot', 'manhattan', got '{sim}'"
        ));
    }

    // Optional knobs. `indexProvider` selects flat (default) vs HNSW;
    // the `vector.hnsw.*` keys are honored only when the provider is
    // HNSW but we validate ranges regardless to surface typos at DDL
    // time rather than silently ignoring them.
    if let Some(provider) = opts.get("vector.indexProvider") {
        let p = match provider {
            IndexConfigValue::String(s) => s.as_str(),
            other => {
                return Err(anyhow!(
                    "`vector.indexProvider` must be a string, got {other:?}"
                ))
            }
        };
        if !(p.eq_ignore_ascii_case("flat") || p.eq_ignore_ascii_case("hnsw")) {
            return Err(anyhow!(
                "`vector.indexProvider` must be 'flat' or 'hnsw', got '{p}'"
            ));
        }
    }

    validate_hnsw_int(opts, "vector.hnsw.m", 4, 128)?;
    validate_hnsw_int(opts, "vector.hnsw.ef_construction", 16, 2000)?;
    validate_hnsw_int(opts, "vector.hnsw.ef_search", 16, 2000)?;

    if let Some(value) = opts.get("vector.populate.async") {
        match value {
            IndexConfigValue::Bool(_) => {}
            other => {
                return Err(anyhow!(
                    "`vector.populate.async` must be a boolean, got {other:?}"
                ));
            }
        }
    }

    if let Some(value) = opts.get("vector.hnsw.quantization") {
        let q = match value {
            IndexConfigValue::String(s) => s.as_str(),
            other => {
                return Err(anyhow!(
                    "`vector.hnsw.quantization` must be a string, got {other:?}"
                ));
            }
        };
        if !(q.eq_ignore_ascii_case("none") || q.eq_ignore_ascii_case("int8")) {
            return Err(anyhow!(
                "`vector.hnsw.quantization` must be 'none' or 'int8', got '{q}'"
            ));
        }
        // int8 stores i8 coords; only cosine (scale-invariant)
        // preserves correct ranking under the implicit ×127 scaling.
        // Other metrics return a degenerate score range.
        if q.eq_ignore_ascii_case("int8") && !normalized.eq_ignore_ascii_case("cosine") {
            return Err(anyhow!(
                "`vector.hnsw.quantization` = 'int8' currently requires `vector.similarity_function` = 'cosine'"
            ));
        }
    }

    Ok(())
}

fn validate_hnsw_int(
    opts: &BTreeMap<String, IndexConfigValue>,
    key: &str,
    min: i64,
    max: i64,
) -> Result<()> {
    let Some(value) = opts.get(key) else {
        return Ok(());
    };
    let n = match value {
        IndexConfigValue::Integer(n) => *n,
        other => {
            return Err(anyhow!("`{key}` must be a positive integer, got {other:?}"));
        }
    };
    if !(min..=max).contains(&n) {
        return Err(anyhow!("`{key}` must be in {min}..={max}, got {n}"));
    }
    Ok(())
}

fn evaluate_literal_expr(expr: &Expr) -> Result<IndexConfigValue> {
    match expr {
        Expr::Integer(v, _) => Ok(IndexConfigValue::Integer(*v)),
        Expr::Float(v, _) => Ok(IndexConfigValue::Number(*v)),
        Expr::String(v, _) => Ok(IndexConfigValue::String(v.clone())),
        Expr::Bool(v, _) => Ok(IndexConfigValue::Bool(*v)),
        Expr::Null(_) => Ok(IndexConfigValue::Null),
        Expr::List(items, _) => {
            let values = items
                .iter()
                .map(evaluate_literal_expr)
                .collect::<Result<Vec<_>>>()?;
            Ok(IndexConfigValue::List(values))
        }
        Expr::Map(entries, _) => {
            let mut map = BTreeMap::new();
            for (k, v) in entries {
                map.insert(k.clone(), evaluate_literal_expr(v)?);
            }
            Ok(IndexConfigValue::Map(map))
        }
        Expr::Unary {
            op: lora_ast::UnaryOp::Neg,
            expr: inner,
            ..
        } => match evaluate_literal_expr(inner)? {
            IndexConfigValue::Integer(v) => Ok(IndexConfigValue::Integer(-v)),
            IndexConfigValue::Number(v) => Ok(IndexConfigValue::Number(-v)),
            other => Err(anyhow!(
                "unary minus only valid on numbers in OPTIONS, found {:?}",
                other
            )),
        },
        other => Err(anyhow!(
            "OPTIONS values must be literals; encountered non-literal expression: {:?}",
            other
        )),
    }
}

fn map_create_index_error(err: CreateIndexError) -> anyhow::Error {
    anyhow!("[{}] {err}", err.gql_status())
}

fn map_drop_index_error(err: DropIndexError) -> anyhow::Error {
    anyhow!("[{}] {err}", err.gql_status())
}

fn map_create_constraint_error(err: CreateConstraintError) -> anyhow::Error {
    anyhow!("[{}] {err}", err.gql_status())
}

fn map_drop_constraint_error(err: DropConstraintError) -> anyhow::Error {
    anyhow!("[{}] {err}", err.gql_status())
}

fn build_constraint_request(
    cmd: &CreateConstraint,
    params: &BTreeMap<String, LoraValue>,
) -> Result<ConstraintRequest> {
    let name = match &cmd.name {
        ConstraintNameSpec::Literal(n) => n.clone(),
        ConstraintNameSpec::Parameter(p) => resolve_string_param(p, params)?,
    };
    let entity = match cmd.entity {
        AstIndexEntityKind::Node => StoredIndexEntity::Node,
        AstIndexEntityKind::Relationship => StoredIndexEntity::Relationship,
    };
    let kind = match &cmd.kind {
        AstConstraintKind::Unique => StoredConstraintKind::Unique,
        AstConstraintKind::Existence => StoredConstraintKind::Existence,
        AstConstraintKind::NodeKey => StoredConstraintKind::NodeKey,
        AstConstraintKind::RelationshipKey => StoredConstraintKind::RelationshipKey,
        AstConstraintKind::PropertyType(expr) => {
            StoredConstraintKind::PropertyType(lower_property_type(expr)?)
        }
    };
    Ok(ConstraintRequest {
        name,
        kind,
        entity,
        label: cmd.label.clone(),
        properties: cmd.properties.clone(),
    })
}

fn lower_property_type(expr: &AstPropertyTypeExpr) -> Result<StoredPropertyType> {
    let mut alternatives = Vec::with_capacity(expr.alternatives.len());
    for term in &expr.alternatives {
        alternatives.push(lower_property_type_term(term)?);
    }
    Ok(StoredPropertyType { alternatives })
}

fn lower_property_type_term(term: &AstPropertyTypeTerm) -> Result<StoredPropertyTypeTerm> {
    match term {
        AstPropertyTypeTerm::Scalar(scalar) => {
            let mapped = match scalar {
                AstScalarType::Boolean => StoredScalarType::Boolean,
                AstScalarType::String => StoredScalarType::String,
                AstScalarType::Integer => StoredScalarType::Integer,
                AstScalarType::Float => StoredScalarType::Float,
                AstScalarType::Date => StoredScalarType::Date,
                AstScalarType::LocalTime => StoredScalarType::LocalTime,
                AstScalarType::ZonedTime => StoredScalarType::ZonedTime,
                AstScalarType::LocalDateTime => StoredScalarType::LocalDateTime,
                AstScalarType::ZonedDateTime => StoredScalarType::ZonedDateTime,
                AstScalarType::Duration => StoredScalarType::Duration,
                AstScalarType::Point => StoredScalarType::Point,
                AstScalarType::Map => {
                    return Err(anyhow!(
                        "[22N90] property type unsupported in constraint: MAP is not supported in property type constraints"
                    ));
                }
                AstScalarType::Any => {
                    return Err(anyhow!(
                        "[22N90] property type unsupported in constraint: ANY is not supported in property type constraints"
                    ));
                }
            };
            Ok(StoredPropertyTypeTerm::Scalar(mapped))
        }
        AstPropertyTypeTerm::List { inner, not_null } => {
            if !not_null {
                return Err(anyhow!(
                    "[22N90] property type unsupported in constraint: LIST element type must be `NOT NULL`"
                ));
            }
            let lowered = lower_property_type_term(inner)?;
            Ok(StoredPropertyTypeTerm::List {
                inner: Box::new(lowered),
                not_null: *not_null,
            })
        }
        AstPropertyTypeTerm::Vector { coord, dimension } => {
            let coord = match coord {
                AstVectorCoordType::Int8 => StoredVectorCoordType::Int8,
                AstVectorCoordType::Int16 => StoredVectorCoordType::Int16,
                AstVectorCoordType::Int32 => StoredVectorCoordType::Int32,
                AstVectorCoordType::Int64 => StoredVectorCoordType::Int64,
                AstVectorCoordType::Float32 => StoredVectorCoordType::Float32,
                AstVectorCoordType::Float64 => StoredVectorCoordType::Float64,
            };
            Ok(StoredPropertyTypeTerm::Vector {
                coord,
                dimension: *dimension,
            })
        }
    }
}

fn constraint_to_row(def: ConstraintDefinition) -> Row {
    let ConstraintDefinition {
        name,
        kind,
        entity,
        label,
        properties,
        owned_index,
    } = def;
    let entity_str = entity.as_str().to_string();
    let type_tag = kind.type_tag(entity).to_string();
    let property_type = property_type_display(&kind);
    let owned_index = owned_index
        .map(LoraValue::String)
        .unwrap_or(LoraValue::Null);

    row_from_columns([
        NamedColumn::new("name", LoraValue::String(name)),
        NamedColumn::new("type", LoraValue::String(type_tag)),
        NamedColumn::new("entityType", LoraValue::String(entity_str)),
        NamedColumn::new(
            "labelsOrTypes",
            LoraValue::List(vec![LoraValue::String(label)]),
        ),
        NamedColumn::new(
            "properties",
            LoraValue::List(properties.into_iter().map(LoraValue::String).collect()),
        ),
        NamedColumn::new("ownedIndex", owned_index),
        NamedColumn::new(
            "propertyType",
            property_type
                .map(LoraValue::String)
                .unwrap_or(LoraValue::Null),
        ),
    ])
}

fn property_type_display(kind: &StoredConstraintKind) -> Option<String> {
    match kind {
        StoredConstraintKind::PropertyType(t) => Some(t.to_string()),
        _ => None,
    }
}

fn definition_to_row(def: IndexDefinition) -> Row {
    let IndexDefinition {
        name,
        kind,
        entity,
        label,
        additional_labels,
        properties,
        options,
        state,
        ..
    } = def;
    let labels = label
        .into_iter()
        .chain(additional_labels)
        .map(LoraValue::String)
        .collect();
    let options_map: BTreeMap<String, LoraValue> = options
        .into_iter()
        .map(|(k, v)| (k, index_config_to_lora_value(v)))
        .collect();

    row_from_columns([
        NamedColumn::new("name", LoraValue::String(name)),
        NamedColumn::new("type", LoraValue::String(kind.as_str().to_string())),
        NamedColumn::new("entityType", LoraValue::String(entity.as_str().to_string())),
        NamedColumn::new("labelsOrTypes", LoraValue::List(labels)),
        NamedColumn::new(
            "properties",
            LoraValue::List(properties.into_iter().map(LoraValue::String).collect()),
        ),
        NamedColumn::new("options", LoraValue::Map(options_map)),
        NamedColumn::new("state", LoraValue::String(state.as_str().to_string())),
        NamedColumn::new(
            "populationPercent",
            LoraValue::Float(match state {
                lora_store::StoredIndexState::Online => 100.0,
                lora_store::StoredIndexState::Populating => 0.0,
            }),
        ),
    ])
}

/// Translate a catalog `IndexConfigValue` into a Cypher-native
/// `LoraValue` so `SHOW INDEXES` surfaces the user's OPTIONS map
/// directly. Nested maps and lists recurse.
fn index_config_to_lora_value(v: IndexConfigValue) -> LoraValue {
    match v {
        IndexConfigValue::Number(n) => LoraValue::Float(n),
        IndexConfigValue::Integer(n) => LoraValue::Int(n),
        IndexConfigValue::String(s) => LoraValue::String(s),
        IndexConfigValue::Bool(b) => LoraValue::Bool(b),
        IndexConfigValue::List(xs) => {
            LoraValue::List(xs.into_iter().map(index_config_to_lora_value).collect())
        }
        IndexConfigValue::Map(m) => LoraValue::Map(
            m.into_iter()
                .map(|(k, v)| (k, index_config_to_lora_value(v)))
                .collect(),
        ),
        IndexConfigValue::Null => LoraValue::Null,
    }
}
