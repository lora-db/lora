//! Predicate filter operator source.

use lora_analyzer::ResolvedExpr;
use lora_store::GraphStorage;

use crate::errors::{ExecResult, ExecutorError};
use crate::eval::eval_truthy_result;
use crate::value::Row;

use super::{RowSource, StreamCtx};

/// Pulls upstream rows until one matches `predicate`, then yields it.
pub struct FilterSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    predicate: &'a ResolvedExpr,
}

impl<'a, S: GraphStorage> FilterSource<'a, S> {
    pub(super) fn new(
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
                    if eval_truthy_result(self.predicate, &row, &eval_ctx)
                        .map_err(ExecutorError::RuntimeError)?
                    {
                        return Ok(Some(row));
                    }
                }
                None => return Ok(None),
            }
        }
    }
}
