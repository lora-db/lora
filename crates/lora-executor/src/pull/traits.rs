//! Pull-pipeline plan walker and executor entry points.
//!
//! Cursor/source basics, context, hydration, stream-shape classification,
//! and result-column inference live in sibling modules. This file keeps the
//! code that turns physical plans into row cursors plus the public read/write
//! pull executors.

use std::collections::BTreeMap;
use std::sync::Arc;

use lora_compiler::physical::{
    ExpandExec, FilterExec, HashAggregationExec, LimitExec, NodeByLabelScanExec,
    NodeByPointScanExec, NodeByPropertyRangeScanExec, NodeByPropertyScanExec, NodeByTextScanExec,
    NodeScanExec, OptionalMatchExec, PathBuildExec, PhysicalNodeId, PhysicalOp, PhysicalPlan,
    ProjectionExec, RelByPointScanExec, RelByPropertyRangeScanExec, RelByTextScanExec, SortExec,
    UnwindExec,
};
use lora_compiler::CompiledQuery;
use lora_store::GraphStorage;

use crate::errors::ExecResult;
use crate::eval::{clear_eval_error, eval_expr};
use crate::executor::{ExecutionContext, Executor};
use crate::profile::wrap_metered;
use crate::value::{LoraValue, Row};

use super::aggregate::HashAggregationSource;
use super::expand::{ExpandSource, VariableLengthExpandSource};
use super::filter::FilterSource;
use super::optional::OptionalMatchSource;
use super::path::PathBuildSource;
use super::projection::{DistinctSource, ProjectionSource, UnwindSource};
use super::scan::{
    BufferedIndexScanSource, NodeByLabelScanSource, NodeByPropertyScanSource, NodeScanSource,
};
use super::sort::{LimitSource, SortSource};
use super::union::UnionSource;
use super::{drain, ArgumentSource, BufferedRowSource, HydratingSource, RowSource, StreamCtx};

// ---------------------------------------------------------------------------
// Compiled-query → streaming entry helpers
// ---------------------------------------------------------------------------

/// Build a streaming `RowSource` for an entire compiled query,
/// handling both the no-UNION and UNION cases. Replaces the
/// "UNION-bearing → BufferedRowSource" fallback that previously
/// sat in `PullExecutor::open_compiled`.
///
/// For non-UNION plans this is a thin wrapper around
/// [`build_streaming`] + [`HydratingSource`]. For UNION plans, we
/// build a streaming chain per branch (each ending in its own
/// `HydratingSource` so its node / relationship references are
/// resolved against the same view of storage), then combine them
/// through [`UnionSource`].
pub(super) fn compiled_to_streaming<'a, S: GraphStorage + 'a>(
    compiled: &'a CompiledQuery,
    storage: &'a S,
    params: BTreeMap<String, LoraValue>,
) -> ExecResult<Box<dyn RowSource + 'a>> {
    let params = Arc::new(params);

    if compiled.unions.is_empty() {
        let plan = &compiled.physical;
        let inner = build_streaming(plan, plan.root, storage, params)?;
        return Ok(Box::new(HydratingSource::new(inner, storage)));
    }

    let mut branches: Vec<Box<dyn RowSource + 'a>> = Vec::with_capacity(compiled.unions.len() + 1);

    let head_inner = build_streaming(
        &compiled.physical,
        compiled.physical.root,
        storage,
        params.clone(),
    )?;
    branches.push(Box::new(HydratingSource::new(head_inner, storage)));

    let mut needs_dedup = false;
    for branch in &compiled.unions {
        let inner = build_streaming(
            &branch.physical,
            branch.physical.root,
            storage,
            params.clone(),
        )?;
        branches.push(Box::new(HydratingSource::new(inner, storage)));
        if !branch.all {
            needs_dedup = true;
        }
    }

    Ok(Box::new(UnionSource::new(branches, needs_dedup)))
}

// ---------------------------------------------------------------------------
// Plan walker
// ---------------------------------------------------------------------------

/// True iff this op has a per-operator streaming source. Operators
/// that aren't on this list fall back to a single materialized
/// [`Executor::execute_subtree`] call wrapped as a [`BufferedRowSource`].
pub(super) fn is_streaming_op(op: &PhysicalOp) -> bool {
    match op {
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
        | PhysicalOp::Filter(_)
        | PhysicalOp::Unwind(_)
        | PhysicalOp::Limit(_)
        // Sort is internally O(N) but exposed as a `RowSource`:
        // it drains its input on the first pull, sorts in place,
        // then yields lazily. This lets a write op (CREATE / SET /
        // DELETE) above an ORDER BY stream its writes one row at
        // a time instead of forcing the whole subtree to
        // materialize before the first write.
        | PhysicalOp::Sort(_)
        | PhysicalOp::HashAggregation(_)
        | PhysicalOp::OptionalMatch(_)
        | PhysicalOp::PathBuild(_)
        // Projection (both `DISTINCT` and non-`DISTINCT`). The
        // `DISTINCT` form drains + dedups internally and yields
        // lazily via `DistinctSource`.
        | PhysicalOp::Projection(_) => true,
        // Single-hop expands are fully per-edge. Variable-length expands still
        // allocate the current source row's BFS result, then yield lazily.
        PhysicalOp::Expand(_) => true,
        _ => false,
    }
}

/// If `node_id` is a streamable write operator
/// (Create / Set / Delete / Remove / Merge), return its input
/// `PhysicalNodeId`. Used by [`MutablePullExecutor::open_compiled`]
/// to detect plans that can be driven by [`StreamingWriteCursor`].
pub(crate) fn write_op_input(
    plan: &PhysicalPlan,
    node_id: PhysicalNodeId,
) -> Option<PhysicalNodeId> {
    match &plan.nodes[node_id] {
        PhysicalOp::Create(o) => Some(o.input),
        PhysicalOp::Set(o) => Some(o.input),
        PhysicalOp::Delete(o) => Some(o.input),
        PhysicalOp::Remove(o) => Some(o.input),
        PhysicalOp::Merge(o) => Some(o.input),
        _ => None,
    }
}

/// True if every operator in the subtree rooted at `node_id` is
/// covered by [`is_streaming_op`] (and therefore by
/// [`build_streaming`] without falling back to buffered execution).
///
/// Used by the mutable executor to decide whether write operators
/// can pull their input row-by-row instead of materializing it.
pub(crate) fn subtree_is_fully_streaming(plan: &PhysicalPlan, node_id: PhysicalNodeId) -> bool {
    let op = &plan.nodes[node_id];
    if !is_streaming_op(op) {
        return false;
    }
    let child = match op {
        PhysicalOp::Argument(_) => return true,
        PhysicalOp::NodeScan(o) => o.input,
        PhysicalOp::NodeByLabelScan(o) => o.input,
        PhysicalOp::NodeByPropertyScan(o) => o.input,
        PhysicalOp::NodeByPropertyRangeScan(o) => o.input,
        PhysicalOp::NodeByTextScan(o) => o.input,
        PhysicalOp::NodeByPointScan(o) => o.input,
        PhysicalOp::RelByPropertyRangeScan(o) => o.input,
        PhysicalOp::RelByTextScan(o) => o.input,
        PhysicalOp::RelByPointScan(o) => o.input,
        PhysicalOp::Filter(o) => Some(o.input),
        PhysicalOp::Unwind(o) => Some(o.input),
        PhysicalOp::Limit(o) => Some(o.input),
        PhysicalOp::Expand(o) => Some(o.input),
        PhysicalOp::Projection(o) => Some(o.input),
        PhysicalOp::Sort(o) => Some(o.input),
        PhysicalOp::HashAggregation(o) => Some(o.input),
        PhysicalOp::OptionalMatch(o) => Some(o.input),
        PhysicalOp::PathBuild(o) => Some(o.input),
        // Already filtered by is_streaming_op above.
        _ => return false,
    };
    match child {
        None => true,
        Some(c) => subtree_is_fully_streaming(plan, c),
    }
}

pub(crate) fn build_streaming<'a, S: GraphStorage + 'a>(
    plan: &'a PhysicalPlan,
    node_id: PhysicalNodeId,
    storage: &'a S,
    params: Arc<BTreeMap<String, LoraValue>>,
) -> ExecResult<Box<dyn RowSource + 'a>> {
    build_streaming_inner(plan, node_id, storage, params).map(|src| wrap_metered(node_id, src))
}

fn build_streaming_inner<'a, S: GraphStorage + 'a>(
    plan: &'a PhysicalPlan,
    node_id: PhysicalNodeId,
    storage: &'a S,
    params: Arc<BTreeMap<String, LoraValue>>,
) -> ExecResult<Box<dyn RowSource + 'a>> {
    let op = &plan.nodes[node_id];

    if !is_streaming_op(op) {
        return build_buffered_subtree(plan, node_id, storage, &params);
    }

    match op {
        PhysicalOp::Argument(_) => Ok(Box::new(ArgumentSource::new())),

        PhysicalOp::NodeScan(NodeScanExec { input, var }) => {
            let upstream = open_input(plan, *input, storage, params.clone())?;
            Ok(Box::new(NodeScanSource::new(upstream, storage, *var)))
        }

        PhysicalOp::NodeByLabelScan(NodeByLabelScanExec { input, var, labels }) => {
            let upstream = open_input(plan, *input, storage, params.clone())?;
            Ok(Box::new(NodeByLabelScanSource::new(
                upstream, storage, *var, labels,
            )))
        }

        PhysicalOp::NodeByPropertyScan(NodeByPropertyScanExec {
            input,
            var,
            labels,
            key,
            value,
        }) => {
            let upstream = open_input(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(NodeByPropertyScanSource::new(
                upstream, ctx, *var, labels, key, value,
            )))
        }

        PhysicalOp::NodeByPropertyRangeScan(op @ NodeByPropertyRangeScanExec { input, .. }) => {
            let upstream = open_input(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(BufferedIndexScanSource::node_range(
                upstream, ctx, op,
            )))
        }

        PhysicalOp::NodeByTextScan(op @ NodeByTextScanExec { input, .. }) => {
            let upstream = open_input(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(BufferedIndexScanSource::node_text(
                upstream, ctx, op,
            )))
        }

        PhysicalOp::NodeByPointScan(op @ NodeByPointScanExec { input, .. }) => {
            let upstream = open_input(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(BufferedIndexScanSource::node_point(
                upstream, ctx, op,
            )))
        }

        PhysicalOp::RelByPropertyRangeScan(op @ RelByPropertyRangeScanExec { input, .. }) => {
            let upstream = open_input(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(BufferedIndexScanSource::rel_range(
                upstream, ctx, op,
            )))
        }

        PhysicalOp::RelByTextScan(op @ RelByTextScanExec { input, .. }) => {
            let upstream = open_input(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(BufferedIndexScanSource::rel_text(
                upstream, ctx, op,
            )))
        }

        PhysicalOp::RelByPointScan(op @ RelByPointScanExec { input, .. }) => {
            let upstream = open_input(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(BufferedIndexScanSource::rel_point(
                upstream, ctx, op,
            )))
        }

        PhysicalOp::Expand(ExpandExec {
            input,
            src,
            rel,
            dst,
            types,
            direction,
            rel_properties,
            range,
        }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            match range.as_ref() {
                Some(range) => Ok(Box::new(VariableLengthExpandSource::new(
                    upstream, ctx, *src, *rel, *dst, types, *direction, range,
                ))),
                None => Ok(Box::new(ExpandSource::new(
                    upstream,
                    ctx,
                    *src,
                    *rel,
                    *dst,
                    types,
                    *direction,
                    rel_properties.as_ref(),
                ))),
            }
        }

        PhysicalOp::Filter(FilterExec { input, predicate }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(FilterSource::new(upstream, ctx, predicate)))
        }

        PhysicalOp::Projection(ProjectionExec {
            input,
            distinct,
            items,
            include_existing,
        }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            let proj: Box<dyn RowSource + 'a> = Box::new(ProjectionSource::new(
                upstream,
                ctx,
                items,
                *include_existing,
            ));
            if *distinct {
                Ok(Box::new(DistinctSource::new(proj)))
            } else {
                Ok(proj)
            }
        }

        PhysicalOp::Unwind(UnwindExec { input, expr, alias }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(UnwindSource::new(upstream, ctx, expr, *alias)))
        }

        PhysicalOp::Limit(LimitExec { input, skip, limit }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            // Skip / limit expressions are evaluated against an
            // empty row (matching the buffered executor semantics).
            let ctx = StreamCtx::new(storage, params);
            let eval_ctx = ctx.eval_ctx();
            let scratch = Row::new();
            let skip_n = skip
                .as_ref()
                .and_then(|e| eval_expr(e, &scratch, &eval_ctx).as_i64())
                .unwrap_or(0)
                .max(0) as usize;
            let limit_n = limit
                .as_ref()
                .and_then(|e| eval_expr(e, &scratch, &eval_ctx).as_i64())
                .map(|n| n.max(0) as usize);
            Ok(Box::new(LimitSource::new(upstream, skip_n, limit_n)))
        }

        PhysicalOp::Sort(SortExec {
            input,
            items,
            top_k,
        }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(SortSource::new_with_top_k(
                upstream, ctx, items, *top_k,
            )))
        }

        PhysicalOp::HashAggregation(HashAggregationExec {
            input,
            group_by,
            aggregates,
        }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(HashAggregationSource::new(
                upstream, ctx, group_by, aggregates,
            )))
        }

        PhysicalOp::OptionalMatch(OptionalMatchExec {
            input,
            inner,
            new_vars,
        }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(OptionalMatchSource::new(
                upstream, ctx, plan, *inner, new_vars,
            )))
        }

        PhysicalOp::PathBuild(PathBuildExec {
            input,
            output,
            node_vars,
            rel_vars,
            shortest_path_all,
        }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(PathBuildSource::new(
                upstream,
                ctx,
                *output,
                node_vars,
                rel_vars,
                *shortest_path_all,
            )))
        }

        // Already filtered out by `is_streaming_op`.
        _ => unreachable!("non-streaming op reached streaming branch: {op:?}"),
    }
}

/// Open an upstream input source. `Option<PhysicalNodeId>` parents
/// (NodeScan / NodeByLabelScan) treat `None` as "start from a single
/// empty row".
fn open_input<'a, S: GraphStorage + 'a>(
    plan: &'a PhysicalPlan,
    input: Option<PhysicalNodeId>,
    storage: &'a S,
    params: Arc<BTreeMap<String, LoraValue>>,
) -> ExecResult<Box<dyn RowSource + 'a>> {
    match input {
        Some(input) => build_streaming(plan, input, storage, params),
        None => Ok(Box::new(ArgumentSource::new())),
    }
}

/// Materialized fallback: drain the subtree through the existing
/// `Executor` and present the result as a [`BufferedRowSource`]. This
/// remains the leaf path for operators that have no cursor-shaped
/// source yet (most notably variable-length expansion inside a larger
/// streaming tree) and for write operators in the read-only pull
/// executor.
fn build_buffered_subtree<'a, S: GraphStorage + 'a>(
    plan: &'a PhysicalPlan,
    node_id: PhysicalNodeId,
    storage: &'a S,
    params: &Arc<BTreeMap<String, LoraValue>>,
) -> ExecResult<Box<dyn RowSource + 'a>> {
    // The `Executor` consumes its `ExecutionContext` so we must
    // clone the params map for the fallback. In practice this is
    // small (typically empty or a handful of named parameters).
    let executor = Executor::new(ExecutionContext {
        storage,
        params: (**params).clone(),
    });
    let rows = executor.execute_subtree(plan, node_id)?;
    Ok(Box::new(BufferedRowSource::new(rows)))
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Pull-based read-only executor.
pub struct PullExecutor<'a, S: GraphStorage> {
    storage: &'a S,
    params: BTreeMap<String, LoraValue>,
}

impl<'a, S: GraphStorage> PullExecutor<'a, S> {
    pub fn new(storage: &'a S, params: BTreeMap<String, LoraValue>) -> Self {
        Self { storage, params }
    }

    /// Open a streaming cursor for a compiled query.
    ///
    /// Both no-UNION and UNION-bearing plans go through
    /// [`compiled_to_streaming`]: UNION drains its branches via
    /// [`UnionSource`] (memory unchanged from the previous buffered
    /// path; UNION is inherently O(N) before dedup), but the
    /// consumer side is now streaming so any downstream pipeline
    /// composes uniformly.
    pub fn open_compiled(self, compiled: &'a CompiledQuery) -> ExecResult<Box<dyn RowSource + 'a>>
    where
        S: 'a,
    {
        clear_eval_error();
        compiled_to_streaming(compiled, self.storage, self.params)
    }
}

/// Drain a freshly opened cursor into a `Vec<Row>`. Convenience for
/// callers that want the streaming entry point but a buffered result.
pub fn collect_compiled<'a, S: GraphStorage + 'a>(
    storage: &'a S,
    params: BTreeMap<String, LoraValue>,
    compiled: &'a CompiledQuery,
) -> ExecResult<Vec<Row>> {
    let mut cursor = PullExecutor::new(storage, params).open_compiled(compiled)?;
    drain(cursor.as_mut())
}
