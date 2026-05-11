//! Outer OPTIONAL MATCH operator source.

use lora_analyzer::symbols::VarId;
use lora_compiler::physical::{PhysicalNodeId, PhysicalPlan};
use lora_store::GraphStorage;

use crate::errors::{ExecResult, ExecutorError};
use crate::executor::{merge_optional_rows, null_extend_optional_row, optional_rows_compatible};
use crate::value::Row;

use super::{build_streaming, drain, RowSource, StreamCtx};

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
    state: OptionalMatchState,
}

// Keep `Row` inline: boxing would allocate for every upstream row on the
// streaming hot path, and this state is stored once inside `OptionalMatchSource`.
#[allow(clippy::large_enum_variant)]
enum OptionalMatchState {
    AwaitingInput,
    Scanning {
        input_row: Row,
        inner_idx: usize,
        matched: bool,
    },
}

impl<'a, S: GraphStorage> OptionalMatchSource<'a, S> {
    pub(super) fn new(
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
            state: OptionalMatchState::AwaitingInput,
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
            if matches!(self.state, OptionalMatchState::AwaitingInput) {
                let Some(input_row) = self.upstream.next_row()? else {
                    return Ok(None);
                };
                self.state = OptionalMatchState::Scanning {
                    input_row,
                    inner_idx: 0,
                    matched: false,
                };
            }

            let Some(inner_rows) = self.inner_rows.as_ref() else {
                return Err(ExecutorError::RuntimeError(
                    "OPTIONAL MATCH inner rows were not initialized".into(),
                ));
            };
            let OptionalMatchState::Scanning {
                input_row,
                inner_idx,
                matched,
            } = &mut self.state
            else {
                return Err(ExecutorError::RuntimeError(
                    "OPTIONAL MATCH cursor entered an invalid state".into(),
                ));
            };

            while *inner_idx < inner_rows.len() {
                let inner_row = &inner_rows[*inner_idx];
                *inner_idx += 1;

                if !optional_rows_compatible(input_row, inner_row) {
                    continue;
                }

                *matched = true;
                return Ok(Some(merge_optional_rows(input_row, inner_row)));
            }

            let OptionalMatchState::Scanning {
                input_row, matched, ..
            } = std::mem::replace(&mut self.state, OptionalMatchState::AwaitingInput)
            else {
                return Err(ExecutorError::RuntimeError(
                    "OPTIONAL MATCH cursor entered an invalid state".into(),
                ));
            };
            if !matched {
                return Ok(Some(null_extend_optional_row(input_row, self.new_vars)));
            }
        }
    }
}
