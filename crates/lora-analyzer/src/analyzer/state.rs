use crate::{errors::*, resolved::*, scope::*, symbols::*};
use lora_ast::{
    Document, Expr, Query, QueryPart, ReadingClause, SinglePartQuery, SingleQuery, Statement,
    UpdatingClause,
};
use lora_store::GraphCatalog;
use std::collections::BTreeMap;

pub struct Analyzer<'a, S: GraphCatalog + ?Sized> {
    pub(super) storage: &'a S,
    pub(super) scopes: ScopeStack,
    pub(super) symbols: SymbolTable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PatternContext {
    Read,
    /// OPTIONAL MATCH — tolerate unknown labels/types (they just won't match).
    OptionalRead,
    Write,
}

impl<'a, S: GraphCatalog + ?Sized> Analyzer<'a, S> {
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

    pub(super) fn validate_label_name(
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

    pub(super) fn validate_relationship_type_name(
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

    pub(super) fn property_access_allowed(&self, base: &ResolvedExpr, key: &str) -> bool {
        match base {
            ResolvedExpr::Map(_) => true,
            _ => {
                self.storage.has_property_key(key)
                    || (self.storage.node_count() == 0 && self.storage.relationship_count() == 0)
            }
        }
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
                    .map(|p| (p.name.clone(), p.explicit_alias))
                    .collect(),
            );
        }
    }
    None
}
