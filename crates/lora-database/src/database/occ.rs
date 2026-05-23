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
use lora_store::{GraphStorage, GraphStorageMut};

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

        if live_fast_path_safe(compiled) {
            self.run_live_fast_with_durable_recorder(run)
        } else {
            self.run_with_durable_recorder(run)
        }
    }
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
            PhysicalOp::Delete(_) => {
                writes.push("delete");
            }
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
