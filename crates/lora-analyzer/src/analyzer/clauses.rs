use super::expressions::expr_contains_aggregate;
use super::state::{Analyzer, PatternContext};
use crate::{errors::*, resolved::*, symbols::*};
use lora_ast::{
    Create, Delete, Expr, Foreach, InQueryCall, Match, Merge, ProjectionBody, ProjectionItem,
    Remove, RemoveItem, Return, Set, SetItem, Span, Unwind, UpdatingClause, Variable, With,
};
use lora_store::GraphCatalog;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub(super) struct ExportedAlias {
    pub(super) name: String,
    pub(super) id: VarId,
}

#[derive(Debug, Clone)]
pub(super) struct AnalyzedProjectionBody {
    pub(super) items: Vec<ResolvedProjection>,
    pub(super) lifted_aggregates: Vec<ResolvedProjection>,
    pub(super) include_existing: bool,
    pub(super) exported_aliases: Vec<ExportedAlias>,
    pub(super) order: Vec<ResolvedSortItem>,
    pub(super) skip: Option<ResolvedExpr>,
    pub(super) limit: Option<ResolvedExpr>,
}

impl<'a, S: GraphCatalog + ?Sized> Analyzer<'a, S> {
    pub(super) fn analyze_match(&mut self, m: &Match) -> Result<ResolvedMatch, SemanticError> {
        let ctx = if m.optional {
            PatternContext::OptionalRead
        } else {
            PatternContext::Read
        };
        let pattern = self.analyze_pattern(&m.pattern, ctx)?;
        let where_ = m
            .where_
            .as_ref()
            .map(|e| self.analyze_expr(e))
            .transpose()?;

        if let Some(ref w) = where_ {
            if expr_contains_aggregate(w) {
                return Err(SemanticError::AggregationInWhere);
            }
        }

        Ok(ResolvedMatch {
            optional: m.optional,
            pattern,
            where_,
        })
    }

    pub(super) fn analyze_unwind(&mut self, u: &Unwind) -> Result<ResolvedUnwind, SemanticError> {
        let expr = self.analyze_expr(&u.expr)?;
        let alias = self.declare_fresh_variable(&u.alias.name)?;
        // UNWIND iterates a list — typically `$rows` from a batched
        // import, where each element is a map of arbitrary keys. The
        // analyzer can't know the row shape, so let property access on
        // the alias skip the graph-catalog check.
        self.dynamic_property_vars.insert(alias);

        Ok(ResolvedUnwind { expr, alias })
    }

    /// `CALL proc(args) YIELD f [AS a], ... [WHERE cond]` as a clause.
    ///
    /// Supported procedures are the index queries. The call is rewritten
    /// into clauses the rest of the pipeline already runs:
    ///
    /// ```text
    /// UNWIND index.<op>(args) AS <hit>
    /// WITH *, <hit>.f AS a, ... [WHERE cond]
    /// ```
    ///
    /// so each hit becomes a row whose yielded variables are ordinary
    /// bindings: a yielded `node` is a bound node that later `MATCH`
    /// patterns expand from, and `score` is a number.
    pub(super) fn analyze_in_query_call(
        &mut self,
        call: &InQueryCall,
    ) -> Result<Vec<ResolvedClause>, SemanticError> {
        let span = call.span;
        let qualified = call.procedure.name.parts.join(".");
        let (function, entity_field) = match qualified.to_ascii_lowercase().as_str() {
            "db.index.fulltext.querynodes" => ("fulltext_nodes", "node"),
            "db.index.fulltext.queryrelationships" => ("fulltext_relationships", "relationship"),
            "db.index.vector.querynodes" => ("vector_nodes", "node"),
            "db.index.vector.queryrelationships" => ("vector_relationships", "relationship"),
            _ => {
                return Err(SemanticError::UnsupportedFeature(format!(
                    "unknown procedure `{qualified}` (supported in CALL ... YIELD: \
                     db.index.fulltext.queryNodes, db.index.fulltext.queryRelationships, \
                     db.index.vector.queryNodes, db.index.vector.queryRelationships)"
                )))
            }
        };
        if call.yield_items.is_empty() {
            return Err(SemanticError::UnsupportedFeature(format!(
                "CALL {qualified}(...) inside a query needs YIELD (fields: {entity_field}, score)"
            )));
        }

        // Unique per call site so two CALLs in one query don't collide.
        let hit_name = format!("__call_hit_{}", span.start);
        let hit = Variable {
            name: hit_name,
            span,
        };
        let unwind = Unwind {
            expr: Expr::FunctionCall {
                name: vec!["index".to_string(), function.to_string()],
                distinct: false,
                args: call.procedure.args.clone(),
                span,
            },
            alias: hit.clone(),
            span,
        };

        let mut items = vec![ProjectionItem::Star { span }];
        for item in &call.yield_items {
            let field = item
                .field
                .clone()
                .unwrap_or_else(|| item.alias.name.clone());
            if field != entity_field && field != "score" {
                return Err(SemanticError::UnsupportedFeature(format!(
                    "{qualified} yields `{entity_field}` and `score`, not `{field}`"
                )));
            }
            items.push(ProjectionItem::Expr {
                expr: Expr::Property {
                    expr: Box::new(Expr::Variable(hit.clone())),
                    key: field,
                    span: item.span,
                },
                alias: Some(item.alias.clone()),
                span: item.span,
            });
        }
        let with = With {
            body: ProjectionBody {
                distinct: false,
                items,
                order: Vec::new(),
                skip: None,
                limit: None,
                span,
            },
            where_: call.where_.clone(),
            span,
        };

        Ok(vec![
            ResolvedClause::Unwind(self.analyze_unwind(&unwind)?),
            ResolvedClause::With(self.analyze_with(&with)?),
        ])
    }

    pub(super) fn analyze_create(&mut self, c: &Create) -> Result<ResolvedCreate, SemanticError> {
        let pattern = self.analyze_pattern(&c.pattern, PatternContext::Write)?;
        Ok(ResolvedCreate { pattern })
    }

    pub(super) fn analyze_merge(&mut self, m: &Merge) -> Result<ResolvedMerge, SemanticError> {
        let pattern_part = self.analyze_pattern_part(&m.pattern_part, PatternContext::Write)?;
        let mut actions = Vec::with_capacity(m.actions.len());

        for action in &m.actions {
            actions.push(ResolvedMergeAction {
                on_match: action.on_match,
                set: self.analyze_set(&action.set)?,
            });
        }

        Ok(ResolvedMerge {
            pattern_part,
            actions,
        })
    }

    pub(super) fn analyze_delete(&mut self, d: &Delete) -> Result<ResolvedDelete, SemanticError> {
        let expressions = d
            .expressions
            .iter()
            .map(|e| self.analyze_expr(e))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ResolvedDelete {
            detach: d.detach,
            expressions,
        })
    }

    pub(super) fn analyze_set(&mut self, s: &Set) -> Result<ResolvedSet, SemanticError> {
        let mut items = Vec::with_capacity(s.items.len());

        for item in &s.items {
            match item {
                SetItem::SetProperty { target, value, .. } => {
                    // SET target (e.g. n.prop) allows new property names since
                    // the SET is creating/updating properties.
                    items.push(ResolvedSetItem::SetProperty {
                        target: self.analyze_expr_write_property(target)?,
                        value: self.analyze_expr(value)?,
                    });
                }
                SetItem::SetVariable {
                    variable, value, ..
                } => {
                    let var = self.resolve_required_variable(&variable.name)?;
                    items.push(ResolvedSetItem::SetVariable {
                        variable: var,
                        value: self.analyze_expr(value)?,
                    });
                }
                SetItem::MutateVariable {
                    variable, value, ..
                } => {
                    let var = self.resolve_required_variable(&variable.name)?;
                    items.push(ResolvedSetItem::MutateVariable {
                        variable: var,
                        value: self.analyze_expr(value)?,
                    });
                }
                SetItem::SetLabels {
                    variable, labels, ..
                } => {
                    let var = self.resolve_required_variable(&variable.name)?;
                    for label in labels {
                        self.validate_label_name(label, PatternContext::Write)?;
                    }
                    items.push(ResolvedSetItem::SetLabels {
                        variable: var,
                        labels: labels.clone(),
                    });
                }
            }
        }

        Ok(ResolvedSet { items })
    }

    pub(super) fn analyze_foreach(
        &mut self,
        f: &Foreach,
    ) -> Result<ResolvedForeach, SemanticError> {
        // The list expression is evaluated in the outer scope.
        let list = self.analyze_expr(&f.list)?;

        // Push the loop variable into a fresh scope so the body sees it
        // and shadowing rules track properly. Snapshot the outer scope
        // so we can restore it once the body is analyzed — the loop
        // variable must not leak into clauses that follow FOREACH.
        let outer = self.visible_bindings();
        let var_id = self.declare_fresh_variable(&f.variable.name)?;

        let mut body = Vec::with_capacity(f.body.len());
        for clause in &f.body {
            body.push(self.analyze_foreach_body_clause(clause)?);
        }

        self.replace_scope(outer);

        Ok(ResolvedForeach {
            variable: var_id,
            list,
            body,
        })
    }

    /// Analyze one body clause inside `FOREACH`. The body is restricted
    /// to updating clauses (Create / Merge / Delete / Set / Remove /
    /// nested Foreach) — reading clauses and RETURN are not legal there.
    fn analyze_foreach_body_clause(
        &mut self,
        uc: &UpdatingClause,
    ) -> Result<ResolvedClause, SemanticError> {
        match uc {
            UpdatingClause::Create(c) => Ok(ResolvedClause::Create(self.analyze_create(c)?)),
            UpdatingClause::Merge(m) => Ok(ResolvedClause::Merge(self.analyze_merge(m)?)),
            UpdatingClause::Delete(d) => Ok(ResolvedClause::Delete(self.analyze_delete(d)?)),
            UpdatingClause::Set(s) => Ok(ResolvedClause::Set(self.analyze_set(s)?)),
            UpdatingClause::Remove(r) => Ok(ResolvedClause::Remove(self.analyze_remove(r)?)),
            UpdatingClause::Foreach(f) => Ok(ResolvedClause::Foreach(self.analyze_foreach(f)?)),
        }
    }

    pub(super) fn analyze_remove(&mut self, r: &Remove) -> Result<ResolvedRemove, SemanticError> {
        let mut items = Vec::with_capacity(r.items.len());

        for item in &r.items {
            match item {
                RemoveItem::Labels {
                    variable, labels, ..
                } => {
                    let var = self.resolve_required_variable(&variable.name)?;
                    items.push(ResolvedRemoveItem::Labels {
                        variable: var,
                        labels: labels.clone(),
                    });
                }
                RemoveItem::Property { expr, .. } => {
                    items.push(ResolvedRemoveItem::Property {
                        expr: self.analyze_expr(expr)?,
                    });
                }
            }
        }

        Ok(ResolvedRemove { items })
    }

    pub(super) fn analyze_return(&mut self, r: &Return) -> Result<ResolvedReturn, SemanticError> {
        let analyzed = self.analyze_projection_body(&r.body)?;

        Ok(ResolvedReturn {
            distinct: r.body.distinct,
            items: analyzed.items,
            lifted_aggregates: analyzed.lifted_aggregates,
            include_existing: analyzed.include_existing,
            order: analyzed.order,
            skip: analyzed.skip,
            limit: analyzed.limit,
        })
    }

    pub(super) fn analyze_with(&mut self, w: &With) -> Result<ResolvedWith, SemanticError> {
        let old_scope = self.visible_bindings();
        let analyzed = self.analyze_projection_body(&w.body)?;

        let mut new_scope = BTreeMap::<String, VarId>::new();

        if analyzed.include_existing {
            for (name, id) in old_scope {
                new_scope.insert(name, id);
            }
        }

        for exported in &analyzed.exported_aliases {
            new_scope.insert(exported.name.clone(), exported.id);
        }

        self.replace_scope(new_scope);

        let where_ = w
            .where_
            .as_ref()
            .map(|e| self.analyze_expr(e))
            .transpose()?;

        Ok(ResolvedWith {
            distinct: w.body.distinct,
            items: analyzed.items,
            lifted_aggregates: analyzed.lifted_aggregates,
            include_existing: analyzed.include_existing,
            order: analyzed.order,
            skip: analyzed.skip,
            limit: analyzed.limit,
            where_,
        })
    }

    fn analyze_projection_body(
        &mut self,
        body: &ProjectionBody,
    ) -> Result<AnalyzedProjectionBody, SemanticError> {
        let first_var = self.symbols.next_var();
        let mut items = Vec::new();
        let mut include_existing = false;
        let mut exported_aliases = Vec::new();
        let mut seen_alias_names = BTreeSet::new();

        for item in &body.items {
            match item {
                ProjectionItem::Expr { expr, alias, span } => {
                    let resolved = self.analyze_expr(expr)?;

                    let explicit = alias.is_some();
                    let name = if let Some(var) = alias {
                        if !seen_alias_names.insert(var.name.clone()) {
                            return Err(SemanticError::DuplicateProjectionAlias(var.name.clone()));
                        }
                        var.name.clone()
                    } else {
                        projection_name(expr)
                    };

                    let output = self.symbols.new_var();

                    exported_aliases.push(ExportedAlias {
                        name: name.clone(),
                        id: output,
                    });

                    items.push(ResolvedProjection {
                        expr: resolved,
                        output,
                        name: name.into(),
                        explicit_alias: explicit,
                        span: *span,
                    });
                }

                ProjectionItem::Star { .. } => {
                    include_existing = true;
                }
            }
        }

        // The items as written, before lifting rewrites them: an ORDER BY key
        // that restates one sorts by its column.
        let written: Vec<(String, VarId, bool)> = items
            .iter()
            .map(|item| {
                (
                    format!("{:?}", item.expr),
                    item.output,
                    expr_contains_aggregate(&item.expr),
                )
            })
            .collect();
        let mut lifted_aggregates = self.lift_nested_aggregates(&mut items, first_var)?;

        // Build a lookup from alias names to their output VarIds so ORDER BY
        // can reference projection aliases (e.g. ORDER BY name when RETURN p.name AS name).
        let alias_map: BTreeMap<String, VarId> = exported_aliases
            .iter()
            .map(|a| (a.name.clone(), a.id))
            .collect();

        let order = body
            .order
            .iter()
            .map(|item| {
                let expr = self.analyze_expr_with_aliases(&item.expr, &alias_map)?;
                Ok(ResolvedSortItem {
                    expr,
                    direction: item.direction,
                })
            })
            .collect::<Result<Vec<_>, SemanticError>>()?;
        let order = self.lift_order_aggregates(
            order,
            &written,
            &mut lifted_aggregates,
            body.distinct,
            first_var,
        )?;

        let skip = body
            .skip
            .as_ref()
            .map(|e| self.analyze_expr(e))
            .transpose()?;
        let limit = body
            .limit
            .as_ref()
            .map(|e| self.analyze_expr(e))
            .transpose()?;

        Ok(AnalyzedProjectionBody {
            items,
            lifted_aggregates,
            include_existing,
            exported_aliases,
            order,
            skip,
            limit,
        })
    }
}

impl<'a, S: GraphCatalog + ?Sized> Analyzer<'a, S> {
    /// Split every item that nests an aggregate inside a larger expression
    /// (`size(collect(x))`, `{c: count(*)}`): each aggregate call becomes a
    /// hidden column and the item reads it, so the planner can aggregate
    /// first and evaluate the item on the grouped row. Inside such an item,
    /// a sub-expression that restates a grouping key reads that key's column.
    ///
    /// Anything else the item reads from before this projection would be a
    /// different value per row of the group, so it is rejected, as in
    /// openCypher. `first_var` is the first id this projection allocated:
    /// lower ids are those earlier bindings, higher ones its own aliases and
    /// comprehension locals.
    fn lift_nested_aggregates(
        &mut self,
        items: &mut [ResolvedProjection],
        first_var: VarId,
    ) -> Result<Vec<ResolvedProjection>, SemanticError> {
        if !items
            .iter()
            .any(|item| expr_contains_aggregate(&item.expr) && !is_bare_aggregate(&item.expr))
        {
            return Ok(Vec::new());
        }

        // Grouping keys, compared by their derived `Debug` form (structural
        // and deterministic), with the column each one lands in.
        let keys: Vec<(String, VarId)> = items
            .iter()
            .filter(|item| !expr_contains_aggregate(&item.expr))
            .map(|item| (format!("{:?}", item.expr), item.output))
            .collect();

        let mut lifted = Vec::new();
        for item in items.iter_mut() {
            if !expr_contains_aggregate(&item.expr) || is_bare_aggregate(&item.expr) {
                continue;
            }
            let mut expr = item.expr.clone();
            lift_aggregates_in(
                &mut expr,
                &keys,
                &mut || self.symbols.new_var(),
                &mut lifted,
                item.span,
            );

            if reads_ungrouped(&expr, &keys, first_var) {
                return Err(SemanticError::ImplicitGroupingKey(item.name.to_string()));
            }
            item.expr = expr;
        }
        Ok(lifted)
    }

    /// Point ORDER BY keys of an aggregating projection at what they mean
    /// on the grouped rows. A key that restates an item (as written, before
    /// lifting) sorts by that item's column; an aggregate in any other key
    /// is lifted into a hidden column like a nested one in an item, the
    /// planner keeping it until the sort. A projection that doesn't
    /// aggregate can't aggregate in ORDER BY, and after DISTINCT a key can
    /// only aggregate what is projected (openCypher rejects both).
    fn lift_order_aggregates(
        &mut self,
        order: Vec<ResolvedSortItem>,
        written: &[(String, VarId, bool)],
        lifted: &mut Vec<ResolvedProjection>,
        distinct: bool,
        first_var: VarId,
    ) -> Result<Vec<ResolvedSortItem>, SemanticError> {
        let aggregating = written.iter().any(|(_, _, aggregates)| *aggregates);
        if !aggregating {
            if order.iter().any(|key| expr_contains_aggregate(&key.expr)) {
                return Err(SemanticError::AggregationInOrderBy(
                    "the projection doesn't aggregate".into(),
                ));
            }
            return Ok(order);
        }

        let keys: Vec<(String, VarId)> = written
            .iter()
            .filter(|(_, _, aggregates)| !aggregates)
            .map(|(shape, output, _)| (shape.clone(), *output))
            .collect();
        order
            .into_iter()
            .map(|mut key| {
                let shape = format!("{:?}", key.expr);
                if let Some((_, output, _)) = written.iter().find(|(s, _, _)| *s == shape) {
                    key.expr = ResolvedExpr::Variable(*output);
                    return Ok(key);
                }
                if !expr_contains_aggregate(&key.expr) {
                    return Ok(key);
                }
                if distinct {
                    return Err(SemanticError::AggregationInOrderBy(
                        "after DISTINCT it can only sort by an aggregate that is projected".into(),
                    ));
                }
                lift_aggregates_in(
                    &mut key.expr,
                    &keys,
                    &mut || self.symbols.new_var(),
                    lifted,
                    Span::default(),
                );
                if reads_ungrouped(&key.expr, &keys, first_var) {
                    return Err(SemanticError::ImplicitGroupingKey("ORDER BY".into()));
                }
                Ok(key)
            })
            .collect()
    }
}

/// Whether `expr`, after lifting, reads a binding from before the
/// projection (an id below `first_var`) that isn't a grouping key's
/// column: a value that differs per row of the group.
fn reads_ungrouped(expr: &ResolvedExpr, keys: &[(String, VarId)], first_var: VarId) -> bool {
    let mut reads = BTreeSet::new();
    expr.collect_vars(&mut reads);
    reads
        .iter()
        .any(|v| *v < first_var && !keys.iter().any(|(_, key)| key == v))
}

/// Whether `expr` is itself one aggregate call, the shape the aggregation
/// operator evaluates directly.
fn is_bare_aggregate(expr: &ResolvedExpr) -> bool {
    matches!(expr, ResolvedExpr::Function { function, .. } if function.is_aggregate())
}

/// Replace each aggregate call in `expr` with a fresh variable, recording
/// the call as a hidden projection in `lifted`; replace each aggregate-free
/// sub-expression that restates a grouping key with that key's column.
fn lift_aggregates_in(
    expr: &mut ResolvedExpr,
    keys: &[(String, VarId)],
    new_var: &mut dyn FnMut() -> VarId,
    lifted: &mut Vec<ResolvedProjection>,
    span: Span,
) {
    if is_bare_aggregate(expr) {
        let output = new_var();
        let call = std::mem::replace(expr, ResolvedExpr::Variable(output));
        lifted.push(ResolvedProjection {
            expr: call,
            output,
            name: format!("  aggregate {}", output.0).into(),
            explicit_alias: false,
            span,
        });
        return;
    }
    // Not a key, but its parts may be (`size(n.tags)` with `n.tags` grouped).
    if !keys.is_empty() && !expr_contains_aggregate(expr) {
        let shape = format!("{:?}", expr);
        if let Some((_, output)) = keys.iter().find(|(key, _)| *key == shape) {
            *expr = ResolvedExpr::Variable(*output);
            return;
        }
    }
    let mut go = |e: &mut ResolvedExpr| lift_aggregates_in(e, keys, new_var, lifted, span);
    match expr {
        ResolvedExpr::Variable(_) | ResolvedExpr::Literal(_) | ResolvedExpr::Parameter(_) => {}
        ResolvedExpr::Property { expr, .. } | ResolvedExpr::Unary { expr, .. } => go(expr),
        ResolvedExpr::Binary { lhs, rhs, .. } => {
            go(lhs);
            go(rhs);
        }
        ResolvedExpr::Function { args, .. } => args.iter_mut().for_each(go),
        ResolvedExpr::List(items) => items.iter_mut().for_each(go),
        ResolvedExpr::Map(items) => items.iter_mut().for_each(|(_, v)| go(v)),
        ResolvedExpr::Case {
            input,
            alternatives,
            else_expr,
        } => {
            if let Some(e) = input {
                go(e);
            }
            for (w, t) in alternatives {
                go(w);
                go(t);
            }
            if let Some(e) = else_expr {
                go(e);
            }
        }
        ResolvedExpr::ListPredicate {
            list, predicate, ..
        } => {
            go(list);
            go(predicate);
        }
        ResolvedExpr::ListComprehension {
            list,
            filter,
            map_expr,
            ..
        } => {
            go(list);
            if let Some(e) = filter {
                go(e);
            }
            if let Some(e) = map_expr {
                go(e);
            }
        }
        ResolvedExpr::Reduce {
            init, list, expr, ..
        } => {
            go(init);
            go(list);
            go(expr);
        }
        ResolvedExpr::MapProjection { base, selectors } => {
            go(base);
            for selector in selectors {
                if let ResolvedMapSelector::Literal(_, e) = selector {
                    go(e);
                }
            }
        }
        ResolvedExpr::Index { expr, index } => {
            go(expr);
            go(index);
        }
        ResolvedExpr::Slice { expr, from, to } => {
            go(expr);
            if let Some(e) = from {
                go(e);
            }
            if let Some(e) = to {
                go(e);
            }
        }
        // Their pattern reads row bindings directly; aggregates can't nest there.
        ResolvedExpr::ExistsSubquery { .. } | ResolvedExpr::PatternComprehension { .. } => {}
    }
}

fn projection_name(expr: &Expr) -> String {
    match expr {
        Expr::Variable(v) => v.name.clone(),
        Expr::Property { key, .. } => key.clone(),
        Expr::FunctionCall { name, .. } => {
            name.last().cloned().unwrap_or_else(|| "expr".to_string())
        }
        _ => "expr".to_string(),
    }
}
