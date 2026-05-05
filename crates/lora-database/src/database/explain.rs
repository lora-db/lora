//! `Database::explain` — plan-only query inspection.
//!
//! `explain` parses, analyzes, and compiles a query, then returns the
//! resulting [`QueryPlan`]. The executor is *never* invoked, so calling
//! `explain` on a mutating query (CREATE / MERGE / SET / DELETE / REMOVE)
//! reports the plan without producing any side effects.
//!
//! `params` is accepted for API symmetry with `execute()` and reserved
//! for a future cost model — v1 does not consult parameter values when
//! producing the plan tree.

use std::any::Any;
use std::collections::BTreeMap;

use lora_compiler::plan_tree_from_compiled;
use lora_executor::{classify_stream, plan_result_columns, LoraValue};
use lora_store::{GraphStorage, GraphStorageMut};

use crate::database::Database;
use crate::error::LoraError;
use crate::explain::{PlanShape, QueryPlan};

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Compile `query` and return the plan that *would* run.
    ///
    /// The executor is not invoked: `explain` is a pure planning call
    /// and never produces side effects. Mutating queries return their
    /// plan without touching the graph.
    ///
    /// `params` is accepted for symmetry with `execute()`. The returned
    /// plan is identical regardless of the parameter values today;
    /// future cost-model work may use parameter values for selectivity
    /// estimation, so callers should pass real values when they have
    /// them.
    pub fn explain(
        &self,
        query: &str,
        _params: Option<BTreeMap<String, LoraValue>>,
    ) -> Result<QueryPlan, LoraError> {
        let store = self.read_store();
        let compiled = self
            .compile_query_cached(query, &*store)
            .map_err(LoraError::from_anyhow)?;
        let tree = plan_tree_from_compiled(&compiled);
        let shape: PlanShape = classify_stream(&compiled).into();
        let result_columns = plan_result_columns(&compiled.physical);
        Ok(QueryPlan {
            query: query.to_string(),
            tree,
            shape,
            result_columns,
        })
    }
}
