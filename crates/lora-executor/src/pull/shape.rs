use lora_compiler::physical::{PhysicalOp, PhysicalPlan};
use lora_compiler::CompiledQuery;

/// Classification of a compiled query, used by the database layer to
/// decide whether `db.stream` needs a hidden staged transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamShape {
    /// No mutating operator anywhere in the plan or any of its
    /// UNION branches. Safe to stream against the live store.
    ReadOnly,
    /// Has at least one mutating operator (Create / Merge / Delete /
    /// Set / Remove). The host should run this against a staged
    /// graph and only publish on cursor exhaustion.
    Mutating,
}

impl StreamShape {
    pub fn is_mutating(self) -> bool {
        matches!(self, StreamShape::Mutating)
    }
}

fn plan_is_mutating(plan: &PhysicalPlan) -> bool {
    plan.nodes.iter().any(|op| {
        matches!(
            op,
            PhysicalOp::Create(_)
                | PhysicalOp::Merge(_)
                | PhysicalOp::Delete(_)
                | PhysicalOp::Set(_)
                | PhysicalOp::Remove(_)
                | PhysicalOp::Foreach(_)
        )
    })
}

/// Classify a compiled query for streaming. Treats any UNION branch
/// the same as the head: a single mutating op anywhere across the
/// compiled query promotes the whole query to `Mutating`.
pub fn classify_stream(compiled: &CompiledQuery) -> StreamShape {
    if plan_is_mutating(&compiled.physical)
        || compiled
            .unions
            .iter()
            .any(|b| plan_is_mutating(&b.physical))
    {
        StreamShape::Mutating
    } else {
        StreamShape::ReadOnly
    }
}
