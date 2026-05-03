//! Outer OPTIONAL MATCH operator source.

use lora_analyzer::symbols::VarId;
use lora_compiler::physical::{PhysicalNodeId, PhysicalPlan};
use lora_store::GraphStorage;

use crate::errors::ExecResult;
use crate::value::{LoraValue, Row};

use super::traits::{build_streaming, drain, RowSource, StreamCtx};

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
