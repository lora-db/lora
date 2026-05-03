//! Projection / Distinct / Unwind operator sources.
//!
//! - [`ProjectionSource`] — evaluate each projection item per row.
//! - [`DistinctSource`] — streaming dedup of projection output.
//! - [`UnwindSource`] — explode each upstream row's list expression
//!   into one row per element.

use std::collections::BTreeSet;

use lora_analyzer::symbols::VarId;
use lora_analyzer::{ResolvedExpr, ResolvedProjection};
use lora_store::GraphStorage;

use crate::errors::{ExecResult, ExecutorError};
use crate::eval::eval_expr_result;
use crate::executor::GroupValueKey;
use crate::value::{LoraValue, Row};

use super::traits::{RowSource, StreamCtx};

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
    pub(super) fn new(
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
                        let value = eval_expr_result(&item.expr, &projected, &eval_ctx)
                            .map_err(ExecutorError::RuntimeError)?;
                        projected.insert_named(item.output, item.name.clone(), value);
                    }
                    Ok(Some(projected))
                } else {
                    let mut projected = Row::new();
                    for item in self.items {
                        let value = eval_expr_result(&item.expr, &row, &eval_ctx)
                            .map_err(ExecutorError::RuntimeError)?;
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
    pub(super) fn new(
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
                    let value = eval_expr_result(self.expr, &row, &eval_ctx)
                        .map_err(ExecutorError::RuntimeError)?;
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

/// Streaming DISTINCT source. Backs `Projection { distinct: true }`.
/// It keeps only the seen key set, then yields each first-seen row as
/// soon as upstream produces it.
pub struct DistinctSource<'a> {
    upstream: Box<dyn RowSource + 'a>,
    seen: BTreeSet<Vec<GroupValueKey>>,
}

impl<'a> DistinctSource<'a> {
    pub(super) fn new(upstream: Box<dyn RowSource + 'a>) -> Self {
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
