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

use std::collections::{BTreeMap, BTreeSet};
use tracing::{debug, error, trace};
use web_time::Instant;

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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DeleteTarget {
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
    /// The row a writing `CALL { ... }` body's bottom `Argument` yields:
    /// the outer row it runs for. `None` outside such a body.
    argument_seed: Option<Row>,
    /// When set, existence constraints on created entities are checked
    /// once the statement finishes rather than at `CREATE`, so a later
    /// `SET` (or `ON CREATE SET`) in the same statement can supply the
    /// property. See [`plan_defers_existence`].
    defer_existence: bool,
    /// Entities created while `defer_existence` is on, still to check.
    pending_existence: Vec<EntityTarget>,
}

impl<'a, S: GraphStorageMut> MutableExecutor<'a, S> {
    pub fn new(ctx: MutableExecutionContext<'a, S>) -> Self {
        Self {
            ctx,
            deadline: None,
            argument_seed: None,
            defer_existence: false,
            pending_existence: Vec::new(),
        }
    }

    pub fn with_deadline(ctx: MutableExecutionContext<'a, S>, deadline: Option<Instant>) -> Self {
        Self {
            ctx,
            deadline,
            argument_seed: None,
            defer_existence: false,
            pending_existence: Vec::new(),
        }
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
        let _deadline_scope = crate::cancel::DeadlineScope::enter(self.deadline);
        let rows = self.execute_rows(plan)?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    pub fn execute_rows(&mut self, plan: &PhysicalPlan) -> ExecResult<Vec<Row>> {
        self.defer_existence = plan_defers_existence(plan);
        let rows = self.execute_plan_rows(plan)?;
        self.check_pending_existence()?;
        Ok(rows)
    }

    /// Defer existence checks on created entities to the end of the
    /// statement (see [`plan_defers_existence`]); the caller then runs
    /// [`Self::check_pending_existence`].
    pub(crate) fn defer_existence_checks(&mut self, defer: bool) {
        self.defer_existence = defer;
    }

    /// Check the existence constraints deferred so far, clearing them.
    pub(crate) fn check_pending_existence(&mut self) -> ExecResult<()> {
        for target in std::mem::take(&mut self.pending_existence) {
            let checked = match target {
                EntityTarget::Node(id) => self.ctx.storage.check_node_existence_constraints(id),
                EntityTarget::Relationship(id) => self
                    .ctx
                    .storage
                    .check_relationship_existence_constraints(id),
            };
            checked.map_err(ExecutorError::ConstraintViolation)?;
        }
        Ok(())
    }

    fn execute_plan_rows(&mut self, plan: &PhysicalPlan) -> ExecResult<Vec<Row>> {
        let _deadline_scope = crate::cancel::DeadlineScope::enter(self.deadline);
        self.check_deadline()?;
        // Clear any error residue that a previous query on this thread may have
        // left in the thread-local eval-error slot.
        clear_eval_error();

        let rows = self.execute_node(plan, plan.root)?;
        if plan_ends_in_write(plan) {
            return Ok(Vec::new());
        }
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
        let _deadline_scope = crate::cancel::DeadlineScope::enter(self.deadline);
        let rows = self.execute_compiled_rows(compiled)?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    pub fn execute_compiled_rows(&mut self, compiled: &CompiledQuery) -> ExecResult<Vec<Row>> {
        let _deadline_scope = crate::cancel::DeadlineScope::enter(self.deadline);
        self.check_deadline()?;
        self.defer_existence = plan_defers_existence(&compiled.physical)
            || !compiled.unions.is_empty()
                && compiled
                    .unions
                    .iter()
                    .any(|b| plan_defers_existence(&b.physical));
        if compiled.unions.is_empty() {
            let rows = self.execute_plan_rows(&compiled.physical)?;
            self.check_pending_existence()?;
            return Ok(rows);
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

        self.check_pending_existence()?;
        Ok(all_rows)
    }

    fn execute_and_hydrate(&mut self, plan: &PhysicalPlan) -> ExecResult<Vec<Row>> {
        self.check_deadline()?;
        let rows = self.execute_node(plan, plan.root)?;
        if plan_ends_in_write(plan) {
            return Ok(Vec::new());
        }
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
            PhysicalOp::Foreach(op) => self.exec_foreach(plan, op),
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
        Ok(vec![self.argument_seed.clone().unwrap_or_default()])
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

        unwind_rows(input_rows, op, &eval_ctx)
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

        if super::optional::optional_can_correlate(plan, op.inner) {
            let storage_ref: &S = &*self.ctx.storage;
            return super::optional::correlated_optional_match_rows(
                storage_ref,
                &self.ctx.params,
                plan,
                op.inner,
                input_rows,
                &op.new_vars,
            );
        }

        // Fallback: execute the inner plan once, uncorrelated, and join.
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

        if crate::pull::subtree_has_write(plan, op.inner) {
            // A writing body runs on this executor, once per outer row,
            // with the outer row seeded into its bottom `Argument`. Each
            // run sees the writes of the runs before it.
            let unit = op.new_vars.is_empty();
            for outer_row in input_rows {
                self.check_deadline()?;
                let prev = self.argument_seed.replace(outer_row.clone());
                let inner_rows = self.execute_node(plan, op.inner);
                self.argument_seed = prev;
                let inner_rows = inner_rows?;
                if unit {
                    // A unit subquery keeps the outer row as it is, once,
                    // however many rows its body produced.
                    out.push(outer_row);
                    continue;
                }
                for inner_row in inner_rows {
                    out.push(crate::executor::merge_optional_rows(&outer_row, &inner_row));
                }
            }
            return Ok(out);
        }

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
        // Inside a writing `CALL { ... }` body the input's bottom
        // `Argument` yields the outer row.
        let mut upstream = match self.argument_seed.clone() {
            Some(seed) => {
                crate::pull::build_streaming_seeded(plan, input, storage_ref, params, seed)?
            }
            None => crate::pull::build_streaming(plan, input, storage_ref, params)?,
        };

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

    fn collect_delete_targets(
        &self,
        value: &LoraValue,
        targets: &mut BTreeSet<DeleteTarget>,
    ) -> ExecResult<()> {
        match value {
            LoraValue::Null => Ok(()),

            LoraValue::Node(node_id) => {
                targets.insert(DeleteTarget::Node(*node_id));
                Ok(())
            }

            LoraValue::Relationship(rel_id) => {
                targets.insert(DeleteTarget::Relationship(*rel_id));
                Ok(())
            }

            LoraValue::List(values) => {
                for v in values {
                    self.collect_delete_targets(v, targets)?;
                }
                Ok(())
            }

            other => Err(ExecutorError::InvalidDeleteTarget {
                found: value_kind(other),
            }),
        }
    }

    fn validate_delete_targets(
        &self,
        targets: &BTreeSet<DeleteTarget>,
        detach: bool,
    ) -> ExecResult<()> {
        for target in targets {
            match target {
                DeleteTarget::Relationship(rel_id) => {
                    if !self.ctx.storage.contains_relationship(*rel_id) {
                        return Err(ExecutorError::DeleteRelationshipFailed { rel_id: *rel_id });
                    }
                }
                DeleteTarget::Node(node_id) if !detach => {
                    if !self.ctx.storage.contains_node(*node_id) {
                        return Err(ExecutorError::DeleteNodeWithRelationships {
                            node_id: *node_id,
                        });
                    }
                    let has_external_relationship = self
                        .ctx
                        .storage
                        .relationship_ids_of(*node_id, Direction::Undirected)
                        .into_iter()
                        .any(|rel_id| !targets.contains(&DeleteTarget::Relationship(rel_id)));
                    if has_external_relationship {
                        return Err(ExecutorError::DeleteNodeWithRelationships {
                            node_id: *node_id,
                        });
                    }
                }
                DeleteTarget::Node(_) => {}
            }
        }
        Ok(())
    }

    fn delete_target(&mut self, target: DeleteTarget, detach: bool) -> ExecResult<()> {
        match target {
            DeleteTarget::Node(node_id) => {
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
            DeleteTarget::Relationship(rel_id) => {
                let ok = self.ctx.storage.delete_relationship(rel_id);
                if ok {
                    Ok(())
                } else {
                    Err(ExecutorError::DeleteRelationshipFailed { rel_id })
                }
            }
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
    /// pattern. If found, bind its variables in the row and return true.
    /// On a miss the row is left untouched, so the create path sees only
    /// the variables that were bound before the MERGE.
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
                let expected_props = self.merge_expected_props(properties.as_ref(), row);
                let Some(id) = self
                    .merge_node_candidates(labels, &expected_props)
                    .into_iter()
                    .find(|&id| self.merge_node_matches(id, labels, &expected_props))
                else {
                    return Ok(false);
                };
                if let Some(var_id) = var {
                    row.insert(*var_id, LoraValue::Node(id));
                }
                Ok(true)
            }

            ResolvedPatternElement::ShortestPath { .. } => {
                // ShortestPath is not valid in MERGE context
                Ok(false)
            }

            ResolvedPatternElement::NodeChain { head, chain } => {
                // The head is usually bound by an earlier clause; otherwise
                // every node matching it is a possible start.
                let head_candidates = match head.var.and_then(|v| row.get(v)) {
                    Some(LoraValue::Node(id)) => vec![*id],
                    _ => {
                        let expected = self.merge_expected_props(head.properties.as_ref(), row);
                        self.merge_node_candidates(&head.labels, &expected)
                            .into_iter()
                            .filter(|&id| self.merge_node_matches(id, &head.labels, &expected))
                            .collect()
                    }
                };

                for head_id in head_candidates {
                    let mut trial = row.clone();
                    if let Some(var_id) = head.var {
                        trial.insert(var_id, LoraValue::Node(head_id));
                    }
                    let mut used_rels = Vec::with_capacity(chain.len());
                    if self.match_merge_chain(&mut trial, head_id, chain, &mut used_rels) {
                        *row = trial;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    /// Match `chain` from `current`, backtracking over every candidate
    /// edge. A step node or relationship already bound in the row (by an
    /// earlier clause or earlier in the chain) must be the one reached;
    /// the same relationship is never used twice in one pattern.
    fn match_merge_chain(
        &self,
        row: &mut Row,
        current: NodeId,
        chain: &[lora_analyzer::ResolvedChain],
        used_rels: &mut Vec<u64>,
    ) -> bool {
        let Some((step, rest)) = chain.split_first() else {
            return true;
        };

        let bound_dst = match step.node.var.and_then(|v| row.get(v)) {
            Some(LoraValue::Node(id)) => Some(*id),
            _ => None,
        };
        let bound_rel = match step.rel.var.and_then(|v| row.get(v)) {
            Some(LoraValue::Relationship(id)) => Some(*id),
            _ => None,
        };
        let expected_node = self.merge_expected_props(step.node.properties.as_ref(), row);
        let expected_rel = self.merge_expected_props(step.rel.properties.as_ref(), row);

        let edges = self
            .ctx
            .storage
            .expand_ids(current, step.rel.direction, &step.rel.types);
        for (rel_id, node_id) in edges {
            if bound_dst.is_some_and(|id| id != node_id)
                || bound_rel.is_some_and(|id| id != rel_id)
                || used_rels.contains(&rel_id)
            {
                continue;
            }
            if !self.merge_node_matches(node_id, &step.node.labels, &expected_node) {
                continue;
            }
            if let Some(LoraValue::Map(expected_map)) = &expected_rel {
                let rel_ok = self
                    .ctx
                    .storage
                    .with_relationship(rel_id, |rel_rec| {
                        expected_map.iter().all(|(key, expected_val)| {
                            rel_rec
                                .properties
                                .get(key.as_str())
                                .map(|actual| value_matches_property_value(expected_val, actual))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);
                if !rel_ok {
                    continue;
                }
            }

            let mut next = row.clone();
            if let Some(rel_var) = step.rel.var {
                next.insert(rel_var, LoraValue::Relationship(rel_id));
            }
            if let Some(node_var) = step.node.var {
                next.insert(node_var, LoraValue::Node(node_id));
            }
            used_rels.push(rel_id);
            if self.match_merge_chain(&mut next, node_id, rest, used_rels) {
                *row = next;
                return true;
            }
            used_rels.pop();
        }
        false
    }

    fn merge_expected_props(
        &self,
        properties: Option<&ResolvedExpr>,
        row: &Row,
    ) -> Option<LoraValue> {
        let eval_ctx = EvalContext {
            storage: &*self.ctx.storage,
            params: &self.ctx.params,
        };
        properties.map(|e| eval_expr(e, row, &eval_ctx))
    }

    /// Candidate ids for a MERGE node pattern. `MERGE (n:L {key: $k})`
    /// looks the key up in the property index instead of scanning every
    /// `:L` node, so an upsert costs the same on a large label as on a
    /// small one. Candidates are re-checked by [`Self::merge_node_matches`].
    fn merge_node_candidates(
        &self,
        labels: &[Vec<String>],
        expected_props: &Option<LoraValue>,
    ) -> Vec<NodeId> {
        let indexed = match expected_props {
            Some(LoraValue::Map(expected)) => {
                merge_candidates_from_index(&*self.ctx.storage, labels, expected)
            }
            _ => None,
        };
        match indexed {
            Some(ids) => ids,
            None if labels.is_empty() => self.ctx.storage.all_node_ids(),
            None => scan_node_ids_for_label_groups(&*self.ctx.storage, labels),
        }
    }

    fn merge_node_matches(
        &self,
        id: NodeId,
        labels: &[Vec<String>],
        expected_props: &Option<LoraValue>,
    ) -> bool {
        self.ctx
            .storage
            .with_node(id, |node| {
                if !node_matches_label_groups(&node.labels, labels) {
                    return false;
                }
                if let Some(LoraValue::Map(expected)) = expected_props {
                    return expected.iter().all(|(key, expected_value)| {
                        node.properties
                            .get(key.as_str())
                            .map(|actual| value_matches_property_value(expected_value, actual))
                            .unwrap_or(false)
                    });
                }
                true
            })
            .unwrap_or(false)
    }

    fn exec_delete(&mut self, plan: &PhysicalPlan, op: &DeleteExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let mut targets = BTreeSet::new();

        for row in &input_rows {
            for expr in &op.expressions {
                let value = {
                    let eval_ctx = EvalContext {
                        storage: &*self.ctx.storage,
                        params: &self.ctx.params,
                    };
                    eval_expr(expr, row, &eval_ctx)
                };
                self.collect_delete_targets(&value, &mut targets)?;
            }
        }

        self.validate_delete_targets(&targets, op.detach)?;

        for target in &targets {
            if let DeleteTarget::Relationship(_) = target {
                self.delete_target(*target, op.detach)?;
            }
        }
        for target in targets {
            if let DeleteTarget::Node(_) = target {
                self.delete_target(target, op.detach)?;
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

    /// `FOREACH (var IN list | body...)` — for each input row, evaluate
    /// the list and run the body once per element with `var` bound to
    /// that element. Each iteration runs on a fresh clone of the row
    /// so any new bindings the body introduces (e.g. anonymous
    /// `CREATE` node VarIds) don't leak between iterations or back to
    /// the outer scope. Side effects on the graph persist; the outer
    /// row is emitted unchanged.
    fn exec_foreach(&mut self, plan: &PhysicalPlan, op: &ForeachExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let mut out = Vec::with_capacity(input_rows.len());

        for row in input_rows {
            let list_value = {
                let eval_ctx = EvalContext {
                    storage: &*self.ctx.storage,
                    params: &self.ctx.params,
                };
                eval_expr(&op.list, &row, &eval_ctx)
            };

            let elements: Vec<LoraValue> = match list_value {
                LoraValue::List(items) => items,
                LoraValue::Null => Vec::new(),
                other => {
                    return Err(ExecutorError::RuntimeError(format!(
                        "FOREACH expects a list, got {}",
                        value_kind(&other)
                    )));
                }
            };

            for element in elements {
                // Fresh row per iteration so body-introduced bindings
                // don't reuse VarIds across iterations.
                let mut iter_row = row.clone();
                iter_row.insert(op.variable, element);
                for clause in &op.body {
                    self.apply_foreach_body_clause(&mut iter_row, clause)?;
                }
            }

            out.push(row);
        }

        Ok(out)
    }

    /// Apply one resolved updating clause to `row` for its side effect
    /// inside a `FOREACH` body. Only updating clauses (Create / Merge /
    /// Delete / Set / Remove / nested Foreach) are legal here; the
    /// analyzer guarantees that.
    fn apply_foreach_body_clause(
        &mut self,
        row: &mut Row,
        clause: &lora_analyzer::ResolvedClause,
    ) -> ExecResult<()> {
        use lora_analyzer::ResolvedClause;
        match clause {
            ResolvedClause::Create(c) => self.apply_create_pattern(row, &c.pattern),
            ResolvedClause::Set(s) => {
                for item in &s.items {
                    self.apply_set_item(row, item)?;
                }
                Ok(())
            }
            ResolvedClause::Remove(r) => {
                for item in &r.items {
                    self.apply_remove_item(row, item)?;
                }
                Ok(())
            }
            ResolvedClause::Delete(d) => {
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
            ResolvedClause::Merge(m) => {
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
            ResolvedClause::Foreach(nested) => {
                let list_value = {
                    let eval_ctx = EvalContext {
                        storage: &*self.ctx.storage,
                        params: &self.ctx.params,
                    };
                    eval_expr(&nested.list, row, &eval_ctx)
                };

                let elements: Vec<LoraValue> = match list_value {
                    LoraValue::List(items) => items,
                    LoraValue::Null => Vec::new(),
                    other => {
                        return Err(ExecutorError::RuntimeError(format!(
                            "FOREACH expects a list, got {}",
                            value_kind(&other)
                        )));
                    }
                };

                for element in elements {
                    let mut iter_row = row.clone();
                    iter_row.insert(nested.variable, element);
                    for inner in &nested.body {
                        self.apply_foreach_body_clause(&mut iter_row, inner)?;
                    }
                }

                Ok(())
            }
            other => Err(ExecutorError::RuntimeError(format!(
                "FOREACH body may only contain updating clauses, got {:?}",
                std::mem::discriminant(other)
            ))),
        }
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

        // `SET n.a = null` removes the property.
        if matches!(new_value, LoraValue::Null) {
            return match owner {
                LoraValue::Node(node_id) => {
                    self.remove_entity_property(EntityTarget::Node(node_id), property)
                }
                LoraValue::Relationship(rel_id) => {
                    self.remove_entity_property(EntityTarget::Relationship(rel_id), property)
                }
                other => Err(ExecutorError::InvalidSetTarget {
                    found: value_kind(&other),
                }),
            };
        }

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

    /// Remove one property, checking constraints first. Removing a
    /// property the entity does not have is a no-op.
    fn remove_entity_property(&mut self, target: EntityTarget, property: &str) -> ExecResult<()> {
        match target {
            EntityTarget::Node(node_id) => {
                if let Err(msg) = self
                    .ctx
                    .storage
                    .check_node_remove_property_against_constraints(node_id, property)
                {
                    return Err(ExecutorError::ConstraintViolation(msg));
                }
                self.ctx.storage.remove_node_property(node_id, property);
            }
            EntityTarget::Relationship(rel_id) => {
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
            }
        }
        Ok(())
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
            // `SET n = {a: null}` leaves `a` absent.
            if matches!(v, LoraValue::Null) {
                continue;
            }
            let prop = lora_value_to_property(v)
                .map_err(|e| ExecutorError::RuntimeError(e.to_string()))?;
            props.insert(lora_store::intern_owned(k), prop);
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
                    // `SET n += {a: null}` removes `a`.
                    if matches!(v, LoraValue::Null) {
                        self.remove_entity_property(target, &k)?;
                        continue;
                    }
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
                    if matches!(v, LoraValue::Null) {
                        self.remove_entity_property(target, &k)?;
                        continue;
                    }
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
        let checked = if self.defer_existence {
            self.ctx
                .storage
                .check_node_create_deferring_existence(&flat_labels, &properties)
        } else {
            self.ctx
                .storage
                .check_node_create_against_constraints(&flat_labels, &properties)
        };
        checked.map_err(ExecutorError::ConstraintViolation)?;
        let created = self
            .ctx
            .storage
            .try_create_node(flat_labels, properties)
            .ok_or(ExecutorError::NodeCreateFailed)?;
        if self.defer_existence {
            self.pending_existence.push(EntityTarget::Node(created.id));
        }

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

        let checked = if self.defer_existence {
            self.ctx
                .storage
                .check_relationship_create_deferring_existence(rel_type, &properties)
        } else {
            self.ctx
                .storage
                .check_relationship_create_against_constraints(rel_type, &properties)
        };
        checked.map_err(ExecutorError::ConstraintViolation)?;

        let created = self
            .ctx
            .storage
            .create_relationship(src, dst, rel_type, properties)
            .ok_or_else(|| ExecutorError::RelationshipCreateFailed {
                src,
                dst,
                rel_type: rel_type.clone(),
            })?;
        if self.defer_existence {
            self.pending_existence
                .push(EntityTarget::Relationship(created.id));
        }

        if let Some(var_id) = rel.var {
            row.insert(var_id, LoraValue::Relationship(created.id));
        }

        Ok(created.id)
    }
}

/// Whether existence constraints on entities a plan creates must wait
/// for the end of the statement. They can be checked at `CREATE` only
/// when nothing after it can add a property: every write is a `CREATE`
/// or a `DELETE`, with at most one `CREATE`. Checking early keeps a
/// failing create from mutating anything, which the in-place write path
/// relies on.
pub(crate) fn plan_defers_existence(plan: &PhysicalPlan) -> bool {
    let mut creates = 0;
    for op in &plan.nodes {
        match op {
            PhysicalOp::Create(_) => creates += 1,
            PhysicalOp::Delete(_) => {}
            PhysicalOp::Merge(_)
            | PhysicalOp::Set(_)
            | PhysicalOp::Remove(_)
            | PhysicalOp::Foreach(_) => return true,
            _ => {}
        }
    }
    creates > 1
}

/// Whether a plan is a write statement with no `RETURN` (its root is the
/// write operator itself). Such a statement produces no result rows, as
/// in other Cypher databases; the write operator's pass-through rows
/// would otherwise leak as anonymous `_0` columns carrying internal ids.
pub(crate) fn plan_ends_in_write(plan: &PhysicalPlan) -> bool {
    match &plan.nodes[plan.root] {
        PhysicalOp::Create(_)
        | PhysicalOp::Merge(_)
        | PhysicalOp::Set(_)
        | PhysicalOp::Delete(_)
        | PhysicalOp::Remove(_)
        | PhysicalOp::Foreach(_) => true,
        // A query ending in a unit `CALL { ... }` returns no rows.
        PhysicalOp::CallSubquery(op) => op.new_vars.is_empty(),
        _ => false,
    }
}

/// Candidate nodes for a MERGE node pattern from the property index, or
/// `None` to fall back to a label scan. Every candidate is still checked
/// against the full pattern, so the only requirement is that no real
/// match is missed. MERGE compares `1` and `1.0` as equal while the index
/// keys them apart, so numbers look up both images; values without an
/// exact index image (lists, maps, NaN, floats beyond 2^53) scan.
fn merge_candidates_from_index<S: lora_store::GraphStorage>(
    storage: &S,
    labels: &[Vec<String>],
    expected: &std::collections::BTreeMap<String, LoraValue>,
) -> Option<Vec<lora_store::NodeId>> {
    use lora_store::PropertyValue;

    // A single required label scopes the lookup; otherwise look up
    // across labels and let the pattern check filter.
    let label = match labels {
        [group] if group.len() == 1 => Some(group[0].as_str()),
        _ => None,
    };
    let (key, value) = expected.iter().find(|(_, v)| {
        matches!(
            v,
            LoraValue::String(_) | LoraValue::Bool(_) | LoraValue::Int(_)
        ) || matches!(v, LoraValue::Float(f) if f.is_finite() && f.abs() < 9_007_199_254_740_992.0)
    })?;
    let images: Vec<PropertyValue> = match value {
        LoraValue::String(s) => vec![PropertyValue::String(s.clone())],
        LoraValue::Bool(b) => vec![PropertyValue::Bool(*b)],
        LoraValue::Int(i) => vec![PropertyValue::Int(*i), PropertyValue::Float(*i as f64)],
        LoraValue::Float(f) => {
            let mut v = vec![PropertyValue::Float(*f)];
            if f.fract() == 0.0 {
                v.push(PropertyValue::Int(*f as i64));
            }
            v
        }
        _ => return None,
    };
    let mut ids: Vec<lora_store::NodeId> = images
        .iter()
        .flat_map(|image| storage.find_node_ids_by_property(label, key, image))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    Some(ids)
}
