//! Parse/analyze/compile helpers for [`Database`].
//!
//! Keeping these methods separate from the execution routing code makes the
//! query path easier to scan: this module owns turning query text or an AST
//! into a cached [`CompiledQuery`], while `execute` decides how to run it.

use std::any::Any;
use std::sync::Arc;

use anyhow::Result;
use lora_analyzer::Analyzer;
use lora_ast::Document;
use lora_compiler::{CompiledQuery, Compiler};
use lora_parser::parse_query;
use lora_store::{GraphStorage, GraphStorageMut};

use crate::database::Database;

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    pub(super) fn compile_document_against(
        &self,
        document: &Document,
        store: &S,
    ) -> Result<CompiledQuery> {
        let resolved = {
            let mut analyzer = Analyzer::new(store);
            analyzer.analyze(document)?
        };

        Ok(Compiler::compile(&resolved))
    }

    /// Return a cached compiled plan for `query`, or compile + cache one
    /// against the supplied store. The store is only touched on cache
    /// miss, so a steady-state hot query never reaches the analyzer or
    /// the compiler.
    pub(crate) fn compile_query_cached(
        &self,
        query: &str,
        store: &S,
    ) -> Result<Arc<CompiledQuery>> {
        if let Some(plan) = self.plan_cache.get(query) {
            return Ok(plan);
        }
        let document = parse_query(query)?;
        let plan = Arc::new(self.compile_document_against(&document, store)?);
        self.plan_cache.insert(query, plan.clone());
        Ok(plan)
    }
}
