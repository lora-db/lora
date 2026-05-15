//! `CALL { ... }` subquery operator source.
//!
//! For each outer row, the inner sub-plan is rebuilt with the outer
//! row as the seed for its bottom `Argument`, then drained. Each
//! produced inner row is merged with the outer row and emitted —
//! the semantics is an inner join on shared bindings (the seed
//! already carries the outer bindings; the inner sub-plan extends
//! them with whatever its final RETURN exposes).

use std::sync::Arc;

use lora_analyzer::symbols::VarId;
use lora_compiler::physical::{PhysicalNodeId, PhysicalPlan};
use lora_store::GraphStorage;

use crate::errors::ExecResult;
use crate::executor::merge_optional_rows;
use crate::value::{LoraValue, Row};

use super::traits::build_streaming_seeded;
use super::{drain, RowSource};

pub struct CallSubquerySource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    plan: &'a PhysicalPlan,
    inner: PhysicalNodeId,
    storage: &'a S,
    params: Arc<std::collections::BTreeMap<String, LoraValue>>,
    new_vars: &'a [VarId],
    pending: std::vec::IntoIter<Row>,
    pending_outer: Option<Row>,
}

impl<'a, S: GraphStorage> CallSubquerySource<'a, S> {
    pub(super) fn new(
        upstream: Box<dyn RowSource + 'a>,
        plan: &'a PhysicalPlan,
        inner: PhysicalNodeId,
        storage: &'a S,
        params: Arc<std::collections::BTreeMap<String, LoraValue>>,
        new_vars: &'a [VarId],
    ) -> Self {
        Self {
            upstream,
            plan,
            inner,
            storage,
            params,
            new_vars,
            pending: Vec::new().into_iter(),
            pending_outer: None,
        }
    }
}

impl<'a, S: GraphStorage> RowSource for CallSubquerySource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if let Some(inner_row) = self.pending.next() {
                let outer = self
                    .pending_outer
                    .as_ref()
                    .expect("pending_outer set when pending iter has rows");
                return Ok(Some(merge_optional_rows(outer, &inner_row)));
            }

            let Some(outer_row) = self.upstream.next_row()? else {
                return Ok(None);
            };

            // Run the inner sub-plan with the outer row injected as the
            // bottom Argument's single row, so MATCH inside the CALL
            // sees outer-bound variables and aggregations are computed
            // per outer row.
            let mut inner_source = build_streaming_seeded(
                self.plan,
                self.inner,
                self.storage,
                self.params.clone(),
                outer_row.clone(),
            )?;
            let inner_rows = drain(inner_source.as_mut())?;

            if inner_rows.is_empty() {
                // CALL with no inner rows: drop the outer row (inner join).
                // Null-extend behaviour is reserved for OPTIONAL MATCH.
                let _ = self.new_vars; // kept for future null-extension toggle
                continue;
            }

            self.pending_outer = Some(outer_row);
            self.pending = inner_rows.into_iter();
        }
    }
}
