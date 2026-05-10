//! Node-scan operator sources.
//!
//! - [`NodeScanSource`] — every node in the graph.
//! - [`NodeByLabelScanSource`] — nodes matching a label group filter.
//! - [`NodeByPropertyScanSource`] — nodes matching an indexed
//!   `(label, property = value)` filter.

use lora_analyzer::symbols::VarId;
use lora_analyzer::ResolvedExpr;
use lora_compiler::physical::{
    NodeByPointScanExec, NodeByPropertyRangeScanExec, NodeByTextScanExec, RelByPointScanExec,
    RelByPropertyRangeScanExec, RelByTextScanExec,
};
use lora_store::{GraphStorage, NodeId};

use crate::errors::ExecResult;
use crate::eval::eval_expr;
use crate::executor::{
    bound_node_id_for_expand, indexed_node_property_candidates, label_group_candidates_prefiltered,
    node_by_point_scan_rows, node_by_property_range_scan_rows, node_by_text_scan_rows,
    node_matches_label_groups, node_matches_property_filter, rel_by_point_scan_rows,
    rel_by_property_range_scan_rows, rel_by_text_scan_rows, scan_node_ids_for_label_groups,
};
use crate::value::{LoraValue, Row};

use super::{RowSource, StreamCtx};

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
    pub(super) fn new(upstream: Box<dyn RowSource + 'a>, storage: &'a S, var: VarId) -> Self {
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
            if let Some(id) = bound_node_id_for_expand(row_ref, self.var)? {
                if self.cur_emitted {
                    self.cur_row = None;
                    continue;
                }
                self.cur_emitted = true;
                if self.storage.has_node(id) {
                    let row = self.cur_row.take().unwrap();
                    self.cur_emitted = false;
                    return Ok(Some(row));
                }
                self.cur_row = None;
                continue;
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
    candidates_prefiltered: bool,
    cur_row: Option<Row>,
    cur_ids: Vec<NodeId>,
    cur_idx: usize,
    cur_emitted: bool,
}

impl<'a, S: GraphStorage> NodeByLabelScanSource<'a, S> {
    pub(super) fn new(
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
            candidates_prefiltered: label_group_candidates_prefiltered(labels),
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

            if let Some(id) = bound_node_id_for_expand(row_ref, self.var)? {
                if self.cur_emitted {
                    self.cur_row = None;
                    continue;
                }
                self.cur_emitted = true;
                let labels_ok = self
                    .storage
                    .with_node(id, |n| node_matches_label_groups(&n.labels, self.labels))
                    .unwrap_or(false);
                if labels_ok {
                    let row = self.cur_row.take().unwrap();
                    self.cur_emitted = false;
                    return Ok(Some(row));
                }
                self.cur_row = None;
                continue;
            }

            if self.cur_idx == 0 && self.cur_ids.is_empty() {
                self.cur_ids = scan_node_ids_for_label_groups(self.storage, self.labels);
            }

            while self.cur_idx < self.cur_ids.len() {
                let id = self.cur_ids[self.cur_idx];
                self.cur_idx += 1;
                if !self.candidates_prefiltered {
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
    pub(super) fn new(
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

    #[inline]
    fn clear_current(&mut self) {
        self.cur_row = None;
        self.cur_expected = None;
        self.cur_ids.clear();
        self.cur_idx = 0;
        self.cur_emitted = false;
        self.cur_prefiltered = false;
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

            if let Some(id) = bound_node_id_for_expand(row_ref, self.var)? {
                if self.cur_emitted {
                    self.clear_current();
                    continue;
                }
                self.cur_emitted = true;
                if node_matches_property_filter(
                    self.ctx.storage,
                    id,
                    self.labels,
                    self.key,
                    expected,
                ) {
                    let row = self.cur_row.take().unwrap();
                    self.clear_current();
                    return Ok(Some(row));
                }
                self.clear_current();
                continue;
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

            self.clear_current();
        }
    }
}

/// Streams the newer catalog-backed index scans one upstream row at a
/// time. Each refill delegates to the buffered helper for a single row,
/// so predicate semantics stay centralized while callers above this
/// source still get pull-shaped backpressure between upstream rows.
pub struct BufferedIndexScanSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    op: BufferedIndexScanOp<'a>,
    pending: std::vec::IntoIter<Row>,
}

enum BufferedIndexScanOp<'a> {
    NodeRange(&'a NodeByPropertyRangeScanExec),
    NodeText(&'a NodeByTextScanExec),
    NodePoint(&'a NodeByPointScanExec),
    RelRange(&'a RelByPropertyRangeScanExec),
    RelText(&'a RelByTextScanExec),
    RelPoint(&'a RelByPointScanExec),
}

impl<'a, S: GraphStorage> BufferedIndexScanSource<'a, S> {
    pub(super) fn node_range(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        op: &'a NodeByPropertyRangeScanExec,
    ) -> Self {
        Self::new(upstream, ctx, BufferedIndexScanOp::NodeRange(op))
    }

    pub(super) fn node_text(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        op: &'a NodeByTextScanExec,
    ) -> Self {
        Self::new(upstream, ctx, BufferedIndexScanOp::NodeText(op))
    }

    pub(super) fn node_point(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        op: &'a NodeByPointScanExec,
    ) -> Self {
        Self::new(upstream, ctx, BufferedIndexScanOp::NodePoint(op))
    }

    pub(super) fn rel_range(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        op: &'a RelByPropertyRangeScanExec,
    ) -> Self {
        Self::new(upstream, ctx, BufferedIndexScanOp::RelRange(op))
    }

    pub(super) fn rel_text(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        op: &'a RelByTextScanExec,
    ) -> Self {
        Self::new(upstream, ctx, BufferedIndexScanOp::RelText(op))
    }

    pub(super) fn rel_point(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        op: &'a RelByPointScanExec,
    ) -> Self {
        Self::new(upstream, ctx, BufferedIndexScanOp::RelPoint(op))
    }

    fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        op: BufferedIndexScanOp<'a>,
    ) -> Self {
        Self {
            upstream,
            ctx,
            op,
            pending: Vec::new().into_iter(),
        }
    }

    fn refill(&mut self, row: Row) -> ExecResult<()> {
        let rows = match self.op {
            BufferedIndexScanOp::NodeRange(op) => node_by_property_range_scan_rows(
                self.ctx.storage,
                &self.ctx.params,
                vec![row],
                op,
                None,
            )?,
            BufferedIndexScanOp::NodeText(op) => {
                node_by_text_scan_rows(self.ctx.storage, &self.ctx.params, vec![row], op, None)?
            }
            BufferedIndexScanOp::NodePoint(op) => {
                node_by_point_scan_rows(self.ctx.storage, &self.ctx.params, vec![row], op, None)?
            }
            BufferedIndexScanOp::RelRange(op) => rel_by_property_range_scan_rows(
                self.ctx.storage,
                &self.ctx.params,
                vec![row],
                op,
                None,
            )?,
            BufferedIndexScanOp::RelText(op) => {
                rel_by_text_scan_rows(self.ctx.storage, &self.ctx.params, vec![row], op, None)?
            }
            BufferedIndexScanOp::RelPoint(op) => {
                rel_by_point_scan_rows(self.ctx.storage, &self.ctx.params, vec![row], op, None)?
            }
        };
        self.pending = rows.into_iter();
        Ok(())
    }
}

impl<'a, S: GraphStorage> RowSource for BufferedIndexScanSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if let Some(row) = self.pending.next() {
                return Ok(Some(row));
            }

            let Some(row) = self.upstream.next_row()? else {
                return Ok(None);
            };
            self.refill(row)?;
        }
    }
}
