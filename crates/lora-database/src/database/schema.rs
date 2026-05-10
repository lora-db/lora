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
use std::time::Instant;

use anyhow::{anyhow, Result};
use lora_analyzer::symbols::VarId;
use lora_ast::{
    CreateIndex, DropIndex, Expr, IndexEntityKind as AstIndexEntityKind, IndexKind as AstIndexKind,
    IndexNameSpec, IndexOptions, SchemaCommand, ShowIndexes,
};
use lora_executor::{LoraValue, Row};
use lora_store::{
    CreateIndexError, CreateIndexOutcome, DropIndexError, GraphStorage, GraphStorageMut,
    IndexConfigValue, IndexDefinition, IndexRequest, StoredIndexEntity, StoredIndexKind,
};

use crate::database::Database;

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
        let words: Vec<String> = query
            .split_whitespace()
            .take(4)
            .map(|word| word.to_ascii_uppercase())
            .collect();

        matches!(
            words.as_slice(),
            [show, indexes, ..] if show == "SHOW" && (indexes == "INDEXES" || indexes == "INDEX")
        ) || matches!(
            words.as_slice(),
            [create, index, ..] if create == "CREATE" && index == "INDEX"
        ) || matches!(
            words.as_slice(),
            [create, kind, index, ..]
                if create == "CREATE"
                    && matches!(kind.as_str(), "RANGE" | "TEXT" | "POINT" | "LOOKUP")
                    && index == "INDEX"
        ) || matches!(
            words.as_slice(),
            [drop, index, ..] if drop == "DROP" && index == "INDEX"
        )
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
            SchemaCommand::ShowIndexes(cmd) => self.execute_show_indexes(cmd),
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

    fn execute_show_indexes(&self, _cmd: &ShowIndexes) -> Result<Vec<Row>> {
        let snapshot = self.read_store();
        let indexes = snapshot.list_indexes();
        Ok(indexes.into_iter().map(definition_to_row).collect())
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
    };

    let entity = match cmd.entity {
        AstIndexEntityKind::Node => StoredIndexEntity::Node,
        AstIndexEntityKind::Relationship => StoredIndexEntity::Relationship,
    };

    let options = match cmd.options.as_ref() {
        Some(opts) => evaluate_options(opts)?,
        None => BTreeMap::new(),
    };

    Ok(IndexRequest {
        explicit_name,
        kind,
        entity,
        label: cmd.label.clone(),
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

fn definition_to_row(def: IndexDefinition) -> Row {
    let mut row = Row::new();
    let columns: [(u32, &str, LoraValue); 7] = [
        (0, "name", LoraValue::String(def.name.clone())),
        (1, "type", LoraValue::String(def.kind.as_str().to_string())),
        (
            2,
            "entityType",
            LoraValue::String(def.entity.as_str().to_string()),
        ),
        (
            3,
            "labelsOrTypes",
            LoraValue::List(match &def.label {
                Some(label) => vec![LoraValue::String(label.clone())],
                None => Vec::new(),
            }),
        ),
        (
            4,
            "properties",
            LoraValue::List(
                def.properties
                    .iter()
                    .cloned()
                    .map(LoraValue::String)
                    .collect(),
            ),
        ),
        (
            5,
            "state",
            LoraValue::String(def.state.as_str().to_string()),
        ),
        (
            6,
            "populationPercent",
            LoraValue::Float(match def.state {
                lora_store::StoredIndexState::Online => 100.0,
                lora_store::StoredIndexState::Populating => 0.0,
            }),
        ),
    ];

    for (idx, name, value) in columns {
        row.insert_named(VarId(idx), name, value);
    }
    row
}
