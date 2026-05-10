//! Read-only buffered executor: lower a [`PhysicalPlan`] into a fully
//! materialized `Vec<Row>` without touching the store.
//!
//! [`Executor`] mirrors the operator set of the streaming pipeline in
//! `crate::pull` so write operators can fall back to it for subtrees
//! that are not fully streamable. Aggregation and DISTINCT projection
//! reuse the streaming `StreamableAggSpec` / `AggState` machinery
//! (re-exported as `crate::pull::*` for that purpose) on the
//! fold-only fast path; everything else materializes.

use crate::errors::{ExecResult, ExecutorError};
use crate::eval::{clear_eval_error, EvalContext};
#[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
use crate::eval::{eval_expr, eval_expr_result, eval_truthy_result};
use crate::value::{LoraValue, Row};
use crate::{project_rows, ExecuteOptions, QueryResult};

use lora_compiler::physical::*;
use lora_compiler::CompiledQuery;
use lora_store::GraphStorage;

use std::collections::BTreeMap;
use std::time::Instant;
use tracing::{error, trace};

use super::aggregate_rows;
use super::helpers::{
    build_path_value, check_deadline_at, dedup_rows, expand_rows, expand_var_len_rows,
    filter_rows_checked, filter_shortest_paths, hydrate_node_record, hydrate_relationship_record,
    limit_rows, node_by_label_scan_rows, node_by_property_scan_rows, node_scan_rows,
    project_rows_checked, unwind_rows,
};
#[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
use super::helpers::{
    dedup_rows_by_vars, indexed_node_property_candidates, label_group_candidates_prefiltered,
    node_matches_label_groups, node_matches_property_filter, scan_node_ids_for_label_groups,
};
use super::optional_match_rows;
use super::sort_rows_with_top_k;

#[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
const PARALLEL_ROW_THRESHOLD: usize = 20_000;

pub struct ExecutionContext<'a, S: GraphStorage> {
    pub storage: &'a S,
    pub params: BTreeMap<String, LoraValue>,
}

pub struct Executor<'a, S: GraphStorage> {
    ctx: ExecutionContext<'a, S>,
    deadline: Option<Instant>,
}

impl<'a, S: GraphStorage> Executor<'a, S> {
    pub fn new(ctx: ExecutionContext<'a, S>) -> Self {
        Self {
            ctx,
            deadline: None,
        }
    }

    pub fn with_deadline(ctx: ExecutionContext<'a, S>, deadline: Option<Instant>) -> Self {
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
}

impl<'a, S: GraphStorage> Executor<'a, S> {
    pub fn execute(
        &self,
        plan: &PhysicalPlan,
        options: Option<ExecuteOptions>,
    ) -> ExecResult<QueryResult> {
        let rows = self.execute_rows(plan)?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    pub fn execute_compiled(
        &self,
        compiled: &CompiledQuery,
        options: Option<ExecuteOptions>,
    ) -> ExecResult<QueryResult> {
        let rows = self.execute_compiled_rows(compiled)?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    pub fn execute_compiled_rows(&self, compiled: &CompiledQuery) -> ExecResult<Vec<Row>> {
        self.check_deadline()?;
        if compiled.unions.is_empty() {
            return self.execute_rows(&compiled.physical);
        }

        clear_eval_error();

        let mut all_rows = self.execute_rows(&compiled.physical)?;
        let mut needs_dedup = false;

        for branch in &compiled.unions {
            self.check_deadline()?;
            let branch_rows = self.execute_rows(&branch.physical)?;
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

    /// Execute a compiled read-only query, using rayon only for the narrow
    /// materialized-safe subset (scan/filter/projection) and only above a
    /// measured threshold. Smaller plans and unsupported operators fall back
    /// to the normal buffered executor.
    pub fn execute_compiled_rows_parallel_safe(
        &self,
        compiled: &CompiledQuery,
    ) -> ExecResult<Vec<Row>>
    where
        S: Sync,
    {
        #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
        {
            self.check_deadline()?;
            if compiled.unions.is_empty() && plan_is_parallel_safe(&compiled.physical) {
                return self.execute_rows_parallel_safe(&compiled.physical);
            }
        }

        self.execute_compiled_rows(compiled)
    }

    pub fn execute_rows(&self, plan: &PhysicalPlan) -> ExecResult<Vec<Row>> {
        self.check_deadline()?;
        // Clear any error residue that a previous query on this thread may have
        // left in the thread-local eval-error slot.
        clear_eval_error();

        let rows = self.execute_node(plan, plan.root)?;
        Ok(rows
            .into_iter()
            .map(|row| self.hydrate_row(row))
            .collect::<Vec<_>>())
    }

    fn hydrate_row(&self, row: Row) -> Row {
        let mut out = Row::new();

        for (var, name, value) in row.into_iter_named() {
            out.insert_named(var, name, self.hydrate_value(value));
        }

        out
    }

    /// Buffered execution of an arbitrary subplan. Public to the
    /// crate so the pull pipeline can fall back to materialized
    /// execution for operators that have no streaming source yet.
    pub(crate) fn execute_subtree(
        &self,
        plan: &PhysicalPlan,
        node_id: PhysicalNodeId,
    ) -> ExecResult<Vec<Row>> {
        self.execute_node(plan, node_id)
    }

    fn execute_node(&self, plan: &PhysicalPlan, node_id: PhysicalNodeId) -> ExecResult<Vec<Row>> {
        self.check_deadline()?;
        trace!("read-only execute_node start: node_id={node_id:?}");

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
            PhysicalOp::OptionalMatch(op) => self.exec_optional_match(plan, op),
            PhysicalOp::PathBuild(op) => self.exec_path_build(plan, op),
            PhysicalOp::Create(_) => Err(ExecutorError::ReadOnlyCreate { node_id }),
            PhysicalOp::Merge(_) => Err(ExecutorError::ReadOnlyMerge { node_id }),
            PhysicalOp::Delete(_) => Err(ExecutorError::ReadOnlyDelete { node_id }),
            PhysicalOp::Set(_) => Err(ExecutorError::ReadOnlySet { node_id }),
            PhysicalOp::Remove(_) => Err(ExecutorError::ReadOnlyRemove { node_id }),
        };

        match &result {
            Ok(rows) => trace!(
                "read-only execute_node ok: node_id={node_id:?}, rows={}",
                rows.len()
            ),
            Err(err) => error!("read-only execute_node failed: node_id={node_id:?}, error={err}"),
        }

        result
    }

    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    fn execute_rows_parallel_safe(&self, plan: &PhysicalPlan) -> ExecResult<Vec<Row>>
    where
        S: Sync,
    {
        self.check_deadline()?;
        clear_eval_error();

        let rows = self.execute_node_parallel_safe(plan, plan.root)?;
        if rows.len() < PARALLEL_ROW_THRESHOLD {
            return Ok(rows
                .into_iter()
                .map(|row| self.hydrate_row(row))
                .collect::<Vec<_>>());
        }

        use rayon::prelude::*;
        Ok(rows
            .into_par_iter()
            .map(|row| self.hydrate_row(row))
            .collect::<Vec<_>>())
    }

    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    fn execute_node_parallel_safe(
        &self,
        plan: &PhysicalPlan,
        node_id: PhysicalNodeId,
    ) -> ExecResult<Vec<Row>>
    where
        S: Sync,
    {
        self.check_deadline()?;
        match &plan.nodes[node_id] {
            PhysicalOp::Argument(op) => self.exec_argument(op),
            PhysicalOp::NodeScan(op) => self.exec_node_scan_parallel_safe(plan, op),
            PhysicalOp::NodeByLabelScan(op) => self.exec_node_by_label_scan_parallel_safe(plan, op),
            PhysicalOp::NodeByPropertyScan(op) => {
                self.exec_node_by_property_scan_parallel_safe(plan, op)
            }
            PhysicalOp::Filter(op) => self.exec_filter_parallel_safe(plan, op),
            PhysicalOp::Projection(op) => self.exec_projection_parallel_safe(plan, op),
            _ => unreachable!("parallel-safe executor called with unsupported operator"),
        }
    }

    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    fn exec_node_scan_parallel_safe(
        &self,
        plan: &PhysicalPlan,
        op: &NodeScanExec,
    ) -> ExecResult<Vec<Row>>
    where
        S: Sync,
    {
        let base_rows = match op.input {
            Some(input) => self.execute_node_parallel_safe(plan, input)?,
            None => vec![Row::new()],
        };
        let node_ids = self.ctx.storage.all_node_ids();
        if base_rows.len().saturating_mul(node_ids.len()) < PARALLEL_ROW_THRESHOLD {
            return node_scan_rows(self.ctx.storage, base_rows, op, self.deadline);
        }

        use rayon::prelude::*;
        if base_rows.len() == 1 {
            let row = base_rows.into_iter().next().expect("len checked above");
            if let Some(existing_id) = super::helpers::bound_node_id_for_expand(&row, op.var)? {
                return Ok(if self.ctx.storage.has_node(existing_id) {
                    vec![row]
                } else {
                    Vec::new()
                });
            }

            return node_ids
                .into_par_iter()
                .map(|id| {
                    if let Some(deadline) = self.deadline {
                        check_deadline_at(deadline)?;
                    }
                    let mut new_row = row.clone();
                    new_row.insert(op.var, LoraValue::Node(id));
                    Ok(new_row)
                })
                .collect();
        }

        let chunks: ExecResult<Vec<Vec<Row>>> = base_rows
            .into_par_iter()
            .map(|row| {
                if let Some(deadline) = self.deadline {
                    check_deadline_at(deadline)?;
                }
                if let Some(existing_id) = super::helpers::bound_node_id_for_expand(&row, op.var)? {
                    return Ok(if self.ctx.storage.has_node(existing_id) {
                        vec![row]
                    } else {
                        Vec::new()
                    });
                }

                let mut out = Vec::with_capacity(node_ids.len());
                for &id in &node_ids {
                    if let Some(deadline) = self.deadline {
                        check_deadline_at(deadline)?;
                    }
                    let mut new_row = row.clone();
                    new_row.insert(op.var, LoraValue::Node(id));
                    out.push(new_row);
                }
                Ok(out)
            })
            .collect();
        Ok(chunks?.into_iter().flatten().collect())
    }

    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    fn exec_node_by_label_scan_parallel_safe(
        &self,
        plan: &PhysicalPlan,
        op: &NodeByLabelScanExec,
    ) -> ExecResult<Vec<Row>>
    where
        S: Sync,
    {
        let base_rows = match op.input {
            Some(input) => self.execute_node_parallel_safe(plan, input)?,
            None => vec![Row::new()],
        };
        let candidate_ids = scan_node_ids_for_label_groups(self.ctx.storage, &op.labels);
        if base_rows.len().saturating_mul(candidate_ids.len()) < PARALLEL_ROW_THRESHOLD {
            return node_by_label_scan_rows(self.ctx.storage, base_rows, op, self.deadline);
        }

        let candidates_prefiltered = label_group_candidates_prefiltered(&op.labels);
        use rayon::prelude::*;
        if base_rows.len() == 1 {
            let row = base_rows.into_iter().next().expect("len checked above");
            if let Some(existing_id) = super::helpers::bound_node_id_for_expand(&row, op.var)? {
                let labels_ok = self
                    .ctx
                    .storage
                    .with_node(existing_id, |n| {
                        node_matches_label_groups(&n.labels, &op.labels)
                    })
                    .unwrap_or(false);
                return Ok(if labels_ok { vec![row] } else { Vec::new() });
            }

            return candidate_ids
                .into_par_iter()
                .filter_map(|id| {
                    if let Some(deadline) = self.deadline {
                        if let Err(err) = check_deadline_at(deadline) {
                            return Some(Err(err));
                        }
                    }
                    if !candidates_prefiltered {
                        let labels_ok = self
                            .ctx
                            .storage
                            .with_node(id, |n| node_matches_label_groups(&n.labels, &op.labels))
                            .unwrap_or(false);
                        if !labels_ok {
                            return None;
                        }
                    }
                    let mut new_row = row.clone();
                    new_row.insert(op.var, LoraValue::Node(id));
                    Some(Ok(new_row))
                })
                .collect();
        }

        let chunks: ExecResult<Vec<Vec<Row>>> = base_rows
            .into_par_iter()
            .map(|row| {
                if let Some(deadline) = self.deadline {
                    check_deadline_at(deadline)?;
                }
                if let Some(existing_id) = super::helpers::bound_node_id_for_expand(&row, op.var)? {
                    let labels_ok = self
                        .ctx
                        .storage
                        .with_node(existing_id, |n| {
                            node_matches_label_groups(&n.labels, &op.labels)
                        })
                        .unwrap_or(false);
                    return Ok(if labels_ok { vec![row] } else { Vec::new() });
                }

                let mut out = Vec::with_capacity(candidate_ids.len());
                for &id in &candidate_ids {
                    if let Some(deadline) = self.deadline {
                        check_deadline_at(deadline)?;
                    }
                    if !candidates_prefiltered {
                        let labels_ok = self
                            .ctx
                            .storage
                            .with_node(id, |n| node_matches_label_groups(&n.labels, &op.labels))
                            .unwrap_or(false);
                        if !labels_ok {
                            continue;
                        }
                    }
                    let mut new_row = row.clone();
                    new_row.insert(op.var, LoraValue::Node(id));
                    out.push(new_row);
                }
                Ok(out)
            })
            .collect();
        Ok(chunks?.into_iter().flatten().collect())
    }

    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    fn exec_node_by_property_scan_parallel_safe(
        &self,
        plan: &PhysicalPlan,
        op: &NodeByPropertyScanExec,
    ) -> ExecResult<Vec<Row>>
    where
        S: Sync,
    {
        let base_rows = match op.input {
            Some(input) => self.execute_node_parallel_safe(plan, input)?,
            None => vec![Row::new()],
        };
        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };
        use rayon::prelude::*;

        if base_rows.len() == 1 {
            let row = base_rows.into_iter().next().expect("len checked above");
            if let Some(deadline) = self.deadline {
                check_deadline_at(deadline)?;
            }
            let expected = eval_expr(&op.value, &row, &eval_ctx);
            if let Some(existing_id) = super::helpers::bound_node_id_for_expand(&row, op.var)? {
                return Ok(
                    if node_matches_property_filter(
                        self.ctx.storage,
                        existing_id,
                        &op.labels,
                        &op.key,
                        &expected,
                    ) {
                        vec![row]
                    } else {
                        Vec::new()
                    },
                );
            }

            let candidates =
                indexed_node_property_candidates(self.ctx.storage, &op.labels, &op.key, &expected);
            if candidates.ids.len() < PARALLEL_ROW_THRESHOLD {
                let mut out = Vec::with_capacity(candidates.ids.len());
                for id in candidates.ids {
                    if !candidates.prefiltered
                        && !node_matches_property_filter(
                            self.ctx.storage,
                            id,
                            &op.labels,
                            &op.key,
                            &expected,
                        )
                    {
                        continue;
                    }
                    let mut new_row = row.clone();
                    new_row.insert(op.var, LoraValue::Node(id));
                    out.push(new_row);
                }
                return Ok(out);
            }

            return candidates
                .ids
                .into_par_iter()
                .filter_map(|id| {
                    if let Some(deadline) = self.deadline {
                        if let Err(err) = check_deadline_at(deadline) {
                            return Some(Err(err));
                        }
                    }
                    if !candidates.prefiltered
                        && !node_matches_property_filter(
                            self.ctx.storage,
                            id,
                            &op.labels,
                            &op.key,
                            &expected,
                        )
                    {
                        return None;
                    }
                    let mut new_row = row.clone();
                    new_row.insert(op.var, LoraValue::Node(id));
                    Some(Ok(new_row))
                })
                .collect();
        }

        if base_rows.len() < PARALLEL_ROW_THRESHOLD {
            return node_by_property_scan_rows(
                self.ctx.storage,
                &self.ctx.params,
                base_rows,
                op,
                self.deadline,
            );
        }

        let chunks: ExecResult<Vec<Vec<Row>>> = base_rows
            .into_par_iter()
            .map(|row| {
                if let Some(deadline) = self.deadline {
                    check_deadline_at(deadline)?;
                }
                let expected = eval_expr(&op.value, &row, &eval_ctx);

                if let Some(existing_id) = super::helpers::bound_node_id_for_expand(&row, op.var)? {
                    return Ok(
                        if node_matches_property_filter(
                            self.ctx.storage,
                            existing_id,
                            &op.labels,
                            &op.key,
                            &expected,
                        ) {
                            vec![row]
                        } else {
                            Vec::new()
                        },
                    );
                }

                let candidates = indexed_node_property_candidates(
                    self.ctx.storage,
                    &op.labels,
                    &op.key,
                    &expected,
                );
                let mut out = Vec::with_capacity(candidates.ids.len());
                for id in candidates.ids {
                    if let Some(deadline) = self.deadline {
                        check_deadline_at(deadline)?;
                    }
                    if !candidates.prefiltered
                        && !node_matches_property_filter(
                            self.ctx.storage,
                            id,
                            &op.labels,
                            &op.key,
                            &expected,
                        )
                    {
                        continue;
                    }
                    let mut new_row = row.clone();
                    new_row.insert(op.var, LoraValue::Node(id));
                    out.push(new_row);
                }
                Ok(out)
            })
            .collect();
        Ok(chunks?.into_iter().flatten().collect())
    }

    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    fn exec_filter_parallel_safe(
        &self,
        plan: &PhysicalPlan,
        op: &FilterExec,
    ) -> ExecResult<Vec<Row>>
    where
        S: Sync,
    {
        let input_rows = self.execute_node_parallel_safe(plan, op.input)?;
        if input_rows.len() < PARALLEL_ROW_THRESHOLD {
            let eval_ctx = EvalContext {
                storage: self.ctx.storage,
                params: &self.ctx.params,
            };
            return filter_rows_checked(input_rows, &op.predicate, &eval_ctx);
        }

        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };
        use rayon::prelude::*;
        let filtered: ExecResult<Vec<Option<Row>>> = input_rows
            .into_par_iter()
            .map(|row| {
                if let Some(deadline) = self.deadline {
                    check_deadline_at(deadline)?;
                }
                let keep = eval_truthy_result(&op.predicate, &row, &eval_ctx)
                    .map_err(ExecutorError::RuntimeError)?;
                Ok(if keep { Some(row) } else { None })
            })
            .collect();
        Ok(filtered?.into_iter().flatten().collect())
    }

    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    fn exec_projection_parallel_safe(
        &self,
        plan: &PhysicalPlan,
        op: &ProjectionExec,
    ) -> ExecResult<Vec<Row>>
    where
        S: Sync,
    {
        let input_rows = self.execute_node_parallel_safe(plan, op.input)?;
        if input_rows.len() < PARALLEL_ROW_THRESHOLD {
            let eval_ctx = EvalContext {
                storage: self.ctx.storage,
                params: &self.ctx.params,
            };
            return project_rows_checked(input_rows, op, &eval_ctx);
        }

        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };
        use rayon::prelude::*;
        let projected: ExecResult<Vec<Row>> = input_rows
            .into_par_iter()
            .map(|row| {
                if let Some(deadline) = self.deadline {
                    check_deadline_at(deadline)?;
                }
                if op.include_existing {
                    let mut projected = row;
                    for item in &op.items {
                        let value = eval_expr_result(&item.expr, &projected, &eval_ctx)
                            .map_err(ExecutorError::RuntimeError)?;
                        projected.insert_named(item.output, item.name.clone(), value);
                    }
                    Ok(projected)
                } else {
                    let mut projected = Row::new();
                    for item in &op.items {
                        let value = eval_expr_result(&item.expr, &row, &eval_ctx)
                            .map_err(ExecutorError::RuntimeError)?;
                        projected.insert_named(item.output, item.name.clone(), value);
                    }
                    Ok(projected)
                }
            })
            .collect();
        let rows = projected?;
        Ok(if op.distinct {
            dedup_rows_by_vars(rows)
        } else {
            rows
        })
    }

    fn exec_argument(&self, _op: &ArgumentExec) -> ExecResult<Vec<Row>> {
        Ok(vec![Row::new()])
    }

    fn exec_node_scan(&self, plan: &PhysicalPlan, op: &NodeScanExec) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };

        node_scan_rows(self.ctx.storage, base_rows, op, self.deadline)
    }

    fn exec_node_by_label_scan(
        &self,
        plan: &PhysicalPlan,
        op: &NodeByLabelScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };

        node_by_label_scan_rows(self.ctx.storage, base_rows, op, self.deadline)
    }

    fn exec_node_by_property_scan(
        &self,
        plan: &PhysicalPlan,
        op: &NodeByPropertyScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };

        node_by_property_scan_rows(
            self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_node_by_property_range_scan(
        &self,
        plan: &PhysicalPlan,
        op: &lora_compiler::NodeByPropertyRangeScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::node_by_property_range_scan_rows(
            self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_node_by_text_scan(
        &self,
        plan: &PhysicalPlan,
        op: &lora_compiler::NodeByTextScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::node_by_text_scan_rows(
            self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_node_by_point_scan(
        &self,
        plan: &PhysicalPlan,
        op: &lora_compiler::NodeByPointScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::node_by_point_scan_rows(
            self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_rel_by_property_range_scan(
        &self,
        plan: &PhysicalPlan,
        op: &lora_compiler::RelByPropertyRangeScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::rel_by_property_range_scan_rows(
            self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_rel_by_text_scan(
        &self,
        plan: &PhysicalPlan,
        op: &lora_compiler::RelByTextScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::rel_by_text_scan_rows(
            self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_rel_by_point_scan(
        &self,
        plan: &PhysicalPlan,
        op: &lora_compiler::RelByPointScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };
        super::helpers::rel_by_point_scan_rows(
            self.ctx.storage,
            &self.ctx.params,
            base_rows,
            op,
            self.deadline,
        )
    }

    fn exec_expand(&self, plan: &PhysicalPlan, op: &ExpandExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        if let Some(range) = &op.range {
            expand_var_len_rows(self.ctx.storage, input_rows, op, range)
        } else {
            expand_rows(self.ctx.storage, &self.ctx.params, input_rows, op)
        }
    }

    fn exec_filter(&self, plan: &PhysicalPlan, op: &FilterExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };

        filter_rows_checked(input_rows, &op.predicate, &eval_ctx)
    }

    fn exec_projection(&self, plan: &PhysicalPlan, op: &ProjectionExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
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

    fn exec_unwind(&self, plan: &PhysicalPlan, op: &UnwindExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };

        Ok(unwind_rows(input_rows, op, &eval_ctx))
    }

    fn exec_hash_aggregation(
        &self,
        plan: &PhysicalPlan,
        op: &HashAggregationExec,
    ) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
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

    fn exec_sort(&self, plan: &PhysicalPlan, op: &SortExec) -> ExecResult<Vec<Row>> {
        let mut rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };

        sort_rows_with_top_k(&mut rows, &op.items, &eval_ctx, op.top_k);

        Ok(rows)
    }

    fn exec_limit(&self, plan: &PhysicalPlan, op: &LimitExec) -> ExecResult<Vec<Row>> {
        let rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };

        Ok(limit_rows(rows, op, &eval_ctx))
    }

    fn exec_optional_match(
        &self,
        plan: &PhysicalPlan,
        op: &OptionalMatchExec,
    ) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;

        // The inner plan is built to start from Argument (an empty row) and is
        // read-only, so its output does not depend on the upstream input. Execute
        // it once and reuse the result across every input row, instead of
        // producing |input_rows| × |inner_rows| allocations.
        let inner_rows = self.execute_node(plan, op.inner)?;

        Ok(optional_match_rows(input_rows, &inner_rows, &op.new_vars))
    }

    fn exec_path_build(&self, plan: &PhysicalPlan, op: &PathBuildExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let mut rows: Vec<Row> = input_rows
            .into_iter()
            .map(|mut row| {
                let path = build_path_value(&row, &op.node_vars, &op.rel_vars, self.ctx.storage);
                row.insert(op.output, path);
                row
            })
            .collect();

        if let Some(all) = op.shortest_path_all {
            rows = filter_shortest_paths(rows, op.output, all);
        }
        Ok(rows)
    }
}

#[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
fn plan_is_parallel_safe(plan: &PhysicalPlan) -> bool {
    subtree_is_parallel_safe(plan, plan.root)
}

#[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
fn subtree_is_parallel_safe(plan: &PhysicalPlan, node_id: PhysicalNodeId) -> bool {
    match &plan.nodes[node_id] {
        PhysicalOp::Argument(_) => true,
        PhysicalOp::NodeScan(op) => op
            .input
            .map(|input| subtree_is_parallel_safe(plan, input))
            .unwrap_or(true),
        PhysicalOp::NodeByLabelScan(op) => op
            .input
            .map(|input| subtree_is_parallel_safe(plan, input))
            .unwrap_or(true),
        PhysicalOp::NodeByPropertyScan(op) => op
            .input
            .map(|input| subtree_is_parallel_safe(plan, input))
            .unwrap_or(true),
        PhysicalOp::Filter(op) => subtree_is_parallel_safe(plan, op.input),
        PhysicalOp::Projection(op) => subtree_is_parallel_safe(plan, op.input),
        PhysicalOp::NodeByPropertyRangeScan(_)
        | PhysicalOp::NodeByTextScan(_)
        | PhysicalOp::NodeByPointScan(_)
        | PhysicalOp::RelByPropertyRangeScan(_)
        | PhysicalOp::RelByTextScan(_)
        | PhysicalOp::RelByPointScan(_) => false,
        PhysicalOp::Expand(_)
        | PhysicalOp::Unwind(_)
        | PhysicalOp::HashAggregation(_)
        | PhysicalOp::Sort(_)
        | PhysicalOp::Limit(_)
        | PhysicalOp::Create(_)
        | PhysicalOp::Merge(_)
        | PhysicalOp::Delete(_)
        | PhysicalOp::Set(_)
        | PhysicalOp::Remove(_)
        | PhysicalOp::OptionalMatch(_)
        | PhysicalOp::PathBuild(_) => false,
    }
}
