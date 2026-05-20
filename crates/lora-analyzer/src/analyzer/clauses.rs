use super::expressions::expr_contains_aggregate;
use super::state::{Analyzer, PatternContext};
use crate::{errors::*, resolved::*, symbols::*};
use lora_ast::{
    Create, Delete, Expr, Foreach, InQueryCall, Match, Merge, ProjectionBody, ProjectionItem,
    Remove, RemoveItem, Return, Set, SetItem, Unwind, UpdatingClause, With,
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

        Ok(ResolvedUnwind { expr, alias })
    }

    pub(super) fn analyze_in_query_call(
        &mut self,
        _call: &InQueryCall,
    ) -> Result<ResolvedClause, SemanticError> {
        Err(SemanticError::UnsupportedFeature(
            "CALL ... YIELD is not yet supported by the analyzer".into(),
        ))
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
                        name,
                        explicit_alias: explicit,
                        span: *span,
                    });
                }

                ProjectionItem::Star { .. } => {
                    include_existing = true;
                }
            }
        }

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
            include_existing,
            exported_aliases,
            order,
            skip,
            limit,
        })
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
