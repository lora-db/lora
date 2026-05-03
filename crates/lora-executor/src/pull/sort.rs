//! Order-preserving operator sources.
//!
//! - [`SortSource`] — drains upstream on first pull, sorts the buffer
//!   in place, then yields rows lazily.
//! - [`LimitSource`] — `SKIP` / `LIMIT`. Fully streaming.

use lora_store::GraphStorage;

use crate::errors::ExecResult;
use crate::value::Row;

use super::traits::{drain, RowSource, StreamCtx};

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
    pub(super) fn new(
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
