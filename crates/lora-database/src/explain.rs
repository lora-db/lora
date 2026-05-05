//! Public result types for `Database::explain` and `Database::profile`.
//!
//! These are deliberately separate from the `QueryResult` family used by
//! `execute()` so the language bindings can surface plan / profile
//! payloads without trying to fit them into the row-shaped result type.
//!
//! `QueryPlan` is what `explain()` returns; the query is parsed,
//! analyzed, and compiled but never executed. `QueryProfile` is what
//! `profile()` returns; the query is fully executed (including any
//! mutations) and the plan tree is decorated with coarse runtime
//! metrics.

use std::collections::BTreeMap;

use lora_compiler::PlanTree;

/// Whether a compiled plan is read-only or potentially mutates the
/// graph. Mirrors `lora_executor::StreamShape` but stays a stable
/// public surface for bindings that don't pull in the executor crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanShape {
    ReadOnly,
    Mutating,
}

impl PlanShape {
    pub fn is_mutating(self) -> bool {
        matches!(self, PlanShape::Mutating)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PlanShape::ReadOnly => "readOnly",
            PlanShape::Mutating => "mutating",
        }
    }
}

impl From<lora_executor::StreamShape> for PlanShape {
    fn from(value: lora_executor::StreamShape) -> Self {
        match value {
            lora_executor::StreamShape::ReadOnly => PlanShape::ReadOnly,
            lora_executor::StreamShape::Mutating => PlanShape::Mutating,
        }
    }
}

/// Result of `Database::explain`.
#[derive(Debug, Clone)]
pub struct QueryPlan {
    /// The exact query text the caller submitted.
    pub query: String,
    /// Operator tree, leaf-most first under each node.
    pub tree: PlanTree,
    /// Whether running this plan would (potentially) mutate the graph.
    pub shape: PlanShape,
    /// Result column names in projection order. Empty for plans
    /// without a top-level projection (e.g. plans that only mutate).
    pub result_columns: Vec<String>,
}

/// Coarse-grained per-query runtime metrics. v1 reports totals, not
/// per-operator timings; per-operator instrumentation is reserved
/// for a future phase that will not change the public surface.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileMetrics {
    /// Wall-clock time spent inside the executor for this query.
    pub total_elapsed_ns: u64,
    /// Number of rows produced before result-format projection.
    pub total_rows: u64,
    /// Whether at least one mutating operator ran.
    pub mutated: bool,
    /// Reserved for future operator-level metrics. Present today as
    /// an empty map so consumers can pattern-match on the field
    /// without breaking when v2 starts populating it.
    pub per_operator: BTreeMap<usize, OperatorMetrics>,
}

/// Per-operator metrics. Reserved for a future phase; today no
/// operator populates this.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperatorMetrics {
    pub rows: u64,
    pub db_hits: u64,
    pub elapsed_ns: u64,
    pub next_calls: u64,
}

/// Result of `Database::profile`.
#[derive(Debug, Clone)]
pub struct QueryProfile {
    /// The plan that was profiled. Same shape as `QueryPlan` from
    /// `explain()`.
    pub plan: QueryPlan,
    /// Runtime metrics gathered during execution.
    pub metrics: ProfileMetrics,
}
