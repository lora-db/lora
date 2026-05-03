//! Pull-pipeline trait, plan walker, and public entry points.
//!
//! This file owns:
//! - The [`RowSource`] cursor trait, [`drain`] helper, and the shared
//!   [`StreamCtx`] that every operator source borrows storage and bound
//!   parameters from.
//! - The buffered fallback ([`BufferedRowSource`]) and the leaf
//!   [`ArgumentSource`].
//! - The top-of-pipeline [`HydratingSource`] and its
//!   [`hydrate_value`] helper.
//! - The plan walker (`is_streaming_op`, `subtree_is_fully_streaming`,
//!   `build_streaming`, `compiled_to_streaming`, `write_op_input`,
//!   `open_input`, `build_buffered_subtree`).
//! - The public [`PullExecutor`] / [`MutablePullExecutor`] entry points
//!   plus the mutable cursor machinery ([`StreamingWriteCursor`],
//!   [`MutableUnionSource`], [`StoragePtr`]).
//! - [`collect_compiled`], [`StreamShape`] / [`classify_stream`], and
//!   [`plan_result_columns`] / [`compiled_result_columns`].

use std::collections::{BTreeMap, BTreeSet};
use std::mem::ManuallyDrop;
use std::sync::Arc;

use lora_compiler::physical::{
    ExpandExec, FilterExec, HashAggregationExec, LimitExec, NodeByLabelScanExec,
    NodeByPropertyScanExec, NodeScanExec, OptionalMatchExec, PathBuildExec, PhysicalNodeId,
    PhysicalOp, PhysicalPlan, ProjectionExec, SortExec, UnwindExec,
};
use lora_compiler::CompiledQuery;
use lora_store::{GraphStorage, GraphStorageMut};

use crate::errors::{ExecResult, ExecutorError};
use crate::eval::{clear_eval_error, eval_expr, EvalContext};
use crate::executor::{
    hydrate_node_record, hydrate_relationship_record, ExecutionContext, Executor, GroupValueKey,
    MutableExecutionContext, MutableExecutor,
};
use crate::value::{LoraValue, Row};

use super::aggregate::HashAggregationSource;
use super::expand::{ExpandSource, VariableLengthExpandSource};
use super::filter::FilterSource;
use super::optional::OptionalMatchSource;
use super::path::PathBuildSource;
use super::projection::{DistinctSource, ProjectionSource, UnwindSource};
use super::scan::{NodeByLabelScanSource, NodeByPropertyScanSource, NodeScanSource};
use super::sort::{LimitSource, SortSource};
use super::union::UnionSource;

/// Fallible pull-based row cursor.
///
/// Each call to [`RowSource::next_row`] returns the next row,
/// `Ok(None)` when the cursor is exhausted, or an error if execution
/// fails. The cursor stays in a valid state after an error — callers
/// may drop it without observing additional side effects.
pub trait RowSource {
    /// Pull the next row.
    fn next_row(&mut self) -> ExecResult<Option<Row>>;
}

/// Drain a row source into a `Vec<Row>`, propagating the first error.
pub fn drain<S: RowSource + ?Sized>(source: &mut S) -> ExecResult<Vec<Row>> {
    let mut out = Vec::new();
    while let Some(row) = source.next_row()? {
        out.push(row);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Shared streaming context
// ---------------------------------------------------------------------------

/// Storage + bound parameters shared by every operator source in a
/// pull pipeline. `Clone` is one pointer-copy plus an `Arc::clone`
/// (params), so passing it by value down the build tree is
/// effectively free, while consolidating "the two pieces every
/// expression-evaluating source needs" into one field.
#[derive(Clone)]
pub(super) struct StreamCtx<'a, S: GraphStorage> {
    pub storage: &'a S,
    pub params: Arc<BTreeMap<String, LoraValue>>,
}

impl<'a, S: GraphStorage> StreamCtx<'a, S> {
    pub(super) fn new(storage: &'a S, params: Arc<BTreeMap<String, LoraValue>>) -> Self {
        Self { storage, params }
    }

    /// Build a borrowing [`EvalContext`] for use inside an
    /// operator's `next_row` method. Cheap — two pointer reads.
    pub(super) fn eval_ctx<'b>(&'b self) -> EvalContext<'b, S> {
        EvalContext {
            storage: self.storage,
            params: &self.params,
        }
    }
}

// ---------------------------------------------------------------------------
// Buffered fallback
// ---------------------------------------------------------------------------

/// Buffered cursor backed by a pre-computed `Vec<Row>`. Used both as
/// a simple "rows already collected" adapter and as the leaf fallback
/// for operators whose internals still require full materialization.
pub struct BufferedRowSource {
    iter: std::vec::IntoIter<Row>,
}

impl BufferedRowSource {
    pub fn new(rows: Vec<Row>) -> Self {
        Self {
            iter: rows.into_iter(),
        }
    }
}

impl RowSource for BufferedRowSource {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        Ok(self.iter.next())
    }
}

// ---------------------------------------------------------------------------
// Leaf "yield one empty row" source
// ---------------------------------------------------------------------------

/// Yields a single empty row exactly once. The bottom of every plan
/// chain that doesn't start with an explicit input.
pub struct ArgumentSource {
    yielded: bool,
}

impl ArgumentSource {
    pub fn new() -> Self {
        Self { yielded: false }
    }
}

impl Default for ArgumentSource {
    fn default() -> Self {
        Self::new()
    }
}

impl RowSource for ArgumentSource {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        if self.yielded {
            Ok(None)
        } else {
            self.yielded = true;
            Ok(Some(Row::new()))
        }
    }
}

// ---------------------------------------------------------------------------
// Top-of-pipeline hydration
// ---------------------------------------------------------------------------

/// Top-of-pipeline hydration. Replaces node / relationship id
/// references in each emitted row with their full hydrated map form,
/// matching the buffered executor's post-execution hydration step.
pub struct HydratingSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    storage: &'a S,
}

impl<'a, S: GraphStorage> HydratingSource<'a, S> {
    pub(super) fn new(upstream: Box<dyn RowSource + 'a>, storage: &'a S) -> Self {
        Self { upstream, storage }
    }
}

impl<'a, S: GraphStorage> RowSource for HydratingSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        match self.upstream.next_row()? {
            None => Ok(None),
            Some(row) => {
                let mut out = Row::new();
                for (var, name, value) in row.into_iter_named() {
                    out.insert_named(var, name, hydrate_value(value, self.storage));
                }
                Ok(Some(out))
            }
        }
    }
}

pub(super) fn hydrate_value<S: GraphStorage>(value: LoraValue, storage: &S) -> LoraValue {
    match value {
        LoraValue::Node(id) => storage
            .with_node(id, hydrate_node_record)
            .unwrap_or(LoraValue::Null),
        LoraValue::Relationship(id) => storage
            .with_relationship(id, hydrate_relationship_record)
            .unwrap_or(LoraValue::Null),
        LoraValue::List(values) => LoraValue::List(
            values
                .into_iter()
                .map(|v| hydrate_value(v, storage))
                .collect(),
        ),
        LoraValue::Map(map) => LoraValue::Map(
            map.into_iter()
                .map(|(k, v)| (k, hydrate_value(v, storage)))
                .collect(),
        ),
        other => other,
    }
}

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
pub(super) fn write_op_input(
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

        PhysicalOp::Sort(SortExec { input, items }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(SortSource::new(upstream, ctx, items)))
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

/// Pull-based read-write executor. Wraps the existing
/// [`MutableExecutor`] under the same row-cursor API. Mutations are
/// applied during `open_compiled`; the returned cursor yields the
/// resulting rows lazily.
pub struct MutablePullExecutor<'a, S: GraphStorageMut> {
    storage: &'a mut S,
    params: BTreeMap<String, LoraValue>,
}

impl<'a, S: GraphStorageMut + GraphStorage> MutablePullExecutor<'a, S> {
    pub fn new(storage: &'a mut S, params: BTreeMap<String, LoraValue>) -> Self {
        Self { storage, params }
    }

    /// Open a cursor for a compiled write query.
    ///
    /// Fast path: when a branch root is one of `Create` / `Set` /
    /// `Delete` / `Remove` / `Merge` and its input subtree is fully
    /// streamable, returns a [`StreamingWriteCursor`] that pulls input
    /// row-by-row and applies the per-row write through
    /// [`MutableExecutor::apply_write_op`]. `UNION ALL` plans stream
    /// one branch at a time. Plain `UNION` drains branches first so
    /// rows can be deduplicated by name.
    ///
    /// Fallback: a branch that is not streamable materializes through
    /// [`MutableExecutor::execute_rows`] and wraps the result in a
    /// [`BufferedRowSource`].
    pub fn open_compiled(self, compiled: &'a CompiledQuery) -> ExecResult<Box<dyn RowSource + 'a>>
    where
        S: 'a,
    {
        if compiled.unions.is_empty() {
            return open_mutable_plan_cursor(self.storage, &compiled.physical, self.params);
        }

        MutableUnionSource::open(self.storage, compiled, self.params)
            .map(|source| Box::new(source) as Box<dyn RowSource + 'a>)
    }
}

fn open_mutable_plan_cursor<'a, S: GraphStorageMut + GraphStorage + 'a>(
    storage: &'a mut S,
    plan: &'a PhysicalPlan,
    params: BTreeMap<String, LoraValue>,
) -> ExecResult<Box<dyn RowSource + 'a>> {
    if let Some(input) = write_op_input(plan, plan.root) {
        if subtree_is_fully_streaming(plan, input) {
            return StreamingWriteCursor::open(storage, plan, plan.root, params)
                .map(|c| Box::new(c) as Box<dyn RowSource + 'a>);
        }
    }

    let mut executor = MutableExecutor::new(MutableExecutionContext { storage, params });
    let rows = executor.execute_rows(plan)?;
    Ok(Box::new(BufferedRowSource::new(rows)))
}

#[derive(Clone, Copy)]
struct StoragePtr<S> {
    ptr: *mut S,
}

impl<S> StoragePtr<S> {
    fn from_mut(storage: &mut S) -> Self {
        Self {
            ptr: storage as *mut S,
        }
    }

    unsafe fn as_ref<'a>(&self) -> &'a S {
        unsafe { &*self.ptr }
    }

    unsafe fn as_mut<'a>(&self) -> &'a mut S {
        unsafe { &mut *self.ptr }
    }
}

/// Mutable UNION cursor. `UNION ALL` streams one branch at a time
/// against the same staged graph. Plain `UNION` streams branch-by-branch
/// while retaining only a seen-key set for deduplication.
pub struct MutableUnionSource<'a, S: GraphStorageMut + GraphStorage + 'a> {
    storage_ptr: StoragePtr<S>,
    compiled: &'a CompiledQuery,
    params: BTreeMap<String, LoraValue>,
    branch_idx: usize,
    current: Option<Box<dyn RowSource + 'a>>,
    needs_dedup: bool,
    seen: BTreeSet<Vec<(String, GroupValueKey)>>,
    _phantom: std::marker::PhantomData<&'a mut S>,
}

impl<'a, S: GraphStorageMut + GraphStorage + 'a> MutableUnionSource<'a, S> {
    fn open(
        storage: &'a mut S,
        compiled: &'a CompiledQuery,
        params: BTreeMap<String, LoraValue>,
    ) -> ExecResult<Self> {
        let needs_dedup = compiled.unions.iter().any(|branch| !branch.all);
        Ok(Self {
            storage_ptr: StoragePtr::from_mut(storage),
            compiled,
            params,
            branch_idx: 0,
            current: None,
            needs_dedup,
            seen: BTreeSet::new(),
            _phantom: std::marker::PhantomData,
        })
    }

    fn branch_count(&self) -> usize {
        self.compiled.unions.len() + 1
    }

    fn branch_plan(&self, idx: usize) -> &'a PhysicalPlan {
        if idx == 0 {
            &self.compiled.physical
        } else {
            &self.compiled.unions[idx - 1].physical
        }
    }

    fn open_branch(&mut self, idx: usize) -> ExecResult<Box<dyn RowSource + 'a>> {
        let plan = self.branch_plan(idx);
        // SAFETY: MutableUnionSource keeps at most one branch cursor
        // alive at a time. `current` is dropped before advancing to
        // the next branch, so each mutable reborrow is temporally
        // disjoint.
        let storage = unsafe { self.storage_ptr.as_mut() };
        open_mutable_plan_cursor(storage, plan, self.params.clone())
    }
}

impl<'a, S: GraphStorageMut + GraphStorage + 'a> RowSource for MutableUnionSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if self.branch_idx >= self.branch_count() {
                return Ok(None);
            }

            if self.current.is_none() {
                self.current = Some(self.open_branch(self.branch_idx)?);
            }

            match self
                .current
                .as_mut()
                .expect("current branch initialized above")
                .next_row()?
            {
                Some(row) => {
                    if self.needs_dedup {
                        let key = row
                            .iter_named()
                            .map(|(_, name, val)| {
                                (name.into_owned(), GroupValueKey::from_value(val))
                            })
                            .collect();
                        if !self.seen.insert(key) {
                            continue;
                        }
                    }
                    return Ok(Some(row));
                }
                None => {
                    self.current.take();
                    self.branch_idx += 1;
                }
            }
        }
    }
}

/// Streaming write cursor for plans whose root is one of
/// `Create` / `Set` / `Delete` / `Remove` / `Merge` and whose input
/// subtree is fully streamable.
///
/// # Layout invariant
///
/// The cursor owns a raw alias of the original `&'a mut S`.
/// Its `upstream` was constructed using a `&'a S` reborrow derived
/// from `storage_ptr` via unsafe lifetime extension. This is sound
/// because the existing read-side `RowSource` impls (see
/// `NodeScanSource::cur_ids`, `ExpandSource::cur_edges`, etc.)
/// materialize their iteration state into owned `Vec`s at
/// construction or first call, so no live `&S` borrow into storage
/// persists across `next_row` calls. Read-only access happens
/// transiently inside each `upstream.next_row` call; mutable access
/// happens between calls inside [`MutableExecutor::apply_write_op`].
/// The borrows never overlap in time.
///
/// # Drop order
///
/// `upstream` must drop before any caller may regain `&mut S` access
/// to the underlying storage. The explicit `Drop` impl enforces
/// that order — `ManuallyDrop` lets us force the sequence.
pub struct StreamingWriteCursor<'a, S: GraphStorageMut + GraphStorage + 'a> {
    /// SAFETY: borrows from `*storage_ptr`. Must drop first.
    upstream: ManuallyDrop<Box<dyn RowSource + 'a>>,
    /// Raw alias of the `&'a mut S` handed in at construction. Used
    /// as `&S` by `upstream` and as `&mut S` inside this cursor's `next_row`.
    storage_ptr: StoragePtr<S>,
    /// Physical plan — kept alive for the per-row op borrow.
    plan: &'a PhysicalPlan,
    /// Index into `plan.nodes` of the write operator.
    /// We re-fetch the op per call so this struct doesn't need to
    /// be parameterized by the specific op type.
    write_op_node: PhysicalNodeId,
    /// Parameters; cloned per row into a fresh `MutableExecutor`.
    /// In typical bulk-write workloads this is empty or tiny.
    params: BTreeMap<String, LoraValue>,
    _phantom: std::marker::PhantomData<&'a mut S>,
}

impl<'a, S: GraphStorageMut + GraphStorage + 'a> StreamingWriteCursor<'a, S> {
    /// Build a cursor. Caller must already have verified that
    /// `plan.nodes[write_op_node]` is a streamable write op via
    /// [`write_op_input`] and [`subtree_is_fully_streaming`].
    pub(crate) fn open(
        storage: &'a mut S,
        plan: &'a PhysicalPlan,
        write_op_node: PhysicalNodeId,
        params: BTreeMap<String, LoraValue>,
    ) -> ExecResult<Self> {
        let input = match write_op_input(plan, write_op_node) {
            Some(i) => i,
            None => {
                return Err(ExecutorError::RuntimeError(format!(
                    "StreamingWriteCursor::open called with non-write node {write_op_node:?}"
                )));
            }
        };
        let storage_ptr = StoragePtr::from_mut(storage);

        // SAFETY: see struct-level comment.
        let storage_ref: &'a S = unsafe { storage_ptr.as_ref() };
        let upstream = build_streaming(plan, input, storage_ref, Arc::new(params.clone()))?;

        Ok(Self {
            upstream: ManuallyDrop::new(upstream),
            storage_ptr,
            plan,
            write_op_node,
            params,
            _phantom: std::marker::PhantomData,
        })
    }
}

impl<'a, S: GraphStorageMut + GraphStorage + 'a> RowSource for StreamingWriteCursor<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        let mut row = match self.upstream.next_row()? {
            Some(r) => r,
            None => return Ok(None),
        };

        // SAFETY: upstream's `next_row` has returned, so its
        // dormant `&S` borrow is not in active use right now. We
        // reborrow `&mut S` for the per-row write and drop the
        // borrow before the next pull.
        let storage_mut: &mut S = unsafe { self.storage_ptr.as_mut() };
        let mut exec = MutableExecutor::new(MutableExecutionContext {
            storage: storage_mut,
            params: self.params.clone(),
        });
        let op = &self.plan.nodes[self.write_op_node];
        exec.apply_write_op(op, &mut row)?;
        let row = exec.hydrate_row(row);
        Ok(Some(row))
    }
}

impl<'a, S: GraphStorageMut + GraphStorage + 'a> Drop for StreamingWriteCursor<'a, S> {
    fn drop(&mut self) {
        // SAFETY: drop `upstream` first to release its borrow into
        // `*storage_ptr`. Subsequent fields drop via the normal
        // field-drop sequence and don't touch storage.
        unsafe {
            ManuallyDrop::drop(&mut self.upstream);
        }
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

// ---------------------------------------------------------------------------
// Stream classification
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Plan-derived result columns
// ---------------------------------------------------------------------------

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
        PhysicalOp::Filter(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Unwind(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Create(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Merge(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Delete(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Set(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Remove(p) => plan_columns_at(plan, p.input),
        PhysicalOp::Argument(_)
        | PhysicalOp::NodeScan(_)
        | PhysicalOp::NodeByLabelScan(_)
        | PhysicalOp::NodeByPropertyScan(_)
        | PhysicalOp::Expand(_) => None,
    }
}

/// Result column names for a compiled query (head plan; UNION branches
/// must produce the same shape so the head's columns are authoritative).
pub fn compiled_result_columns(compiled: &CompiledQuery) -> Vec<String> {
    plan_result_columns(&compiled.physical)
}
