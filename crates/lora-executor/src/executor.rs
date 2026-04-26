use crate::errors::{value_kind, ExecResult, ExecutorError};
use crate::eval::{eval_expr, take_eval_error, EvalContext};
use crate::value::{lora_value_to_property, LoraPath, LoraValue, Row};
use crate::{project_rows, ExecuteOptions, QueryResult};

use lora_analyzer::{
    symbols::VarId, ResolvedExpr, ResolvedPattern, ResolvedPatternElement, ResolvedPatternPart,
    ResolvedRemoveItem, ResolvedSetItem, ResolvedSortItem,
};
use lora_ast::{Direction, RangeLiteral};
use lora_compiler::physical::*;
use lora_compiler::CompiledQuery;
use lora_store::{GraphStorage, GraphStorageMut, NodeId, Properties, PropertyValue};

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;
use tracing::{debug, error, trace};

pub struct ExecutionContext<'a, S: GraphStorage> {
    pub storage: &'a S,
    pub params: std::collections::BTreeMap<String, LoraValue>,
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

#[inline]
fn check_deadline_at(deadline: Instant) -> ExecResult<()> {
    if Instant::now() >= deadline {
        Err(ExecutorError::QueryTimeout)
    } else {
        Ok(())
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

        let _ = take_eval_error();

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
        let _ = take_eval_error();

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

        Ok(input_rows
            .into_iter()
            .filter(|row| eval_expr(&op.predicate, row, &eval_ctx).is_truthy())
            .collect())
    }

    fn exec_projection(&self, plan: &PhysicalPlan, op: &ProjectionExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: self.ctx.storage,
            params: &self.ctx.params,
        };

        let mut out = Vec::with_capacity(input_rows.len());

        for row in input_rows {
            // include_existing=true means we carry all upstream columns — move
            // the row into the projection instead of cloning every value.
            if op.include_existing {
                let mut projected = row;
                for item in &op.items {
                    let value = eval_expr(&item.expr, &projected, &eval_ctx);
                    if let Some(err) = take_eval_error() {
                        return Err(ExecutorError::RuntimeError(err));
                    }
                    projected.insert_named(item.output, item.name.clone(), value);
                }
                out.push(projected);
            } else {
                let mut projected = Row::new();
                for item in &op.items {
                    let value = eval_expr(&item.expr, &row, &eval_ctx);
                    if let Some(err) = take_eval_error() {
                        return Err(ExecutorError::RuntimeError(err));
                    }
                    projected.insert_named(item.output, item.name.clone(), value);
                }
                out.push(projected);
            }
        }

        Ok(if op.distinct {
            dedup_rows_by_vars(out)
        } else {
            out
        })
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

        let mut groups: BTreeMap<Vec<GroupValueKey>, Vec<Row>> = BTreeMap::new();

        if op.group_by.is_empty() {
            groups.insert(Vec::new(), input_rows);
        } else {
            for row in input_rows {
                let key = op
                    .group_by
                    .iter()
                    .map(|proj| GroupValueKey::from_value(&eval_expr(&proj.expr, &row, &eval_ctx)))
                    .collect::<Vec<_>>();

                groups.entry(key).or_default().push(row);
            }
        }

        let mut out = Vec::new();

        for rows in groups.into_values() {
            let mut result = Row::new();

            if let Some(first) = rows.first() {
                for proj in &op.group_by {
                    let value = self.hydrate_value(eval_expr(&proj.expr, first, &eval_ctx));
                    result.insert_named(proj.output, proj.name.clone(), value);
                }
            }

            for proj in &op.aggregates {
                let value = compute_aggregate_expr(&proj.expr, &rows, &eval_ctx);
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

fn properties_to_value_map(props: &Properties) -> LoraValue {
    let mut map = BTreeMap::new();
    for (k, v) in props.iter() {
        map.insert(k.clone(), LoraValue::from(v));
    }
    LoraValue::Map(map)
}

pub struct MutableExecutionContext<'a, S: GraphStorageMut> {
    pub storage: &'a mut S,
    pub params: std::collections::BTreeMap<String, LoraValue>,
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

    #[inline]
    fn check_loop_deadline(deadline: Option<Instant>) -> ExecResult<()> {
        if let Some(deadline) = deadline {
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
        let _ = take_eval_error();

        let rows = self.execute_node(plan, plan.root)?;
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

        let _ = take_eval_error();

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
        &mut self,
        plan: &PhysicalPlan,
        op: &NodeByLabelScanExec,
    ) -> ExecResult<Vec<Row>> {
        let base_rows = match op.input {
            Some(input) => self.execute_node(plan, input)?,
            None => vec![Row::new()],
        };

        let candidate_ids = scan_node_ids_for_label_groups(&*self.ctx.storage, &op.labels);
        let candidates_prefiltered = label_group_candidates_prefiltered(&op.labels);
        let mut out = Vec::new();

        let deadline = self.deadline;
        for row in base_rows {
            Self::check_loop_deadline(deadline)?;
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
                Self::check_loop_deadline(deadline)?;
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

        Ok(out)
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

        let mut out = Vec::new();

        let deadline = self.deadline;
        for row in base_rows {
            Self::check_loop_deadline(deadline)?;
            let expected = {
                let eval_ctx = EvalContext {
                    storage: &*self.ctx.storage,
                    params: &self.ctx.params,
                };
                eval_expr(&op.value, &row, &eval_ctx)
            };

            if let Some(existing) = row.get(op.var) {
                match existing {
                    LoraValue::Node(existing_id) => {
                        if node_matches_property_filter(
                            &*self.ctx.storage,
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

            let candidates = indexed_node_property_candidates(
                &*self.ctx.storage,
                &op.labels,
                &op.key,
                &expected,
            );
            for id in candidates.ids {
                Self::check_loop_deadline(deadline)?;
                if !candidates.prefiltered
                    && !node_matches_property_filter(
                        &*self.ctx.storage,
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

    fn exec_expand(&mut self, plan: &PhysicalPlan, op: &ExpandExec) -> ExecResult<Vec<Row>> {
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
        &mut self,
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
                &*self.ctx.storage,
                src_node_id,
                op.direction,
                &op.types,
                min_hops,
                max_hops,
            );

            for result in expansions {
                let mut new_row = row.clone();
                new_row.insert(op.dst, LoraValue::Node(result.dst_node_id));

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
            storage: &*self.ctx.storage,
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

    fn exec_filter(&mut self, plan: &PhysicalPlan, op: &FilterExec) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: &*self.ctx.storage,
            params: &self.ctx.params,
        };

        Ok(input_rows
            .into_iter()
            .filter(|row| eval_expr(&op.predicate, row, &eval_ctx).is_truthy())
            .collect())
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

        let mut out = Vec::with_capacity(input_rows.len());

        for row in input_rows {
            if op.include_existing {
                let mut projected = row;
                for item in &op.items {
                    let value = eval_expr(&item.expr, &projected, &eval_ctx);
                    if let Some(err) = take_eval_error() {
                        return Err(ExecutorError::RuntimeError(err));
                    }
                    projected.insert_named(item.output, item.name.clone(), value);
                }
                out.push(projected);
            } else {
                let mut projected = Row::new();
                for item in &op.items {
                    let value = eval_expr(&item.expr, &row, &eval_ctx);
                    if let Some(err) = take_eval_error() {
                        return Err(ExecutorError::RuntimeError(err));
                    }
                    projected.insert_named(item.output, item.name.clone(), value);
                }
                out.push(projected);
            }
        }

        Ok(if op.distinct {
            dedup_rows_by_vars(out)
        } else {
            out
        })
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
        &mut self,
        plan: &PhysicalPlan,
        op: &HashAggregationExec,
    ) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: &*self.ctx.storage,
            params: &self.ctx.params,
        };

        let mut groups: BTreeMap<Vec<GroupValueKey>, Vec<Row>> = BTreeMap::new();

        if op.group_by.is_empty() {
            groups.insert(Vec::new(), input_rows);
        } else {
            for row in input_rows {
                let key = op
                    .group_by
                    .iter()
                    .map(|proj| GroupValueKey::from_value(&eval_expr(&proj.expr, &row, &eval_ctx)))
                    .collect::<Vec<_>>();

                groups.entry(key).or_default().push(row);
            }
        }

        let mut out = Vec::new();

        for rows in groups.into_values() {
            let mut result = Row::new();

            if let Some(first) = rows.first() {
                for proj in &op.group_by {
                    let value = self.hydrate_value(eval_expr(&proj.expr, first, &eval_ctx));
                    result.insert_named(proj.output, proj.name.clone(), value);
                }
            }

            for proj in &op.aggregates {
                let value = compute_aggregate_expr(&proj.expr, &rows, &eval_ctx);
                result.insert_named(proj.output, proj.name.clone(), value);
            }

            out.push(result);
        }

        Ok(out)
    }

    fn exec_sort(&mut self, plan: &PhysicalPlan, op: &SortExec) -> ExecResult<Vec<Row>> {
        let mut rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: &*self.ctx.storage,
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

    fn exec_limit(&mut self, plan: &PhysicalPlan, op: &LimitExec) -> ExecResult<Vec<Row>> {
        let mut rows = self.execute_node(plan, op.input)?;
        let eval_ctx = EvalContext {
            storage: &*self.ctx.storage,
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
        &mut self,
        plan: &PhysicalPlan,
        op: &OptionalMatchExec,
    ) -> ExecResult<Vec<Row>> {
        let input_rows = self.execute_node(plan, op.input)?;

        // Inner plan is read-only and input-independent; execute once and reuse.
        let inner_rows = self.execute_node(plan, op.inner)?;

        let mut out = Vec::new();

        for input_row in input_rows {
            let mut matched = false;

            for inner_row in &inner_rows {
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

                    let _ = step.rel.types.first();
                    let direction = step.rel.direction;

                    // ID-only traversal; look up records by reference only for
                    // candidates that pass the label/property filters.
                    let edges =
                        self.ctx
                            .storage
                            .expand_ids(current_node_id, direction, &step.rel.types);

                    // Try to find a matching edge + target node
                    let mut found = false;
                    for (rel_id, node_id) in edges {
                        // Check target node labels and (optional) properties.
                        let node_ok = self
                            .ctx
                            .storage
                            .with_node(node_id, |node_rec| {
                                if !node_matches_label_groups(&node_rec.labels, &step.node.labels) {
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
                            continue;
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
                            continue;
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
                        break;
                    }

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
                self.ctx
                    .storage
                    .set_node_property(node_id, property.clone(), prop);
                Ok(())
            }
            LoraValue::Relationship(rel_id) => {
                let prop = lora_value_to_property(new_value)
                    .map_err(|e| ExecutorError::RuntimeError(e.to_string()))?;
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
                self.ctx.storage.remove_node_property(node_id, property);
                Ok(())
            }
            LoraValue::Relationship(rel_id) => {
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
                self.ctx.storage.replace_node_properties(node_id, props);
            }
            EntityTarget::Relationship(rel_id) => {
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
                    self.ctx.storage.set_node_property(node_id, k, prop);
                }
            }
            EntityTarget::Relationship(rel_id) => {
                for (k, v) in map {
                    let prop = lora_value_to_property(v)
                        .map_err(|e| ExecutorError::RuntimeError(e.to_string()))?;
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
        var: Option<lora_analyzer::symbols::VarId>,
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

/// Dedup rows that share the same schema (same VarId set). Compares rows by
/// a Vec<GroupValueKey> keyed on VarId iteration order — avoids the per-row
/// column-name String clones of `dedup_rows`. Used by DISTINCT projection.
pub(crate) fn dedup_rows_by_vars(rows: Vec<Row>) -> Vec<Row> {
    let mut seen: BTreeSet<Vec<GroupValueKey>> = BTreeSet::new();
    let mut out = Vec::new();

    for row in rows {
        let key: Vec<GroupValueKey> = row
            .iter()
            .map(|(_, val)| GroupValueKey::from_value(val))
            .collect();
        if seen.insert(key) {
            out.push(row);
        }
    }

    out
}

/// Dedup rows using named entries so rows with different VarIds but the same
/// column name + value are collapsed. Needed for UNION where each branch has
/// its own VarIds.
pub(crate) fn dedup_rows(rows: Vec<Row>) -> Vec<Row> {
    let mut seen: BTreeSet<Vec<(String, GroupValueKey)>> = BTreeSet::new();
    let mut out = Vec::new();

    for row in rows {
        let key: Vec<(String, GroupValueKey)> = row
            .iter_named()
            .map(|(_, name, val)| (name.into_owned(), GroupValueKey::from_value(val)))
            .collect();
        if seen.insert(key) {
            out.push(row);
        }
    }

    out
}

fn eval_properties_expr<S: GraphStorage>(
    expr: &ResolvedExpr,
    row: &Row,
    storage: &S,
    params: &std::collections::BTreeMap<String, LoraValue>,
) -> ExecResult<Properties> {
    let eval_ctx = EvalContext { storage, params };

    match eval_expr(expr, row, &eval_ctx) {
        LoraValue::Map(map) => {
            let mut out = Properties::new();
            for (k, v) in map {
                let prop = lora_value_to_property(v)
                    .map_err(|e| ExecutorError::RuntimeError(e.to_string()))?;
                out.insert(k, prop);
            }
            Ok(out)
        }
        other => Err(ExecutorError::ExpectedPropertyMap {
            found: value_kind(&other),
        }),
    }
}

pub(crate) fn compute_aggregate_expr<S: GraphStorage>(
    expr: &ResolvedExpr,
    rows: &[Row],
    eval_ctx: &EvalContext<'_, S>,
) -> LoraValue {
    match expr {
        ResolvedExpr::Function {
            name,
            distinct,
            args,
        } => {
            let func = name.to_ascii_lowercase();

            match func.as_str() {
                "count" => {
                    if args.is_empty() {
                        return LoraValue::Int(rows.len() as i64);
                    }

                    let mut values = rows
                        .iter()
                        .map(|r| eval_expr(&args[0], r, eval_ctx))
                        .filter(|v| !matches!(v, LoraValue::Null))
                        .collect::<Vec<_>>();

                    if *distinct {
                        values = dedup_values(values);
                    }

                    LoraValue::Int(values.len() as i64)
                }

                "collect" => {
                    if args.is_empty() {
                        return LoraValue::List(Vec::new());
                    }

                    let mut values = rows
                        .iter()
                        .map(|r| eval_expr(&args[0], r, eval_ctx))
                        .collect::<Vec<_>>();

                    if *distinct {
                        values = dedup_values(values);
                    }

                    LoraValue::List(values)
                }

                "sum" => {
                    if args.is_empty() {
                        return LoraValue::Null;
                    }

                    let mut values = rows
                        .iter()
                        .map(|r| eval_expr(&args[0], r, eval_ctx))
                        .collect::<Vec<_>>();

                    if *distinct {
                        values = dedup_values(values);
                    }

                    let nums = values
                        .into_iter()
                        .filter_map(as_f64_lossy)
                        .collect::<Vec<_>>();

                    if nums.is_empty() {
                        LoraValue::Null
                    } else if nums.iter().all(|n| n.fract() == 0.0) {
                        LoraValue::Int(nums.iter().sum::<f64>() as i64)
                    } else {
                        LoraValue::Float(nums.iter().sum::<f64>())
                    }
                }

                "avg" => {
                    if args.is_empty() {
                        return LoraValue::Null;
                    }

                    let mut values = rows
                        .iter()
                        .map(|r| eval_expr(&args[0], r, eval_ctx))
                        .collect::<Vec<_>>();

                    if *distinct {
                        values = dedup_values(values);
                    }

                    let nums = values
                        .into_iter()
                        .filter_map(as_f64_lossy)
                        .collect::<Vec<_>>();

                    if nums.is_empty() {
                        LoraValue::Null
                    } else {
                        LoraValue::Float(nums.iter().sum::<f64>() / nums.len() as f64)
                    }
                }

                "min" => {
                    if args.is_empty() {
                        return LoraValue::Null;
                    }

                    let mut values = rows
                        .iter()
                        .map(|r| eval_expr(&args[0], r, eval_ctx))
                        .filter(|v| !matches!(v, LoraValue::Null))
                        .collect::<Vec<_>>();

                    if *distinct {
                        values = dedup_values(values);
                    }

                    values
                        .into_iter()
                        .min_by(compare_values_total)
                        .unwrap_or(LoraValue::Null)
                }

                "max" => {
                    if args.is_empty() {
                        return LoraValue::Null;
                    }

                    let mut values = rows
                        .iter()
                        .map(|r| eval_expr(&args[0], r, eval_ctx))
                        .filter(|v| !matches!(v, LoraValue::Null))
                        .collect::<Vec<_>>();

                    if *distinct {
                        values = dedup_values(values);
                    }

                    values
                        .into_iter()
                        .max_by(compare_values_total)
                        .unwrap_or(LoraValue::Null)
                }

                "stdev" | "stdevp" => {
                    if args.is_empty() {
                        return LoraValue::Null;
                    }

                    let nums: Vec<f64> = rows
                        .iter()
                        .map(|r| eval_expr(&args[0], r, eval_ctx))
                        .filter_map(as_f64_lossy)
                        .collect();

                    let is_population = func == "stdevp";

                    if nums.is_empty() || (!is_population && nums.len() < 2) {
                        return LoraValue::Float(0.0);
                    }

                    let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                    let variance_sum: f64 = nums.iter().map(|x| (x - mean).powi(2)).sum();
                    let denom = if is_population {
                        nums.len() as f64
                    } else {
                        (nums.len() - 1) as f64
                    };
                    LoraValue::Float((variance_sum / denom).sqrt())
                }

                "percentilecont" => {
                    if args.len() < 2 {
                        return LoraValue::Null;
                    }

                    let percentile = eval_expr(&args[1], &rows[0], eval_ctx)
                        .as_f64()
                        .unwrap_or(0.5);
                    let mut nums: Vec<f64> = rows
                        .iter()
                        .map(|r| eval_expr(&args[0], r, eval_ctx))
                        .filter_map(as_f64_lossy)
                        .collect();

                    if nums.is_empty() {
                        return LoraValue::Null;
                    }

                    nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

                    let index = percentile * (nums.len() - 1) as f64;
                    let lower = index.floor() as usize;
                    let upper = index.ceil() as usize;
                    let fraction = index - lower as f64;

                    if lower == upper || upper >= nums.len() {
                        LoraValue::Float(nums[lower])
                    } else {
                        LoraValue::Float(nums[lower] * (1.0 - fraction) + nums[upper] * fraction)
                    }
                }

                "percentiledisc" => {
                    if args.len() < 2 {
                        return LoraValue::Null;
                    }

                    let percentile = eval_expr(&args[1], &rows[0], eval_ctx)
                        .as_f64()
                        .unwrap_or(0.5);
                    let mut nums: Vec<f64> = rows
                        .iter()
                        .map(|r| eval_expr(&args[0], r, eval_ctx))
                        .filter_map(as_f64_lossy)
                        .collect();

                    if nums.is_empty() {
                        return LoraValue::Null;
                    }

                    nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

                    let index = (percentile * (nums.len() - 1) as f64).round() as usize;
                    let index = index.min(nums.len() - 1);
                    LoraValue::Float(nums[index])
                }

                _ => rows
                    .first()
                    .map(|r| eval_expr(expr, r, eval_ctx))
                    .unwrap_or(LoraValue::Null),
            }
        }

        _ => rows
            .first()
            .map(|r| eval_expr(expr, r, eval_ctx))
            .unwrap_or(LoraValue::Null),
    }
}

pub(crate) fn compare_sort_item<S: GraphStorage>(
    item: &ResolvedSortItem,
    a: &Row,
    b: &Row,
    eval_ctx: &EvalContext<'_, S>,
) -> Ordering {
    let av = eval_expr(&item.expr, a, eval_ctx);
    let bv = eval_expr(&item.expr, b, eval_ctx);

    let ascending = matches!(item.direction, lora_ast::SortDirection::Asc);
    compare_values_for_sort(&av, &bv, ascending)
}

fn dedup_values(values: Vec<LoraValue>) -> Vec<LoraValue> {
    let mut seen: BTreeSet<GroupValueKey> = BTreeSet::new();
    let mut out = Vec::new();

    for value in values {
        let key = GroupValueKey::from_value(&value);
        if seen.insert(key) {
            out.push(value);
        }
    }

    out
}

fn as_f64_lossy(v: LoraValue) -> Option<f64> {
    match v {
        LoraValue::Int(i) => Some(i as f64),
        LoraValue::Float(f) => Some(f),
        _ => None,
    }
}

fn compare_values_for_sort(a: &LoraValue, b: &LoraValue, ascending: bool) -> Ordering {
    let ord = match (a, b) {
        (LoraValue::Null, LoraValue::Null) => Ordering::Equal,
        (LoraValue::Null, _) => Ordering::Greater,
        (_, LoraValue::Null) => Ordering::Less,
        _ => compare_values_total(a, b),
    };

    if ascending {
        ord
    } else {
        ord.reverse()
    }
}

fn compare_values_total(a: &LoraValue, b: &LoraValue) -> Ordering {
    use LoraValue::*;

    match (a, b) {
        (Bool(x), Bool(y)) => x.cmp(y),
        (Int(x), Int(y)) => x.cmp(y),
        (Float(x), Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Int(x), Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal),
        (Float(x), Int(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal),
        (String(x), String(y)) => x.cmp(y),
        (Node(x), Node(y)) => x.cmp(y),
        (Relationship(x), Relationship(y)) => x.cmp(y),
        (Date(x), Date(y)) => x.cmp(y),
        (DateTime(x), DateTime(y)) => x.cmp(y),
        (Duration(x), Duration(y)) => x.cmp(y),
        (Vector(x), Vector(y)) => x.to_key_string().cmp(&y.to_key_string()),
        _ => type_rank(a)
            .cmp(&type_rank(b))
            .then_with(|| format!("{a:?}").cmp(&format!("{b:?}"))),
    }
}

pub fn value_matches_property_value(expected: &LoraValue, actual: &PropertyValue) -> bool {
    match (expected, actual) {
        (LoraValue::Null, PropertyValue::Null) => true,
        (LoraValue::Bool(a), PropertyValue::Bool(b)) => a == b,
        (LoraValue::Int(a), PropertyValue::Int(b)) => a == b,
        (LoraValue::Float(a), PropertyValue::Float(b)) => a == b,
        (LoraValue::Int(a), PropertyValue::Float(b)) => (*a as f64) == *b,
        (LoraValue::Float(a), PropertyValue::Int(b)) => *a == (*b as f64),
        (LoraValue::String(a), PropertyValue::String(b)) => a == b,

        (LoraValue::List(xs), PropertyValue::List(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|(x, y)| value_matches_property_value(x, y))
        }

        (LoraValue::Map(xm), PropertyValue::Map(ym)) => xm.iter().all(|(k, xv)| {
            ym.get(k)
                .map(|yv| value_matches_property_value(xv, yv))
                .unwrap_or(false)
        }),

        (LoraValue::Date(a), PropertyValue::Date(b)) => a == b,
        (LoraValue::DateTime(a), PropertyValue::DateTime(b)) => a == b,
        (LoraValue::LocalDateTime(a), PropertyValue::LocalDateTime(b)) => a == b,
        (LoraValue::Time(a), PropertyValue::Time(b)) => a == b,
        (LoraValue::LocalTime(a), PropertyValue::LocalTime(b)) => a == b,
        (LoraValue::Duration(a), PropertyValue::Duration(b)) => a == b,
        (LoraValue::Point(a), PropertyValue::Point(b)) => a == b,
        (LoraValue::Vector(a), PropertyValue::Vector(b)) => a == b,

        _ => false,
    }
}

pub(crate) fn node_matches_property_filter<S: GraphStorage>(
    storage: &S,
    node_id: NodeId,
    labels: &[Vec<String>],
    key: &str,
    expected: &LoraValue,
) -> bool {
    storage
        .with_node(node_id, |node| {
            node_matches_label_groups(&node.labels, labels)
                && node
                    .properties
                    .get(key)
                    .map(|actual| value_matches_property_value(expected, actual))
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}

fn single_label_hint(labels: &[Vec<String>]) -> Option<&str> {
    if labels.len() == 1 && labels[0].len() == 1 {
        Some(labels[0][0].as_str())
    } else {
        None
    }
}

fn property_lookup_values(expected: &LoraValue) -> Option<Vec<PropertyValue>> {
    let property = lora_value_to_property(expected.clone()).ok()?;
    let mut values = vec![property.clone()];

    match property {
        PropertyValue::Int(i) => {
            values.push(PropertyValue::Float(i as f64));
        }
        PropertyValue::Float(f)
            if f.is_finite()
                && f.fract() == 0.0
                && f >= i64::MIN as f64
                && f <= i64::MAX as f64 =>
        {
            values.push(PropertyValue::Int(f as i64));
        }
        _ => {}
    }

    Some(values)
}

pub(crate) struct NodePropertyCandidates {
    pub(crate) ids: Vec<NodeId>,
    pub(crate) prefiltered: bool,
}

pub(crate) fn indexed_node_property_candidates<S: GraphStorage>(
    storage: &S,
    labels: &[Vec<String>],
    key: &str,
    expected: &LoraValue,
) -> NodePropertyCandidates {
    let Some(values) = property_lookup_values(expected) else {
        return NodePropertyCandidates {
            ids: scan_node_ids_for_label_groups(storage, labels),
            prefiltered: false,
        };
    };

    let label_hint = single_label_hint(labels);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        for id in storage.find_node_ids_by_property(label_hint, key, &value) {
            if seen.insert(id) {
                out.push(id);
            }
        }
    }
    NodePropertyCandidates {
        ids: out,
        prefiltered: labels.is_empty() || label_hint.is_some(),
    }
}

/// Build a LoraPath from the node and relationship variables currently in a row.
///
/// For variable-length relationships (stored as a List of Relationship values),
/// intermediate nodes are reconstructed from the storage by walking the
/// relationship chain.
pub(crate) fn build_path_value<S: GraphStorage>(
    row: &Row,
    node_vars: &[VarId],
    rel_vars: &[VarId],
    storage: &S,
) -> LoraValue {
    let mut raw_nodes = Vec::new();
    let mut rels = Vec::new();
    let mut has_var_len = false;

    for &nv in node_vars {
        match row.get(nv) {
            Some(LoraValue::Node(id)) => raw_nodes.push(*id),
            Some(LoraValue::List(items)) => {
                for item in items {
                    if let LoraValue::Node(id) = item {
                        raw_nodes.push(*id);
                    }
                }
            }
            _ => {}
        }
    }

    for &rv in rel_vars {
        match row.get(rv) {
            Some(LoraValue::Relationship(id)) => rels.push(*id),
            Some(LoraValue::List(items)) => {
                has_var_len = true;
                for item in items {
                    if let LoraValue::Relationship(id) = item {
                        rels.push(*id);
                    }
                }
            }
            _ => {}
        }
    }

    // For variable-length paths, reconstruct the full node sequence from the
    // relationship chain. raw_nodes typically only has [start, end] but the
    // path needs all intermediate nodes as well.
    let nodes = if has_var_len && !rels.is_empty() && raw_nodes.len() == 2 {
        let start = raw_nodes[0];
        let mut ordered = Vec::with_capacity(rels.len() + 1);
        ordered.push(start);
        let mut current = start;
        for &rel_id in &rels {
            if let Some((src, dst)) = storage.relationship_endpoints(rel_id) {
                let next = if src == current { dst } else { src };
                ordered.push(next);
                current = next;
            }
        }
        ordered
    } else {
        raw_nodes
    };

    LoraValue::Path(LoraPath { nodes, rels })
}

fn type_rank(v: &LoraValue) -> u8 {
    match v {
        LoraValue::Null => 0,
        LoraValue::Bool(_) => 1,
        LoraValue::Int(_) | LoraValue::Float(_) => 2,
        LoraValue::String(_) => 3,
        LoraValue::Date(_) => 4,
        LoraValue::DateTime(_) => 5,
        LoraValue::LocalDateTime(_) => 6,
        LoraValue::Time(_) => 7,
        LoraValue::LocalTime(_) => 8,
        LoraValue::Duration(_) => 9,
        LoraValue::Point(_) => 10,
        LoraValue::Vector(_) => 11,
        LoraValue::List(_) => 12,
        LoraValue::Map(_) => 13,
        LoraValue::Node(_) => 14,
        LoraValue::Relationship(_) => 15,
        LoraValue::Path(_) => 16,
    }
}

/// Check whether a node's labels satisfy all label groups.
/// Each group is a disjunction (OR): the node must have at least one label
/// from the group.  Groups are conjunctive (AND): all groups must be satisfied.
pub(crate) fn node_matches_label_groups(node_labels: &[String], groups: &[Vec<String>]) -> bool {
    groups
        .iter()
        .all(|group| group.iter().any(|l| node_labels.iter().any(|nl| nl == l)))
}

/// Scan the graph for candidate node IDs matching the label groups. Uses the
/// label index for the pick-first-label phase and avoids cloning NodeRecords.
pub(crate) fn scan_node_ids_for_label_groups<S: GraphStorage>(
    storage: &S,
    groups: &[Vec<String>],
) -> Vec<lora_store::NodeId> {
    if groups.is_empty() {
        storage.all_node_ids()
    } else if groups.len() == 1 && groups[0].len() == 1 {
        storage.node_ids_by_label(&groups[0][0])
    } else if groups.len() == 1 && groups[0].len() > 1 {
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for label in &groups[0] {
            for id in storage.node_ids_by_label(label) {
                if seen.insert(id) {
                    out.push(id);
                }
            }
        }
        out
    } else {
        storage.node_ids_by_label(&groups[0][0])
    }
}

pub(crate) fn label_group_candidates_prefiltered(groups: &[Vec<String>]) -> bool {
    groups.len() <= 1
}

pub(crate) fn hydrate_node_record(node: &lora_store::NodeRecord) -> LoraValue {
    let mut map = BTreeMap::new();
    map.insert("kind".to_string(), LoraValue::String("node".to_string()));
    map.insert("id".to_string(), LoraValue::Int(node.id as i64));
    map.insert(
        "labels".to_string(),
        LoraValue::List(
            node.labels
                .iter()
                .map(|s| LoraValue::String(s.clone()))
                .collect(),
        ),
    );
    map.insert(
        "properties".to_string(),
        properties_to_value_map(&node.properties),
    );
    LoraValue::Map(map)
}

pub(crate) fn hydrate_relationship_record(rel: &lora_store::RelationshipRecord) -> LoraValue {
    let mut map = BTreeMap::new();
    map.insert(
        "kind".to_string(),
        LoraValue::String("relationship".to_string()),
    );
    map.insert("id".to_string(), LoraValue::Int(rel.id as i64));
    map.insert("startId".to_string(), LoraValue::Int(rel.src as i64));
    map.insert("endId".to_string(), LoraValue::Int(rel.dst as i64));
    map.insert("type".to_string(), LoraValue::String(rel.rel_type.clone()));
    map.insert(
        "properties".to_string(),
        properties_to_value_map(&rel.properties),
    );
    LoraValue::Map(map)
}

/// Flatten label groups into a simple Vec<String> (for CREATE/MERGE where
/// disjunction doesn't apply — all labels are created).
fn flatten_label_groups(groups: &[Vec<String>]) -> Vec<String> {
    groups.iter().flat_map(|g| g.iter().cloned()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GroupValueKey {
    Null,
    Bool(bool),
    Int(i64),
    Float(String),
    String(String),
    List(Vec<GroupValueKey>),
    Map(Vec<(String, GroupValueKey)>),
    Node(u64),
    Relationship(u64),
}

impl GroupValueKey {
    pub(crate) fn from_value(v: &LoraValue) -> Self {
        match v {
            LoraValue::Null => Self::Null,
            LoraValue::Bool(x) => Self::Bool(*x),
            LoraValue::Int(x) => Self::Int(*x),
            LoraValue::Float(x) => Self::Float(x.to_string()),
            LoraValue::String(x) => Self::String(x.clone()),
            LoraValue::List(xs) => Self::List(xs.iter().map(Self::from_value).collect()),
            LoraValue::Map(m) => Self::Map(
                m.iter()
                    .map(|(k, v)| (k.clone(), Self::from_value(v)))
                    .collect(),
            ),
            LoraValue::Node(id) => Self::Node(*id),
            LoraValue::Relationship(id) => Self::Relationship(*id),
            LoraValue::Path(_) => Self::Null,
            // Temporal types: use their string representation as group key
            LoraValue::Date(d) => Self::String(d.to_string()),
            LoraValue::DateTime(dt) => Self::String(dt.to_string()),
            LoraValue::LocalDateTime(dt) => Self::String(dt.to_string()),
            LoraValue::Time(t) => Self::String(t.to_string()),
            LoraValue::LocalTime(t) => Self::String(t.to_string()),
            LoraValue::Duration(dur) => Self::String(dur.to_string()),
            LoraValue::Point(p) => Self::String(p.to_string()),
            LoraValue::Vector(v) => Self::String(format!("vector:{}", v.to_key_string())),
        }
    }
}

/// Compute effective (min_hops, max_hops) from a `RangeLiteral`.
///
/// Lora semantics:
/// - `*`       → 1..∞   (start=None, end=None)
/// - `*2..5`   → 2..5   (start=Some(2), end=Some(5))
/// - `*..3`    → 1..3   (start=None, end=Some(3))
/// - `*2..`    → 2..∞   (start=Some(2), end=None)
/// - `*3`      → 3..3   (start=Some(3), end=None, no dots → exactly 3)
/// - `*0..1`   → 0..1
///
/// For unbounded upper, we cap at `MAX_VAR_LEN_HOPS` to prevent runaway.
const MAX_VAR_LEN_HOPS: u64 = 100;

pub(crate) fn resolve_range(range: &RangeLiteral) -> (u64, u64) {
    let min_hops = range.start.unwrap_or(1);
    let max_hops = range.end.unwrap_or(MAX_VAR_LEN_HOPS);
    (min_hops, max_hops)
}

/// An entry produced during BFS variable-length expansion.
pub(crate) struct VarLenResult {
    /// The destination node at the end of this path.
    pub(crate) dst_node_id: NodeId,
    /// The relationship IDs traversed (in order).
    pub(crate) rel_ids: Vec<u64>,
}

/// Perform variable-length expansion from `start_node_id` following
/// relationships of the given `types` and `direction`, collecting all
/// reachable nodes at hop distances in `[min_hops, max_hops]`.
///
/// Uses BFS with relationship-uniqueness per path (each path does not
/// reuse the same relationship, but may revisit nodes).
pub(crate) fn variable_length_expand<S: GraphStorage>(
    storage: &S,
    start_node_id: NodeId,
    direction: Direction,
    types: &[String],
    min_hops: u64,
    max_hops: u64,
) -> Vec<VarLenResult> {
    let mut results = Vec::new();

    // Each frontier entry: (current_node_id, relationships_used_so_far)
    let mut frontier: Vec<(NodeId, Vec<u64>)> = vec![(start_node_id, Vec::new())];

    for depth in 1..=max_hops {
        // On the final hop we don't need to build next_frontier at all; every
        // path gets recorded and then the loop terminates. Avoids one full
        // pass of Vec clones on deep traversals.
        let is_last_hop = depth == max_hops;
        let mut next_frontier: Vec<(NodeId, Vec<u64>)> = Vec::new();

        for (current_node, rels_used) in &frontier {
            // ID-only expand avoids cloning full records/properties for every
            // neighbour on every hop.
            for (rel_id, neighbor_id) in storage.expand_ids(*current_node, direction, types) {
                // Relationship-uniqueness: skip if this relationship was already
                // traversed on this particular path.
                if rels_used.contains(&rel_id) {
                    continue;
                }

                if is_last_hop {
                    // Terminal hop: just record the result. Allocate rel_ids
                    // once (no duplicate clone) by extending a fresh copy.
                    if depth >= min_hops {
                        let mut rel_ids = Vec::with_capacity(rels_used.len() + 1);
                        rel_ids.extend_from_slice(rels_used);
                        rel_ids.push(rel_id);
                        results.push(VarLenResult {
                            dst_node_id: neighbor_id,
                            rel_ids,
                        });
                    }
                    continue;
                }

                let mut new_rels = Vec::with_capacity(rels_used.len() + 1);
                new_rels.extend_from_slice(rels_used);
                new_rels.push(rel_id);

                if depth >= min_hops {
                    results.push(VarLenResult {
                        dst_node_id: neighbor_id,
                        rel_ids: new_rels.clone(),
                    });
                }

                next_frontier.push((neighbor_id, new_rels));
            }
        }

        if is_last_hop || next_frontier.is_empty() {
            break;
        }

        frontier = next_frontier;
    }

    // Handle min_hops == 0: include the start node itself at depth 0.
    if min_hops == 0 {
        results.insert(
            0,
            VarLenResult {
                dst_node_id: start_node_id,
                rel_ids: Vec::new(),
            },
        );
    }

    results
}

/// Filter rows to keep only shortest paths.
/// `all` = false → keep one shortest path; `all` = true → keep all shortest.
pub(crate) fn filter_shortest_paths(rows: Vec<Row>, path_var: VarId, all: bool) -> Vec<Row> {
    if rows.is_empty() {
        return rows;
    }

    // Compute path length for each row
    let lengths: Vec<usize> = rows
        .iter()
        .map(|row| match row.get(path_var) {
            Some(LoraValue::Path(p)) => p.rels.len(),
            _ => usize::MAX,
        })
        .collect();

    let min_len = lengths.iter().copied().min().unwrap_or(usize::MAX);

    let mut result: Vec<Row> = rows
        .into_iter()
        .zip(lengths.iter())
        .filter(|(_, len)| **len == min_len)
        .map(|(row, _)| row)
        .collect();

    if !all && result.len() > 1 {
        result.truncate(1);
    }

    result
}
