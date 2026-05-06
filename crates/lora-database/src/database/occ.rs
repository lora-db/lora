//! Auto-commit write path for `InMemoryGraph`.
//!
//! This module is the dispatcher between [`Database::execute_with_params`]
//! and the canonical mutating shape in [`super::write_guard`]. It
//! builds a `MutableExecutor` against the live store and hands it to
//! [`Database::run_with_durable_recorder`], which owns the writer
//! mutex, recorder arm/commit/abort lifecycle, and the
//! managed-snapshot trigger. The single-writer design (`Arc::make_mut`
//! against the live `Arc<S>`) and the failure trade-off — a query
//! that fails mid-execution can leave the live graph partially
//! mutated, but never the durable log — are documented in
//! [`super::write_guard`].

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use lora_compiler::CompiledQuery;
use lora_executor::{LoraValue, MutableExecutionContext, MutableExecutor, Row};
use lora_store::{GraphStorage, GraphStorageMut};

use crate::database::Database;

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Auto-commit a mutating query. Builds a `MutableExecutor`
    /// against the staged graph and routes through the canonical
    /// write shape in [`Database::run_with_durable_recorder`].
    pub(crate) fn execute_mutating_optimistic(
        &self,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
        compiled: &Arc<CompiledQuery>,
    ) -> Result<Vec<Row>> {
        self.run_with_durable_recorder(|staged| {
            let mut executor = MutableExecutor::with_deadline(
                MutableExecutionContext {
                    storage: staged,
                    params,
                },
                deadline,
            );
            executor
                .execute_compiled_rows(compiled)
                .map_err(anyhow::Error::from)
        })
    }
}
