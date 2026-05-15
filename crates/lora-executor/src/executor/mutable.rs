//! Mutable buffered executor: applies CREATE / MERGE / DELETE / SET /
//! REMOVE on top of the read-side operator set.
//!
//! [`MutableExecutor`] mirrors the read-only [`super::immutable::Executor`]
//! for all read operators (so a write op above any read subtree
//! materializes the same way) and adds the per-row write
//! implementations. The streaming pull pipeline in `crate::pull` runs
//! `MutableExecutor::apply_write_op` row-by-row through the
//! `StreamingWriteCursor` fast path; the buffered `exec_*` methods
//! here handle the fallback when a write op's input subtree is not
//! fully streamable.

use crate::errors::{value_kind, ExecResult, ExecutorError};
use crate::eval::{clear_eval_error, eval_expr, EvalContext};
use crate::value::{lora_value_to_property, LoraValue, Row};
use crate::{project_rows, ExecuteOptions, QueryResult};

use lora_analyzer::{
    symbols::VarId, ResolvedExpr, ResolvedPattern, ResolvedPatternElement, ResolvedPatternPart,
    ResolvedRemoveItem, ResolvedSetItem,
};
use lora_ast::Direction;
use lora_compiler::physical::*;
use lora_compiler::CompiledQuery;
use lora_store::{GraphStorageMut, NodeId, Properties};

use std::collections::BTreeMap;
use std::time::Instant;
use tracing::{debug, error, trace};

use super::aggregate_rows;
use super::helpers::{
    build_path_value, check_deadline_at, dedup_rows, eval_properties_expr, expand_rows,
    expand_var_len_rows, filter_rows_checked, filter_shortest_paths, flatten_label_groups,
    hydrate_node_record, hydrate_relationship_record, limit_rows, node_by_label_scan_rows,
    node_by_property_scan_rows, node_matches_label_groups, node_scan_rows, plan_may_need_hydration,
    project_rows_checked, scan_node_ids_for_label_groups, unwind_rows,
    value_matches_property_value,
};
use super::optional_match_rows;
use super::sort_rows_with_top_k;

/// Lightweight target for SET property-mutation paths. Lets the SET logic
/// borrow the row entry (just pulling out the id) instead of cloning the
/// whole `LoraValue`.
#[derive(Clone, Copy)]
enum EntityTarget {
    Node(NodeId),
    Relationship(u64),
}

fn entity_target_from_value(value: &LoraValue) -> ExecResult<EntityTarget> {
    match value {
        LoraValue::Node(id) => Ok(EntityTarget::Node(*id)),
        LoraValue::Relationship(id) => Ok(EntityTarget::Relationship(*id)),
        other => Err(ExecutorError::InvalidSetTarget {
            found: value_kind(other),
        }),
    }
}

pub struct MutableExecutionContext<'a, S: GraphStorageMut> {
    pub storage: &'a mut S,
    pub params: BTreeMap<String, LoraValue>,
}

pub struct MutableExecutor<'a, S: GraphStorageMut> {
    ctx: MutableExecutionContext<'a, S>,
    deadline: Option<Instant>,
}

impl<'a, S: GraphStorageMut> MutableExecutor<'a, S> {
    pub fn new(ctx: MutableExecutionContext<'a, S>) -> Self {
        Self {
            ctx,
            deadline: None,
        }
    }

    pub fn with_deadline(ctx: MutableExecutionContext<'a, S>, deadline: Option<Instant>) -> Self {
        Self { ctx, deadline }
    }

    #[inline]
    fn check_deadline(&self) -> ExecResult<()> {
        if let Some(deadline) = self.deadline {
            check_deadline_at(deadline)
        } else {
            Ok(())
        }
    }

    pub fn execute(
        &mut self,
        plan: &PhysicalPlan,
        options: Option<ExecuteOptions>,
    ) -> ExecResult<QueryResult> {
        let rows = self.execute_rows(plan)?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    pub fn execute_rows(&mut self, plan: &PhysicalPlan) -> ExecResult<Vec<Row>> {
        self.check_deadline()?;
        // Clear any error residue that a previous query on this thread may have
        // left in the thread-local eval-error slot.
        clear_eval_error();

        let rows = self.execute_node(plan, plan.root)?;
        if !plan_may_need_hydration(plan) {
            return Ok(rows);
        }
        Ok(rows
            .into_iter()
            .map(|row| self.hydrate_row(row))
            .collect::<Vec<_>>())
    }

    /// Execute a compiled query that may include UNION branches.
    pub fn execute_compiled(
        &mut self,
        compiled: &CompiledQuery,
        options: Option<ExecuteOptions>,
    ) -> ExecResult<QueryResult> {
        let rows = self.execute_compiled_rows(compiled)?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    pub fn execute_compiled_rows(&mut self, compiled: &CompiledQuery) -> ExecResult<Vec<Row>> {
        self.check_deadline()?;
        if compiled.unions.is_empty() {
            return self.execute_rows(&compiled.physical);
        }

        clear_eval_error();

        // Execute the head branch.
        let mut all_rows = self.execute_and_hydrate(&compiled.physical)?;

        // Execute each UNION branch and combine.
        // Track whether any branch uses plain UNION (dedup needed).
        let mut needs_dedup = false;

        for branch in &compiled.unions {
            self.check_deadline()?;
            let branch_rows = self.execute_and_hydrate(&branch.physical)?;
            all_rows.extend(branch_rows);

            if !branch.all {
                needs_dedup = true;
            }
        }

        if needs_dedup {
            all_rows = dedup_rows(all_rows);
        }

        Ok(all_rows)
    }

    fn execute_and_hydrate(&mut self, plan: &PhysicalPlan) -> ExecResult<Vec<Row>> {
        self.check_deadline()?;
        let rows = self.execute_node(plan, plan.root)?;
        if !plan_may_need_hydration(plan) {
            return Ok(rows);
        }
        Ok(rows.into_iter().map(|row| self.hydrate_row(row)).collect())
    }

    pub(crate) fn hydrate_row(&self, row: Row) -> Row {
        let mut out = Row::new();

        for (var, name, value) in row.into_iter_named() {
            out.insert_named(var, name, self.hydrate_value(value));
        }

        out
    }

    fn execute_node(
        &mut self,
        plan: &PhysicalPlan,
        node_id: PhysicalNodeId,
    ) -> ExecResult<Vec<Row>> {
        self.check_deadline()?;
        trace!("mutable execute_node start: node_id={node_id:?}");

        let result = match &plan.nodes[node_id] {
            PhysicalOp::Argument(op) => self.exec_argument(op),
            PhysicalOp::NodeScan(op) => self.exec_node_scan(plan, op),
            PhysicalOp::NodeByLabelScan(op) => self.exec_node_by_label_scan(plan, op),
            PhysicalOp::NodeByPropertyScan(op) => self.exec_node_by_property_scan(plan, op),
            PhysicalOp::NodeByPropertyRangeScan(op) => {
                self.exec_node_by_property_range_scan(plan, op)
            }
            PhysicalOp::NodeByTextScan(op) => self.exec_node_by_text_scan(plan, op),
            PhysicalOp::NodeByPointScan(op) => self.exec_node_by_point_scan(plan, op),
            PhysicalOp::RelByPropertyRangeScan(op) => {
                self.exec_rel_by_property_range_scan(plan, op)
            }
            PhysicalOp::RelByTextScan(op) => self.exec_rel_by_text_scan(plan, op),
            PhysicalOp::RelByPointScan(op) => self.exec_rel_by_point_scan(plan, op),
            PhysicalOp::Expand(op) => self.exec_expand(plan, op),
            PhysicalOp::Filter(op) => self.exec_filter(plan, op),
            PhysicalOp::Projection(op) => self.exec_projection(plan, op),
            PhysicalOp::Unwind(op) => self.exec_unwind(plan, op),
            PhysicalOp::HashAggregation(op) => self.exec_hash_aggregation(plan, op),
            PhysicalOp::Sort(op) => self.exec_sort(plan, op),
            PhysicalOp::Limit(op) => self.exec_limit(plan, op),
            PhysicalOp::Create(op) => self.exec_create(plan, op),
            PhysicalOp::Merge(op) => self.exec_merge(plan, op),
            PhysicalOp::Delete(op) => self.exec_delete(plan, op),
            PhysicalOp::Set(op) => self.exec_set(plan, op),
            PhysicalOp::Remove(op) => self.exec_remove(plan, op),
            PhysicalOp::OptionalMatch(op) => self.exec_optional_match(plan, op),
            PhysicalOp::CallSubquery(op) => self.exec_call_subquery(plan, op),
            PhysicalOp::PathBuild(op) => self.exec_path_build(plan, op),
        };

        match &result {
            Ok(rows) => trace!(
                "mutable execute_node ok: node_id={node_id:?}, rows={}",
                rows.len()
            ),
            Err(err) => error!("mutable execute_node failed: node_id={node_id:?}, error={err}"),
        }

        result
    }

    fn exec_argument(&self, _op: &ArgumentExec) -> ExecResult<Vec<Row>> {
        Ok(vec![Row::new()])
    }

    fn exec_node_scan(&mut self, plan: &PhysicalPlan, op: &NodeScanExec) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };

        node_scan_rows(&*self.ctx.storage, base_rows, op, self.deadline)
    }

    fn exec_node_by_label_scan(
        &mut self,
        plan: &PhysicalPlan,
        op: &NodeByLabelScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };

        node_by_label_scan_rows(&*self.ctx.storage, base_rows, op, self.deadline)
    }

    fn exec_node_by_property_scan(
        &mut self,
        plan: &PhysicalPlan,
        op: &NodeByPropertyScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };

        node_by_property_scan_rows(
            &*self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_node_by_property_range_scan(
        &mut self,
        plan: &PhysicalPlan,
        op: &lora_compiler::NodeByPropertyRangeScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::node_by_property_range_scan_rows(
            &*self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_node_by_text_scan(
        &mut self,
        plan: &PhysicalPlan,
        op: &lora_compiler::NodeByTextScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::node_by_text_scan_rows(
            &*self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_node_by_point_scan(
        &mut self,
        plan: &PhysicalPlan,
        op: &lora_compiler::NodeByPointScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::node_by_point_scan_rows(
            &*self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_rel_by_property_range_scan(
        &mut self,
        plan: &PhysicalPlan,
        op: &lora_compiler::RelByPropertyRangeScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::rel_by_property_range_scan_rows(
            &*self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_rel_by_text_scan(
        &mut self,
        plan: &PhysicalPlan,
        op: &lora_compiler::RelByTextScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::rel_by_text_scan_rows(
            &*self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_rel_by_point_scan(
        &mut self,
        plan: &PhysicalPlan,
        op: &lora_compiler::RelByPointScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::rel_by_point_scan_rows(
            &*self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_expand(&mut self, plan: &PhysicalPlan, op: &ExpandExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        if let Some(range) = &op.range {
            expand_var_len_rows(&*self.ctx.storage, input_rows, op, range)
        } else {
            expand_rows(&*self.ctx.storage, &self.ctx.params, input_rows, op)
        }
    }

    fn exec_filter(&mut self, plan: &PhysicalPlan, op: &FilterExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: &*self.ctx.storage,
            params: &self.ctx.params,
        };

        filter_rows_checked(input_rows, &op.predicate, &eval_ctx)
    }

    fn exec_projection(
        &mut self,
        plan: &PhysicalPlan,
        op: &ProjectionExec,
    ) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: &*self.ctx.storage,
            params: &self.ctx.params,
        };

        project_rows_checked(input_rows, op, &eval_ctx)
    }

    fn hydrate_value(&self, value: LoraValue) -> LoraValue {
        match value {
            LoraValue::Node(id) => self.hydrate_node(id),
            LoraValue::Relationship(id) => self.hydrate_relationship(id),
            LoraValue::List(values) => {
                LoraValue::List(values.into_iter().map(|v| self.hydrate_value(v)).collect())
            }
            LoraValue::Map(map) => LoraValue::Map(
                map.into_iter()
                    .map(|(k, v)| (k, self.hydrate_value(v)))
                    .collect(),
            ),
            other => other,
        }
    }

    fn hydrate_node(&self, id: u64) -> LoraValue {
        self.ctx
            .storage
            .with_node(id, hydrate_node_record)
            .unwrap_or(LoraValue::Null)
    }

    fn hydrate_relationship(&self, id: u64) -> LoraValue {
        self.ctx
            .storage
            .with_relationship(id, hydrate_relationship_record)
            .unwrap_or(LoraValue::Null)
    }

    fn exec_unwind(&mut self, plan: &PhysicalPlan, op: &UnwindExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: &*self.ctx.storage,
            params: &self.ctx.params,
        };

        Ok(unwind_rows(input_rows, op, &eval_ctx))
    }

    fn exec_hash_aggregation(
        &mut self,
        plan: &PhysicalPlan,
        op: &HashAggregationExec,
    ) -> ExecResult<Vec<Row>> {
        if let Some(rows) =
            super::helpers::count_all_scan_aggregation_rows(&*self.ctx.storage, plan, op)
        {
            return Ok(rows);
        }

        let input_rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: &*self.ctx.storage,
            params: &self.ctx.params,
        };

        aggregate_rows(
            input_rows,
            &op.group_by,
            &op.aggregates,
            &eval_ctx,
            |value| self.hydrate_value(value),
        )
    }

    fn exec_sort(&mut self, plan: &PhysicalPlan, op: &SortExec) -> ExecResult<Vec<Row>> {
        let mut rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: &*self.ctx.storage,
            params: &self.ctx.params,
        };

        sort_rows_with_top_k(&mut rows, &op.items, &eval_ctx, op.top_k);

        Ok(rows)
    }

    fn exec_limit(&mut self, plan: &PhysicalPlan, op: &LimitExec) -> ExecResult<Vec<Row>> {
        let rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: &*self.ctx.storage,
            params: &self.ctx.params,
        };

        Ok(limit_rows(rows, op, &eval_ctx))
    }

    fn exec_optional_match(
        &mut self,
        plan: &PhysicalPlan,
        op: &OptionalMatchExec,
    ) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;

        // Inner plan is read-only and input-independent; execute once and reuse.
        let inner_rows = self.execute_node(plan, op.inner)?;

        Ok(optional_match_rows(input_rows, &inner_rows, &op.new_vars))
    }

    fn exec_call_subquery(
        &mut self,
        plan: &PhysicalPlan,
        op: &CallSubqueryExec,
    ) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let mut out = Vec::with_capacity(input_rows.len());
        let params = std::sync::Arc::new(self.ctx.params.clone());
        let storage_ref: &S = &*self.ctx.storage;
        for outer_row in input_rows {
            let mut inner_source = crate::pull::build_streaming_seeded(
                plan,
                op.inner,
                storage_ref,
                params.clone(),
                outer_row.clone(),
            )?;
            let inner_rows = crate::pull::drain(inner_source.as_mut())?;
            for inner_row in inner_rows {
                out.push(crate::executor::merge_optional_rows(&outer_row, &inner_row));
            }
        }
        Ok(out)
    }

    fn exec_path_build(&mut self, plan: &PhysicalPlan, op: &PathBuildExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let mut rows: Vec<Row> = input_rows
            .into_iter()
            .map(|mut row| {
                let path = build_path_value(&row, &op.node_vars, &op.rel_vars, &*self.ctx.storage);
                row.insert(op.output, path);
                row
            })
            .collect();

        if let Some(all) = op.shortest_path_all {
            rows = filter_shortest_paths(rows, op.output, all);
        }
        Ok(rows)
    }

    fn exec_create(&mut self, plan: &PhysicalPlan, op: &CreateExec) -> ExecResult<Vec<Row>> {
        // Fast path: if the input subtree is fully streamable (no
        // nested writes, no blocking operators), pull rows one at a
        // time and apply the create pattern per row, instead of
        // materializing the whole input. The output Vec still
        // accumulates — auto-commit-side output streaming is M1.b.
        if crate::pull::subtree_is_fully_streaming(plan, op.input) {
            return self.exec_create_streaming_input(plan, op);
        }

        let input_rows = self.execute_node(plan, op.input)?;
        let mut out = Vec::with_capacity(input_rows.len());

        for mut row in input_rows {
            self.apply_create_pattern(&mut row, &op.pattern)?;
            out.push(row);
        }

        Ok(out)
    }

    /// Generic streaming-input loop for write operators whose input
    /// subtree is fully streamable. Opens a pull-based read cursor
    /// over the input subtree, calls `apply` per row, and accumulates
    /// the resulting rows.
    ///
    /// # Safety
    ///
    /// The upstream [`crate::pull::RowSource`] needs `&S` while it
    /// lives; the per-row `apply` callback needs `&mut S` (via
    /// `&mut self`). The existing read-side `RowSource` impls
    /// materialize their iteration state into owned `Vec`s at
    /// construction time (see `NodeScanSource::cur_ids`,
    /// `ExpandSource::cur_edges`, etc. in `pull.rs`), so no live
    /// `&S` borrow into storage persists across `next_row` calls.
    /// We exploit that by deriving the read borrow from a raw
    /// pointer — Rust then doesn't see the shared/mutable conflict
    /// at compile time, and the dynamic access pattern is
    /// non-aliasing: read-only inside `next_row`, then mutable
    /// inside `apply`, never both at the same instant.
    fn streaming_apply<F>(
        &mut self,
        plan: &PhysicalPlan,
        input: PhysicalNodeId,
        mut apply: F,
    ) -> ExecResult<Vec<Row>>
    where
        F: FnMut(&mut Self, &mut Row) -> ExecResult<()>,
    {
        use std::sync::Arc;

        let storage_ptr: *mut S = self.ctx.storage as *mut S;
        let params = Arc::new(self.ctx.params.clone());

        // SAFETY: see method-level comment.
        let storage_ref: &S = unsafe { &*storage_ptr };
        let mut upstream = crate::pull::build_streaming(plan, input, storage_ref, params)?;

        let mut out = Vec::new();
        while let Some(mut row) = upstream.next_row()? {
            apply(self, &mut row)?;
            out.push(row);
        }

        Ok(out)
    }

    /// Streaming-input variant of [`Self::exec_create`]. Delegates
    /// to [`Self::streaming_apply`].
    fn exec_create_streaming_input(
        &mut self,
        plan: &PhysicalPlan,
        op: &CreateExec,
    ) -> ExecResult<Vec<Row>> {
        self.streaming_apply(plan, op.input, |this, row| {
            this.apply_create_pattern(row, &op.pattern)
        })
    }

    fn apply_remove_item(&mut self, row: &Row, item: &ResolvedRemoveItem) -> ExecResult<()> {
        match item {
            ResolvedRemoveItem::Labels { variable, labels } => match row.get(*variable) {
                Some(LoraValue::Node(node_id)) => {
                    let node_id = *node_id;
                    for label in labels {
                        self.ctx.storage.remove_node_label(node_id, label);
                    }
                    Ok(())
                }
                Some(other) => Err(ExecutorError::ExpectedNodeForRemoveLabels {
                    found: value_kind(other),
                }),
                None => Err(ExecutorError::UnboundVariableForRemove {
                    var: format!("{variable:?}"),
                }),
            },

            ResolvedRemoveItem::Property { expr } => self.remove_property_from_expr(row, expr),
        }
    }

    fn delete_value(&mut self, value: LoraValue, detach: bool) -> ExecResult<()> {
        match value {
            LoraValue::Null => Ok(()),

            LoraValue::Node(node_id) => {
                if detach {
                    self.ctx.storage.detach_delete_node(node_id);
                    Ok(())
                } else {
                    let ok = self.ctx.storage.delete_node(node_id);
                    if ok {
                        Ok(())
                    } else {
                        Err(ExecutorError::DeleteNodeWithRelationships { node_id })
                    }
                }
            }

            LoraValue::Relationship(rel_id) => {
                let ok = self.ctx.storage.delete_relationship(rel_id);
                if ok {
                    Ok(())
                } else {
                    Err(ExecutorError::DeleteRelationshipFailed { rel_id })
                }
            }

            LoraValue::List(values) => {
                for v in values {
                    self.delete_value(v, detach)?;
                }
                Ok(())
            }

            other => Err(ExecutorError::InvalidDeleteTarget {
                found: value_kind(&other),
            }),
        }
    }

    fn exec_merge(&mut self, plan: &PhysicalPlan, op: &MergeExec) -> ExecResult<Vec<Row>> {
        // Streaming-input fast path when the input subtree is fully
        // streamable. Per-row work (probe → optionally create →
        // ON MATCH / ON CREATE actions) is identical to the
        // materialized branch below.
        if crate::pull::subtree_is_fully_streaming(plan, op.input) {
            return self.streaming_apply(plan, op.input, |this, row| {
                let already_bound = this.pattern_part_is_bound(row, &op.pattern_part);
                let matched = if already_bound {
                    true
                } else {
                    this.try_match_merge_pattern(row, &op.pattern_part)?
                };
                if !matched {
                    this.apply_create_pattern_part(row, &op.pattern_part)?;
                }
                for action in &op.actions {
                    if action.on_match == matched {
                        for item in &action.set.items {
                            this.apply_set_item(row, item)?;
                        }
                    }
                }
                Ok(())
            });
        }

        let input_rows = self.execute_node(plan, op.input)?;
        let mut out = Vec::with_capacity(input_rows.len());

        for mut row in input_rows {
            // First check if the pattern variable is already bound in the row.
            let already_bound = self.pattern_part_is_bound(&row, &op.pattern_part);

            let matched = if already_bound {
                true
            } else {
                // Try to find an existing match in the graph.
                self.try_match_merge_pattern(&mut row, &op.pattern_part)?
            };

            if !matched {
                self.apply_create_pattern_part(&mut row, &op.pattern_part)?;
            }

            for action in &op.actions {
                if action.on_match == matched {
                    for item in &action.set.items {
                        self.apply_set_item(&row, item)?;
                    }
                }
            }

            out.push(row);
        }

        Ok(out)
    }

    /// Try to find an existing node/pattern in the graph matching the MERGE
    /// pattern. If found, bind the variable in the row and return true.
    fn try_match_merge_pattern(
        &self,
        row: &mut Row,
        part: &ResolvedPatternPart,
    ) -> ExecResult<bool> {
        match &part.element {
            ResolvedPatternElement::Node {
                var,
                labels,
                properties,
            } => {
                // ID-only candidate discovery; borrow the record during
                // label/property filtering to avoid cloning non-matches.
                let candidate_ids = if labels.is_empty() {
                    self.ctx.storage.all_node_ids()
                } else {
                    scan_node_ids_for_label_groups(&*self.ctx.storage, labels)
                };

                // Filter by properties if specified
                let eval_ctx = EvalContext {
                    storage: &*self.ctx.storage,
                    params: &self.ctx.params,
                };
                let expected_props = properties.as_ref().map(|e| eval_expr(e, row, &eval_ctx));

                for id in candidate_ids {
                    let matched = self
                        .ctx
                        .storage
                        .with_node(id, |node| {
                            if !node_matches_label_groups(&node.labels, labels) {
                                return false;
                            }
                            if let Some(LoraValue::Map(expected)) = &expected_props {
                                let all_match = expected.iter().all(|(key, expected_value)| {
                                    node.properties
                                        .get(key)
                                        .map(|actual| {
                                            value_matches_property_value(expected_value, actual)
                                        })
                                        .unwrap_or(false)
                                });
                                if !all_match {
                                    return false;
                                }
                            }
                            true
                        })
                        .unwrap_or(false);

                    if !matched {
                        continue;
                    }

                    // Found a match — bind the variable
                    if let Some(var_id) = var {
                        row.insert(*var_id, LoraValue::Node(id));
                    }
                    return Ok(true);
                }

                Ok(false)
            }

            ResolvedPatternElement::ShortestPath { .. } => {
                // ShortestPath is not valid in MERGE context
                Ok(false)
            }

            ResolvedPatternElement::NodeChain { head, chain } => {
                // Resolve the head node — it should be already bound in the row.
                let head_node_id = if let Some(var_id) = head.var {
                    if let Some(LoraValue::Node(id)) = row.get(var_id) {
                        *id
                    } else {
                        // Try to match head node as a standalone node pattern.
                        let node_matched = self.try_match_merge_pattern(
                            row,
                            &ResolvedPatternPart {
                                binding: None,
                                element: ResolvedPatternElement::Node {
                                    var: head.var,
                                    labels: head.labels.clone(),
                                    properties: head.properties.clone(),
                                },
                            },
                        )?;
                        if !node_matched {
                            return Ok(false);
                        }
                        match row.get(var_id) {
                            Some(LoraValue::Node(id)) => *id,
                            _ => return Ok(false),
                        }
                    }
                } else {
                    return Ok(false);
                };

                let mut current_node_id = head_node_id;

                for step in chain {
                    let eval_ctx = EvalContext {
                        storage: &*self.ctx.storage,
                        params: &self.ctx.params,
                    };

                    let direction = step.rel.direction;

                    // Visit ID-only traversal candidates without allocating a
                    // transient edge Vec for each MERGE chain step.
                    let mut found = false;
                    let _ = self.ctx.storage.try_for_each_expand_id(
                        current_node_id,
                        direction,
                        &step.rel.types,
                        |rel_id, node_id| {
                            // Check target node labels and (optional) properties.
                            let node_ok = self
                                .ctx
                                .storage
                                .with_node(node_id, |node_rec| {
                                    if !node_matches_label_groups(
                                        &node_rec.labels,
                                        &step.node.labels,
                                    ) {
                                        return false;
                                    }
                                    if let Some(props_expr) = &step.node.properties {
                                        let expected = eval_expr(props_expr, row, &eval_ctx);
                                        if let LoraValue::Map(expected_map) = &expected {
                                            let all_match =
                                                expected_map.iter().all(|(key, expected_val)| {
                                                    node_rec
                                                        .properties
                                                        .get(key)
                                                        .map(|actual| {
                                                            value_matches_property_value(
                                                                expected_val,
                                                                actual,
                                                            )
                                                        })
                                                        .unwrap_or(false)
                                                });
                                            if !all_match {
                                                return false;
                                            }
                                        }
                                    }
                                    true
                                })
                                .unwrap_or(false);
                            if !node_ok {
                                return Ok::<(), ()>(());
                            }

                            // Check relationship properties.
                            let rel_ok = self
                                .ctx
                                .storage
                                .with_relationship(rel_id, |rel_rec| {
                                    if let Some(rel_props_expr) = &step.rel.properties {
                                        let expected = eval_expr(rel_props_expr, row, &eval_ctx);
                                        if let LoraValue::Map(expected_map) = &expected {
                                            let all_match =
                                                expected_map.iter().all(|(key, expected_val)| {
                                                    rel_rec
                                                        .properties
                                                        .get(key)
                                                        .map(|actual| {
                                                            value_matches_property_value(
                                                                expected_val,
                                                                actual,
                                                            )
                                                        })
                                                        .unwrap_or(false)
                                                });
                                            if !all_match {
                                                return false;
                                            }
                                        }
                                    }
                                    true
                                })
                                .unwrap_or(false);
                            if !rel_ok {
                                return Ok(());
                            }

                            // Match found — bind variables
                            if let Some(rel_var) = step.rel.var {
                                row.insert(rel_var, LoraValue::Relationship(rel_id));
                            }
                            if let Some(node_var) = step.node.var {
                                row.insert(node_var, LoraValue::Node(node_id));
                            }
                            current_node_id = node_id;
                            found = true;
                            Err(())
                        },
                    );

                    if !found {
                        return Ok(false);
                    }
                }

                Ok(true)
            }
        }
    }

    fn exec_delete(&mut self, plan: &PhysicalPlan, op: &DeleteExec) -> ExecResult<Vec<Row>> {
        if crate::pull::subtree_is_fully_streaming(plan, op.input) {
            let detach = op.detach;
            return self.streaming_apply(plan, op.input, |this, row| {
                for expr in &op.expressions {
                    let value = {
                        let eval_ctx = EvalContext {
                            storage: &*this.ctx.storage,
                            params: &this.ctx.params,
                        };
                        eval_expr(expr, row, &eval_ctx)
                    };
                    this.delete_value(value, detach)?;
                }
                Ok(())
            });
        }

        let input_rows = self.execute_node(plan, op.input)?;

        for row in &input_rows {
            for expr in &op.expressions {
                let value = {
                    let eval_ctx = EvalContext {
                        storage: &*self.ctx.storage,
                        params: &self.ctx.params,
                    };
                    eval_expr(expr, row, &eval_ctx)
                };

                self.delete_value(value, op.detach)?;
            }
        }

        Ok(input_rows)
    }

    fn exec_set(&mut self, plan: &PhysicalPlan, op: &SetExec) -> ExecResult<Vec<Row>> {
        if crate::pull::subtree_is_fully_streaming(plan, op.input) {
            return self.streaming_apply(plan, op.input, |this, row| {
                for item in &op.items {
                    this.apply_set_item(row, item)?;
                }
                Ok(())
            });
        }

        let input_rows = self.execute_node(plan, op.input)?;

        for row in &input_rows {
            for item in &op.items {
                self.apply_set_item(row, item)?;
            }
        }

        Ok(input_rows)
    }

    fn exec_remove(&mut self, plan: &PhysicalPlan, op: &RemoveExec) -> ExecResult<Vec<Row>> {
        if crate::pull::subtree_is_fully_streaming(plan, op.input) {
            return self.streaming_apply(plan, op.input, |this, row| {
                for item in &op.items {
                    this.apply_remove_item(row, item)?;
                }
                Ok(())
            });
        }

        let input_rows = self.execute_node(plan, op.input)?;

        for row in &input_rows {
            for item in &op.items {
                self.apply_remove_item(row, item)?;
            }
        }

        Ok(input_rows)
    }

    fn apply_set_item(&mut self, row: &Row, item: &ResolvedSetItem) -> ExecResult<()> {
        match item {
            ResolvedSetItem::SetProperty { target, value } => {
                let new_value = {
                    let eval_ctx = EvalContext {
                        storage: &*self.ctx.storage,
                        params: &self.ctx.params,
                    };
                    eval_expr(value, row, &eval_ctx)
                };

                self.set_property_from_expr(row, target, new_value)
            }

            ResolvedSetItem::SetVariable { variable, value } => {
                // Only need the entity's id — peek at the binding by reference.
                let entity_ref =
                    row.get(*variable)
                        .ok_or(ExecutorError::UnboundVariableForSet {
                            var: format!("{variable:?}"),
                        })?;
                let entity_target = entity_target_from_value(entity_ref)?;

                let new_value = {
                    let eval_ctx = EvalContext {
                        storage: &*self.ctx.storage,
                        params: &self.ctx.params,
                    };
                    eval_expr(value, row, &eval_ctx)
                };

                self.overwrite_entity_target(entity_target, new_value)
            }

            ResolvedSetItem::MutateVariable { variable, value } => {
                let entity_ref =
                    row.get(*variable)
                        .ok_or(ExecutorError::UnboundVariableForSet {
                            var: format!("{variable:?}"),
                        })?;
                let entity_target = entity_target_from_value(entity_ref)?;

                let patch = {
                    let eval_ctx = EvalContext {
                        storage: &*self.ctx.storage,
                        params: &self.ctx.params,
                    };
                    eval_expr(value, row, &eval_ctx)
                };

                self.mutate_entity_target(entity_target, patch)
            }

            ResolvedSetItem::SetLabels { variable, labels } => match row.get(*variable) {
                Some(LoraValue::Node(node_id)) => {
                    let node_id = *node_id;
                    for label in labels {
                        if let Err(msg) = self
                            .ctx
                            .storage
                            .check_node_add_label_against_constraints(node_id, label)
                        {
                            return Err(ExecutorError::ConstraintViolation(msg));
                        }
                        self.ctx.storage.add_node_label(node_id, label);
                    }
                    Ok(())
                }
                Some(other) => Err(ExecutorError::ExpectedNodeForSetLabels {
                    found: value_kind(other),
                }),
                None => Err(ExecutorError::UnboundVariableForSet {
                    var: format!("{variable:?}"),
                }),
            },
        }
    }

    fn set_property_from_expr(
        &mut self,
        row: &Row,
        target_expr: &ResolvedExpr,
        new_value: LoraValue,
    ) -> ExecResult<()> {
        let ResolvedExpr::Property { expr, property } = target_expr else {
            return Err(ExecutorError::UnsupportedSetTarget);
        };

        let owner = {
            let eval_ctx = EvalContext {
                storage: &*self.ctx.storage,
                params: &self.ctx.params,
            };
            eval_expr(expr, row, &eval_ctx)
        };

        match owner {
            LoraValue::Node(node_id) => {
                let prop = lora_value_to_property(new_value)
                    .map_err(|e| ExecutorError::RuntimeError(e.to_string()))?;
                if let Err(msg) = self
                    .ctx
                    .storage
                    .check_node_set_property_against_constraints(node_id, property, &prop)
                {
                    return Err(ExecutorError::ConstraintViolation(msg));
                }
                self.ctx
                    .storage
                    .set_node_property(node_id, property.clone(), prop);
                Ok(())
            }
            LoraValue::Relationship(rel_id) => {
                let prop = lora_value_to_property(new_value)
                    .map_err(|e| ExecutorError::RuntimeError(e.to_string()))?;
                if let Err(msg) = self
                    .ctx
                    .storage
                    .check_relationship_set_property_against_constraints(rel_id, property, &prop)
                {
                    return Err(ExecutorError::ConstraintViolation(msg));
                }
                self.ctx
                    .storage
                    .set_relationship_property(rel_id, property.clone(), prop);
                Ok(())
            }
            other => Err(ExecutorError::InvalidSetTarget {
                found: value_kind(&other),
            }),
        }
    }

    fn remove_property_from_expr(&mut self, row: &Row, expr: &ResolvedExpr) -> ExecResult<()> {
        let ResolvedExpr::Property {
            expr: owner_expr,
            property,
        } = expr
        else {
            return Err(ExecutorError::UnsupportedRemoveTarget);
        };

        let owner = {
            let eval_ctx = EvalContext {
                storage: &*self.ctx.storage,
                params: &self.ctx.params,
            };
            eval_expr(owner_expr, row, &eval_ctx)
        };

        match owner {
            LoraValue::Node(node_id) => {
                if let Err(msg) = self
                    .ctx
                    .storage
                    .check_node_remove_property_against_constraints(node_id, property)
                {
                    return Err(ExecutorError::ConstraintViolation(msg));
                }
                self.ctx.storage.remove_node_property(node_id, property);
                Ok(())
            }
            LoraValue::Relationship(rel_id) => {
                if let Err(msg) = self
                    .ctx
                    .storage
                    .check_relationship_remove_property_against_constraints(rel_id, property)
                {
                    return Err(ExecutorError::ConstraintViolation(msg));
                }
                self.ctx
                    .storage
                    .remove_relationship_property(rel_id, property);
                Ok(())
            }
            other => Err(ExecutorError::InvalidRemoveTarget {
                found: value_kind(&other),
            }),
        }
    }

    fn overwrite_entity_target(
        &mut self,
        target: EntityTarget,
        new_value: LoraValue,
    ) -> ExecResult<()> {
        let LoraValue::Map(map) = new_value else {
            return Err(ExecutorError::ExpectedPropertyMap {
                found: value_kind(&new_value),
            });
        };

        let mut props: Properties = Properties::new();
        for (k, v) in map {
            let prop = lora_value_to_property(v)
                .map_err(|e| ExecutorError::RuntimeError(e.to_string()))?;
            props.insert(k, prop);
        }

        match target {
            EntityTarget::Node(node_id) => {
                if let Err(msg) = self
                    .ctx
                    .storage
                    .check_node_replace_properties_against_constraints(node_id, &props)
                {
                    return Err(ExecutorError::ConstraintViolation(msg));
                }
                self.ctx.storage.replace_node_properties(node_id, props);
            }
            EntityTarget::Relationship(rel_id) => {
                if let Err(msg) = self
                    .ctx
                    .storage
                    .check_relationship_replace_properties_against_constraints(rel_id, &props)
                {
                    return Err(ExecutorError::ConstraintViolation(msg));
                }
                self.ctx
                    .storage
                    .replace_relationship_properties(rel_id, props);
            }
        }
        Ok(())
    }

    fn mutate_entity_target(
        &mut self,
        target: EntityTarget,
        patch_value: LoraValue,
    ) -> ExecResult<()> {
        let LoraValue::Map(map) = patch_value else {
            return Err(ExecutorError::ExpectedPropertyMap {
                found: value_kind(&patch_value),
            });
        };

        match target {
            EntityTarget::Node(node_id) => {
                for (k, v) in map {
                    let prop = lora_value_to_property(v)
                        .map_err(|e| ExecutorError::RuntimeError(e.to_string()))?;
                    if let Err(msg) = self
                        .ctx
                        .storage
                        .check_node_set_property_against_constraints(node_id, &k, &prop)
                    {
                        return Err(ExecutorError::ConstraintViolation(msg));
                    }
                    self.ctx.storage.set_node_property(node_id, k, prop);
                }
            }
            EntityTarget::Relationship(rel_id) => {
                for (k, v) in map {
                    let prop = lora_value_to_property(v)
                        .map_err(|e| ExecutorError::RuntimeError(e.to_string()))?;
                    if let Err(msg) = self
                        .ctx
                        .storage
                        .check_relationship_set_property_against_constraints(rel_id, &k, &prop)
                    {
                        return Err(ExecutorError::ConstraintViolation(msg));
                    }
                    self.ctx.storage.set_relationship_property(rel_id, k, prop);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn apply_create_pattern(
        &mut self,
        row: &mut Row,
        pattern: &ResolvedPattern,
    ) -> ExecResult<()> {
        for part in &pattern.parts {
            self.apply_create_pattern_part(row, part)?;
        }
        Ok(())
    }

    /// Apply a single per-row write for any of the streamable write
    /// operators (Create / Set / Delete / Remove / Merge). Used by
    /// the [`crate::pull::StreamingWriteCursor`] auto-commit fast
    /// path: the cursor pulls one input row from a read upstream,
    /// hands it here for the side effect, and emits the row back.
    pub(crate) fn apply_write_op(&mut self, op: &PhysicalOp, row: &mut Row) -> ExecResult<()> {
        match op {
            PhysicalOp::Create(c) => self.apply_create_pattern(row, &c.pattern),
            PhysicalOp::Set(s) => {
                for item in &s.items {
                    self.apply_set_item(row, item)?;
                }
                Ok(())
            }
            PhysicalOp::Delete(d) => {
                let detach = d.detach;
                for expr in &d.expressions {
                    let value = {
                        let eval_ctx = EvalContext {
                            storage: &*self.ctx.storage,
                            params: &self.ctx.params,
                        };
                        eval_expr(expr, row, &eval_ctx)
                    };
                    self.delete_value(value, detach)?;
                }
                Ok(())
            }
            PhysicalOp::Remove(r) => {
                for item in &r.items {
                    self.apply_remove_item(row, item)?;
                }
                Ok(())
            }
            PhysicalOp::Merge(m) => {
                let already_bound = self.pattern_part_is_bound(row, &m.pattern_part);
                let matched = if already_bound {
                    true
                } else {
                    self.try_match_merge_pattern(row, &m.pattern_part)?
                };
                if !matched {
                    self.apply_create_pattern_part(row, &m.pattern_part)?;
                }
                for action in &m.actions {
                    if action.on_match == matched {
                        for item in &action.set.items {
                            self.apply_set_item(row, item)?;
                        }
                    }
                }
                Ok(())
            }
            other => Err(ExecutorError::RuntimeError(format!(
                "apply_write_op called on non-write op: {other:?}"
            ))),
        }
    }

    fn apply_create_pattern_part(
        &mut self,
        row: &mut Row,
        part: &ResolvedPatternPart,
    ) -> ExecResult<()> {
        if part.binding.is_some() {
            trace!("create pattern part has path binding; path materialization not implemented");
        }

        let _ = self.apply_create_pattern_element(row, &part.element)?;
        Ok(())
    }

    fn apply_create_pattern_element(
        &mut self,
        row: &mut Row,
        element: &ResolvedPatternElement,
    ) -> ExecResult<Option<LoraValue>> {
        match element {
            ResolvedPatternElement::Node {
                var,
                labels,
                properties,
            } => {
                let node_id =
                    self.materialize_node_pattern(row, *var, labels, properties.as_ref())?;
                Ok(Some(LoraValue::Node(node_id)))
            }

            ResolvedPatternElement::NodeChain { head, chain } => {
                let mut current_node_id = self.materialize_node_pattern(
                    row,
                    head.var,
                    &head.labels,
                    head.properties.as_ref(),
                )?;

                for link in chain {
                    let next_node_id = self.materialize_node_pattern(
                        row,
                        link.node.var,
                        &link.node.labels,
                        link.node.properties.as_ref(),
                    )?;

                    let _ = self.materialize_relationship_pattern(
                        row,
                        current_node_id,
                        next_node_id,
                        &link.rel,
                    )?;

                    current_node_id = next_node_id;
                }

                Ok(Some(LoraValue::Node(current_node_id)))
            }

            ResolvedPatternElement::ShortestPath { .. } => {
                // ShortestPath is not valid in CREATE context
                Ok(None)
            }
        }
    }

    fn pattern_part_is_bound(&self, row: &Row, part: &ResolvedPatternPart) -> bool {
        match &part.element {
            ResolvedPatternElement::Node { var, .. } => var.and_then(|v| row.get(v)).is_some(),

            ResolvedPatternElement::ShortestPath { .. } => false,

            ResolvedPatternElement::NodeChain { head, chain } => {
                let head_ok = head.var.and_then(|v| row.get(v)).is_some();

                let chain_ok = chain.iter().all(|link| {
                    let node_ok = link.node.var.and_then(|v| row.get(v)).is_some();
                    // For MERGE, anonymous relationships cannot be considered
                    // "bound" because we have no variable to check. The merge
                    // must search the graph to see if the relationship exists.
                    let rel_ok = match link.rel.var {
                        Some(v) => row.get(v).is_some(),
                        None => false,
                    };
                    node_ok && rel_ok
                });

                head_ok && chain_ok
            }
        }
    }

    fn materialize_node_pattern(
        &mut self,
        row: &mut Row,
        var: Option<VarId>,
        labels: &[Vec<String>],
        properties: Option<&ResolvedExpr>,
    ) -> ExecResult<u64> {
        if let Some(var_id) = var {
            if let Some(LoraValue::Node(id)) = row.get(var_id) {
                return Ok(*id);
            }
        }

        let properties = match properties {
            Some(expr) => eval_properties_expr(expr, row, &*self.ctx.storage, &self.ctx.params)?,
            None => Properties::new(),
        };

        let flat_labels = flatten_label_groups(labels);
        debug!("creating node with labels={flat_labels:?}");
        if let Err(msg) = self
            .ctx
            .storage
            .check_node_create_against_constraints(&flat_labels, &properties)
        {
            return Err(ExecutorError::ConstraintViolation(msg));
        }
        let created = self.ctx.storage.create_node(flat_labels, properties);

        if let Some(var_id) = var {
            row.insert(var_id, LoraValue::Node(created.id));
        }

        Ok(created.id)
    }

    fn materialize_relationship_pattern(
        &mut self,
        row: &mut Row,
        left_node_id: u64,
        right_node_id: u64,
        rel: &lora_analyzer::ResolvedRel,
    ) -> ExecResult<u64> {
        if let Some(var_id) = rel.var {
            if let Some(LoraValue::Relationship(id)) = row.get(var_id) {
                let id = *id;
                if let Some((src, dst)) = self.ctx.storage.relationship_endpoints(id) {
                    let endpoints_match = match rel.direction {
                        Direction::Right | Direction::Undirected => {
                            src == left_node_id && dst == right_node_id
                        }
                        Direction::Left => src == right_node_id && dst == left_node_id,
                    };

                    if endpoints_match {
                        return Ok(id);
                    }
                }
            }
        }

        if rel.range.is_some() {
            return Err(ExecutorError::UnsupportedCreateRelationshipRange);
        }

        let (src, dst) = match rel.direction {
            Direction::Right | Direction::Undirected => (left_node_id, right_node_id),
            Direction::Left => (right_node_id, left_node_id),
        };

        let rel_type = rel
            .types
            .first()
            .ok_or(ExecutorError::MissingRelationshipType)?;

        if rel_type.is_empty() {
            return Err(ExecutorError::MissingRelationshipType);
        }

        let properties = match rel.properties.as_ref() {
            Some(expr) => eval_properties_expr(expr, row, &*self.ctx.storage, &self.ctx.params)?,
            None => Properties::new(),
        };

        debug!("creating relationship: src={src}, dst={dst}, type={rel_type}");

        if let Err(msg) = self
            .ctx
            .storage
            .check_relationship_create_against_constraints(rel_type, &properties)
        {
            return Err(ExecutorError::ConstraintViolation(msg));
        }

        let created = self
            .ctx
            .storage
            .create_relationship(src, dst, rel_type, properties)
            .ok_or_else(|| ExecutorError::RelationshipCreateFailed {
                src,
                dst,
                rel_type: rel_type.clone(),
            })?;

        if let Some(var_id) = rel.var {
            row.insert(var_id, LoraValue::Relationship(created.id));
        }

        Ok(created.id)
    }
}
