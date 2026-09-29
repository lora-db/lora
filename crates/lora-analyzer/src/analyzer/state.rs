use crate::{errors::*, resolved::*, scope::*, symbols::*};
use lora_ast::{
    Document, Expr, Query, QueryPart, ReadingClause, SinglePartQuery, SingleQuery, Statement,
    UpdatingClause,
};
use lora_store::GraphCatalog;
use std::collections::{BTreeMap, BTreeSet};

pub struct Analyzer<'a, S: GraphCatalog + ?Sized> {
    /// Analysis deliberately does not read the stored data: whether a
    /// query is valid must not depend on which labels, types or keys
    /// happen to exist right now. The catalog type stays in the signature
    /// so callers keep constructing the analyzer against their store.
    pub(super) _catalog: std::marker::PhantomData<&'a S>,
    pub(super) scopes: ScopeStack,
    pub(super) symbols: SymbolTable,
    /// Variables whose runtime value shape isn't tracked by the analyzer
    /// (UNWIND-bound elements, anything that may legitimately hold a map
    /// or list of maps). Property access on these vars must skip the
    /// graph-catalog check — the caller can't know which keys the data
    /// carries until rows are bound at execution time.
    pub(super) dynamic_property_vars: BTreeSet<VarId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PatternContext {
    Read,
    /// OPTIONAL MATCH — tolerate unknown labels/types (they just won't match).
    OptionalRead,
    Write,
}

impl<'a, S: GraphCatalog + ?Sized> Analyzer<'a, S> {
    pub fn new(_storage: &'a S) -> Self {
        Self {
            _catalog: std::marker::PhantomData,
            scopes: ScopeStack::new(),
            symbols: SymbolTable::default(),
            dynamic_property_vars: BTreeSet::new(),
        }
    }

    pub fn analyze(&mut self, doc: &Document) -> Result<ResolvedQuery, SemanticError> {
        match &doc.statement {
            Statement::Query(q) => self.analyze_query(q),
            Statement::Schema(_) => Err(SemanticError::UnsupportedFeature(
                "schema commands are dispatched outside the analyzer".to_string(),
            )),
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
            clauses.extend(self.analyze_reading_clause(rc)?);
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
            clauses.extend(self.analyze_reading_clause(rc)?);
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
    ) -> Result<Vec<ResolvedClause>, SemanticError> {
        Ok(match rc {
            ReadingClause::Match(m) => vec![ResolvedClause::Match(self.analyze_match(m)?)],
            ReadingClause::Unwind(u) => vec![ResolvedClause::Unwind(self.analyze_unwind(u)?)],
            ReadingClause::InQueryCall(c) => self.analyze_in_query_call(c)?,
            ReadingClause::CallSubquery(c) => {
                vec![ResolvedClause::CallSubquery(self.analyze_call_subquery(c)?)]
            }
        })
    }

    /// Analyze a CALL { ... } subquery. The inner body is analyzed
    /// with the outer scope visible (so MATCH inside the CALL can
    /// reference outer-bound variables). After analysis the outer
    /// scope is restored and the inner final RETURN's projection
    /// aliases are injected as new bindings.
    fn analyze_call_subquery(
        &mut self,
        call: &lora_ast::CallSubquery,
    ) -> Result<ResolvedCallSubquery, SemanticError> {
        let outer = self.visible_bindings();

        if !call.body.unions.is_empty() {
            return Err(SemanticError::UnsupportedFeature(
                "UNION inside CALL { ... } is not yet supported".into(),
            ));
        }

        let inner_clauses = self.analyze_single_query(&call.body.head)?;

        let return_items: Vec<(String, VarId)> = match inner_clauses.last() {
            Some(ResolvedClause::Return(ret)) => ret
                .items
                .iter()
                .map(|p| (p.name.to_string(), p.output))
                .collect(),
            _ => {
                return Err(SemanticError::UnsupportedFeature(
                    "CALL { ... } subquery must end with RETURN".into(),
                ));
            }
        };

        let mut new_scope = outer;
        for (name, id) in &return_items {
            new_scope.insert(name.clone(), *id);
        }
        self.replace_scope(new_scope);

        Ok(ResolvedCallSubquery {
            clauses: inner_clauses,
            return_vars: return_items.into_iter().map(|(_, id)| id).collect(),
        })
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
            UpdatingClause::Foreach(f) => Ok(ResolvedClause::Foreach(self.analyze_foreach(f)?)),
        }
    }

    pub(super) fn analyze_property_map_expr(
        &mut self,
        expr: &Expr,
    ) -> Result<ResolvedExpr, SemanticError> {
        match expr {
            Expr::Map(_, _) | Expr::Parameter(_, _) => self.analyze_expr(expr),
            _ => Err(SemanticError::ExpectedPropertyMap(
                expr.span().start,
                expr.span().end,
            )),
        }
    }

    pub(super) fn resolve_required_variable(&self, name: &str) -> Result<VarId, SemanticError> {
        self.scopes
            .resolve(name)
            .ok_or_else(|| SemanticError::UnknownVariable(name.to_string()))
    }

    pub(super) fn declare_fresh_variable(&mut self, name: &str) -> Result<VarId, SemanticError> {
        if self.scopes.resolve(name).is_some() {
            return Err(SemanticError::DuplicateVariable(name.to_string()));
        }

        let id = self.symbols.new_var();
        self.scopes.declare(name.to_string(), id);
        Ok(id)
    }

    pub(super) fn declare_or_reuse_variable(&mut self, name: &str) -> Result<VarId, SemanticError> {
        if let Some(id) = self.scopes.resolve(name) {
            Ok(id)
        } else {
            let id = self.symbols.new_var();
            self.scopes.declare(name.to_string(), id);
            Ok(id)
        }
    }

    /// Label names are never rejected. Standard Cypher treats a label no
    /// node carries as an empty match, not an error, and checking against
    /// the stored data made the *kind* of answer depend on what happens to
    /// exist right now: the same query errored after the last `:Comment`
    /// was deleted, but not on an empty database. A pattern with an unknown
    /// label simply matches nothing.
    pub(super) fn validate_label_name(
        &self,
        _label: &str,
        _context: PatternContext,
    ) -> Result<(), SemanticError> {
        Ok(())
    }

    /// Relationship types follow the same rule as labels: an unknown type
    /// matches nothing.
    pub(super) fn validate_relationship_type_name(
        &self,
        _rel_type: &str,
        _context: PatternContext,
    ) -> Result<(), SemanticError> {
        Ok(())
    }

    /// Analyze an expression that is the target of a SET operation.
    /// Property names on the left side of SET are always allowed (new property creation).
    pub(super) fn analyze_expr_write_property(
        &mut self,
        expr: &Expr,
    ) -> Result<ResolvedExpr, SemanticError> {
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

    /// Property access is always allowed: reading a key no entity carries
    /// yields `null` (standard Cypher), including a key written earlier in
    /// the same statement. Checking the stored catalog made the query's
    /// validity depend on the data.
    pub(super) fn property_access_allowed(&self, _base: &ResolvedExpr, _key: &str) -> bool {
        true
    }

    pub(super) fn visible_bindings(&self) -> BTreeMap<String, VarId> {
        self.scopes.visible_bindings()
    }

    pub(super) fn replace_scope(&mut self, bindings: BTreeMap<String, VarId>) {
        self.scopes.clear();
        for (name, id) in bindings {
            self.scopes.declare(name, id);
        }
    }
}

/// Extract column names and explicit-alias flags from the RETURN clause.
fn return_column_info(clauses: &[ResolvedClause]) -> Option<Vec<(String, bool)>> {
    for clause in clauses.iter().rev() {
        if let ResolvedClause::Return(ret) = clause {
            return Some(
                ret.items
                    .iter()
                    .map(|p| (p.name.to_string(), p.explicit_alias))
                    .collect(),
            );
        }
    }
    None
}
