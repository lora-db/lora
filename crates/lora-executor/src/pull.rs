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
//! * [`ExpandSource`] (single-hop)
//! * [`VariableLengthExpandSource`]
//! * [`FilterSource`]
//! * [`ProjectionSource`]
//! * [`DistinctSource`]
//! * [`UnwindSource`]
//! * [`LimitSource`]
//! * [`SortSource`] (buffers internally, yields lazily)
//! * [`HashAggregationSource`] (buffers internally, yields lazily)
//! * [`OptionalMatchSource`] (streams outer input, buffers inner once)
//! * [`PathBuildSource`]
//!
//! Blocking internals such as sort, aggregation, and shortest-path
//! filtering still allocate where the Cypher semantics require a
//! complete input set. Deduping operators keep only their seen-key
//! state and stream rows as soon as a new key appears.
//!
//! Hydration happens once at the top of the pipeline — operator
//! sources yield raw rows so intermediate evaluations work on
//! storage-borrowed values, and the topmost [`HydratingSource`]
//! converts node / relationship references to their full hydrated
//! map form before the row leaves the cursor.

use std::collections::{BTreeMap, BTreeSet};
use std::mem::ManuallyDrop;
use std::sync::Arc;

use lora_analyzer::symbols::VarId;
use lora_analyzer::{ResolvedExpr, ResolvedProjection};
use lora_ast::{Direction, RangeLiteral};
use lora_compiler::physical::{
    ExpandExec, FilterExec, HashAggregationExec, LimitExec, NodeByLabelScanExec,
    NodeByPropertyScanExec, NodeScanExec, OptionalMatchExec, PathBuildExec, PhysicalNodeId,
    PhysicalOp, PhysicalPlan, ProjectionExec, SortExec, UnwindExec,
};
use lora_compiler::CompiledQuery;
use lora_store::{GraphStorage, GraphStorageMut, NodeId};

use crate::errors::{value_kind, ExecResult, ExecutorError};
use crate::eval::{eval_expr, take_eval_error, EvalContext};
use crate::executor::{
    build_path_value, compute_aggregate_expr, hydrate_node_record, hydrate_relationship_record,
    indexed_node_property_candidates, label_group_candidates_prefiltered,
    node_matches_label_groups, node_matches_property_filter, resolve_range,
    scan_node_ids_for_label_groups, value_matches_property_value, ExecutionContext, Executor,
    GroupValueKey, MutableExecutionContext, MutableExecutor,
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
            let candidates_prefiltered = label_group_candidates_prefiltered(self.labels);
            while self.cur_idx < self.cur_ids.len() {
                let id = self.cur_ids[self.cur_idx];
                self.cur_idx += 1;
                if !candidates_prefiltered {
                    let labels_ok = self
                        .storage
                        .with_node(id, |n| node_matches_label_groups(&n.labels, self.labels))
                        .unwrap_or(false);
                    if !labels_ok {
                        continue;
                    }
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

/// Streams `(input × indexed-property nodes)`. The property index supplies
/// candidate ids and each candidate is re-checked under the full label and
/// property equality semantics.
pub struct NodeByPropertyScanSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    var: VarId,
    labels: &'a [Vec<String>],
    key: &'a str,
    value: &'a ResolvedExpr,
    cur_row: Option<Row>,
    cur_expected: Option<LoraValue>,
    cur_ids: Vec<NodeId>,
    cur_idx: usize,
    cur_emitted: bool,
    cur_prefiltered: bool,
}

impl<'a, S: GraphStorage> NodeByPropertyScanSource<'a, S> {
    fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        var: VarId,
        labels: &'a [Vec<String>],
        key: &'a str,
        value: &'a ResolvedExpr,
    ) -> Self {
        Self {
            upstream,
            ctx,
            var,
            labels,
            key,
            value,
            cur_row: None,
            cur_expected: None,
            cur_ids: Vec::new(),
            cur_idx: 0,
            cur_emitted: false,
            cur_prefiltered: false,
        }
    }
}

impl<'a, S: GraphStorage> RowSource for NodeByPropertyScanSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if self.cur_row.is_none() {
                match self.upstream.next_row()? {
                    Some(row) => {
                        let expected = eval_expr(self.value, &row, &self.ctx.eval_ctx());
                        let candidates = indexed_node_property_candidates(
                            self.ctx.storage,
                            self.labels,
                            self.key,
                            &expected,
                        );
                        self.cur_ids = candidates.ids;
                        self.cur_prefiltered = candidates.prefiltered;
                        self.cur_row = Some(row);
                        self.cur_expected = Some(expected);
                        self.cur_idx = 0;
                        self.cur_emitted = false;
                    }
                    None => return Ok(None),
                }
            }

            let row_ref = self.cur_row.as_ref().unwrap();
            let expected = self.cur_expected.as_ref().unwrap();

            if let Some(existing) = row_ref.get(self.var) {
                if self.cur_emitted {
                    self.cur_row = None;
                    self.cur_expected = None;
                    self.cur_ids.clear();
                    continue;
                }
                self.cur_emitted = true;
                match existing {
                    LoraValue::Node(id) => {
                        if node_matches_property_filter(
                            self.ctx.storage,
                            *id,
                            self.labels,
                            self.key,
                            expected,
                        ) {
                            let row = self.cur_row.take().unwrap();
                            self.cur_expected = None;
                            self.cur_ids.clear();
                            self.cur_emitted = false;
                            return Ok(Some(row));
                        }
                        self.cur_row = None;
                        self.cur_expected = None;
                        self.cur_ids.clear();
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

            while self.cur_idx < self.cur_ids.len() {
                let id = self.cur_ids[self.cur_idx];
                self.cur_idx += 1;
                if !self.cur_prefiltered
                    && !node_matches_property_filter(
                        self.ctx.storage,
                        id,
                        self.labels,
                        self.key,
                        expected,
                    )
                {
                    continue;
                }
                let mut new_row = row_ref.clone();
                new_row.insert(self.var, LoraValue::Node(id));
                return Ok(Some(new_row));
            }

            self.cur_row = None;
            self.cur_expected = None;
            self.cur_ids.clear();
        }
    }
}

/// Single-hop expansion. For each input row, walks edges from `src`
/// through the configured `direction` and `types` and emits one row
/// per matching `(rel, dst)` pair, optionally filtering by relationship
/// properties.
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

/// Variable-length expansion streams its upstream and walks one input row's BFS
/// frontier incrementally, yielding each matching path as it is discovered.
pub struct VariableLengthExpandSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    src: VarId,
    rel: Option<VarId>,
    dst: VarId,
    types: &'a [String],
    direction: Direction,
    min_hops: u64,
    max_hops: u64,
    cur_row: Option<Row>,
    pending_zero_hop: bool,
    frontier: Vec<(NodeId, Vec<u64>)>,
    frontier_idx: usize,
    next_frontier: Vec<(NodeId, Vec<u64>)>,
    depth: u64,
    cur_path_node: Option<NodeId>,
    cur_path_rels: Vec<u64>,
    cur_edges: Vec<(u64, NodeId)>,
    cur_edge_idx: usize,
}

impl<'a, S: GraphStorage> VariableLengthExpandSource<'a, S> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        src: VarId,
        rel: Option<VarId>,
        dst: VarId,
        types: &'a [String],
        direction: Direction,
        range: &'a RangeLiteral,
    ) -> Self {
        let (min_hops, max_hops) = resolve_range(range);
        Self {
            upstream,
            ctx,
            src,
            rel,
            dst,
            types,
            direction,
            min_hops,
            max_hops,
            cur_row: None,
            pending_zero_hop: false,
            frontier: Vec::new(),
            frontier_idx: 0,
            next_frontier: Vec::new(),
            depth: 1,
            cur_path_node: None,
            cur_path_rels: Vec::new(),
            cur_edges: Vec::new(),
            cur_edge_idx: 0,
        }
    }

    fn start_row(&mut self, row: Row, src_id: NodeId) {
        self.cur_row = Some(row);
        self.pending_zero_hop = self.min_hops == 0;
        self.frontier.clear();
        self.frontier.push((src_id, Vec::new()));
        self.frontier_idx = 0;
        self.next_frontier.clear();
        self.depth = 1;
        self.cur_path_node = None;
        self.cur_path_rels.clear();
        self.cur_edges.clear();
        self.cur_edge_idx = 0;
    }

    fn clear_current_row(&mut self) {
        self.cur_row = None;
        self.pending_zero_hop = false;
        self.frontier.clear();
        self.frontier_idx = 0;
        self.next_frontier.clear();
        self.depth = 1;
        self.cur_path_node = None;
        self.cur_path_rels.clear();
        self.cur_edges.clear();
        self.cur_edge_idx = 0;
    }

    fn row_for_path(&self, dst_node_id: NodeId, rel_ids: &[u64]) -> Row {
        let mut new_row = self
            .cur_row
            .as_ref()
            .expect("cur_row is set while yielding variable-length results")
            .clone();
        new_row.insert(self.dst, LoraValue::Node(dst_node_id));

        if let Some(rel_var) = self.rel {
            let rels = rel_ids
                .iter()
                .copied()
                .map(LoraValue::Relationship)
                .collect();
            new_row.insert(rel_var, LoraValue::List(rels));
        }

        new_row
    }

    fn advance_frontier(&mut self) -> bool {
        if self.next_frontier.is_empty() || self.depth >= self.max_hops {
            return false;
        }

        std::mem::swap(&mut self.frontier, &mut self.next_frontier);
        self.next_frontier.clear();
        self.frontier_idx = 0;
        self.depth += 1;
        true
    }
}

impl<'a, S: GraphStorage> RowSource for VariableLengthExpandSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if self.cur_row.is_none() {
                match self.upstream.next_row()? {
                    Some(row) => {
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
                        self.start_row(row, src_id);
                    }
                    None => return Ok(None),
                }
            }

            if self.pending_zero_hop {
                self.pending_zero_hop = false;
                if let Some(LoraValue::Node(src_id)) = self
                    .cur_row
                    .as_ref()
                    .expect("cur_row is initialized before zero-hop yield")
                    .get(self.src)
                {
                    return Ok(Some(self.row_for_path(*src_id, &[])));
                }
            }

            loop {
                if self.depth > self.max_hops {
                    self.clear_current_row();
                    break;
                }

                if self.cur_path_node.is_none() {
                    if self.frontier_idx >= self.frontier.len() {
                        if self.advance_frontier() {
                            continue;
                        }
                        self.clear_current_row();
                        break;
                    }

                    let (node_id, rels) = &self.frontier[self.frontier_idx];
                    self.frontier_idx += 1;
                    self.cur_path_node = Some(*node_id);
                    self.cur_path_rels.clone_from(rels);
                    self.cur_edges =
                        self.ctx
                            .storage
                            .expand_ids(*node_id, self.direction, self.types);
                    self.cur_edge_idx = 0;
                }

                while self.cur_edge_idx < self.cur_edges.len() {
                    let (rel_id, neighbor_id) = self.cur_edges[self.cur_edge_idx];
                    self.cur_edge_idx += 1;

                    if self.cur_path_rels.contains(&rel_id) {
                        continue;
                    }

                    let mut rel_ids = Vec::with_capacity(self.cur_path_rels.len() + 1);
                    rel_ids.extend_from_slice(&self.cur_path_rels);
                    rel_ids.push(rel_id);

                    if self.depth < self.max_hops {
                        self.next_frontier.push((neighbor_id, rel_ids.clone()));
                    }

                    if self.depth >= self.min_hops {
                        return Ok(Some(self.row_for_path(neighbor_id, &rel_ids)));
                    }
                }

                self.cur_path_node = None;
                self.cur_path_rels.clear();
                self.cur_edges.clear();
                self.cur_edge_idx = 0;
            }
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
/// per upstream row. `DISTINCT` projection wraps this source in
/// [`DistinctSource`], which keeps a seen-key set and yields lazily.
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

/// Lazy-buffered Sort source. On the first call to `next_row`,
/// drains the entire upstream into a `Vec`, sorts it by the plan's
/// sort items, then yields one row at a time on subsequent calls.
///
/// Memory is O(N) in the number of input rows — Sort can't avoid
/// that. The win is that everything *above* a `SortSource` (typically
/// a write op like CREATE / SET) streams: the auto-commit pipeline
/// pulls one sorted row, applies the per-row write, and emits,
/// instead of materializing both Sort's output and the write op's
/// output.
pub struct SortSource<'a, S: GraphStorage> {
    state: SortState<'a, S>,
}

enum SortState<'a, S: GraphStorage> {
    Pending {
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        items: &'a [lora_analyzer::ResolvedSortItem],
    },
    Yielding(std::vec::IntoIter<Row>),
}

impl<'a, S: GraphStorage> SortSource<'a, S> {
    fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        items: &'a [lora_analyzer::ResolvedSortItem],
    ) -> Self {
        Self {
            state: SortState::Pending {
                upstream,
                ctx,
                items,
            },
        }
    }

    /// Drain upstream into a vector and sort it by the plan's
    /// sort items. Called from `next_row` on the first invocation.
    fn materialize(
        upstream: &mut Box<dyn RowSource + 'a>,
        ctx: &StreamCtx<'a, S>,
        items: &[lora_analyzer::ResolvedSortItem],
    ) -> ExecResult<Vec<Row>> {
        let mut rows = drain(upstream.as_mut())?;
        let eval_ctx = ctx.eval_ctx();
        rows.sort_by(|a, b| {
            for item in items {
                let ord = crate::executor::compare_sort_item(item, a, b, &eval_ctx);
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            std::cmp::Ordering::Equal
        });
        Ok(rows)
    }
}

impl<'a, S: GraphStorage> RowSource for SortSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            match &mut self.state {
                SortState::Pending {
                    upstream,
                    ctx,
                    items,
                } => {
                    let rows = Self::materialize(upstream, ctx, items)?;
                    self.state = SortState::Yielding(rows.into_iter());
                    // fall through to the Yielding match on the next iteration.
                }
                SortState::Yielding(it) => return Ok(it.next()),
            }
        }
    }
}

/// Streaming DISTINCT source. Backs `Projection { distinct: true }`.
/// It keeps only the seen key set, then yields each first-seen row as
/// soon as upstream produces it.
pub struct DistinctSource<'a> {
    upstream: Box<dyn RowSource + 'a>,
    seen: BTreeSet<Vec<GroupValueKey>>,
}

impl<'a> DistinctSource<'a> {
    fn new(upstream: Box<dyn RowSource + 'a>) -> Self {
        Self {
            upstream,
            seen: BTreeSet::new(),
        }
    }
}

impl<'a> RowSource for DistinctSource<'a> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        while let Some(row) = self.upstream.next_row()? {
            let key = row
                .iter()
                .map(|(_, val)| GroupValueKey::from_value(val))
                .collect();
            if self.seen.insert(key) {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }
}

/// Lazy-buffered aggregation source. Aggregation must observe every
/// input row before it can emit the first group, so this source drains
/// upstream on first pull, builds grouped rows, then yields them one
/// at a time to downstream consumers.
pub struct HashAggregationSource<'a, S: GraphStorage> {
    state: HashAggregationState<'a, S>,
}

enum HashAggregationState<'a, S: GraphStorage> {
    Pending {
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        group_by: &'a [ResolvedProjection],
        aggregates: &'a [ResolvedProjection],
    },
    Yielding(std::vec::IntoIter<Row>),
}

impl<'a, S: GraphStorage> HashAggregationSource<'a, S> {
    fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        group_by: &'a [ResolvedProjection],
        aggregates: &'a [ResolvedProjection],
    ) -> Self {
        Self {
            state: HashAggregationState::Pending {
                upstream,
                ctx,
                group_by,
                aggregates,
            },
        }
    }

    fn materialize(
        upstream: &mut Box<dyn RowSource + 'a>,
        ctx: &StreamCtx<'a, S>,
        group_by: &[ResolvedProjection],
        aggregates: &[ResolvedProjection],
    ) -> ExecResult<Vec<Row>> {
        let input_rows = drain(upstream.as_mut())?;
        let eval_ctx = ctx.eval_ctx();
        let mut groups: BTreeMap<Vec<GroupValueKey>, Vec<Row>> = BTreeMap::new();

        if group_by.is_empty() {
            groups.insert(Vec::new(), input_rows);
        } else {
            for row in input_rows {
                let key = group_by
                    .iter()
                    .map(|proj| GroupValueKey::from_value(&eval_expr(&proj.expr, &row, &eval_ctx)))
                    .collect::<Vec<_>>();
                groups.entry(key).or_default().push(row);
            }
        }

        let mut out = Vec::new();
        for rows in groups.into_values() {
            let mut result = Row::new();
            if let Some(first) = rows.first() {
                for proj in group_by {
                    let value = hydrate_value(eval_expr(&proj.expr, first, &eval_ctx), ctx.storage);
                    result.insert_named(proj.output, proj.name.clone(), value);
                }
            }
            for proj in aggregates {
                let value = compute_aggregate_expr(&proj.expr, &rows, &eval_ctx);
                result.insert_named(proj.output, proj.name.clone(), value);
            }
            out.push(result);
        }

        Ok(out)
    }
}

impl<'a, S: GraphStorage> RowSource for HashAggregationSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            match &mut self.state {
                HashAggregationState::Pending {
                    upstream,
                    ctx,
                    group_by,
                    aggregates,
                } => {
                    let rows = Self::materialize(upstream, ctx, group_by, aggregates)?;
                    self.state = HashAggregationState::Yielding(rows.into_iter());
                }
                HashAggregationState::Yielding(it) => return Ok(it.next()),
            }
        }
    }
}

/// Streaming outer OPTIONAL MATCH source. The optional inner plan is
/// independent of each incoming row in the current physical plan, so
/// it is materialized once, then matched against each outer row as
/// the outer cursor advances.
pub struct OptionalMatchSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    plan: &'a PhysicalPlan,
    inner: PhysicalNodeId,
    new_vars: &'a [VarId],
    inner_rows: Option<Vec<Row>>,
    cur_input: Option<Row>,
    cur_inner_idx: usize,
    cur_matched: bool,
}

impl<'a, S: GraphStorage> OptionalMatchSource<'a, S> {
    fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        plan: &'a PhysicalPlan,
        inner: PhysicalNodeId,
        new_vars: &'a [VarId],
    ) -> Self {
        Self {
            upstream,
            ctx,
            plan,
            inner,
            new_vars,
            inner_rows: None,
            cur_input: None,
            cur_inner_idx: 0,
            cur_matched: false,
        }
    }

    fn ensure_inner_rows(&mut self) -> ExecResult<()> {
        if self.inner_rows.is_none() {
            let mut inner = build_streaming(
                self.plan,
                self.inner,
                self.ctx.storage,
                self.ctx.params.clone(),
            )?;
            self.inner_rows = Some(drain(inner.as_mut())?);
        }
        Ok(())
    }
}

impl<'a, S: GraphStorage> RowSource for OptionalMatchSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        self.ensure_inner_rows()?;
        loop {
            if self.cur_input.is_none() {
                match self.upstream.next_row()? {
                    Some(input_row) => {
                        self.cur_input = Some(input_row);
                        self.cur_inner_idx = 0;
                        self.cur_matched = false;
                    }
                    None => return Ok(None),
                }
            }

            let inner_rows = self
                .inner_rows
                .as_ref()
                .expect("ensure_inner_rows initializes inner_rows");
            let input_row = self
                .cur_input
                .as_ref()
                .expect("cur_input is initialized above");

            while self.cur_inner_idx < inner_rows.len() {
                let inner_row = &inner_rows[self.cur_inner_idx];
                self.cur_inner_idx += 1;

                let compatible = input_row
                    .iter()
                    .all(|(var, val)| match inner_row.get(*var) {
                        Some(inner_val) => inner_val == val,
                        None => true,
                    });
                if !compatible {
                    continue;
                }

                let mut merged = input_row.clone();
                for (var, name, val) in inner_row.iter_named() {
                    if !merged.contains_key(*var) {
                        merged.insert_named(*var, name.into_owned(), val.clone());
                    }
                }
                self.cur_matched = true;
                return Ok(Some(merged));
            }

            let mut input_row = self
                .cur_input
                .take()
                .expect("cur_input is initialized while finishing optional row");
            if !self.cur_matched {
                for &var_id in self.new_vars {
                    if !input_row.contains_key(var_id) {
                        input_row.insert(var_id, LoraValue::Null);
                    }
                }
                return Ok(Some(input_row));
            }
        }
    }
}

/// Path-building source. Ordinary path construction is one-in/one-out.
/// Shortest-path filtering still has to compare the complete path set,
/// so that mode drains internally before yielding.
pub struct PathBuildSource<'a, S: GraphStorage> {
    state: PathBuildState<'a, S>,
}

enum PathBuildState<'a, S: GraphStorage> {
    Streaming {
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        output: VarId,
        node_vars: &'a [VarId],
        rel_vars: &'a [VarId],
    },
    PendingShortest {
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        output: VarId,
        node_vars: &'a [VarId],
        rel_vars: &'a [VarId],
        all: bool,
    },
    Yielding(std::vec::IntoIter<Row>),
}

impl<'a, S: GraphStorage> PathBuildSource<'a, S> {
    fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        output: VarId,
        node_vars: &'a [VarId],
        rel_vars: &'a [VarId],
        shortest_path_all: Option<bool>,
    ) -> Self {
        let state = match shortest_path_all {
            Some(all) => PathBuildState::PendingShortest {
                upstream,
                ctx,
                output,
                node_vars,
                rel_vars,
                all,
            },
            None => PathBuildState::Streaming {
                upstream,
                ctx,
                output,
                node_vars,
                rel_vars,
            },
        };
        Self { state }
    }

    fn attach_path(
        mut row: Row,
        ctx: &StreamCtx<'a, S>,
        output: VarId,
        node_vars: &[VarId],
        rel_vars: &[VarId],
    ) -> Row {
        let path = build_path_value(&row, node_vars, rel_vars, ctx.storage);
        row.insert(output, path);
        row
    }

    fn shortest_path_rows(
        upstream: &mut Box<dyn RowSource + 'a>,
        ctx: &StreamCtx<'a, S>,
        output: VarId,
        node_vars: &[VarId],
        rel_vars: &[VarId],
        all: bool,
    ) -> ExecResult<Vec<Row>> {
        let mut best_len: Option<usize> = None;
        let mut best_rows = Vec::new();

        while let Some(row) = upstream.next_row()? {
            let row = Self::attach_path(row, ctx, output, node_vars, rel_vars);
            let path_len = match row.get(output) {
                Some(LoraValue::Path(path)) => path.rels.len(),
                _ => usize::MAX,
            };

            match best_len {
                None => {
                    best_len = Some(path_len);
                    best_rows.push(row);
                }
                Some(current) if path_len < current => {
                    best_len = Some(path_len);
                    best_rows.clear();
                    best_rows.push(row);
                }
                Some(current) if path_len == current && all => best_rows.push(row),
                _ => {}
            }
        }

        Ok(best_rows)
    }
}

impl<'a, S: GraphStorage> RowSource for PathBuildSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            match &mut self.state {
                PathBuildState::Streaming {
                    upstream,
                    ctx,
                    output,
                    node_vars,
                    rel_vars,
                } => {
                    return Ok(upstream
                        .next_row()?
                        .map(|row| Self::attach_path(row, ctx, *output, node_vars, rel_vars)));
                }
                PathBuildState::PendingShortest {
                    upstream,
                    ctx,
                    output,
                    node_vars,
                    rel_vars,
                    all,
                } => {
                    let rows = Self::shortest_path_rows(
                        upstream, ctx, *output, node_vars, rel_vars, *all,
                    )?;
                    self.state = PathBuildState::Yielding(rows.into_iter());
                }
                PathBuildState::Yielding(it) => return Ok(it.next()),
            }
        }
    }
}

/// Streaming UNION source. Pulls each branch in sequence. `UNION ALL`
/// passes rows through directly; plain `UNION` keeps a seen-key set and
/// yields the first row for each unique named column/value key.
///
/// Replaces the buffered fallback that previously sat in
/// `PullExecutor::open_compiled` for any UNION-bearing plan. The
/// consumer side is now streaming, so a write op on top of a UNION read
/// can stream its writes as the union yields.
pub struct UnionSource<'a> {
    branches: Vec<Box<dyn RowSource + 'a>>,
    branch_idx: usize,
    needs_dedup: bool,
    seen: BTreeSet<Vec<(String, GroupValueKey)>>,
}

impl<'a> UnionSource<'a> {
    fn new(branches: Vec<Box<dyn RowSource + 'a>>, needs_dedup: bool) -> Self {
        Self {
            branches,
            branch_idx: 0,
            needs_dedup,
            seen: BTreeSet::new(),
        }
    }
}

impl<'a> RowSource for UnionSource<'a> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        while self.branch_idx < self.branches.len() {
            match self.branches[self.branch_idx].next_row()? {
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
                    self.branch_idx += 1;
                }
            }
        }
        Ok(None)
    }
}

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
pub(crate) fn compiled_to_streaming<'a, S: GraphStorage + 'a>(
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
pub(crate) fn is_streaming_op(op: &PhysicalOp) -> bool {
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
        let _ = take_eval_error();
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

/// Mutable UNION cursor. `UNION ALL` streams one branch at a time
/// against the same staged graph. Plain `UNION` streams branch-by-branch
/// while retaining only a seen-key set for deduplication.
pub struct MutableUnionSource<'a, S: GraphStorageMut + GraphStorage + 'a> {
    storage_ptr: *mut S,
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
            storage_ptr: storage as *mut S,
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
        let storage = unsafe { &mut *self.storage_ptr };
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
/// The cursor owns a raw alias `*mut S` of the original `&'a mut S`.
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
    /// as `&S` (via `&*ptr`) by `upstream` and as `&mut S` (via
    /// `&mut *ptr`) inside this cursor's `next_row`.
    storage_ptr: *mut S,
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
                return Err(crate::errors::ExecutorError::RuntimeError(format!(
                    "StreamingWriteCursor::open called with non-write node {write_op_node:?}"
                )));
            }
        };
        let storage_ptr: *mut S = storage as *mut S;

        // SAFETY: see struct-level comment.
        let storage_ref: &'a S = unsafe { &*storage_ptr };
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
        let storage_mut: &mut S = unsafe { &mut *self.storage_ptr };
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

#[cfg(test)]
mod tests {
    use super::*;
    use lora_analyzer::symbols::VarId;
    use lora_ast::Span;
    use lora_compiler::physical::{ArgumentExec, ExpandExec, NodeByLabelScanExec, PhysicalPlan};
    use lora_store::{GraphStorageMut, InMemoryGraph};

    #[test]
    fn variable_length_expand_has_streaming_source() {
        let mut graph = InMemoryGraph::new();
        let a = graph.create_node(vec!["N".into()], BTreeMap::new());
        let b = graph.create_node(vec!["N".into()], BTreeMap::new());
        let c = graph.create_node(vec!["N".into()], BTreeMap::new());
        graph
            .create_relationship(a.id, b.id, "R", BTreeMap::new())
            .unwrap();
        graph
            .create_relationship(b.id, c.id, "R", BTreeMap::new())
            .unwrap();

        let src = VarId(0);
        let rel = VarId(1);
        let dst = VarId(2);
        let plan = PhysicalPlan {
            root: 2,
            nodes: vec![
                PhysicalOp::Argument(ArgumentExec),
                PhysicalOp::NodeByLabelScan(NodeByLabelScanExec {
                    input: Some(0),
                    var: src,
                    labels: vec![vec!["N".into()]],
                }),
                PhysicalOp::Expand(ExpandExec {
                    input: 1,
                    src,
                    rel: Some(rel),
                    dst,
                    types: vec!["R".into()],
                    direction: Direction::Right,
                    rel_properties: None,
                    range: Some(RangeLiteral {
                        start: Some(1),
                        end: Some(2),
                        span: Span::default(),
                    }),
                }),
            ],
        };

        assert!(subtree_is_fully_streaming(&plan, plan.root));

        let mut source =
            build_streaming(&plan, plan.root, &graph, Arc::new(BTreeMap::new())).unwrap();
        let rows = drain(source.as_mut()).unwrap();
        let mut rel_lengths = rows
            .iter()
            .map(|row| match row.get(rel).unwrap() {
                LoraValue::List(rels) => rels.len(),
                other => panic!("expected relationship list, got {other:?}"),
            })
            .collect::<Vec<_>>();
        rel_lengths.sort_unstable();

        assert_eq!(rows.len(), 3);
        assert_eq!(rel_lengths, vec![1, 1, 2]);
    }
}
