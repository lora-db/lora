use lora_compiler::physical::{PhysicalNodeId, PhysicalOp, PhysicalPlan};
use lora_compiler::CompiledQuery;

/// Result column names derived from the compiled plan.
///
/// Walks the plan from `root` looking for the topmost projection-shaped
/// node (Projection, HashAggregation). Other operators that wrap a
/// projection (Limit, Sort, PathBuild, OptionalMatch, Filter, Unwind,
/// Create/Merge/Set/Delete/Remove) defer to their input. Returns an
/// empty `Vec` for plans that have no named output (e.g. a bare
/// scan-only plan), preserving the previous "infer from first row"
/// behaviour for those cases.
pub fn plan_result_columns(plan: &PhysicalPlan) -> Vec<String> {
    plan_columns_at(plan, plan.root).unwrap_or_default()
}

fn plan_columns_at(plan: &PhysicalPlan, node: PhysicalNodeId) -> Option<Vec<String>> {
    plan_columns_at_depth(plan, node, 1)
}

fn plan_columns_at_depth(
    plan: &PhysicalPlan,
    node: PhysicalNodeId,
    depth: usize,
) -> Option<Vec<String>> {
    if lora_ast::stack::check_due(depth) {
        return lora_ast::stack::guard(|| plan_columns_at_depth(plan, node, depth + 1));
    }
    match &plan.nodes[node] {
        PhysicalOp::Projection(p) => Some(p.items.iter().map(|i| i.name.to_string()).collect()),
        PhysicalOp::HashAggregation(p) => Some(
            p.group_by
                .iter()
                .chain(p.aggregates.iter())
                .map(|i| i.name.to_string())
                .collect(),
        ),
        PhysicalOp::Limit(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::Sort(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::PathBuild(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::OptionalMatch(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        // CALL { ... } emits an outer×inner row pair; the visible result
        // columns are still whatever the surrounding pipeline named.
        PhysicalOp::CallSubquery(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::Filter(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::Unwind(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::Create(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::Merge(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::Delete(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::Set(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::Remove(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::Foreach(p) => plan_columns_at_depth(plan, p.input, depth + 1),
        PhysicalOp::Argument(_)
        | PhysicalOp::NodeScan(_)
        | PhysicalOp::NodeByLabelScan(_)
        | PhysicalOp::NodeByPropertyScan(_)
        | PhysicalOp::NodeByIdSeek(_)
        | PhysicalOp::RelByIdSeek(_)
        | PhysicalOp::NodeByPropertyRangeScan(_)
        | PhysicalOp::NodeByTextScan(_)
        | PhysicalOp::NodeByPointScan(_)
        | PhysicalOp::RelByPropertyRangeScan(_)
        | PhysicalOp::RelByTextScan(_)
        | PhysicalOp::RelByPointScan(_)
        | PhysicalOp::Expand(_) => None,
    }
}

/// Result column names for a compiled query (head plan; UNION branches
/// must produce the same shape so the head's columns are authoritative).
pub fn compiled_result_columns(compiled: &CompiledQuery) -> Vec<String> {
    plan_result_columns(&compiled.physical)
}
