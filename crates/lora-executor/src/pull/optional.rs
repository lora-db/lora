//! Outer OPTIONAL MATCH operator source.

use lora_analyzer::symbols::VarId;
use lora_compiler::physical::{PhysicalNodeId, PhysicalPlan};
use lora_store::GraphStorage;

use crate::errors::{ExecResult, ExecutorError};
use crate::executor::{merge_optional_rows, null_extend_optional_row, optional_rows_compatible};
use crate::value::Row;

use super::{build_streaming, build_streaming_seeded, drain, RowSource, StreamCtx};

/// Streaming outer OPTIONAL MATCH source.
///
/// When the inner plan is fully streamable it is run once per outer row,
/// seeded with that row, so it expands from the outer row's bound nodes
/// (as the eager executors do). Otherwise the
/// inner plan is materialized once, uncorrelated, and joined against each
/// outer row as the outer cursor advances.
pub struct OptionalMatchSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    plan: &'a PhysicalPlan,
    inner: PhysicalNodeId,
    new_vars: &'a [VarId],
    correlated: bool,
    /// Uncorrelated mode: the inner plan's rows, shared by every outer row.
    shared_inner_rows: Option<Vec<Row>>,
    state: OptionalMatchState,
}

// Keep `Row` inline: boxing would allocate for every upstream row on the
// streaming hot path, and this state is stored once inside `OptionalMatchSource`.
#[allow(clippy::large_enum_variant)]
enum OptionalMatchState {
    AwaitingInput,
    Scanning {
        input_row: Row,
        /// Correlated mode: this outer row's own inner rows.
        own_rows: Vec<Row>,
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
            correlated: crate::executor::optional_can_correlate(plan, inner),
            shared_inner_rows: None,
            state: OptionalMatchState::AwaitingInput,
        }
    }

    fn inner_rows_for(&mut self, input_row: &Row) -> ExecResult<Vec<Row>> {
        if self.correlated {
            let mut inner = build_streaming_seeded(
                self.plan,
                self.inner,
                self.ctx.storage,
                self.ctx.params.clone(),
                input_row.clone(),
            )?;
            return drain(inner.as_mut());
        }
        if self.shared_inner_rows.is_none() {
            let mut inner = build_streaming(
                self.plan,
                self.inner,
                self.ctx.storage,
                self.ctx.params.clone(),
            )?;
            self.shared_inner_rows = Some(drain(inner.as_mut())?);
        }
        Ok(Vec::new())
    }
}

impl<'a, S: GraphStorage> RowSource for OptionalMatchSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if matches!(self.state, OptionalMatchState::AwaitingInput) {
                let Some(input_row) = self.upstream.next_row()? else {
                    return Ok(None);
                };
                let own_rows = self.inner_rows_for(&input_row)?;
                self.state = OptionalMatchState::Scanning {
                    input_row,
                    own_rows,
                    inner_idx: 0,
                    matched: false,
                };
            }

            let OptionalMatchState::Scanning {
                input_row,
                own_rows,
                inner_idx,
                matched,
            } = &mut self.state
            else {
                return Err(ExecutorError::RuntimeError(
                    "OPTIONAL MATCH cursor entered an invalid state".into(),
                ));
            };
            let inner_rows: &[Row] = if self.correlated {
                own_rows
            } else {
                self.shared_inner_rows.as_deref().ok_or_else(|| {
                    ExecutorError::RuntimeError(
                        "OPTIONAL MATCH inner rows were not initialized".into(),
                    )
                })?
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
