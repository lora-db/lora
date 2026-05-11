//! Procedure dispatch for `CALL db.index.vector.queryNodes/Relationships`.
//!
//! These calls bypass the analyzer (which doesn't yet model standalone
//! CALL). The cheap textual prefix router in [`is_procedure_call_text`]
//! catches them, the parser produces a [`StandaloneCall`], and this
//! module evaluates the procedure directly against the catalog + raw
//! storage. The result is a flat row stream that the optional YIELD
//! clause then projects.
//!
//! kNN is computed with a flat scan: enumerate all label-matching
//! entities, fetch the indexed property as [`LoraVector`], score, and
//! keep the top `k` by descending similarity. Correctness only — a
//! dedicated ANN structure is a follow-up.

use std::any::Any;
use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use lora_ast::{Expr, ProcedureInvocationKind, StandaloneCall, YieldItem};
use lora_executor::{LoraValue, Row};
use lora_store::{
    cosine_similarity_bounded, euclidean_similarity, GraphStorage, GraphStorageMut,
    IndexDefinition, LoraVector, PropertyValue, StoredIndexEntity, StoredIndexKind,
    VectorCoordinateType,
};

use crate::database::{
    row_projection::{project_yield_items, row_from_columns, ColumnLookupContext, NamedColumn},
    Database,
};

/// Cheap textual prefix detector for the procedures we route here.
pub(crate) fn is_procedure_call_text(query: &str) -> bool {
    let trimmed = query.trim_start();
    if !trimmed
        .get(..4)
        .map(|s| s.eq_ignore_ascii_case("CALL"))
        .unwrap_or(false)
    {
        return false;
    }
    let rest = trimmed[4..].trim_start();
    rest.starts_with("db.index.vector.queryNodes")
        || rest.starts_with("db.index.vector.queryRelationships")
        || rest.starts_with("db.index.fulltext.queryNodes")
        || rest.starts_with("db.index.fulltext.queryRelationships")
}

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    pub(crate) fn execute_procedure_call(
        &self,
        call: &StandaloneCall,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Vec<Row>> {
        let (name, args) = invocation_parts(&call.procedure)?;
        let qualified = name.parts.join(".");
        let procedure = Procedure::parse(&qualified)?;
        let raw_rows = match procedure.kind {
            ProcedureKind::Vector => self.vector_query(args, &params, procedure.entity)?,
            ProcedureKind::Fulltext => self.fulltext_query(args, &params, procedure.entity)?,
        };
        project_yield(raw_rows, &call.yield_items, call.yield_all)
    }

    fn vector_query(
        &self,
        args: &[Expr],
        params: &BTreeMap<String, LoraValue>,
        entity: StoredIndexEntity,
    ) -> Result<Vec<Row>> {
        let (index_name, k, query_vec) = parse_vector_args(args, params)?;
        let snapshot = self.read_store();
        let def = snapshot
            .get_index(&index_name)
            .ok_or_else(|| anyhow!("no vector index named `{index_name}`"))?;

        validate_procedure_index(&def, StoredIndexKind::Vector, entity, "vector")?;
        let label = def
            .label
            .as_deref()
            .ok_or_else(|| anyhow!("vector index `{index_name}` has no label/type"))?;
        let property = def
            .properties
            .first()
            .ok_or_else(|| anyhow!("vector index `{index_name}` has no property column"))?;

        let similarity = similarity_from_definition(&def)?;
        let expected_dim = expected_dimension(&def);
        if let Some(dim) = expected_dim {
            if query_vec.dimension != dim {
                return Err(anyhow!(
                    "query vector has dimension {} but index `{index_name}` expects {}",
                    query_vec.dimension,
                    dim
                ));
            }
        }

        let scored = score_entities(&*snapshot, entity, label, property, &query_vec, similarity);
        Ok(scored_rows(scored, Some(k), entity))
    }

    fn fulltext_query(
        &self,
        args: &[Expr],
        params: &BTreeMap<String, LoraValue>,
        entity: StoredIndexEntity,
    ) -> Result<Vec<Row>> {
        let (index_name, query_text) = parse_fulltext_args(args, params)?;
        let snapshot = self.read_store();
        let def = snapshot
            .get_index(&index_name)
            .ok_or_else(|| anyhow!("no fulltext index named `{index_name}`"))?;
        validate_procedure_index(&def, StoredIndexKind::Fulltext, entity, "fulltext")?;

        let scored = snapshot.fulltext_search(&index_name, &query_text);
        Ok(scored_rows(scored, None, entity))
    }
}

#[derive(Debug, Clone, Copy)]
struct Procedure {
    kind: ProcedureKind,
    entity: StoredIndexEntity,
}

impl Procedure {
    fn parse(qualified: &str) -> Result<Self> {
        match qualified {
            "db.index.vector.queryNodes" => Ok(Self {
                kind: ProcedureKind::Vector,
                entity: StoredIndexEntity::Node,
            }),
            "db.index.vector.queryRelationships" => Ok(Self {
                kind: ProcedureKind::Vector,
                entity: StoredIndexEntity::Relationship,
            }),
            "db.index.fulltext.queryNodes" => Ok(Self {
                kind: ProcedureKind::Fulltext,
                entity: StoredIndexEntity::Node,
            }),
            "db.index.fulltext.queryRelationships" => Ok(Self {
                kind: ProcedureKind::Fulltext,
                entity: StoredIndexEntity::Relationship,
            }),
            other => Err(anyhow!("unknown procedure: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ProcedureKind {
    Vector,
    Fulltext,
}

#[derive(Clone, Copy)]
enum Similarity {
    Cosine,
    Euclidean,
}

fn similarity_from_definition(def: &IndexDefinition) -> Result<Similarity> {
    let sim = def
        .options
        .get("vector.similarity_function")
        .and_then(|v| match v {
            lora_store::IndexConfigValue::String(s) => Some(s.as_str()),
            _ => None,
        })
        .ok_or_else(|| {
            anyhow!(
                "vector index `{}` is missing `vector.similarity_function`",
                def.name
            )
        })?;
    if sim.eq_ignore_ascii_case("cosine") {
        Ok(Similarity::Cosine)
    } else if sim.eq_ignore_ascii_case("euclidean") {
        Ok(Similarity::Euclidean)
    } else {
        Err(anyhow!("unknown similarity function `{sim}`"))
    }
}

fn expected_dimension(def: &IndexDefinition) -> Option<usize> {
    def.options.get("vector.dimensions").and_then(|v| match v {
        lora_store::IndexConfigValue::Integer(n) if *n > 0 => Some(*n as usize),
        _ => None,
    })
}

fn validate_procedure_index(
    def: &IndexDefinition,
    expected_kind: StoredIndexKind,
    expected_entity: StoredIndexEntity,
    procedure_kind: &str,
) -> Result<()> {
    if def.kind != expected_kind {
        return Err(anyhow!(
            "index `{}` is not a {} index (kind={})",
            def.name,
            expected_kind.as_str(),
            def.kind.as_str()
        ));
    }
    if def.entity != expected_entity {
        return Err(anyhow!(
            "{procedure_kind} index `{}` is on {} entities; procedure expects {}",
            def.name,
            def.entity.as_str(),
            expected_entity.as_str()
        ));
    }
    Ok(())
}

fn score_entities<S: GraphStorage + ?Sized>(
    storage: &S,
    entity: StoredIndexEntity,
    label: &str,
    property: &str,
    query: &LoraVector,
    similarity: Similarity,
) -> Vec<(u64, f64)> {
    let mut scored: Vec<(u64, f64)> = Vec::new();
    match entity {
        StoredIndexEntity::Node => {
            for id in storage.node_ids_by_label(label) {
                let Some(record) = storage.node(id) else {
                    continue;
                };
                let Some(PropertyValue::Vector(v)) = record.property(property) else {
                    continue;
                };
                if let Some(score) = score_pair(v, query, similarity) {
                    scored.push((id, score));
                }
            }
        }
        StoredIndexEntity::Relationship => {
            for id in storage.rel_ids_by_type(label) {
                let Some(record) = storage.relationship(id) else {
                    continue;
                };
                let Some(PropertyValue::Vector(v)) = record.property(property) else {
                    continue;
                };
                if let Some(score) = score_pair(v, query, similarity) {
                    scored.push((id, score));
                }
            }
        }
    }
    scored
}

fn score_pair(a: &LoraVector, b: &LoraVector, similarity: Similarity) -> Option<f64> {
    if a.dimension != b.dimension {
        return None;
    }
    match similarity {
        Similarity::Cosine => cosine_similarity_bounded(a, b),
        Similarity::Euclidean => euclidean_similarity(a, b),
    }
}

fn scored_rows(
    mut scored: Vec<(u64, f64)>,
    limit: Option<usize>,
    entity: StoredIndexEntity,
) -> Vec<Row> {
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    if let Some(limit) = limit {
        scored.truncate(limit);
    }
    scored
        .into_iter()
        .map(|(id, score)| {
            let (name, value) = match entity {
                StoredIndexEntity::Node => ("node", LoraValue::Node(id)),
                StoredIndexEntity::Relationship => ("relationship", LoraValue::Relationship(id)),
            };
            row_from_columns([
                NamedColumn::new(name, value),
                NamedColumn::new("score", LoraValue::Float(score)),
            ])
        })
        .collect()
}

fn invocation_parts(
    invocation: &ProcedureInvocationKind,
) -> Result<(&lora_ast::ProcedureName, &[Expr])> {
    match invocation {
        ProcedureInvocationKind::Explicit(call) => Ok((&call.name, call.args.as_slice())),
        ProcedureInvocationKind::Implicit(name) => Ok((name, &[])),
    }
}

fn parse_fulltext_args(
    args: &[Expr],
    params: &BTreeMap<String, LoraValue>,
) -> Result<(String, String)> {
    if args.len() < 2 || args.len() > 3 {
        return Err(anyhow!(
            "fulltext procedure expects 2 or 3 arguments (indexName, queryString, options? ); got {}",
            args.len()
        ));
    }
    let name = eval_string_arg(&args[0], params, "indexName")?;
    let query = eval_string_arg(&args[1], params, "queryString")?;
    // args[2] (options map) parses but is currently ignored — we don't
    // support skip/limit/analyzer overrides yet.
    Ok((name, query))
}

fn parse_vector_args(
    args: &[Expr],
    params: &BTreeMap<String, LoraValue>,
) -> Result<(String, usize, LoraVector)> {
    if args.len() != 3 {
        return Err(anyhow!(
            "vector procedure expects 3 arguments (indexName, k, query); got {}",
            args.len()
        ));
    }
    let name = eval_string_arg(&args[0], params, "indexName")?;
    let k = eval_usize_arg(&args[1], params, "k")?;
    if k == 0 {
        return Err(anyhow!("k must be positive"));
    }
    let query = eval_vector_arg(&args[2], params)?;
    Ok((name, k, query))
}

fn eval_string_arg(
    expr: &Expr,
    params: &BTreeMap<String, LoraValue>,
    label: &str,
) -> Result<String> {
    match resolve_literal(expr, params)? {
        LoraValue::String(s) => Ok(s),
        other => Err(anyhow!("{label} must be a string, got {other:?}")),
    }
}

fn eval_usize_arg(expr: &Expr, params: &BTreeMap<String, LoraValue>, label: &str) -> Result<usize> {
    match resolve_literal(expr, params)? {
        LoraValue::Int(n) if n >= 0 => {
            usize::try_from(n).map_err(|_| anyhow!("{label} is too large for this platform: {n}"))
        }
        other => Err(anyhow!(
            "{label} must be a non-negative integer, got {other:?}"
        )),
    }
}

fn eval_vector_arg(expr: &Expr, params: &BTreeMap<String, LoraValue>) -> Result<LoraVector> {
    let value = resolve_literal(expr, params)?;
    match value {
        LoraValue::Vector(v) => Ok(v),
        LoraValue::List(items) => list_to_float32_vector(&items),
        other => Err(anyhow!(
            "query must be a VECTOR or LIST<NUMBER>; got {other:?}"
        )),
    }
}

fn list_to_float32_vector(items: &[LoraValue]) -> Result<LoraVector> {
    let mut raw = Vec::with_capacity(items.len());
    for item in items {
        let coord = match item {
            LoraValue::Int(n) => lora_store::RawCoordinate::Int(*n),
            LoraValue::Float(f) => lora_store::RawCoordinate::Float(*f),
            other => {
                return Err(anyhow!(
                    "query list elements must be INTEGER or FLOAT; got {other:?}"
                ))
            }
        };
        raw.push(coord);
    }
    let dim = items.len() as i64;
    LoraVector::try_new(raw, dim, VectorCoordinateType::Float32)
        .map_err(|e| anyhow!("invalid query vector: {e}"))
}

fn resolve_literal(expr: &Expr, params: &BTreeMap<String, LoraValue>) -> Result<LoraValue> {
    match expr {
        Expr::Integer(n, _) => Ok(LoraValue::Int(*n)),
        Expr::Float(f, _) => Ok(LoraValue::Float(*f)),
        Expr::String(s, _) => Ok(LoraValue::String(s.clone())),
        Expr::Bool(b, _) => Ok(LoraValue::Bool(*b)),
        Expr::Null(_) => Ok(LoraValue::Null),
        Expr::Parameter(name, _) => params
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow!("parameter `${name}` not supplied")),
        Expr::List(items, _) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(resolve_literal(item, params)?);
            }
            Ok(LoraValue::List(out))
        }
        Expr::Unary {
            op: lora_ast::UnaryOp::Neg,
            expr: inner,
            ..
        } => match resolve_literal(inner, params)? {
            LoraValue::Int(n) => Ok(LoraValue::Int(-n)),
            LoraValue::Float(f) => Ok(LoraValue::Float(-f)),
            other => Err(anyhow!("cannot negate {other:?}")),
        },
        Expr::FunctionCall { name, args, .. } if matches_vector_ctor(name) => {
            resolve_vector_ctor(args, params)
        }
        other => Err(anyhow!(
            "procedure arguments must be literals or $params; got {other:?}"
        )),
    }
}

fn matches_vector_ctor(name: &[String]) -> bool {
    matches!(name, [n] if n.eq_ignore_ascii_case("vector"))
}

/// Inline support for `vector(list, dim, COORD_TYPE)` literal calls
/// inside procedure-argument position. The coordinate type may be a
/// bare identifier (`FLOAT32`), a string (`'FLOAT32'`), or a $param.
fn resolve_vector_ctor(args: &[Expr], params: &BTreeMap<String, LoraValue>) -> Result<LoraValue> {
    if args.len() != 3 {
        return Err(anyhow!("vector() expects 3 arguments, got {}", args.len()));
    }
    let LoraValue::List(values) = resolve_literal(&args[0], params)? else {
        return Err(anyhow!("vector() first argument must be a list"));
    };
    let dim_value = resolve_literal(&args[1], params)?;
    let LoraValue::Int(dim) = dim_value else {
        return Err(anyhow!(
            "vector() dimension must be integer, got {dim_value:?}"
        ));
    };
    let coord = coord_type_from_expr(&args[2], params)?;
    let mut raw = Vec::with_capacity(values.len());
    for v in values {
        match v {
            LoraValue::Int(n) => raw.push(lora_store::RawCoordinate::Int(n)),
            LoraValue::Float(f) => raw.push(lora_store::RawCoordinate::Float(f)),
            other => {
                return Err(anyhow!(
                    "vector() list elements must be INTEGER or FLOAT; got {other:?}"
                ))
            }
        }
    }
    let v = LoraVector::try_new(raw, dim, coord).map_err(|e| anyhow!("vector(): {e}"))?;
    Ok(LoraValue::Vector(v))
}

fn coord_type_from_expr(
    expr: &Expr,
    params: &BTreeMap<String, LoraValue>,
) -> Result<VectorCoordinateType> {
    let name = match expr {
        Expr::Variable(v) => v.name.clone(),
        Expr::String(s, _) => s.clone(),
        Expr::Parameter(_, _) => {
            let value = resolve_literal(expr, params)?;
            let LoraValue::String(s) = value else {
                return Err(anyhow!(
                    "coordinate type parameter must be a string; got {value:?}"
                ));
            };
            s
        }
        other => {
            return Err(anyhow!("invalid coordinate type expression: {other:?}"));
        }
    };
    VectorCoordinateType::parse(&name).ok_or_else(|| anyhow!("unknown coordinate type `{name}`"))
}

fn project_yield(rows: Vec<Row>, items: &[YieldItem], yield_all: bool) -> Result<Vec<Row>> {
    // `yield_all` corresponds to a future `YIELD *`; absence of both
    // (empty items, not yield_all) means no projection — pass through.
    project_yield_items(rows, items, yield_all, ColumnLookupContext::ProcedureYield)
}
