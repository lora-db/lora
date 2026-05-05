use std::collections::BTreeMap;
use std::sync::Arc;

use lora_store::GraphStorage;

use crate::eval::EvalContext;
use crate::value::LoraValue;

/// Storage + bound parameters shared by every operator source in a
/// pull pipeline. `Clone` is one pointer-copy plus an `Arc::clone`
/// (params), so passing it by value down the build tree is
/// effectively free, while consolidating "the two pieces every
/// expression-evaluating source needs" into one field.
#[derive(Clone)]
pub(crate) struct StreamCtx<'a, S: GraphStorage> {
    pub storage: &'a S,
    pub params: Arc<BTreeMap<String, LoraValue>>,
}

impl<'a, S: GraphStorage> StreamCtx<'a, S> {
    pub(crate) fn new(storage: &'a S, params: Arc<BTreeMap<String, LoraValue>>) -> Self {
        Self { storage, params }
    }

    /// Build a borrowing [`EvalContext`] for use inside an
    /// operator's `next_row` method. Cheap — two pointer reads.
    pub(crate) fn eval_ctx<'b>(&'b self) -> EvalContext<'b, S> {
        EvalContext {
            storage: self.storage,
            params: &self.params,
        }
    }
}
