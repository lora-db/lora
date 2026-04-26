//! Pull-based row pipeline.
//!
//! [`RowSource`] is the fallible row cursor; [`PullExecutor::open_compiled`]
//! and [`MutablePullExecutor::open_compiled`] return a `Box<dyn RowSource + 'a>`
//! representing a streaming query plan execution.
//!
//! ## Architecture
//!
//! The streaming-listed operators have real per-operator
//! [`RowSource`] implementations that pull from their upstream one
//! row at a time:
//!
//! * [`ArgumentSource`]
//! * [`NodeScanSource`]
//! * [`NodeByLabelScanSource`]
//! * [`ExpandSource`] (single-hop only — variable-length still buffers)
//! * [`FilterSource`]
//! * [`ProjectionSource`] (only when `distinct = false`)
//! * [`UnwindSource`]
//! * [`LimitSource`]
//!
//! Blocking / not-yet-streaming operators (Sort, HashAggregation,
//! DISTINCT projection, plain UNION, OptionalMatch, PathBuild,
//! ShortestPath filtering, variable-length Expand, write operators)
//! materialize their subtree through the existing
//! [`Executor::execute_subtree`] kernel and present the result as a
//! [`BufferedRowSource`]. The same kernels back the direct
//! `Executor::execute_compiled_rows` path, so both code paths share
//! one row-computation semantics.
//!
//! Hydration happens once at the top of the pipeline — operator
//! sources yield raw rows so intermediate evaluations work on
//! storage-borrowed values, and the topmost [`HydratingSource`]
//! converts node / relationship references to their full hydrated
//! map form before the row leaves the cursor.

use std::collections::BTreeMap;
use std::sync::Arc;

use lora_analyzer::symbols::VarId;
use lora_analyzer::{ResolvedExpr, ResolvedProjection};
use lora_ast::Direction;
use lora_compiler::physical::{
    ExpandExec, FilterExec, LimitExec, NodeByLabelScanExec, NodeScanExec, PhysicalNodeId,
    PhysicalOp, PhysicalPlan, ProjectionExec, UnwindExec,
};
use lora_compiler::CompiledQuery;
use lora_store::{GraphStorage, GraphStorageMut, NodeId};

use crate::errors::{value_kind, ExecResult, ExecutorError};
use crate::eval::{eval_expr, take_eval_error, EvalContext};
use crate::executor::{
    hydrate_node_record, hydrate_relationship_record, node_matches_label_groups,
    scan_node_ids_for_label_groups, value_matches_property_value, ExecutionContext, Executor,
    MutableExecutionContext, MutableExecutor,
};
use crate::value::{LoraValue, Row};

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
pub(crate) struct StreamCtx<'a, S: GraphStorage> {
    pub storage: &'a S,
    pub params: Arc<BTreeMap<String, LoraValue>>,
}

impl<'a, S: GraphStorage> StreamCtx<'a, S> {
    fn new(storage: &'a S, params: Arc<BTreeMap<String, LoraValue>>) -> Self {
        Self { storage, params }
    }

    /// Build a borrowing [`EvalContext`] for use inside an
    /// operator's `next_row` method. Cheap — two pointer reads.
    fn eval_ctx<'b>(&'b self) -> EvalContext<'b, S> {
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
/// a simple "rows already collected" adapter and as the fallback path
/// for operators that have no streaming source yet (Sort,
/// HashAggregation, DISTINCT, plain UNION, ShortestPath, etc.).
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
// Per-operator streaming sources
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

/// Streams `(input × node_ids)`. For each upstream row, emits one
/// row per node id with `var` bound. If `var` is already bound in
/// the incoming row, the input row passes through iff that node
/// still exists (or fails on a non-node binding).
pub struct NodeScanSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    storage: &'a S,
    var: VarId,
    /// The currently-active input row. `None` means the next call
    /// must pull a fresh row from upstream.
    cur_row: Option<Row>,
    /// All node ids the next call should traverse for the current
    /// input row.
    cur_ids: Vec<NodeId>,
    /// Position into `cur_ids`.
    cur_idx: usize,
    /// Already emitted the current row when `var` was already bound.
    cur_emitted: bool,
}

impl<'a, S: GraphStorage> NodeScanSource<'a, S> {
    fn new(upstream: Box<dyn RowSource + 'a>, storage: &'a S, var: VarId) -> Self {
        Self {
            upstream,
            storage,
            var,
            cur_row: None,
            cur_ids: Vec::new(),
            cur_idx: 0,
            cur_emitted: false,
        }
    }
}

impl<'a, S: GraphStorage> RowSource for NodeScanSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if self.cur_row.is_none() {
                match self.upstream.next_row()? {
                    Some(row) => {
                        self.cur_row = Some(row);
                        self.cur_ids.clear();
                        self.cur_idx = 0;
                        self.cur_emitted = false;
                    }
                    None => return Ok(None),
                }
            }

            let row_ref = self.cur_row.as_ref().unwrap();

            // Already-bound case: emit the input row once iff the
            // bound node still exists; otherwise drop it.
            if let Some(existing) = row_ref.get(self.var) {
                if self.cur_emitted {
                    self.cur_row = None;
                    continue;
                }
                self.cur_emitted = true;
                match existing {
                    LoraValue::Node(id) => {
                        if self.storage.has_node(*id) {
                            let row = self.cur_row.take().unwrap();
                            self.cur_emitted = false;
                            return Ok(Some(row));
                        }
                        self.cur_row = None;
                        continue;
                    }
                    other => {
                        return Err(ExecutorError::ExpectedNodeForExpand {
                            var: format!("{:?}", self.var),
                            found: value_kind(other),
                        });
                    }
                }
            }

            // Unbound case: lazily snapshot all node ids for this
            // input row, then yield one row per id.
            if self.cur_idx == 0 && self.cur_ids.is_empty() {
                self.cur_ids = self.storage.all_node_ids();
            }
            if self.cur_idx >= self.cur_ids.len() {
                self.cur_row = None;
                self.cur_ids.clear();
                continue;
            }
            let id = self.cur_ids[self.cur_idx];
            self.cur_idx += 1;
            let mut new_row = row_ref.clone();
            new_row.insert(self.var, LoraValue::Node(id));
            return Ok(Some(new_row));
        }
    }
}

/// Streams `(input × matching-label nodes)`. Same shape as
/// [`NodeScanSource`] but the candidate ids are produced by
/// [`scan_node_ids_for_label_groups`] and each candidate is
/// re-checked under [`node_matches_label_groups`].
pub struct NodeByLabelScanSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    storage: &'a S,
    var: VarId,
    labels: &'a [Vec<String>],
    cur_row: Option<Row>,
    cur_ids: Vec<NodeId>,
    cur_idx: usize,
    cur_emitted: bool,
}

impl<'a, S: GraphStorage> NodeByLabelScanSource<'a, S> {
    fn new(
        upstream: Box<dyn RowSource + 'a>,
        storage: &'a S,
        var: VarId,
        labels: &'a [Vec<String>],
    ) -> Self {
        Self {
            upstream,
            storage,
            var,
            labels,
            cur_row: None,
            cur_ids: Vec::new(),
            cur_idx: 0,
            cur_emitted: false,
        }
    }
}

impl<'a, S: GraphStorage> RowSource for NodeByLabelScanSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if self.cur_row.is_none() {
                match self.upstream.next_row()? {
                    Some(row) => {
                        self.cur_row = Some(row);
                        self.cur_ids.clear();
                        self.cur_idx = 0;
                        self.cur_emitted = false;
                    }
                    None => return Ok(None),
                }
            }

            let row_ref = self.cur_row.as_ref().unwrap();

            if let Some(existing) = row_ref.get(self.var) {
                if self.cur_emitted {
                    self.cur_row = None;
                    continue;
                }
                self.cur_emitted = true;
                match existing {
                    LoraValue::Node(id) => {
                        let labels_ok = self
                            .storage
                            .with_node(*id, |n| node_matches_label_groups(&n.labels, self.labels))
                            .unwrap_or(false);
                        if labels_ok {
                            let row = self.cur_row.take().unwrap();
                            self.cur_emitted = false;
                            return Ok(Some(row));
                        }
                        self.cur_row = None;
                        continue;
                    }
                    other => {
                        return Err(ExecutorError::ExpectedNodeForExpand {
                            var: format!("{:?}", self.var),
                            found: value_kind(other),
                        });
                    }
                }
            }

            if self.cur_idx == 0 && self.cur_ids.is_empty() {
                self.cur_ids = scan_node_ids_for_label_groups(self.storage, self.labels);
            }

            // Skip non-matching ids cheaply.
            while self.cur_idx < self.cur_ids.len() {
                let id = self.cur_ids[self.cur_idx];
                self.cur_idx += 1;
                let labels_ok = self
                    .storage
                    .with_node(id, |n| node_matches_label_groups(&n.labels, self.labels))
                    .unwrap_or(false);
                if !labels_ok {
                    continue;
                }
                let mut new_row = row_ref.clone();
                new_row.insert(self.var, LoraValue::Node(id));
                return Ok(Some(new_row));
            }

            self.cur_row = None;
            self.cur_ids.clear();
        }
    }
}

/// Single-hop expansion. For each input row, walks edges from `src`
/// through the configured `direction` and `types` and emits one row
/// per matching `(rel, dst)` pair, optionally filtering by relationship
/// properties. Variable-length expansion is delegated to the buffered
/// fallback.
pub struct ExpandSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    src: VarId,
    rel: Option<VarId>,
    dst: VarId,
    types: &'a [String],
    direction: Direction,
    rel_properties: Option<&'a ResolvedExpr>,
    cur_row: Option<Row>,
    cur_edges: Vec<(u64, NodeId)>,
    cur_idx: usize,
}

impl<'a, S: GraphStorage> ExpandSource<'a, S> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        src: VarId,
        rel: Option<VarId>,
        dst: VarId,
        types: &'a [String],
        direction: Direction,
        rel_properties: Option<&'a ResolvedExpr>,
    ) -> Self {
        Self {
            upstream,
            ctx,
            src,
            rel,
            dst,
            types,
            direction,
            rel_properties,
            cur_row: None,
            cur_edges: Vec::new(),
            cur_idx: 0,
        }
    }
}

impl<'a, S: GraphStorage> RowSource for ExpandSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if self.cur_row.is_none() {
                match self.upstream.next_row()? {
                    Some(row) => {
                        // Resolve src now so we can `continue` out of
                        // rows that don't bind it.
                        let src_id = match row.get(self.src) {
                            Some(LoraValue::Node(id)) => *id,
                            Some(other) => {
                                return Err(ExecutorError::ExpectedNodeForExpand {
                                    var: format!("{:?}", self.src),
                                    found: value_kind(other),
                                });
                            }
                            None => continue,
                        };
                        self.cur_edges =
                            self.ctx
                                .storage
                                .expand_ids(src_id, self.direction, self.types);
                        self.cur_idx = 0;
                        self.cur_row = Some(row);
                    }
                    None => return Ok(None),
                }
            }

            let row_ref = self.cur_row.as_ref().unwrap();

            while self.cur_idx < self.cur_edges.len() {
                let (rel_id, dst_id) = self.cur_edges[self.cur_idx];
                self.cur_idx += 1;

                // Relationship-property prefilter.
                if let Some(expr) = self.rel_properties {
                    let actual = self
                        .ctx
                        .storage
                        .with_relationship(rel_id, |rel| rel.properties.clone());
                    let matches = match actual {
                        Some(props) => {
                            let eval_ctx = self.ctx.eval_ctx();
                            let expected = eval_expr(expr, row_ref, &eval_ctx);
                            let LoraValue::Map(map) = expected else {
                                return Err(ExecutorError::ExpectedPropertyMap {
                                    found: value_kind(&expected),
                                });
                            };
                            map.iter().all(|(k, v)| {
                                props
                                    .get(k)
                                    .map(|actual| value_matches_property_value(v, actual))
                                    .unwrap_or(false)
                            })
                        }
                        None => false,
                    };
                    if !matches {
                        continue;
                    }
                }

                // Existing-binding cross-checks for dst and rel.
                if let Some(existing_dst) = row_ref.get(self.dst) {
                    match existing_dst {
                        LoraValue::Node(id) if *id == dst_id => {}
                        LoraValue::Node(_) => continue,
                        other => {
                            return Err(ExecutorError::ExpectedNodeForExpand {
                                var: format!("{:?}", self.dst),
                                found: value_kind(other),
                            });
                        }
                    }
                }
                if let Some(rel_var) = self.rel {
                    if let Some(existing_rel) = row_ref.get(rel_var) {
                        match existing_rel {
                            LoraValue::Relationship(id) if *id == rel_id => {}
                            LoraValue::Relationship(_) => continue,
                            other => {
                                return Err(ExecutorError::ExpectedRelationshipForExpand {
                                    var: format!("{:?}", rel_var),
                                    found: value_kind(other),
                                });
                            }
                        }
                    }
                }

                let mut new_row = row_ref.clone();
                if !new_row.contains_key(self.dst) {
                    new_row.insert(self.dst, LoraValue::Node(dst_id));
                }
                if let Some(rel_var) = self.rel {
                    if !new_row.contains_key(rel_var) {
                        new_row.insert(rel_var, LoraValue::Relationship(rel_id));
                    }
                }
                return Ok(Some(new_row));
            }

            // Edges for the current input row exhausted.
            self.cur_row = None;
            self.cur_edges.clear();
            self.cur_idx = 0;
        }
    }
}

/// Pulls upstream rows until one matches `predicate`, then yields it.
pub struct FilterSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    predicate: &'a ResolvedExpr,
}

impl<'a, S: GraphStorage> FilterSource<'a, S> {
    fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        predicate: &'a ResolvedExpr,
    ) -> Self {
        Self {
            upstream,
            ctx,
            predicate,
        }
    }
}

impl<'a, S: GraphStorage> RowSource for FilterSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            match self.upstream.next_row()? {
                Some(row) => {
                    let eval_ctx = self.ctx.eval_ctx();
                    if eval_expr(self.predicate, &row, &eval_ctx).is_truthy() {
                        return Ok(Some(row));
                    }
                }
                None => return Ok(None),
            }
        }
    }
}

/// Pulls one upstream row, projects each item, returns a single row
/// per upstream row. Streaming-only when `distinct = false`; the
/// distinct path collapses to the buffered fallback because dedup is
/// inherently blocking.
pub struct ProjectionSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    items: &'a [ResolvedProjection],
    include_existing: bool,
}

impl<'a, S: GraphStorage> ProjectionSource<'a, S> {
    fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        items: &'a [ResolvedProjection],
        include_existing: bool,
    ) -> Self {
        Self {
            upstream,
            ctx,
            items,
            include_existing,
        }
    }
}

impl<'a, S: GraphStorage> RowSource for ProjectionSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        match self.upstream.next_row()? {
            None => Ok(None),
            Some(row) => {
                let eval_ctx = self.ctx.eval_ctx();
                if self.include_existing {
                    let mut projected = row;
                    for item in self.items {
                        let value = eval_expr(&item.expr, &projected, &eval_ctx);
                        if let Some(err) = take_eval_error() {
                            return Err(ExecutorError::RuntimeError(err));
                        }
                        projected.insert_named(item.output, item.name.clone(), value);
                    }
                    Ok(Some(projected))
                } else {
                    let mut projected = Row::new();
                    for item in self.items {
                        let value = eval_expr(&item.expr, &row, &eval_ctx);
                        if let Some(err) = take_eval_error() {
                            return Err(ExecutorError::RuntimeError(err));
                        }
                        projected.insert_named(item.output, item.name.clone(), value);
                    }
                    Ok(Some(projected))
                }
            }
        }
    }
}

/// Per upstream row, evaluates the unwind expression and emits one
/// row per element of the resulting list. Null inputs are dropped;
/// scalar inputs are emitted once.
pub struct UnwindSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    expr: &'a ResolvedExpr,
    alias: VarId,
    cur_row: Option<Row>,
    cur_values: Vec<LoraValue>,
    cur_idx: usize,
}

impl<'a, S: GraphStorage> UnwindSource<'a, S> {
    fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        expr: &'a ResolvedExpr,
        alias: VarId,
    ) -> Self {
        Self {
            upstream,
            ctx,
            expr,
            alias,
            cur_row: None,
            cur_values: Vec::new(),
            cur_idx: 0,
        }
    }
}

impl<'a, S: GraphStorage> RowSource for UnwindSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if self.cur_idx < self.cur_values.len() {
                let value = self.cur_values[self.cur_idx].clone();
                self.cur_idx += 1;
                let mut new_row = self
                    .cur_row
                    .as_ref()
                    .expect("cur_values is non-empty implies cur_row is set")
                    .clone();
                new_row.insert(self.alias, value);
                return Ok(Some(new_row));
            }

            self.cur_row = None;
            self.cur_values.clear();
            self.cur_idx = 0;

            match self.upstream.next_row()? {
                None => return Ok(None),
                Some(row) => {
                    let eval_ctx = self.ctx.eval_ctx();
                    let value = eval_expr(self.expr, &row, &eval_ctx);
                    match value {
                        LoraValue::List(values) => {
                            self.cur_row = Some(row);
                            self.cur_values = values;
                            self.cur_idx = 0;
                            // loop around
                        }
                        LoraValue::Null => {
                            // Drop this input row entirely.
                        }
                        scalar => {
                            // Emit one row with the scalar bound.
                            let mut new_row = row;
                            new_row.insert(self.alias, scalar);
                            return Ok(Some(new_row));
                        }
                    }
                }
            }
        }
    }
}

/// Skip the first `skip` rows, emit at most `limit` rows from
/// upstream, then return `None` regardless of whether upstream is
/// exhausted (avoids paying for a partially consumed upstream).
pub struct LimitSource<'a> {
    upstream: Box<dyn RowSource + 'a>,
    skip: usize,
    limit: Option<usize>,
    skipped: usize,
    emitted: usize,
}

impl<'a> LimitSource<'a> {
    fn new(upstream: Box<dyn RowSource + 'a>, skip: usize, limit: Option<usize>) -> Self {
        Self {
            upstream,
            skip,
            limit,
            skipped: 0,
            emitted: 0,
        }
    }
}

impl<'a> RowSource for LimitSource<'a> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        // Drain skip first.
        while self.skipped < self.skip {
            match self.upstream.next_row()? {
                Some(_) => self.skipped += 1,
                None => return Ok(None),
            }
        }
        if let Some(lim) = self.limit {
            if self.emitted >= lim {
                return Ok(None);
            }
        }
        match self.upstream.next_row()? {
            Some(row) => {
                self.emitted += 1;
                Ok(Some(row))
            }
            None => Ok(None),
        }
    }
}

/// Top-of-pipeline hydration. Replaces node / relationship id
/// references in each emitted row with their full hydrated map form,
/// matching the buffered executor's post-execution hydration step.
pub struct HydratingSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    storage: &'a S,
}

impl<'a, S: GraphStorage> HydratingSource<'a, S> {
    fn new(upstream: Box<dyn RowSource + 'a>, storage: &'a S) -> Self {
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

pub(crate) fn hydrate_value<S: GraphStorage>(value: LoraValue, storage: &S) -> LoraValue {
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
// Plan walker
// ---------------------------------------------------------------------------

/// True iff this op has a per-operator streaming source. Operators
/// that aren't on this list fall back to a single materialized
/// [`Executor::execute_subtree`] call wrapped as a [`BufferedRowSource`].
fn is_streaming_op(op: &PhysicalOp) -> bool {
    match op {
        PhysicalOp::Argument(_)
        | PhysicalOp::NodeScan(_)
        | PhysicalOp::NodeByLabelScan(_)
        | PhysicalOp::Filter(_)
        | PhysicalOp::Unwind(_)
        | PhysicalOp::Limit(_) => true,
        // Single-hop only — variable-length expansion has its own
        // BFS allocator and stays buffered for now.
        PhysicalOp::Expand(e) => e.range.is_none(),
        // Distinct projection requires dedup; keep buffered.
        PhysicalOp::Projection(p) => !p.distinct,
        _ => false,
    }
}

fn build_streaming<'a, S: GraphStorage + 'a>(
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

        PhysicalOp::Expand(ExpandExec {
            input,
            src,
            rel,
            dst,
            types,
            direction,
            rel_properties,
            range: _, // None already enforced by `is_streaming_op`
        }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(ExpandSource::new(
                upstream,
                ctx,
                *src,
                *rel,
                *dst,
                types,
                *direction,
                rel_properties.as_ref(),
            )))
        }

        PhysicalOp::Filter(FilterExec { input, predicate }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(FilterSource::new(upstream, ctx, predicate)))
        }

        PhysicalOp::Projection(ProjectionExec {
            input,
            distinct: _, // false enforced by is_streaming_op
            items,
            include_existing,
        }) => {
            let upstream = build_streaming(plan, *input, storage, params.clone())?;
            let ctx = StreamCtx::new(storage, params);
            Ok(Box::new(ProjectionSource::new(
                upstream,
                ctx,
                items,
                *include_existing,
            )))
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
/// is the path for blocking operators (Sort, HashAggregation,
/// DISTINCT, plain UNION, ShortestPath, OptionalMatch, PathBuild,
/// variable-length Expand) and for write operators in the read-only
/// pull executor.
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

    /// Open a cursor for a compiled query.
    ///
    /// UNION-bearing plans collapse to a single [`BufferedRowSource`]
    /// because the head and branches need to be combined (and
    /// optionally deduped) before any row is emitted; the underlying
    /// row computation still runs through the same `Executor`
    /// kernel as the direct path.
    pub fn open_compiled(self, compiled: &'a CompiledQuery) -> ExecResult<Box<dyn RowSource + 'a>>
    where
        S: 'a,
    {
        let _ = take_eval_error();

        // UNION dispatches via the buffered executor since dedup
        // and branch-concatenation are inherently blocking.
        if !compiled.unions.is_empty() {
            let executor = Executor::new(ExecutionContext {
                storage: self.storage,
                params: self.params,
            });
            let rows = executor.execute_compiled_rows(compiled)?;
            return Ok(Box::new(BufferedRowSource::new(rows)));
        }

        let plan = &compiled.physical;
        let params = Arc::new(self.params);
        let inner = build_streaming(plan, plan.root, self.storage, params)?;
        Ok(Box::new(HydratingSource::new(inner, self.storage)))
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

impl<'a, S: GraphStorageMut> MutablePullExecutor<'a, S> {
    pub fn new(storage: &'a mut S, params: BTreeMap<String, LoraValue>) -> Self {
        Self { storage, params }
    }

    pub fn open_compiled(self, compiled: &CompiledQuery) -> ExecResult<Box<dyn RowSource + 'a>>
    where
        S: 'a,
    {
        let mut executor = MutableExecutor::new(MutableExecutionContext {
            storage: self.storage,
            params: self.params,
        });
        let rows = executor.execute_compiled_rows(compiled)?;
        Ok(Box::new(BufferedRowSource::new(rows)))
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
        | PhysicalOp::Expand(_) => None,
    }
}

/// Result column names for a compiled query (head plan; UNION branches
/// must produce the same shape so the head's columns are authoritative).
pub fn compiled_result_columns(compiled: &CompiledQuery) -> Vec<String> {
    plan_result_columns(&compiled.physical)
}
