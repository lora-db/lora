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
    match &plan.nodes[node] {
        PhysicalOp::Projection(p) => Some(p.items.iter().map(|i| i.name.clone()).collect()),
        PhysicalOp::HashAggregation(p) => Some(
            p.group_by
                .iter()
                .chain(p.aggregates.iter())
                .map(|i| i.name.clone())
                .collect(),
        ),
        PhysicalOp::Limit(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Sort(p) => plan_columns_at(plan, p.input),
        PhysicalOp::PathBuild(p) => plan_columns_at(plan, p.input),
        PhysicalOp::OptionalMatch(p) => plan_columns_at(plan, p.input),
        // CALL { ... } emits an outer×inner row pair; the visible result
        // columns are still whatever the surrounding pipeline named.
        PhysicalOp::CallSubquery(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Filter(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Unwind(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Create(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Merge(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Delete(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Set(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Remove(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Foreach(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Argument(_)
        | PhysicalOp::NodeScan(_)
        | PhysicalOp::NodeByLabelScan(_)
        | PhysicalOp::NodeByPropertyScan(_)
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
