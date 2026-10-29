//! Order-preserving operator sources.
//!
//! - [`SortSource`] — drains upstream on first pull, sorts the buffer
//!   in place, then yields rows lazily.
//! - [`PartialSortSource`] — a sort whose input already arrives in order
//!   of its first keys: sorts one run of equal first keys at a time.
//! - [`LimitSource`] — `SKIP` / `LIMIT`. Fully streaming.

use std::cmp::Ordering;

use lora_store::GraphStorage;

use crate::errors::ExecResult;
use crate::eval::eval_expr;
use crate::executor::{compare_sort_key, sort_checked, SortBuffer};
use crate::value::{LoraValue, Row};

use super::{RowSource, StreamCtx};

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
    pub(super) fn new(
        upstream: Box<dyn RowSource + 'a>,
        skip: usize,
        limit: Option<usize>,
    ) -> Self {
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
/// Memory is O(N) in the number of input rows, or O(k) under a
/// `LIMIT` bound (`top_k`), which keeps only the best k rows. The win
/// is that everything *above* a `SortSource` (typically a write op
/// like CREATE / SET) streams: the auto-commit pipeline
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
        top_k: Option<usize>,
    },
    Yielding(std::vec::IntoIter<Row>),
}

impl<'a, S: GraphStorage> SortSource<'a, S> {
    pub(super) fn new_with_top_k(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        items: &'a [lora_analyzer::ResolvedSortItem],
        top_k: Option<usize>,
    ) -> Self {
        Self {
            state: SortState::Pending {
                upstream,
                ctx,
                items,
                top_k,
            },
        }
    }

    /// Drain upstream into a vector and sort it by the plan's
    /// sort items. Called from `next_row` on the first invocation.
    fn materialize(
        upstream: &mut Box<dyn RowSource + 'a>,
        ctx: &StreamCtx<'a, S>,
        items: &[lora_analyzer::ResolvedSortItem],
        top_k: Option<usize>,
    ) -> ExecResult<Vec<Row>> {
        // Rows stream into the buffer: with a bound only the best `top_k`
        // are ever held.
        let eval_ctx = ctx.eval_ctx();
        let mut buffer = SortBuffer::new(items, top_k);
        while let Some(row) = upstream.next_row()? {
            // Checked per row: reading the next one upstream may clear
            // an error this row's key raised.
            sort_checked(|| buffer.push(row, &eval_ctx))?;
        }
        Ok(buffer.finish())
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
                    top_k,
                } => {
                    let rows = Self::materialize(upstream, ctx, items, *top_k)?;
                    self.state = SortState::Yielding(rows.into_iter());
                    // fall through to the Yielding match on the next iteration.
                }
                SortState::Yielding(it) => return Ok(it.next()),
            }
        }
    }
}

/// Sort whose input arrives ordered by its first `presorted` keys (an
/// ordered index walk under it): rows are read one run of equal first
/// keys at a time, and each run is sorted by all the keys (so by the
/// remaining ones) before it is yielded. Under a bound (`ORDER BY ...
/// LIMIT k`) it stops reading once `k` rows are out or queued from
/// complete runs: about `k` rows plus the last run and one row of the
/// next, instead of the whole input.
///
/// The order is the full stable sort's on that input: runs come in
/// input order, which is the first keys' order, and within a run the
/// buffer orders by every key, ties by arrival.
pub struct PartialSortSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    items: &'a [lora_analyzer::ResolvedSortItem],
    presorted: usize,
    /// Rows still to yield, or `None` for no bound.
    remaining: Option<usize>,
    /// The sorted run being yielded.
    ready: std::vec::IntoIter<Row>,
    /// The first row of the next run, with its first keys.
    lookahead: Option<(Vec<LoraValue>, Row)>,
    exhausted: bool,
}

impl<'a, S: GraphStorage> PartialSortSource<'a, S> {
    pub(super) fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        items: &'a [lora_analyzer::ResolvedSortItem],
        presorted: usize,
        bound: Option<usize>,
    ) -> Self {
        Self {
            upstream,
            ctx,
            items,
            presorted: presorted.min(items.len()),
            remaining: bound,
            ready: Vec::new().into_iter(),
            lookahead: None,
            exhausted: false,
        }
    }

    fn lead_keys(&self, row: &Row) -> ExecResult<Vec<LoraValue>> {
        let eval_ctx = self.ctx.eval_ctx();
        sort_checked(|| {
            self.items[..self.presorted]
                .iter()
                .map(|item| eval_expr(&item.expr, row, &eval_ctx))
                .collect()
        })
    }

    fn same_run(&self, a: &[LoraValue], b: &[LoraValue]) -> bool {
        self.items[..self.presorted]
            .iter()
            .zip(a.iter().zip(b))
            .all(|(item, (x, y))| compare_sort_key(item, x, y) == Ordering::Equal)
    }

    /// Read the next run and sort it into `ready`. False at the end.
    fn next_run(&mut self) -> ExecResult<bool> {
        let first = match self.lookahead.take() {
            Some(first) => first,
            None => {
                if self.exhausted {
                    return Ok(false);
                }
                match self.upstream.next_row()? {
                    Some(row) => (self.lead_keys(&row)?, row),
                    None => {
                        self.exhausted = true;
                        return Ok(false);
                    }
                }
            }
        };
        let (run_keys, row) = first;
        let eval_ctx = self.ctx.eval_ctx();
        let mut buffer = SortBuffer::new(self.items, self.remaining);
        sort_checked(|| buffer.push(row, &eval_ctx))?;
        loop {
            let Some(row) = self.upstream.next_row()? else {
                self.exhausted = true;
                break;
            };
            let keys = self.lead_keys(&row)?;
            if !self.same_run(&run_keys, &keys) {
                self.lookahead = Some((keys, row));
                break;
            }
            sort_checked(|| buffer.push(row, &eval_ctx))?;
        }
        self.ready = buffer.finish().into_iter();
        Ok(true)
    }
}

impl<'a, S: GraphStorage> RowSource for PartialSortSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if self.remaining == Some(0) {
                return Ok(None);
            }
            if let Some(row) = self.ready.next() {
                if let Some(remaining) = self.remaining.as_mut() {
                    *remaining -= 1;
                }
                return Ok(Some(row));
            }
            if !self.next_run()? {
                return Ok(None);
            }
        }
    }
}
