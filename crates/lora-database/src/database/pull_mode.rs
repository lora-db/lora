//! Decide whether a read-only query can short-circuit through the
//! pull-shaped collector instead of the full executor.
//!
//! The pull collector is faster than the executor for queries that
//! benefit from honoring an early `LIMIT` — but it can only run when
//! the plan is free of UNION and there's no blocking operator (Sort,
//! HashAggregation, OptionalMatch) sitting between the LIMIT and its
//! scan. When this returns `true`, [`crate::database::Database`] uses
//! `lora_executor::collect_compiled` for the read fast-path; otherwise
//! it falls back to the full executor.
//!
//! The two helpers are kept as free functions because their only state
//! is the borrowed plan they introspect.

use lora_compiler::{CompiledQuery, PhysicalNodeId, PhysicalOp, PhysicalPlan};

pub(crate) fn should_collect_read_via_pull(compiled: &CompiledQuery) -> bool {
    compiled.unions.is_empty() && plan_has_early_limit(&compiled.physical)
}

fn plan_has_early_limit(plan: &PhysicalPlan) -> bool {
    plan.nodes.iter().any(|op| {
        let PhysicalOp::Limit(limit) = op else {
            return false;
        };
        limit.limit.is_some() && !subtree_contains_blocking_limit_input(plan, limit.input)
    })
}

fn subtree_contains_blocking_limit_input(plan: &PhysicalPlan, node_id: PhysicalNodeId) -> bool {
    match &plan.nodes[node_id] {
        PhysicalOp::Sort(_)
        | PhysicalOp::HashAggregation(_)
        | PhysicalOp::OptionalMatch(_)
        | PhysicalOp::CallSubquery(_)
        | PhysicalOp::NodeByPropertyRangeScan(_)
        | PhysicalOp::NodeByTextScan(_)
        | PhysicalOp::NodeByPointScan(_)
        | PhysicalOp::RelByPropertyRangeScan(_)
        | PhysicalOp::RelByTextScan(_)
        | PhysicalOp::RelByPointScan(_) => true,
        PhysicalOp::Argument(_)
        | PhysicalOp::NodeScan(_)
        | PhysicalOp::NodeByLabelScan(_)
        | PhysicalOp::NodeByPropertyScan(_) => false,
        PhysicalOp::Expand(op) => subtree_contains_blocking_limit_input(plan, op.input),
        PhysicalOp::Filter(op) => subtree_contains_blocking_limit_input(plan, op.input),
        PhysicalOp::Projection(op) => subtree_contains_blocking_limit_input(plan, op.input),
        PhysicalOp::Unwind(op) => subtree_contains_blocking_limit_input(plan, op.input),
        PhysicalOp::Limit(op) => subtree_contains_blocking_limit_input(plan, op.input),
        PhysicalOp::PathBuild(op) => subtree_contains_blocking_limit_input(plan, op.input),
        PhysicalOp::Create(_)
        | PhysicalOp::Merge(_)
        | PhysicalOp::Delete(_)
        | PhysicalOp::Set(_)
        | PhysicalOp::Remove(_)
        | PhysicalOp::Foreach(_) => true,
    }
}
