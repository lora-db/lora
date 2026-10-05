//! Auto-commit write path for `InMemoryGraph`.
//!
//! This module is the dispatcher between [`Database::execute_with_params`]
//! and the canonical mutating shape in [`super::write_guard`]. It
//! builds a `MutableExecutor` against the live store and hands it to
//! [`Database::run_with_durable_recorder`], which owns the writer
//! mutex, recorder arm/commit/abort lifecycle, and the
//! managed-snapshot trigger. The single-writer design (`Arc::make_mut`
//! against the live `Arc<S>`) and the failure trade-off — a query
//! that fails mid-execution can leave the live graph partially
//! mutated, but never the durable log — are documented in
//! [`super::write_guard`].

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;
use web_time::Instant;

use anyhow::Result;
use lora_analyzer::{
    LiteralValue, ResolvedExpr, ResolvedPattern, ResolvedPatternElement, ResolvedSetItem,
};
use lora_compiler::physical::{PhysicalOp, PhysicalPlan};
use lora_compiler::CompiledQuery;
use lora_executor::{LoraValue, MutableExecutionContext, MutableExecutor, Row};
use lora_store::{ConstraintDefinition, GraphStorage, GraphStorageMut, StoredIndexEntity};

use crate::database::Database;

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Auto-commit a mutating query. Builds a `MutableExecutor`
    /// against the staged graph and routes through the canonical
    /// write shape in [`Database::run_with_durable_recorder`].
    pub(crate) fn execute_mutating_optimistic(
        &self,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
        compiled: &Arc<CompiledQuery>,
    ) -> Result<Vec<Row>> {
        let run = |staged: &mut S| {
            let mut executor = MutableExecutor::with_deadline(
                MutableExecutionContext {
                    storage: staged,
                    params,
                },
                deadline,
            );
            executor
                .execute_compiled_rows(compiled)
                .map_err(anyhow::Error::from)
        };

        // The live fast path mutates the graph in place with no rollback,
        // which is only sound for plans that cannot fail midway. A deadline
        // (timeout or cancellation) can stop any plan midway, so bounded
        // writes take the staged path, where a failure discards every
        // change instead of leaving a partial write behind. So does a write
        // a constraint can reject on a later row (a duplicate key in an
        // `UNWIND … CREATE`, a `SET` that collides on its second node).
        if deadline.is_none()
            && live_fast_path_safe(compiled)
            && !constraints_may_reject(&self.store.load_full().list_constraints(), compiled)
        {
            let may_delete = compiled
                .physical
                .nodes
                .iter()
                .any(|op| matches!(op, PhysicalOp::Delete(_)));
            self.run_live_fast_with_durable_recorder(may_delete, run)
        } else {
            self.run_with_durable_recorder(run)
        }
    }
}

/// Whether a constraint can reject one of the plan's writes. The fast path
/// only creates nodes and sets properties: a created label with any
/// constraint can fail, and so can a `SET` while any constraint exists
/// (whether its targets are nodes or relationships, and which labels or
/// type they carry, is only known per row).
fn constraints_may_reject(constraints: &[ConstraintDefinition], compiled: &CompiledQuery) -> bool {
    let on_nodes = |c: &&ConstraintDefinition| c.entity == StoredIndexEntity::Node;
    compiled.physical.nodes.iter().any(|op| match op {
        PhysicalOp::Create(create) => create.pattern.parts.iter().any(|part| match &part.element {
            ResolvedPatternElement::Node { labels, .. } => labels.iter().flatten().any(|label| {
                constraints
                    .iter()
                    .filter(on_nodes)
                    .any(|c| c.label == *label)
            }),
            _ => true,
        }),
        PhysicalOp::Set(_) => !constraints.is_empty(),
        _ => false,
    })
}

fn live_fast_path_safe(compiled: &CompiledQuery) -> bool {
    compiled.unions.is_empty() && live_fast_plan_safe(&compiled.physical)
}

fn live_fast_plan_safe(plan: &PhysicalPlan) -> bool {
    let mut create_nodes = Vec::new();
    let mut writes = Vec::new();

    for op in &plan.nodes {
        match op {
            PhysicalOp::Create(create) if create_pattern_is_node_only(&create.pattern) => {
                writes.push("create");
                collect_created_node_vars(&create.pattern, &mut create_nodes);
            }
            PhysicalOp::Set(set) if set.items.iter().all(simple_set_property_item) => {
                writes.push("set");
            }
            PhysicalOp::Delete(delete)
                if !delete.detach
                    && !create_nodes.is_empty()
                    && delete
                        .expressions
                        .iter()
                        .all(|expr| matches!(expr, ResolvedExpr::Variable(v) if create_nodes.contains(v))) =>
            {
                writes.push("delete_created");
            }
            // A plain DELETE fails on a node that still has relationships,
            // possibly after deleting earlier rows; DETACH DELETE can't fail.
            PhysicalOp::Delete(delete) if delete.detach => {
                writes.push("delete");
            }
            PhysicalOp::Delete(_) => return false,
            PhysicalOp::Merge(_)
            | PhysicalOp::Remove(_)
            | PhysicalOp::Foreach(_)
            | PhysicalOp::CallSubquery(_) => return false,
            PhysicalOp::Create(_) | PhysicalOp::Set(_) => return false,
            _ => {}
        }
    }

    matches!(
        writes.as_slice(),
        ["create"] | ["set"] | ["delete"] | ["create", "delete_created"]
    )
}

fn create_pattern_is_node_only(pattern: &ResolvedPattern) -> bool {
    pattern
        .parts
        .iter()
        .all(|part| matches!(part.element, ResolvedPatternElement::Node { .. }))
}

fn collect_created_node_vars(
    pattern: &ResolvedPattern,
    out: &mut Vec<lora_analyzer::symbols::VarId>,
) {
    for part in &pattern.parts {
        if let ResolvedPatternElement::Node { var: Some(var), .. } = &part.element {
            out.push(*var);
        }
    }
}

fn simple_set_property_item(item: &ResolvedSetItem) -> bool {
    match item {
        ResolvedSetItem::SetProperty { target, value } => {
            matches!(
                target,
                ResolvedExpr::Property { expr, .. }
                    if matches!(expr.as_ref(), ResolvedExpr::Variable(_))
            ) && simple_property_value_expr(value)
        }
        _ => false,
    }
}

fn simple_property_value_expr(expr: &ResolvedExpr) -> bool {
    matches!(
        expr,
        ResolvedExpr::Literal(
            LiteralValue::Null
                | LiteralValue::Bool(_)
                | LiteralValue::Integer(_)
                | LiteralValue::Float(_)
                | LiteralValue::String(_)
        ) | ResolvedExpr::Parameter(_)
    )
}
