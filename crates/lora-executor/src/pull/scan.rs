//! Node-scan operator sources.
//!
//! - [`NodeScanSource`] — every node in the graph.
//! - [`NodeByLabelScanSource`] — nodes matching a label group filter.
//! - [`BufferedIndexScanSource`] — id seeks and the catalog-backed
//!   index scans, one upstream row at a time.
//! - [`NodeByPropertyScanSource`] — nodes matching an indexed
//!   `(label, property = value)` filter.

use lora_analyzer::symbols::VarId;
use lora_analyzer::ResolvedExpr;
use lora_compiler::physical::{
    NodeByIdSeekExec, NodeByPointScanExec, NodeByPropertyRangeScanExec, NodeByTextScanExec,
    RelByIdSeekExec, RelByPointScanExec, RelByPropertyRangeScanExec, RelByTextScanExec,
};
use lora_store::{GraphStorage, NodeId};

use crate::errors::{ExecResult, ExecutorError};
use crate::eval::eval_expr;
use crate::executor::{
    bound_node_id_for_expand, label_group_candidates_prefiltered, node_by_id_seek_rows,
    node_by_point_scan_rows, node_by_property_range_scan_rows, node_by_text_scan_rows,
    node_matches_label_groups, property_scan_candidates, property_scan_matches,
    rel_by_id_seek_rows, rel_by_point_scan_rows, rel_by_property_range_scan_rows,
    rel_by_text_scan_rows, scan_node_ids_for_label_groups, NodePropertyCandidates,
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
    /// The node ids of the current input row.
    ids: ScanIds,
    /// Fetch ids a page at a time (see [`StableStoreScope`]); otherwise
    /// all of them before the first row.
    paged: bool,
    /// Already emitted the current row when `var` was already bound.
    cur_emitted: bool,
}

/// Node ids fetched per page of a streaming scan: a scan holds at most
/// this many ids at a time instead of the whole label.
const SCAN_PAGE: usize = 1024;
/// The first page of a scan. Pages double from here to [`SCAN_PAGE`], so a
/// `LIMIT 10` does not fetch a thousand ids.
const FIRST_SCAN_PAGE: usize = 32;

/// The node ids one input row scans: fetched from the store a page at a
/// time, pages doubling from [`FIRST_SCAN_PAGE`] to [`SCAN_PAGE`], or all
/// at once.
struct ScanIds {
    /// The current page.
    page: Vec<NodeId>,
    /// Position into `page`.
    idx: usize,
    /// Where the store's id scan resumes for the next page.
    cursor: u64,
    /// Ids to fetch for the next page.
    page_size: usize,
    /// No page is left to fetch; the current one may still hold ids.
    done: bool,
}

impl ScanIds {
    fn new() -> Self {
        Self {
            page: Vec::new(),
            idx: 0,
            cursor: 0,
            page_size: FIRST_SCAN_PAGE,
            done: false,
        }
    }

    /// Start over, for a new input row.
    fn reset(&mut self) {
        self.page.clear();
        self.idx = 0;
        self.cursor = 0;
        self.page_size = FIRST_SCAN_PAGE;
        self.done = false;
    }

    /// The next id of the current page.
    #[inline]
    fn next(&mut self) -> Option<NodeId> {
        let id = *self.page.get(self.idx)?;
        self.idx += 1;
        Some(id)
    }

    /// Replace the page with the next ids carrying `label`, or of all
    /// nodes.
    fn fetch_page<S: GraphStorage>(&mut self, storage: &S, label: Option<&str>) {
        self.page.clear();
        self.idx = 0;
        self.done = !storage.scan_node_ids(label, &mut self.cursor, self.page_size, &mut self.page);
        self.page_size = (self.page_size * 2).min(SCAN_PAGE);
    }

    /// Replace the page with all of `ids`, the last page.
    fn fetch_all(&mut self, ids: Vec<NodeId>) {
        self.page = ids;
        self.idx = 0;
        self.done = true;
    }
}

thread_local! {
    /// Set while a read-only pipeline is being built: nothing writes to
    /// the store between its pulls.
    static STORE_IS_STABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks the pipelines built on this thread, while it is alive, as
/// reading a store that does not change under them, so their scans may
/// fetch ids a page at a time.
///
/// A source reads the mark when it is built. One built later, while rows
/// are being pulled (the inner side of an `OPTIONAL MATCH`, a subquery
/// body), is outside the scope and copies its ids like a write's scan
/// does: slower to its first row, never wrong.
///
/// Without it a scan copies its ids before yielding the first row. A
/// streaming write needs that: it applies each row's write between pulls,
/// and `MATCH (n:L) CREATE (:L)` over a paged scan would keep finding
/// the nodes it just created.
pub(crate) struct StableStoreScope {
    previous: bool,
}

impl StableStoreScope {
    pub(crate) fn enter() -> Self {
        Self {
            previous: STORE_IS_STABLE.with(|flag| flag.replace(true)),
        }
    }
}

impl Drop for StableStoreScope {
    fn drop(&mut self) {
        STORE_IS_STABLE.with(|flag| flag.set(self.previous));
    }
}

fn store_is_stable() -> bool {
    STORE_IS_STABLE.with(|flag| flag.get())
}

impl<'a, S: GraphStorage> NodeScanSource<'a, S> {
    pub(super) fn new(upstream: Box<dyn RowSource + 'a>, storage: &'a S, var: VarId) -> Self {
        Self {
            upstream,
            storage,
            var,
            cur_row: None,
            ids: ScanIds::new(),
            paged: store_is_stable(),
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
                        self.ids.reset();
                        self.cur_emitted = false;
                    }
                    None => return Ok(None),
                }
            }

            let Some(row_ref) = self.cur_row.as_ref() else {
                self.ids.reset();
                self.cur_emitted = false;
                continue;
            };

            // Already-bound case: emit the input row once iff the
            // bound node still exists; otherwise drop it.
            if let Some(id) = bound_node_id_for_expand(row_ref, self.var)? {
                if self.cur_emitted {
                    self.cur_row = None;
                    continue;
                }
                self.cur_emitted = true;
                if self.storage.has_node(id) {
                    let Some(row) = self.cur_row.take() else {
                        self.cur_emitted = false;
                        continue;
                    };
                    self.cur_emitted = false;
                    return Ok(Some(row));
                }
                self.cur_row = None;
                continue;
            }

            // Unbound case: yield one row per node id, fetching the ids
            // a page at a time.
            if let Some(id) = self.ids.next() {
                let mut new_row = row_ref.clone();
                new_row.insert(self.var, LoraValue::Node(id));
                return Ok(Some(new_row));
            }
            if self.ids.done {
                self.cur_row = None;
            } else if self.paged {
                self.ids.fetch_page(self.storage, None);
            } else {
                self.ids.fetch_all(self.storage.all_node_ids());
            }
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
    /// The one label to scan when the filter is a single label and the
    /// store is stable (see [`StableStoreScope`]): its ids are then
    /// streamed a page at a time. Other scans copy their candidates
    /// before the first row.
    single_label: Option<&'a str>,
    cur_row: Option<Row>,
    /// Candidate ids of the current input row.
    ids: ScanIds,
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
            single_label: match labels {
                [group] if store_is_stable() => match group.as_slice() {
                    [label] => Some(label.as_str()),
                    _ => None,
                },
                _ => None,
            },
            cur_row: None,
            ids: ScanIds::new(),
            cur_emitted: false,
        }
    }

    /// Fetch the next candidates for the current input row.
    fn fill_candidates(&mut self) {
        match self.single_label {
            Some(label) => self.ids.fetch_page(self.storage, Some(label)),
            None => self
                .ids
                .fetch_all(scan_node_ids_for_label_groups(self.storage, self.labels)),
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
                        self.ids.reset();
                        self.cur_emitted = false;
                    }
                    None => return Ok(None),
                }
            }

            let Some(row_ref) = self.cur_row.as_ref() else {
                self.ids.reset();
                self.cur_emitted = false;
                continue;
            };

            if let Some(id) = bound_node_id_for_expand(row_ref, self.var)? {
                if self.cur_emitted {
                    self.cur_row = None;
                    continue;
                }
                self.cur_emitted = true;
                let labels_ok = self
                    .storage
                    .with_node(id, |n| node_matches_label_groups(n.labels(), self.labels))
                    .unwrap_or(false);
                if labels_ok {
                    let Some(row) = self.cur_row.take() else {
                        self.cur_emitted = false;
                        continue;
                    };
                    self.cur_emitted = false;
                    return Ok(Some(row));
                }
                self.cur_row = None;
                continue;
            }

            while let Some(id) = self.ids.next() {
                if !self.candidates_prefiltered {
                    let labels_ok = self
                        .storage
                        .with_node(id, |n| node_matches_label_groups(n.labels(), self.labels))
                        .unwrap_or(false);
                    if !labels_ok {
                        continue;
                    }
                }
                let mut new_row = row_ref.clone();
                new_row.insert(self.var, LoraValue::Node(id));
                return Ok(Some(new_row));
            }

            if self.ids.done {
                self.cur_row = None;
            } else {
                self.fill_candidates();
            }
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
    in_list: bool,
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
        in_list: bool,
    ) -> Self {
        Self {
            upstream,
            ctx,
            var,
            labels,
            key,
            value,
            in_list,
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
                        let candidates = if bound_node_id_for_expand(&row, self.var)?.is_some() {
                            NodePropertyCandidates {
                                ids: Vec::new(),
                                prefiltered: true,
                            }
                        } else {
                            property_scan_candidates(
                                self.ctx.storage,
                                self.labels,
                                self.key,
                                &expected,
                                self.in_list,
                            )
                        };
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

            let (Some(row_ref), Some(expected)) =
                (self.cur_row.as_ref(), self.cur_expected.as_ref())
            else {
                self.clear_current();
                continue;
            };

            if let Some(id) = bound_node_id_for_expand(row_ref, self.var)? {
                if self.cur_emitted {
                    self.clear_current();
                    continue;
                }
                self.cur_emitted = true;
                if property_scan_matches(
                    self.ctx.storage,
                    id,
                    self.labels,
                    self.key,
                    expected,
                    self.in_list,
                ) {
                    let Some(row) = self.cur_row.take() else {
                        self.clear_current();
                        continue;
                    };
                    self.clear_current();
                    return Ok(Some(row));
                }
                self.clear_current();
                continue;
            }

            while self.cur_idx < self.cur_ids.len() {
                let Some(&id) = self.cur_ids.get(self.cur_idx) else {
                    return Err(ExecutorError::RuntimeError(
                        "property scan cursor index moved past candidate ids".into(),
                    ));
                };
                self.cur_idx += 1;
                if !self.cur_prefiltered
                    && !property_scan_matches(
                        self.ctx.storage,
                        id,
                        self.labels,
                        self.key,
                        expected,
                        self.in_list,
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
    NodeId(&'a NodeByIdSeekExec),
    RelId(&'a RelByIdSeekExec),
    NodeRange(&'a NodeByPropertyRangeScanExec),
    NodeText(&'a NodeByTextScanExec),
    NodePoint(&'a NodeByPointScanExec),
    RelRange(&'a RelByPropertyRangeScanExec),
    RelText(&'a RelByTextScanExec),
    RelPoint(&'a RelByPointScanExec),
}

impl<'a, S: GraphStorage> BufferedIndexScanSource<'a, S> {
    pub(super) fn node_id(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        op: &'a NodeByIdSeekExec,
    ) -> Self {
        Self::new(upstream, ctx, BufferedIndexScanOp::NodeId(op))
    }

    pub(super) fn rel_id(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        op: &'a RelByIdSeekExec,
    ) -> Self {
        Self::new(upstream, ctx, BufferedIndexScanOp::RelId(op))
    }

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
            BufferedIndexScanOp::NodeId(op) => {
                node_by_id_seek_rows(self.ctx.storage, &self.ctx.params, vec![row], op, None)?
            }
            BufferedIndexScanOp::RelId(op) => {
                rel_by_id_seek_rows(self.ctx.storage, &self.ctx.params, vec![row], op, None)?
            }
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

/// Ordered range scan (`op.order` set): yields nodes in `ORDER BY op.key`
/// order, pulling ids from the index lazily so a LIMIT above stops it
/// early. See [`crate::executor::OrderedRangeCursor`].
pub struct OrderedRangeScanSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    op: &'a NodeByPropertyRangeScanExec,
    /// The input row, the temporals of another kind still to emit (see
    /// `other_kind_node_ids`), and the cursor over the range.
    current: Option<(
        Row,
        std::vec::IntoIter<lora_store::NodeId>,
        crate::executor::OrderedRangeCursor,
    )>,
}

impl<'a, S: GraphStorage> OrderedRangeScanSource<'a, S> {
    pub(super) fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        op: &'a NodeByPropertyRangeScanExec,
    ) -> Self {
        Self {
            upstream,
            ctx,
            op,
            current: None,
        }
    }
}

impl<'a, S: GraphStorage> RowSource for OrderedRangeScanSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if let Some((row, others, cursor)) = self.current.as_mut() {
                // Temporals of another kind come first, for the Filter above
                // to judge before any LIMIT can stop the scan.
                if let Some(id) = others.next().or_else(|| cursor.next_id(self.ctx.storage)) {
                    let mut out = row.clone();
                    out.insert(self.op.var, LoraValue::Node(id));
                    return Ok(Some(out));
                }
                self.current = None;
            }
            let Some(row) = self.upstream.next_row()? else {
                return Ok(None);
            };
            let range = crate::executor::RangeValues::eval(
                (&self.op.lo, self.op.lo_inclusive),
                (&self.op.hi, self.op.hi_inclusive),
                &self.op.extra_bounds,
                &row,
                &self.ctx.eval_ctx(),
            );
            let others = crate::executor::other_kind_node_ids(
                self.ctx.storage,
                self.op,
                [range.lo.as_ref(), range.hi.as_ref()],
                &mut crate::executor::OtherKindScan::default(),
            );
            let cursor = crate::executor::OrderedRangeCursor::new(self.ctx.storage, self.op, range);
            self.current = Some((row, others.into_iter(), cursor));
        }
    }
}
