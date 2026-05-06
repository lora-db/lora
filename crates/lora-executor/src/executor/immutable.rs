//! Read-only buffered executor: lower a [`PhysicalPlan`] into a fully
//! materialized `Vec<Row>` without touching the store.
//!
//! [`Executor`] mirrors the operator set of the streaming pipeline in
//! `crate::pull` so write operators can fall back to it for subtrees
//! that are not fully streamable. Aggregation and DISTINCT projection
//! reuse the streaming `StreamableAggSpec` / `AggState` machinery
//! (re-exported as `crate::pull::*` for that purpose) on the
//! fold-only fast path; everything else materializes.

use crate::errors::{value_kind, ExecResult, ExecutorError};
use crate::eval::{clear_eval_error, eval_expr, eval_expr_result, EvalContext};
use crate::value::{LoraValue, Row};
use crate::{project_rows, ExecuteOptions, QueryResult};

use lora_analyzer::{ResolvedExpr, ResolvedProjection};
use lora_ast::RangeLiteral;
use lora_compiler::physical::*;
use lora_compiler::CompiledQuery;
use lora_store::{GraphStorage, Properties};

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::time::Instant;
use tracing::{error, trace};

use super::helpers::{
    build_path_value, check_deadline_at, compare_sort_item, compute_aggregate_expr, dedup_rows,
    filter_rows_checked, filter_shortest_paths, hydrate_node_record, hydrate_relationship_record,
    indexed_node_property_candidates, label_group_candidates_prefiltered,
    node_matches_label_groups, node_matches_property_filter, project_rows_checked, resolve_range,
    scan_node_ids_for_label_groups, value_matches_property_value, variable_length_expand,
    GroupValueKey,
};

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

    #[inline]
    fn check_loop_deadline(deadline: Option<Instant>) -> ExecResult<()> {
        if let Some(deadline) = deadline {
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

    fn exec_argument(&self, _op: &ArgumentExec) -> ExecResult<Vec<Row>> {
        Ok(vec![Row::new()])
    }

    fn exec_node_scan(&self, plan: &PhysicalPlan, op: &NodeScanExec) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };

        let node_ids = self.ctx.storage.all_node_ids();
        let mut out = Vec::new();

        let deadline = self.deadline;
        for row in base_rows {
            Self::check_loop_deadline(deadline)?;
            if let Some(existing) = row.get(op.var) {
                match existing {
                    LoraValue::Node(existing_id) => {
                        if self.ctx.storage.has_node(*existing_id) {
                            out.push(row);
                        }
                    }
                    other => {
                        return Err(ExecutorError::ExpectedNodeForExpand {
                            var: format!("{:?}", op.var),
                            found: value_kind(other),
                        });
                    }
                }
                continue;
            }

            for &id in &node_ids {
                Self::check_loop_deadline(deadline)?;
                let mut new_row = row.clone();
                new_row.insert(op.var, LoraValue::Node(id));
                out.push(new_row);
            }
        }

        Ok(out)
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

        let candidate_ids = scan_node_ids_for_label_groups(self.ctx.storage, &op.labels);
        let candidates_prefiltered = label_group_candidates_prefiltered(&op.labels);
        let mut out = Vec::new();

        match self.deadline {
            Some(deadline) => {
                for row in base_rows {
                    check_deadline_at(deadline)?;
                    if let Some(existing) = row.get(op.var) {
                        match existing {
                            LoraValue::Node(existing_id) => {
                                let labels_ok = self
                                    .ctx
                                    .storage
                                    .with_node(*existing_id, |n| {
                                        node_matches_label_groups(&n.labels, &op.labels)
                                    })
                                    .unwrap_or(false);
                                if labels_ok {
                                    out.push(row);
                                }
                            }
                            other => {
                                return Err(ExecutorError::ExpectedNodeForExpand {
                                    var: format!("{:?}", op.var),
                                    found: value_kind(other),
                                });
                            }
                        }
                        continue;
                    }

                    for &id in &candidate_ids {
                        check_deadline_at(deadline)?;
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
                }
            }
            None => {
                for row in base_rows {
                    if let Some(existing) = row.get(op.var) {
                        match existing {
                            LoraValue::Node(existing_id) => {
                                let labels_ok = self
                                    .ctx
                                    .storage
                                    .with_node(*existing_id, |n| {
                                        node_matches_label_groups(&n.labels, &op.labels)
                                    })
                                    .unwrap_or(false);
                                if labels_ok {
                                    out.push(row);
                                }
                            }
                            other => {
                                return Err(ExecutorError::ExpectedNodeForExpand {
                                    var: format!("{:?}", op.var),
                                    found: value_kind(other),
                                });
                            }
                        }
                        continue;
                    }

                    for &id in &candidate_ids {
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
                }
            }
        }

        Ok(out)
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

        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };
        let mut out = Vec::new();

        let deadline = self.deadline;
        for row in base_rows {
            Self::check_loop_deadline(deadline)?;
            let expected = eval_expr(&op.value, &row, &eval_ctx);

            if let Some(existing) = row.get(op.var) {
                match existing {
                    LoraValue::Node(existing_id) => {
                        if node_matches_property_filter(
                            self.ctx.storage,
                            *existing_id,
                            &op.labels,
                            &op.key,
                            &expected,
                        ) {
                            out.push(row);
                        }
                    }
                    other => {
                        return Err(ExecutorError::ExpectedNodeForExpand {
                            var: format!("{:?}", op.var),
                            found: value_kind(other),
                        });
                    }
                }
                continue;
            }

            let candidates =
                indexed_node_property_candidates(self.ctx.storage, &op.labels, &op.key, &expected);
            for id in candidates.ids {
                Self::check_loop_deadline(deadline)?;
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
        }

        Ok(out)
    }

    fn exec_expand(&self, plan: &PhysicalPlan, op: &ExpandExec) -> ExecResult<Vec<Row>> {
        // Variable-length expansion: delegate to iterative expander.
        if let Some(range) = &op.range {
            return self.exec_expand_var_len(plan, op, range);
        }

        let input_rows = self.execute_node(plan, op.input)?;
        let mut out = Vec::new();

        for row in input_rows {
            let src_node_id = match row.get(op.src) {
                Some(LoraValue::Node(id)) => *id,
                Some(other) => {
                    return Err(ExecutorError::ExpectedNodeForExpand {
                        var: format!("{:?}", op.src),
                        found: value_kind(other),
                    });
                }
                None => continue,
            };

            for (rel_id, dst_id) in
                self.ctx
                    .storage
                    .expand_ids(src_node_id, op.direction, &op.types)
            {
                if let Some(expr) = op.rel_properties.as_ref() {
                    let actual_props = self
                        .ctx
                        .storage
                        .with_relationship(rel_id, |rel| rel.properties.clone());
                    let matches = match actual_props {
                        Some(props) => {
                            self.relationship_matches_properties(&props, Some(expr), &row)?
                        }
                        None => false,
                    };
                    if !matches {
                        continue;
                    }
                }

                if let Some(existing_dst) = row.get(op.dst) {
                    match existing_dst {
                        LoraValue::Node(existing_id) if *existing_id == dst_id => {}
                        LoraValue::Node(_) => continue,
                        other => {
                            return Err(ExecutorError::ExpectedNodeForExpand {
                                var: format!("{:?}", op.dst),
                                found: value_kind(other),
                            });
                        }
                    }
                }

                if let Some(rel_var) = op.rel {
                    if let Some(existing_rel) = row.get(rel_var) {
                        match existing_rel {
                            LoraValue::Relationship(existing_id) if *existing_id == rel_id => {}
                            LoraValue::Relationship(_) => continue,
                            other => {
                                return Err(ExecutorError::ExpectedRelationshipForExpand {
                                    var: format!("{:?}", rel_var),
                                    found: value_kind(other),
                                });
                            }
                        }
                    }
                }

                let mut new_row = row.clone();

                if !new_row.contains_key(op.dst) {
                    new_row.insert(op.dst, LoraValue::Node(dst_id));
                }

                if let Some(rel_var) = op.rel {
                    if !new_row.contains_key(rel_var) {
                        new_row.insert(rel_var, LoraValue::Relationship(rel_id));
                    }
                }

                out.push(new_row);
            }
        }

        Ok(out)
    }

    fn exec_expand_var_len(
        &self,
        plan: &PhysicalPlan,
        op: &ExpandExec,
        range: &RangeLiteral,
    ) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let (min_hops, max_hops) = resolve_range(range);
        let mut out = Vec::new();

        for row in input_rows {
            let src_node_id = match row.get(op.src) {
                Some(LoraValue::Node(id)) => *id,
                Some(other) => {
                    return Err(ExecutorError::ExpectedNodeForExpand {
                        var: format!("{:?}", op.src),
                        found: value_kind(other),
                    });
                }
                None => continue,
            };

            let expansions = variable_length_expand(
                self.ctx.storage,
                src_node_id,
                op.direction,
                &op.types,
                min_hops,
                max_hops,
            );

            for result in expansions {
                let mut new_row = row.clone();
                new_row.insert(op.dst, LoraValue::Node(result.dst_node_id));

                // For variable-length patterns, bind the relationship variable
                // to a list of relationship IDs traversed.
                if let Some(rel_var) = op.rel {
                    // Consume rel_ids — it's owned and no longer needed after this.
                    let rel_list = LoraValue::List(
                        result
                            .rel_ids
                            .into_iter()
                            .map(LoraValue::Relationship)
                            .collect(),
                    );
                    new_row.insert(rel_var, rel_list);
                }

                out.push(new_row);
            }
        }

        Ok(out)
    }

    fn relationship_matches_properties(
        &self,
        actual: &Properties,
        expected_expr: Option<&ResolvedExpr>,
        row: &Row,
    ) -> ExecResult<bool> {
        let Some(expr) = expected_expr else {
            return Ok(true);
        };

        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };

        let expected = eval_expr(expr, row, &eval_ctx);

        let LoraValue::Map(expected_map) = expected else {
            return Err(ExecutorError::ExpectedPropertyMap {
                found: value_kind(&expected),
            });
        };

        Ok(expected_map.iter().all(|(key, expected_value)| {
            actual
                .get(key)
                .map(|actual_value| value_matches_property_value(expected_value, actual_value))
                .unwrap_or(false)
        }))
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

        let mut out = Vec::new();

        for row in input_rows {
            match eval_expr(&op.expr, &row, &eval_ctx) {
                LoraValue::List(values) => {
                    for value in values {
                        let mut new_row = row.clone();
                        new_row.insert(op.alias, value);
                        out.push(new_row);
                    }
                }
                LoraValue::Null => {}
                other => {
                    let mut new_row = row;
                    new_row.insert(op.alias, other);
                    out.push(new_row);
                }
            }
        }

        Ok(out)
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

        // Streaming fold fast path. When every aggregate is a fold-only
        // function (count/sum/min/max/avg, no DISTINCT) we never buffer
        // input rows by group — we fold each row's aggregate value into
        // running per-group state. Memory drops from O(input_rows) to
        // O(groups).
        if let Some(specs) = crate::pull::classify_streamable_aggregates(&op.aggregates) {
            return self.exec_hash_aggregation_streaming(
                input_rows,
                &op.group_by,
                &op.aggregates,
                &specs,
                &eval_ctx,
            );
        }

        let mut groups: BTreeMap<Vec<GroupValueKey>, Vec<Row>> = BTreeMap::new();

        if op.group_by.is_empty() {
            groups.insert(Vec::new(), input_rows);
        } else {
            for row in input_rows {
                let mut key = Vec::with_capacity(op.group_by.len());
                for proj in &op.group_by {
                    let value = eval_expr_result(&proj.expr, &row, &eval_ctx)
                        .map_err(ExecutorError::RuntimeError)?;
                    key.push(GroupValueKey::from_value(&value));
                }

                groups.entry(key).or_default().push(row);
            }
        }

        let mut out = Vec::new();

        for rows in groups.into_values() {
            let mut result = Row::new();

            if let Some(first) = rows.first() {
                for proj in &op.group_by {
                    let value = eval_expr_result(&proj.expr, first, &eval_ctx)
                        .map_err(ExecutorError::RuntimeError)?;
                    let value = self.hydrate_value(value);
                    result.insert_named(proj.output, proj.name.clone(), value);
                }
            }

            for proj in &op.aggregates {
                let value = compute_aggregate_expr(&proj.expr, &rows, &eval_ctx)?;
                result.insert_named(proj.output, proj.name.clone(), value);
            }

            out.push(result);
        }

        Ok(out)
    }

    fn exec_hash_aggregation_streaming(
        &self,
        input_rows: Vec<Row>,
        group_by: &[ResolvedProjection],
        aggregates: &[ResolvedProjection],
        specs: &[crate::pull::StreamableAggSpec],
        eval_ctx: &EvalContext<'_, S>,
    ) -> ExecResult<Vec<Row>> {
        // No-group-by fast path: a single accumulator, no BTreeMap.
        if group_by.is_empty() {
            // `count(*)`-only shortcut. The buffered immutable executor
            // already materialised every input row into `Vec<Row>`, so the
            // aggregate is just the row count — no need to fold per row.
            // This is the v0.6 shape that the streaming refactor lost.
            if specs
                .iter()
                .all(|s| matches!(s.kind, crate::pull::StreamableAggKind::CountAll))
            {
                let count = LoraValue::Int(input_rows.len() as i64);
                let mut result = Row::new();
                for proj in aggregates {
                    result.insert_named(proj.output, proj.name.clone(), count.clone());
                }
                return Ok(vec![result]);
            }

            let mut aggs: Vec<crate::pull::AggState> = specs
                .iter()
                .map(|s| crate::pull::AggState::seed(s.kind))
                .collect();
            for row in &input_rows {
                for (i, spec) in specs.iter().enumerate() {
                    let value = match &spec.arg {
                        Some(arg) => eval_expr_result(arg, row, eval_ctx)
                            .map_err(ExecutorError::RuntimeError)?,
                        None => LoraValue::Null,
                    };
                    aggs[i].fold(spec.kind, value);
                }
            }
            let mut result = Row::new();
            for (i, proj) in aggregates.iter().enumerate() {
                let value =
                    std::mem::replace(&mut aggs[i], crate::pull::AggState::seed(specs[i].kind))
                        .finalize(specs[i].kind);
                result.insert_named(proj.output, proj.name.clone(), value);
            }
            return Ok(vec![result]);
        }

        // Group-by path: per-group running accumulator. The first row in
        // each group is retained so we can compute the group_by output
        // expressions later; nothing else from the input is buffered.
        let mut groups: BTreeMap<Vec<GroupValueKey>, (Row, Vec<crate::pull::AggState>)> =
            BTreeMap::new();

        for row in input_rows {
            let mut key = Vec::with_capacity(group_by.len());
            for proj in group_by {
                let value = eval_expr_result(&proj.expr, &row, eval_ctx)
                    .map_err(ExecutorError::RuntimeError)?;
                key.push(GroupValueKey::from_value(&value));
            }

            let entry = groups.entry(key).or_insert_with(|| {
                (
                    row.clone(),
                    specs
                        .iter()
                        .map(|s| crate::pull::AggState::seed(s.kind))
                        .collect(),
                )
            });

            for (i, spec) in specs.iter().enumerate() {
                let value = match &spec.arg {
                    Some(arg) => eval_expr_result(arg, &row, eval_ctx)
                        .map_err(ExecutorError::RuntimeError)?,
                    None => LoraValue::Null,
                };
                entry.1[i].fold(spec.kind, value);
            }
        }

        let mut out = Vec::with_capacity(groups.len());
        for (_, (first_row, mut aggs)) in groups {
            let mut result = Row::new();
            for proj in group_by {
                let value = eval_expr_result(&proj.expr, &first_row, eval_ctx)
                    .map_err(ExecutorError::RuntimeError)?;
                let value = self.hydrate_value(value);
                result.insert_named(proj.output, proj.name.clone(), value);
            }
            for (i, proj) in aggregates.iter().enumerate() {
                let value =
                    std::mem::replace(&mut aggs[i], crate::pull::AggState::seed(specs[i].kind))
                        .finalize(specs[i].kind);
                result.insert_named(proj.output, proj.name.clone(), value);
            }
            out.push(result);
        }
        Ok(out)
    }

    fn exec_sort(&self, plan: &PhysicalPlan, op: &SortExec) -> ExecResult<Vec<Row>> {
        let mut rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };

        rows.sort_by(|a, b| {
            for item in &op.items {
                let ord = compare_sort_item(item, a, b, &eval_ctx);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            Ordering::Equal
        });

        Ok(rows)
    }

    fn exec_limit(&self, plan: &PhysicalPlan, op: &LimitExec) -> ExecResult<Vec<Row>> {
        let mut rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };

        let limit = op
            .limit
            .as_ref()
            .and_then(|e| eval_expr(e, &Row::new(), &eval_ctx).as_i64())
            .unwrap_or(rows.len() as i64)
            .max(0) as usize;

        let skip = op
            .skip
            .as_ref()
            .and_then(|e| eval_expr(e, &Row::new(), &eval_ctx).as_i64())
            .unwrap_or(0)
            .max(0) as usize;

        if skip >= rows.len() {
            return Ok(Vec::new());
        }

        rows.drain(0..skip);
        rows.truncate(limit);
        Ok(rows)
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

        let mut out = Vec::new();

        for input_row in input_rows {
            let mut matched = false;

            for inner_row in &inner_rows {
                // Each variable already bound in input_row must match.
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
                out.push(merged);
                matched = true;
            }

            if !matched {
                let mut null_row = input_row;
                for &var_id in &op.new_vars {
                    if !null_row.contains_key(var_id) {
                        null_row.insert(var_id, LoraValue::Null);
                    }
                }
                out.push(null_row);
            }
        }

        Ok(out)
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
