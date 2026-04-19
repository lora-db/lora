use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Result;
use lora_analyzer::Analyzer;
use lora_ast::Document;
use lora_compiler::{CompiledQuery, Compiler};
use lora_executor::{
    ExecuteOptions, LoraValue, MutableExecutionContext, MutableExecutor, QueryResult,
};
use lora_parser::parse_query;
use lora_store::{GraphStorage, GraphStorageMut, InMemoryGraph};

/// Minimal abstraction any transport can depend on to run Lora queries.
pub trait QueryRunner: Send + Sync + 'static {
    fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult>;
}

/// Owns the graph store and orchestrates parse → analyze → compile → execute.
pub struct Database<S> {
    store: Arc<Mutex<S>>,
}

impl Database<InMemoryGraph> {
    /// Convenience constructor: a fresh, empty in-memory graph database.
    pub fn in_memory() -> Self {
        Self::from_graph(InMemoryGraph::new())
    }
}

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut,
{
    /// Build a database from a pre-wrapped, shared store.
    pub fn new(store: Arc<Mutex<S>>) -> Self {
        Self { store }
    }

    /// Build a database by taking ownership of a bare graph store.
    pub fn from_graph(graph: S) -> Self {
        Self::new(Arc::new(Mutex::new(graph)))
    }

    /// Handle to the underlying shared store — useful for callers that need
    /// to snapshot or share the graph across multiple databases.
    pub fn store(&self) -> &Arc<Mutex<S>> {
        &self.store
    }

    /// Parse a query string into an AST without executing it.
    pub fn parse(&self, query: &str) -> Result<Document> {
        Ok(parse_query(query)?)
    }

    fn lock_store(&self) -> MutexGuard<'_, S> {
        self.store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn compile_query(&self, query: &str) -> Result<(MutexGuard<'_, S>, CompiledQuery)> {
        let document = self.parse(query)?;
        let store = self.lock_store();

        let resolved = {
            let mut analyzer = Analyzer::new(&*store);
            analyzer.analyze(&document)?
        };

        let compiled = Compiler::compile(&resolved);
        Ok((store, compiled))
    }

    /// Execute a query and return its result.
    pub fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult> {
        self.execute_with_params(query, options, BTreeMap::new())
    }

    /// Execute a query with bound parameters.
    pub fn execute_with_params(
        &self,
        query: &str,
        options: Option<ExecuteOptions>,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryResult> {
        let (mut store, compiled) = self.compile_query(query)?;

        let mut executor = MutableExecutor::new(MutableExecutionContext {
            storage: &mut *store,
            params,
        });

        Ok(executor.execute_compiled(&compiled, options)?)
    }
}

impl<S> QueryRunner for Database<S>
where
    S: GraphStorage + GraphStorageMut + Send + 'static,
{
    fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult> {
        Database::execute(self, query, options)
    }
}
