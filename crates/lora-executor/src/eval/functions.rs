//! Function-call dispatcher.
//!
//! Routes every Cypher function call through the namespaced builtin
//! tree in [`super::builtins`]. Lora's canonical surface is namespaced:
//! `toLower(s)` and `coalesce(x, y)` resolve to `string.lower(s)` and
//! `value.coalesce(x, y)`, while casts resolve to `cast.*`. The analyzer
//! [`lora_analyzer::BUILTIN_SPECS`] table plus its alias table are the
//! source of truth for which names exist.

use lora_store::GraphStorage;

use lora_analyzer::FunctionId;

use crate::value::LoraValue;

use super::expr::EvalContext;

pub(super) fn eval_function<S: GraphStorage>(
    function: FunctionId,
    args: &[LoraValue],
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    match function {
        FunctionId::Builtin(spec) => {
            super::builtins::dispatch(spec.name, args, ctx).unwrap_or(LoraValue::Null)
        }
        FunctionId::Aggregate(_) => LoraValue::Null,
    }
}
