pub mod logical;
pub mod optimizer;
pub mod physical;

mod pattern;
mod planner;

pub use logical::*;
pub use optimizer::Optimizer;
pub use physical::*;
pub use planner::Planner;

use lora_analyzer::resolved::ResolvedQuery;

#[derive(Debug, Clone)]
pub struct CompiledQuery {
    pub physical: PhysicalPlan,
    /// Additional UNION branches, each compiled independently.
    pub unions: Vec<CompiledUnionBranch>,
}

#[derive(Debug, Clone)]
pub struct CompiledUnionBranch {
    /// If true, UNION ALL (keep duplicates). If false, plain UNION (deduplicate).
    pub all: bool,
    pub physical: PhysicalPlan,
}

pub struct Compiler;

impl Compiler {
    pub fn compile(query: &ResolvedQuery) -> CompiledQuery {
        let mut planner = Planner::new();
        let logical = planner.plan(query);

        let mut optimizer = Optimizer::new();
        let optimized = optimizer.optimize(logical);

        // Lower by moving the logical plan — it is not needed after lowering.
        let physical = optimizer.lower_to_physical(optimized);

        let unions = query
            .unions
            .iter()
            .map(|union_part| {
                let branch_query = ResolvedQuery {
                    clauses: union_part.clauses.clone(),
                    unions: Vec::new(),
                };
                let mut branch_planner = Planner::new();
                let branch_logical = branch_planner.plan(&branch_query);
                let mut branch_optimizer = Optimizer::new();
                let branch_optimized = branch_optimizer.optimize(branch_logical);
                let branch_physical = branch_optimizer.lower_to_physical(branch_optimized);

                CompiledUnionBranch {
                    all: union_part.all,
                    physical: branch_physical,
                }
            })
            .collect();

        CompiledQuery { physical, unions }
    }
}