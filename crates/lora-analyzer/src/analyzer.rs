use crate::{errors::*, resolved::*, scope::*, symbols::*};
use lora_ast::{
    Create, Delete, Document, Expr, InQueryCall, MapProjectionSelector, Match, Merge, NodePattern,
    Pattern, PatternElement, PatternPart, ProjectionBody, ProjectionItem, Query, QueryPart,
    ReadingClause, RelationshipPattern, Remove, RemoveItem, Return, Set, SetItem, SinglePartQuery,
    SingleQuery, Statement, Unwind, UpdatingClause, With,
};
use lora_store::GraphStorage;
use std::collections::{BTreeMap, BTreeSet};

pub struct Analyzer<'a, S: GraphStorage + ?Sized> {
    storage: &'a S,
    scopes: ScopeStack,
    symbols: SymbolTable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternContext {
    Read,
    /// OPTIONAL MATCH — tolerate unknown labels/types (they just won't match).
    OptionalRead,
    Write,
}

impl<'a, S: GraphStorage + ?Sized> Analyzer<'a, S> {
    pub fn new(storage: &'a S) -> Self {
        Self {
            storage,
            scopes: ScopeStack::new(),
            symbols: SymbolTable::default(),
        }
    }

    pub fn analyze(&mut self, doc: &Document) -> Result<ResolvedQuery, SemanticError> {
        match &doc.statement {
            Statement::Query(q) => self.analyze_query(q),
        }
    }

    fn analyze_query(&mut self, query: &Query) -> Result<ResolvedQuery, SemanticError> {
        let mut clauses = Vec::new();
        let mut unions = Vec::new();

        match query {
            Query::Regular(r) => {
                clauses.extend(self.analyze_single_query(&r.head)?);

                for union_part in &r.unions {
                    // Each UNION branch gets a fresh scope — variables from one
                    // branch must not leak into another.
                    self.scopes.clear();

                    let branch_clauses = self.analyze_single_query(&union_part.query)?;
                    unions.push(ResolvedUnionPart {
                        all: union_part.all,
                        clauses: branch_clauses,
                    });
                }

                // Validate UNION column compatibility: all branches must
                // have the same number of columns. Column names are taken
                // from the first branch (standard Lora semantics).
                if !unions.is_empty() {
                    let head_cols = return_column_info(&clauses);
                    for branch in &unions {
                        let branch_cols = return_column_info(&branch.clauses);
                        if let (Some(hc), Some(bc)) = (&head_cols, &branch_cols) {
                            if hc.len() != bc.len() {
                                return Err(SemanticError::UnionColumnCountMismatch(
                                    hc.len(),
                                    bc.len(),
                                ));
                            }
                            // Validate column names when at least one side
                            // uses an explicit AS alias.
                            for ((h_name, h_explicit), (b_name, b_explicit)) in
                                hc.iter().zip(bc.iter())
                            {
                                if (*h_explicit || *b_explicit) && h_name != b_name {
                                    return Err(SemanticError::UnionColumnNameMismatch(
                                        h_name.clone(),
                                        b_name.clone(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            Query::StandaloneCall(_) => {
                return Err(SemanticError::UnsupportedFeature(
                    "Standalone CALL is not yet supported by the analyzer".into(),
                ));
            }
        }

        Ok(ResolvedQuery { clauses, unions })
    }

    fn analyze_single_query(
        &mut self,
        q: &SingleQuery,
    ) -> Result<Vec<ResolvedClause>, SemanticError> {
        match q {
            SingleQuery::SinglePart(sp) => self.analyze_single_part(sp),
            SingleQuery::MultiPart(mp) => {
                let mut clauses = Vec::new();

                for part in &mp.parts {
                    clauses.extend(self.analyze_query_part(part)?);
                }

                clauses.extend(self.analyze_single_part(&mp.tail)?);
                Ok(clauses)
            }
        }
    }

    fn analyze_query_part(
        &mut self,
        part: &QueryPart,
    ) -> Result<Vec<ResolvedClause>, SemanticError> {
        let mut clauses = Vec::new();

        for rc in &part.reading_clauses {
            clauses.push(self.analyze_reading_clause(rc)?);
        }

        for uc in &part.updating_clauses {
            clauses.push(self.analyze_updating_clause(uc)?);
        }

        clauses.push(ResolvedClause::With(self.analyze_with(&part.with_clause)?));
        Ok(clauses)
    }

    fn analyze_single_part(
        &mut self,
        q: &SinglePartQuery,
    ) -> Result<Vec<ResolvedClause>, SemanticError> {
        let mut clauses = Vec::new();

        for rc in &q.reading_clauses {
            clauses.push(self.analyze_reading_clause(rc)?);
        }

        for uc in &q.updating_clauses {
            clauses.push(self.analyze_updating_clause(uc)?);
        }

        if let Some(ret) = &q.return_clause {
            clauses.push(ResolvedClause::Return(self.analyze_return(ret)?));
        }

        Ok(clauses)
    }

    fn analyze_reading_clause(
        &mut self,
        rc: &ReadingClause,
    ) -> Result<ResolvedClause, SemanticError> {
        match rc {
            ReadingClause::Match(m) => Ok(ResolvedClause::Match(self.analyze_match(m)?)),
            ReadingClause::Unwind(u) => Ok(ResolvedClause::Unwind(self.analyze_unwind(u)?)),
            ReadingClause::InQueryCall(c) => self.analyze_in_query_call(c),
        }
    }

    fn analyze_updating_clause(
        &mut self,
        uc: &UpdatingClause,
    ) -> Result<ResolvedClause, SemanticError> {
        match uc {
            UpdatingClause::Create(c) => Ok(ResolvedClause::Create(self.analyze_create(c)?)),
            UpdatingClause::Merge(m) => Ok(ResolvedClause::Merge(self.analyze_merge(m)?)),
            UpdatingClause::Delete(d) => Ok(ResolvedClause::Delete(self.analyze_delete(d)?)),
            UpdatingClause::Set(s) => Ok(ResolvedClause::Set(self.analyze_set(s)?)),
            UpdatingClause::Remove(r) => Ok(ResolvedClause::Remove(self.analyze_remove(r)?)),
        }
    }

    fn analyze_match(&mut self, m: &Match) -> Result<ResolvedMatch, SemanticError> {
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

    fn analyze_unwind(&mut self, u: &Unwind) -> Result<ResolvedUnwind, SemanticError> {
        let expr = self.analyze_expr(&u.expr)?;
        let alias = self.declare_fresh_variable(&u.alias.name)?;

        Ok(ResolvedUnwind { expr, alias })
    }

    fn analyze_in_query_call(
        &mut self,
        _call: &InQueryCall,
    ) -> Result<ResolvedClause, SemanticError> {
        Err(SemanticError::UnsupportedFeature(
            "CALL ... YIELD is not yet supported by the analyzer".into(),
        ))
    }

    fn analyze_create(&mut self, c: &Create) -> Result<ResolvedCreate, SemanticError> {
        let pattern = self.analyze_pattern(&c.pattern, PatternContext::Write)?;
        Ok(ResolvedCreate { pattern })
    }

    fn analyze_merge(&mut self, m: &Merge) -> Result<ResolvedMerge, SemanticError> {
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

    fn analyze_delete(&mut self, d: &Delete) -> Result<ResolvedDelete, SemanticError> {
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

    fn analyze_set(&mut self, s: &Set) -> Result<ResolvedSet, SemanticError> {
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

    fn analyze_remove(&mut self, r: &Remove) -> Result<ResolvedRemove, SemanticError> {
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

    fn analyze_pattern(
        &mut self,
        p: &Pattern,
        context: PatternContext,
    ) -> Result<ResolvedPattern, SemanticError> {
        let mut parts = Vec::with_capacity(p.parts.len());

        // In read patterns, detect when the same node variable is used at
        // multiple positions with conflicting labels (e.g. (n:X)-[r]->(n:Y)).
        if matches!(context, PatternContext::Read | PatternContext::OptionalRead) {
            let mut node_labels: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for part in &p.parts {
                self.collect_node_var_labels(&part.element, &mut node_labels);
            }
            for (name, labels_list) in &node_labels {
                // Only reject if the variable appears with distinct non-empty label sets
                if labels_list.len() > 1 {
                    let non_empty: Vec<&String> =
                        labels_list.iter().filter(|l| !l.is_empty()).collect();
                    let unique_labels: BTreeSet<&String> = non_empty.iter().copied().collect();
                    if unique_labels.len() > 1 {
                        return Err(SemanticError::DuplicateVariable(name.clone()));
                    }
                }
            }
        }

        for part in &p.parts {
            parts.push(self.analyze_pattern_part(part, context)?);
        }

        Ok(ResolvedPattern { parts })
    }

    /// Collect (variable_name, labels_string) for each node position in a pattern element.
    fn collect_node_var_labels(
        &self,
        el: &PatternElement,
        map: &mut BTreeMap<String, Vec<String>>,
    ) {
        match el {
            PatternElement::NodeChain { head, chain, .. } => {
                if let Some(ref v) = head.variable {
                    let label_str = format_label_groups(&head.labels);
                    map.entry(v.name.clone()).or_default().push(label_str);
                }
                for step in chain {
                    if let Some(ref v) = step.node.variable {
                        let label_str = format_label_groups(&step.node.labels);
                        map.entry(v.name.clone()).or_default().push(label_str);
                    }
                }
            }
            PatternElement::Parenthesized(inner, _) => {
                self.collect_node_var_labels(inner, map);
            }
            PatternElement::ShortestPath { element, .. } => {
                self.collect_node_var_labels(element, map);
            }
        }
    }

    fn analyze_pattern_part(
        &mut self,
        part: &PatternPart,
        context: PatternContext,
    ) -> Result<ResolvedPatternPart, SemanticError> {
        let binding = part
            .binding
            .as_ref()
            .map(|v| self.declare_or_reuse_variable(&v.name))
            .transpose()?;

        let element = self.analyze_pattern_element(&part.element, context)?;

        Ok(ResolvedPatternPart { binding, element })
    }

    fn analyze_pattern_element(
        &mut self,
        el: &PatternElement,
        context: PatternContext,
    ) -> Result<ResolvedPatternElement, SemanticError> {
        match el {
            PatternElement::NodeChain { head, chain, .. } => {
                if chain.is_empty() {
                    let node = self.analyze_node(head, context)?;
                    return Ok(ResolvedPatternElement::Node {
                        var: node.var,
                        labels: node.labels,
                        properties: node.properties,
                    });
                }

                let head = self.analyze_node(head, context)?;
                let mut resolved_chain = Vec::with_capacity(chain.len());

                for step in chain {
                    let rel = self.analyze_relationship(&step.relationship, context)?;
                    let node = self.analyze_node(&step.node, context)?;
                    resolved_chain.push(ResolvedChain { rel, node });
                }

                Ok(ResolvedPatternElement::NodeChain {
                    head,
                    chain: resolved_chain,
                })
            }

            PatternElement::Parenthesized(inner, _) => self.analyze_pattern_element(inner, context),

            PatternElement::ShortestPath { all, element, .. } => {
                let resolved = self.analyze_pattern_element(element, context)?;
                match resolved {
                    ResolvedPatternElement::NodeChain { head, chain } => {
                        Ok(ResolvedPatternElement::ShortestPath {
                            all: *all,
                            head,
                            chain,
                        })
                    }
                    other => Ok(other),
                }
            }
        }
    }

    fn analyze_node(
        &mut self,
        node: &NodePattern,
        context: PatternContext,
    ) -> Result<ResolvedNode, SemanticError> {
        let var = Some(match &node.variable {
            // Named node — declare in scope so user code can reference it.
            Some(v) => self.declare_or_reuse_variable(&v.name)?,
            // Anonymous node (e.g. `(:Person)`) — allocate an internal VarId
            // but do NOT declare it in the scope, so it cannot be referenced
            // by user expressions and will not appear in projections.
            None => self.symbols.new_var(),
        });

        let labels: Vec<Vec<String>> = node
            .labels
            .iter()
            .map(|group| {
                group
                    .iter()
                    .map(|l| {
                        self.validate_label_name(l, context)?;
                        Ok(l.clone())
                    })
                    .collect::<Result<Vec<_>, SemanticError>>()
            })
            .collect::<Result<Vec<_>, SemanticError>>()?;

        let properties = node
            .properties
            .as_ref()
            .map(|e| self.analyze_property_map_expr(e))
            .transpose()?;

        Ok(ResolvedNode {
            var,
            labels,
            properties,
        })
    }

    fn analyze_relationship(
        &mut self,
        rel: &RelationshipPattern,
        context: PatternContext,
    ) -> Result<ResolvedRel, SemanticError> {
        if let Some(detail) = &rel.detail {
            let var = Some(match &detail.variable {
                Some(v) => self.declare_or_reuse_variable(&v.name)?,
                // Anonymous relationship — allocate an internal VarId so the
                // relationship value is stored in the row (needed for path
                // materialization).
                None => self.symbols.new_var(),
            });

            let types = detail
                .types
                .iter()
                .map(|t| {
                    self.validate_relationship_type_name(t, context)?;
                    Ok(t.clone())
                })
                .collect::<Result<Vec<_>, SemanticError>>()?;

            if let Some(range) = &detail.range {
                if let (Some(start), Some(end)) = (range.start, range.end) {
                    if start > end {
                        return Err(SemanticError::InvalidRange(
                            start,
                            end,
                            range.span.start,
                            range.span.end,
                        ));
                    }
                }
            }

            let properties = detail
                .properties
                .as_ref()
                .map(|e| self.analyze_property_map_expr(e))
                .transpose()?;

            Ok(ResolvedRel {
                var,
                types,
                direction: rel.direction,
                range: detail.range.clone(),
                properties,
            })
        } else {
            Ok(ResolvedRel {
                var: None,
                types: Vec::new(),
                direction: rel.direction,
                range: None,
                properties: None,
            })
        }
    }

    /// Analyze an expression, but allow resolution of projection aliases
    /// (used for ORDER BY which can reference aliases from RETURN/WITH).
    fn analyze_expr_with_aliases(
        &mut self,
        expr: &Expr,
        aliases: &BTreeMap<String, VarId>,
    ) -> Result<ResolvedExpr, SemanticError> {
        match expr {
            Expr::Variable(v) => {
                // First try normal scope resolution; only fall back to
                // projection aliases when the variable is not in scope.
                if self.scopes.resolve(&v.name).is_some() {
                    return self.analyze_expr(expr);
                }
                if let Some(&id) = aliases.get(&v.name) {
                    return Ok(ResolvedExpr::Variable(id));
                }
                self.analyze_expr(expr)
            }
            // For property access like `alias.prop`, check if the base is an alias.
            Expr::Property {
                expr: inner,
                key,
                span,
            } => {
                let inner = self.analyze_expr_with_aliases(inner, aliases)?;
                if self.property_access_allowed(&inner, key) {
                    Ok(ResolvedExpr::Property {
                        expr: Box::new(inner),
                        property: key.clone(),
                    })
                } else {
                    Err(SemanticError::UnknownPropertyAt(
                        key.clone(),
                        span.start,
                        span.end,
                    ))
                }
            }
            // For function calls in ORDER BY (e.g. ORDER BY count(p))
            Expr::FunctionCall {
                name,
                distinct,
                args,
                span,
            } => {
                let fn_name = name.join(".");
                validate_function_name(&fn_name, span.start, span.end)?;
                validate_function_arity(&fn_name, args.len())?;

                let args = args
                    .iter()
                    .map(|a| self.analyze_expr_with_aliases(a, aliases))
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(ResolvedExpr::Function {
                    name: fn_name,
                    distinct: *distinct,
                    args,
                })
            }
            _ => self.analyze_expr(expr),
        }
    }

    fn analyze_expr(&mut self, expr: &Expr) -> Result<ResolvedExpr, SemanticError> {
        match expr {
            Expr::Variable(v) => {
                let id = self.resolve_required_variable(&v.name)?;
                Ok(ResolvedExpr::Variable(id))
            }

            Expr::Integer(v, _) => Ok(ResolvedExpr::Literal(LiteralValue::Integer(*v))),
            Expr::Float(v, _) => Ok(ResolvedExpr::Literal(LiteralValue::Float(*v))),
            Expr::String(v, _) => Ok(ResolvedExpr::Literal(LiteralValue::String(v.clone()))),
            Expr::Bool(v, _) => Ok(ResolvedExpr::Literal(LiteralValue::Bool(*v))),
            Expr::Null(_) => Ok(ResolvedExpr::Literal(LiteralValue::Null)),
            Expr::Parameter(name, _) => Ok(ResolvedExpr::Parameter(name.clone())),

            Expr::List(items, _) => {
                let items = items
                    .iter()
                    .map(|e| self.analyze_expr(e))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ResolvedExpr::List(items))
            }

            Expr::Map(items, _) => {
                let mut seen = BTreeSet::new();
                let mut out = Vec::with_capacity(items.len());

                for (k, v) in items {
                    if !seen.insert(k.clone()) {
                        return Err(SemanticError::DuplicateMapKey(k.clone()));
                    }
                    out.push((k.clone(), self.analyze_expr(v)?));
                }

                Ok(ResolvedExpr::Map(out))
            }

            Expr::Property { expr, key, span } => {
                let inner = self.analyze_expr(expr)?;

                if self.property_access_allowed(&inner, key) {
                    Ok(ResolvedExpr::Property {
                        expr: Box::new(inner),
                        property: key.clone(),
                    })
                } else {
                    Err(SemanticError::UnknownPropertyAt(
                        key.clone(),
                        span.start,
                        span.end,
                    ))
                }
            }

            Expr::Binary { lhs, op, rhs, .. } => {
                let lhs = self.analyze_expr(lhs)?;
                let rhs = self.analyze_expr(rhs)?;

                Ok(ResolvedExpr::Binary {
                    lhs: Box::new(lhs),
                    op: *op,
                    rhs: Box::new(rhs),
                })
            }

            Expr::Unary { op, expr, .. } => {
                let expr = self.analyze_expr(expr)?;
                Ok(ResolvedExpr::Unary {
                    op: *op,
                    expr: Box::new(expr),
                })
            }

            Expr::FunctionCall {
                name,
                distinct,
                args,
                span,
                ..
            } => {
                let fn_name = name.join(".");
                validate_function_name(&fn_name, span.start, span.end)?;
                validate_function_arity(&fn_name, args.len())?;

                let args = args
                    .iter()
                    .map(|a| self.analyze_expr(a))
                    .collect::<Result<Vec<_>, _>>()?;

                Ok(ResolvedExpr::Function {
                    name: fn_name,
                    distinct: *distinct,
                    args,
                })
            }

            Expr::ListPredicate {
                kind,
                variable,
                list,
                predicate,
                ..
            } => {
                let list = self.analyze_expr(list)?;
                let var_id = self.symbols.new_var();
                self.scopes.push();
                self.scopes.declare(variable.name.clone(), var_id);
                let predicate = self.analyze_expr(predicate)?;
                self.scopes.pop();

                Ok(ResolvedExpr::ListPredicate {
                    kind: *kind,
                    variable: var_id,
                    list: Box::new(list),
                    predicate: Box::new(predicate),
                })
            }

            Expr::ListComprehension {
                variable,
                list,
                filter,
                map_expr,
                ..
            } => {
                let list = self.analyze_expr(list)?;
                let var_id = self.symbols.new_var();
                self.scopes.push();
                self.scopes.declare(variable.name.clone(), var_id);
                let filter = filter.as_ref().map(|e| self.analyze_expr(e)).transpose()?;
                let map_expr = map_expr
                    .as_ref()
                    .map(|e| self.analyze_expr(e))
                    .transpose()?;
                self.scopes.pop();

                Ok(ResolvedExpr::ListComprehension {
                    variable: var_id,
                    list: Box::new(list),
                    filter: filter.map(Box::new),
                    map_expr: map_expr.map(Box::new),
                })
            }

            Expr::Reduce {
                accumulator,
                init,
                variable,
                list,
                expr,
                ..
            } => {
                let init = self.analyze_expr(init)?;
                let list = self.analyze_expr(list)?;
                let acc_id = self.symbols.new_var();
                let var_id = self.symbols.new_var();
                self.scopes.push();
                self.scopes.declare(accumulator.name.clone(), acc_id);
                self.scopes.declare(variable.name.clone(), var_id);
                let expr = self.analyze_expr(expr)?;
                self.scopes.pop();

                Ok(ResolvedExpr::Reduce {
                    accumulator: acc_id,
                    init: Box::new(init),
                    variable: var_id,
                    list: Box::new(list),
                    expr: Box::new(expr),
                })
            }

            Expr::Index {
                expr: inner, index, ..
            } => {
                let expr = self.analyze_expr(inner)?;
                let index = self.analyze_expr(index)?;
                Ok(ResolvedExpr::Index {
                    expr: Box::new(expr),
                    index: Box::new(index),
                })
            }

            Expr::Slice {
                expr: inner,
                from,
                to,
                ..
            } => {
                let expr = self.analyze_expr(inner)?;
                let from = from
                    .as_ref()
                    .map(|e| self.analyze_expr(e))
                    .transpose()?
                    .map(Box::new);
                let to = to
                    .as_ref()
                    .map(|e| self.analyze_expr(e))
                    .transpose()?
                    .map(Box::new);
                Ok(ResolvedExpr::Slice {
                    expr: Box::new(expr),
                    from,
                    to,
                })
            }

            Expr::MapProjection {
                base, selectors, ..
            } => {
                let base = self.analyze_expr(base)?;
                let mut resolved_selectors = Vec::new();
                for sel in selectors {
                    match sel {
                        MapProjectionSelector::Property(name) => {
                            resolved_selectors.push(ResolvedMapSelector::Property(name.clone()));
                        }
                        MapProjectionSelector::AllProperties => {
                            resolved_selectors.push(ResolvedMapSelector::AllProperties);
                        }
                        MapProjectionSelector::Literal(key, expr) => {
                            let resolved = self.analyze_expr(expr)?;
                            resolved_selectors
                                .push(ResolvedMapSelector::Literal(key.clone(), resolved));
                        }
                    }
                }
                Ok(ResolvedExpr::MapProjection {
                    base: Box::new(base),
                    selectors: resolved_selectors,
                })
            }

            Expr::Case {
                input,
                alternatives,
                else_expr,
                ..
            } => {
                let input = input
                    .as_ref()
                    .map(|e| self.analyze_expr(e))
                    .transpose()?
                    .map(Box::new);

                let alternatives = alternatives
                    .iter()
                    .map(|(when, then)| Ok((self.analyze_expr(when)?, self.analyze_expr(then)?)))
                    .collect::<Result<Vec<_>, SemanticError>>()?;

                let else_expr = else_expr
                    .as_ref()
                    .map(|e| self.analyze_expr(e))
                    .transpose()?
                    .map(Box::new);

                Ok(ResolvedExpr::Case {
                    input,
                    alternatives,
                    else_expr,
                })
            }

            Expr::ExistsSubquery {
                pattern, where_, ..
            } => {
                let resolved_pattern =
                    self.analyze_pattern(pattern, PatternContext::OptionalRead)?;
                let resolved_where = where_.as_ref().map(|e| self.analyze_expr(e)).transpose()?;
                Ok(ResolvedExpr::ExistsSubquery {
                    pattern: resolved_pattern,
                    where_: resolved_where.map(Box::new),
                })
            }

            Expr::PatternComprehension {
                pattern: pat_element,
                where_,
                map_expr,
                ..
            } => {
                // Wrap pattern_element in a PatternPart/Pattern for analysis
                let pat = Pattern {
                    parts: vec![PatternPart {
                        binding: None,
                        element: (**pat_element).clone(),
                        span: map_expr.span(),
                    }],
                    span: map_expr.span(),
                };
                let resolved_pattern = self.analyze_pattern(&pat, PatternContext::OptionalRead)?;
                let resolved_where = where_.as_ref().map(|e| self.analyze_expr(e)).transpose()?;
                let resolved_map = self.analyze_expr(map_expr)?;
                Ok(ResolvedExpr::PatternComprehension {
                    pattern: resolved_pattern,
                    where_: resolved_where.map(Box::new),
                    map_expr: Box::new(resolved_map),
                })
            }
        }
    }

    fn analyze_property_map_expr(&mut self, expr: &Expr) -> Result<ResolvedExpr, SemanticError> {
        match expr {
            Expr::Map(_, _) | Expr::Parameter(_, _) => self.analyze_expr(expr),
            _ => Err(SemanticError::ExpectedPropertyMap(
                expr.span().start,
                expr.span().end,
            )),
        }
    }

    fn analyze_return(&mut self, r: &Return) -> Result<ResolvedReturn, SemanticError> {
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

    fn analyze_with(&mut self, w: &With) -> Result<ResolvedWith, SemanticError> {
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

    fn resolve_required_variable(&self, name: &str) -> Result<VarId, SemanticError> {
        self.scopes
            .resolve(name)
            .ok_or_else(|| SemanticError::UnknownVariable(name.to_string()))
    }

    fn declare_fresh_variable(&mut self, name: &str) -> Result<VarId, SemanticError> {
        if self.scopes.resolve(name).is_some() {
            return Err(SemanticError::DuplicateVariable(name.to_string()));
        }

        let id = self.symbols.new_var();
        self.scopes.declare(name.to_string(), id);
        Ok(id)
    }

    fn declare_or_reuse_variable(&mut self, name: &str) -> Result<VarId, SemanticError> {
        if let Some(id) = self.scopes.resolve(name) {
            Ok(id)
        } else {
            let id = self.symbols.new_var();
            self.scopes.declare(name.to_string(), id);
            Ok(id)
        }
    }

    fn validate_label_name(
        &self,
        label: &str,
        context: PatternContext,
    ) -> Result<(), SemanticError> {
        if matches!(
            context,
            PatternContext::Write | PatternContext::OptionalRead
        ) || self.storage.has_label_name(label)
            || self.storage.node_count() == 0
        {
            Ok(())
        } else {
            Err(SemanticError::UnknownLabel(label.to_string()))
        }
    }

    fn validate_relationship_type_name(
        &self,
        rel_type: &str,
        context: PatternContext,
    ) -> Result<(), SemanticError> {
        if matches!(
            context,
            PatternContext::Write | PatternContext::OptionalRead
        ) || self.storage.has_relationship_type_name(rel_type)
            || self.storage.relationship_count() == 0
        {
            Ok(())
        } else {
            Err(SemanticError::UnknownRelationshipType(rel_type.to_string()))
        }
    }

    /// Analyze an expression that is the target of a SET operation.
    /// Property names on the left side of SET are always allowed (new property creation).
    fn analyze_expr_write_property(&mut self, expr: &Expr) -> Result<ResolvedExpr, SemanticError> {
        match expr {
            Expr::Property {
                expr: inner, key, ..
            } => {
                let inner_resolved = self.analyze_expr(inner)?;
                Ok(ResolvedExpr::Property {
                    expr: Box::new(inner_resolved),
                    property: key.clone(),
                })
            }
            // Fallback to normal analysis for non-property expressions
            other => self.analyze_expr(other),
        }
    }

    fn property_access_allowed(&self, base: &ResolvedExpr, key: &str) -> bool {
        match base {
            ResolvedExpr::Map(_) => true,
            _ => {
                self.storage.has_property_key(key)
                    || (self.storage.node_count() == 0 && self.storage.relationship_count() == 0)
            }
        }
    }

    fn visible_bindings(&self) -> BTreeMap<String, VarId> {
        self.scopes.visible_bindings()
    }

    fn replace_scope(&mut self, bindings: BTreeMap<String, VarId>) {
        self.scopes.clear();
        for (name, id) in bindings {
            self.scopes.declare(name, id);
        }
    }
}

/// Known scalar and aggregate function names accepted by the engine.
const KNOWN_FUNCTIONS: &[&str] = &[
    // Aggregate
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "collect",
    "stdev",
    "stdevp",
    "percentilecont",
    "percentiledisc",
    // Entity introspection
    "id",
    "type",
    "labels",
    "keys",
    "properties",
    // Path functions
    "nodes",
    "relationships",
    // String
    "tolower",
    "toupper",
    "trim",
    "ltrim",
    "rtrim",
    "replace",
    "split",
    "substring",
    "reverse",
    "left",
    "right",
    "lpad",
    "rpad",
    "char_length",
    "normalize",
    // Type conversion / introspection
    "tostring",
    "tointeger",
    "toint",
    "tofloat",
    "toboolean",
    "tobooleanornull",
    "valuetype",
    // Math — basic
    "abs",
    "ceil",
    "floor",
    "round",
    "sqrt",
    "sign",
    // Math — trigonometric / logarithmic
    "log",
    "ln",
    "log10",
    "exp",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "degrees",
    "radians",
    // Math — constants
    "pi",
    "e",
    "rand",
    // List / size
    "size",
    "length",
    "head",
    "tail",
    "last",
    "range",
    // Other
    "coalesce",
    "timestamp",
    // Temporal
    "date",
    "datetime",
    "time",
    "localtime",
    "localdatetime",
    "duration",
    "date.truncate",
    "datetime.truncate",
    "duration.between",
    "duration.indays",
    // Spatial
    "point",
    "distance",
];

const AGGREGATE_FUNCTIONS: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "collect",
    "stdev",
    "stdevp",
    "percentilecont",
    "percentiledisc",
];

/// Returns (min_args, max_args) for known functions. `None` means no upper bound (variadic).
fn function_arity(name: &str) -> Option<(usize, Option<usize>)> {
    match name {
        // Aggregate — all take exactly 1 argument (count can take 0 for count(*))
        "count" => Some((0, Some(1))),
        "sum" | "avg" | "min" | "max" | "collect" | "stdev" | "stdevp" => Some((1, Some(1))),
        "percentilecont" | "percentiledisc" => Some((2, Some(2))),
        // Entity introspection — exactly 1
        "id" | "type" | "labels" | "keys" | "properties" | "nodes" | "relationships" => {
            Some((1, Some(1)))
        }
        // String — 1 arg
        "tolower" | "toupper" | "trim" | "ltrim" | "rtrim" | "reverse" => Some((1, Some(1))),
        // String — 2 args
        "split" | "left" | "right" => Some((2, Some(2))),
        // String — 3 args
        "replace" => Some((3, Some(3))),
        // substring: 2 or 3 args
        "substring" => Some((2, Some(3))),
        // Type conversion — exactly 1
        "tostring" | "tointeger" | "toint" | "tofloat" | "toboolean" | "tobooleanornull"
        | "valuetype" => Some((1, Some(1))),
        // String — lpad/rpad take 3
        "lpad" | "rpad" => Some((3, Some(3))),
        // String — char_length/normalize take 1
        "char_length" | "normalize" => Some((1, Some(1))),
        // Math — exactly 1
        "abs" | "ceil" | "floor" | "round" | "sqrt" | "sign" => Some((1, Some(1))),
        // Math — trig / logarithmic (1 arg)
        "log" | "ln" | "log10" | "exp" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan"
        | "degrees" | "radians" => Some((1, Some(1))),
        // Math — atan2 (2 args)
        "atan2" => Some((2, Some(2))),
        // Math — constants (0 args)
        "pi" | "e" | "rand" => Some((0, Some(0))),
        // List / size
        "size" | "length" | "head" | "tail" | "last" => Some((1, Some(1))),
        // range: 2 or 3
        "range" => Some((2, Some(3))),
        // coalesce: 1+
        "coalesce" => Some((1, None)),
        // timestamp: 0
        "timestamp" => Some((0, Some(0))),
        // Temporal constructors: 0 or 1
        "date" | "datetime" | "time" | "localtime" | "localdatetime" => Some((0, Some(1))),
        // duration: exactly 1
        "duration" => Some((1, Some(1))),
        // Temporal namespace functions: exactly 2
        "date.truncate" | "datetime.truncate" | "duration.between" | "duration.indays" => {
            Some((2, Some(2)))
        }
        // Spatial
        "point" => Some((1, Some(1))),
        "distance" => Some((2, Some(2))),
        _ => None,
    }
}

fn is_aggregate_function(name: &str) -> bool {
    AGGREGATE_FUNCTIONS.contains(&name.to_ascii_lowercase().as_str())
}

fn validate_function_name(name: &str, start: usize, end: usize) -> Result<(), SemanticError> {
    let lower = name.to_ascii_lowercase();
    if KNOWN_FUNCTIONS.contains(&lower.as_str()) {
        Ok(())
    } else {
        Err(SemanticError::UnknownFunction(name.to_string(), start, end))
    }
}

fn validate_function_arity(name: &str, arg_count: usize) -> Result<(), SemanticError> {
    let lower = name.to_ascii_lowercase();
    if let Some((min, max)) = function_arity(&lower) {
        if arg_count < min {
            let expected = if max == Some(min) {
                format!("{min}")
            } else if let Some(mx) = max {
                format!("{min}..{mx}")
            } else {
                format!("at least {min}")
            };
            return Err(SemanticError::WrongArity(
                name.to_string(),
                expected,
                arg_count,
            ));
        }
        if let Some(mx) = max {
            if arg_count > mx {
                let expected = if mx == min {
                    format!("{min}")
                } else {
                    format!("{min}..{mx}")
                };
                return Err(SemanticError::WrongArity(
                    name.to_string(),
                    expected,
                    arg_count,
                ));
            }
        }
    }
    Ok(())
}

/// Format label groups as a string for duplicate-variable detection.
fn format_label_groups(groups: &[impl AsRef<[String]>]) -> String {
    groups
        .iter()
        .map(|g| g.as_ref().join("|"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Extract column names and explicit-alias flags from the RETURN clause.
fn return_column_info(clauses: &[ResolvedClause]) -> Option<Vec<(String, bool)>> {
    for clause in clauses.iter().rev() {
        if let ResolvedClause::Return(ret) = clause {
            return Some(
                ret.items
                    .iter()
                    .map(|p| (p.name.clone(), p.explicit_alias))
                    .collect(),
            );
        }
    }
    None
}

/// Returns true if the resolved expression contains any aggregate function call.
fn expr_contains_aggregate(expr: &ResolvedExpr) -> bool {
    match expr {
        ResolvedExpr::Function { name, args, .. } => {
            if is_aggregate_function(name) {
                return true;
            }
            args.iter().any(expr_contains_aggregate)
        }
        ResolvedExpr::Binary { lhs, rhs, .. } => {
            expr_contains_aggregate(lhs) || expr_contains_aggregate(rhs)
        }
        ResolvedExpr::Unary { expr, .. } => expr_contains_aggregate(expr),
        ResolvedExpr::Property { expr, .. } => expr_contains_aggregate(expr),
        ResolvedExpr::List(items) => items.iter().any(expr_contains_aggregate),
        ResolvedExpr::Map(items) => items.iter().any(|(_, v)| expr_contains_aggregate(v)),
        ResolvedExpr::Case {
            input,
            alternatives,
            else_expr,
        } => {
            input.as_ref().is_some_and(|e| expr_contains_aggregate(e))
                || alternatives
                    .iter()
                    .any(|(w, t)| expr_contains_aggregate(w) || expr_contains_aggregate(t))
                || else_expr
                    .as_ref()
                    .is_some_and(|e| expr_contains_aggregate(e))
        }
        ResolvedExpr::ListPredicate {
            list, predicate, ..
        } => expr_contains_aggregate(list) || expr_contains_aggregate(predicate),
        ResolvedExpr::ListComprehension {
            list,
            filter,
            map_expr,
            ..
        } => {
            expr_contains_aggregate(list)
                || filter.as_ref().is_some_and(|e| expr_contains_aggregate(e))
                || map_expr
                    .as_ref()
                    .is_some_and(|e| expr_contains_aggregate(e))
        }
        ResolvedExpr::Reduce {
            init, list, expr, ..
        } => {
            expr_contains_aggregate(init)
                || expr_contains_aggregate(list)
                || expr_contains_aggregate(expr)
        }
        ResolvedExpr::Index { expr, index } => {
            expr_contains_aggregate(expr) || expr_contains_aggregate(index)
        }
        ResolvedExpr::Slice { expr, from, to } => {
            expr_contains_aggregate(expr)
                || from.as_ref().is_some_and(|e| expr_contains_aggregate(e))
                || to.as_ref().is_some_and(|e| expr_contains_aggregate(e))
        }
        ResolvedExpr::MapProjection { base, selectors } => expr_contains_aggregate(base)
            || selectors.iter().any(
                |s| matches!(s, ResolvedMapSelector::Literal(_, e) if expr_contains_aggregate(e)),
            ),
        ResolvedExpr::ExistsSubquery { .. } | ResolvedExpr::PatternComprehension { .. } => false,
        ResolvedExpr::Variable(_) | ResolvedExpr::Literal(_) | ResolvedExpr::Parameter(_) => false,
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

#[cfg(test)]
mod tests {
    use super::*;
    use lora_parser::parse_query;
    use lora_store::{GraphStorageMut, InMemoryGraph, Properties};

    #[test]
    fn create_allows_new_relationship_type_when_graph_is_not_empty() {
        let mut graph = InMemoryGraph::new();
        let alice = graph.create_node(vec!["User".into()], Properties::new());
        let bob = graph.create_node(vec!["User".into()], Properties::new());
        let _carol = graph.create_node(vec!["User".into()], Properties::new());

        graph
            .create_relationship(alice.id, bob.id, "FOLLOWS", Properties::new())
            .unwrap();

        let doc = parse_query(
            "MATCH (a:User {id: 2}), (b:User {id: 3}) CREATE (a)-[:KNOWS]->(b) RETURN a, b",
        )
        .unwrap();

        let mut analyzer = Analyzer::new(&graph);
        assert!(analyzer.analyze(&doc).is_ok());

        let match_doc = parse_query("MATCH (a)-[:KNOWS]->(b) RETURN a, b").unwrap();
        let mut analyzer = Analyzer::new(&graph);
        assert!(matches!(
            analyzer.analyze(&match_doc),
            Err(SemanticError::UnknownRelationshipType(rel_type)) if rel_type == "KNOWS"
        ));
    }
}

#[derive(Debug, Clone)]
struct ExportedAlias {
    name: String,
    id: VarId,
}

#[derive(Debug, Clone)]
struct AnalyzedProjectionBody {
    items: Vec<ResolvedProjection>,
    include_existing: bool,
    exported_aliases: Vec<ExportedAlias>,
    order: Vec<ResolvedSortItem>,
    skip: Option<ResolvedExpr>,
    limit: Option<ResolvedExpr>,
}
