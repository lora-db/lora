//! Result-row cap for the buffered executors.
//!
//! A caller can cap how many rows a query may return
//! ([`super::Executor::with_max_rows`]). The buffered executors build each
//! operator's whole output before the next one runs, so checking the final
//! result would only fail after a huge result was already built. Instead
//! the cap applies to every operator whose output feeds the result row for
//! row or more: the root, and below it the input of each operator that
//! never drops a row (a plain projection, an unbounded sort, `OPTIONAL
//! MATCH`, the write clauses). Such an operator holding more rows than the
//! cap proves the result would too, so it fails as soon as it gets there.
//! Operators that may drop rows (filters, expansions, aggregations,
//! `LIMIT`) are not capped, since their own output can exceed the
//! result's.
//!
//! Each executor tracks the cap of the operator running now in a
//! [`RowCaps`], set by `execute_node` around each operator. The operator
//! hands it to the row loops that multiply rows (cartesian scans,
//! expansions, `UNWIND`), which check their output length as they go.
//! Code an operator runs on the side (expression evaluation, nested
//! pipelines and executors) never sees it. Without a cap this costs a few
//! plain loads and stores per operator, nothing per row.

use std::sync::atomic::{AtomicUsize, Ordering};

use lora_compiler::physical::{PhysicalNodeId, PhysicalOp, PhysicalPlan};

use crate::errors::{ExecResult, ExecutorError};
use crate::value::Row;

/// No cap.
pub(crate) const UNCAPPED: usize = usize::MAX;

/// Fail once `rows` holds more than `cap` rows.
#[inline]
pub(crate) fn check_row_cap(rows: usize, cap: usize) -> ExecResult<()> {
    if rows > cap {
        Err(ExecutorError::ResultTooLarge { max_rows: cap })
    } else {
        Ok(())
    }
}

/// The caps of one executor. Atomics only so the read-only executor stays
/// `Sync` for its parallel paths; one thread drives `execute_node`, so
/// relaxed loads and stores are enough.
#[derive(Debug)]
pub(crate) struct RowCaps {
    /// The cap of the operator running now.
    op: AtomicUsize,
    /// The cap the operator running now hands to its inputs: `op` when
    /// it never drops a row, [`UNCAPPED`] otherwise.
    input: AtomicUsize,
}

/// The caps to put back once an operator or plan is done.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SavedCaps {
    op: usize,
    input: usize,
}

impl RowCaps {
    pub(crate) const fn new() -> Self {
        Self {
            op: AtomicUsize::new(UNCAPPED),
            input: AtomicUsize::new(UNCAPPED),
        }
    }

    /// The cap of the operator running now, for it to hand to its row
    /// loop.
    #[inline]
    pub(crate) fn op_cap(&self) -> usize {
        self.op.load(Ordering::Relaxed)
    }

    /// Start running a plan whose result is capped at `max_rows` rows
    /// (`None`: not capped).
    pub(crate) fn enter_plan(&self, plan: &PhysicalPlan, max_rows: Option<usize>) -> SavedCaps {
        let saved = self.save();
        // A plan that ends in a write returns no rows.
        let cap = match max_rows {
            Some(max_rows) if !super::plan_ends_in_write(plan) => max_rows,
            _ => UNCAPPED,
        };
        self.set(UNCAPPED, cap);
        saved
    }

    /// Start running operator `node_id`: it takes the cap its parent
    /// hands down and hands it on to its inputs only if it never drops a
    /// row.
    #[inline]
    pub(crate) fn enter_operator(&self, plan: &PhysicalPlan, node_id: PhysicalNodeId) -> SavedCaps {
        let saved = self.save();
        let cap = saved.input;
        let input = if cap != UNCAPPED && keeps_every_row(&plan.nodes[node_id]) {
            cap
        } else {
            UNCAPPED
        };
        self.set(cap, input);
        saved
    }

    /// Run a side input of the current operator uncapped: the inner
    /// pattern of an `OPTIONAL MATCH`, whose rows need not reach the result.
    pub(crate) fn enter_side_input(&self) -> SavedCaps {
        let saved = self.save();
        self.input.store(UNCAPPED, Ordering::Relaxed);
        saved
    }

    #[inline]
    pub(crate) fn restore(&self, saved: SavedCaps) {
        self.set(saved.op, saved.input);
    }

    /// Check the finished output of the operator running now against its
    /// cap.
    #[inline]
    pub(crate) fn check(&self, rows: ExecResult<Vec<Row>>) -> ExecResult<Vec<Row>> {
        let rows = rows?;
        let cap = self.op_cap();
        if rows.len() > cap {
            return Err(over_cap(rows, cap));
        }
        Ok(rows)
    }

    #[inline]
    fn save(&self) -> SavedCaps {
        SavedCaps {
            op: self.op.load(Ordering::Relaxed),
            input: self.input.load(Ordering::Relaxed),
        }
    }

    #[inline]
    fn set(&self, op: usize, input: usize) {
        self.op.store(op, Ordering::Relaxed);
        self.input.store(input, Ordering::Relaxed);
    }
}

#[cold]
#[inline(never)]
fn over_cap(rows: Vec<Row>, cap: usize) -> ExecutorError {
    super::helpers::discard_rows(rows);
    ExecutorError::ResultTooLarge { max_rows: cap }
}

/// Restores an executor's caps when dropped (read-only executor, whose
/// methods only borrow it shared).
pub(crate) struct CapGuard<'a> {
    caps: &'a RowCaps,
    saved: SavedCaps,
}

impl<'a> CapGuard<'a> {
    pub(crate) fn new(caps: &'a RowCaps, saved: SavedCaps) -> Self {
        Self { caps, saved }
    }
}

impl Drop for CapGuard<'_> {
    fn drop(&mut self) {
        self.caps.restore(self.saved);
    }
}

/// Whether `op` outputs at least one row per input row, so its input
/// holding more rows than the cap proves its output does too.
fn keeps_every_row(op: &PhysicalOp) -> bool {
    match op {
        PhysicalOp::Projection(op) => !op.distinct,
        PhysicalOp::Sort(op) => op.top_k.is_none() && op.limit.is_none(),
        PhysicalOp::PathBuild(op) => op.shortest_path_all.is_none(),
        // Every input row comes out at least once; the inner pattern is a
        // side input and runs uncapped (`RowCaps::enter_side_input`).
        PhysicalOp::OptionalMatch(_) => true,
        // One output row per input row (MERGE binds one match or the
        // created pattern; FOREACH emits the outer row unchanged).
        PhysicalOp::Create(_)
        | PhysicalOp::Merge(_)
        | PhysicalOp::Set(_)
        | PhysicalOp::Remove(_)
        | PhysicalOp::Delete(_)
        | PhysicalOp::Foreach(_) => true,
        _ => false,
    }
}
