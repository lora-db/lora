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
            let value = super::builtins::dispatch(spec.name, args, ctx).unwrap_or(LoraValue::Null);
            // `range` checks its length before allocating and builds a
            // flat list; walking a long one again would cost a pass.
            if spec.name == "list.range" {
                return value;
            }
            within_limits(value)
        }
        FunctionId::Aggregate(_) => LoraValue::Null,
    }
}

/// A builtin's result, or null with an error when it breaks a value limit.
/// Builtins that could allocate without bound check their sizes before
/// building; this catches the rest, such as a list function that wraps its
/// argument one level deeper.
fn within_limits(value: LoraValue) -> LoraValue {
    let checked = match &value {
        LoraValue::List(_) | LoraValue::Map(_) => crate::limits::check_value(&value),
        LoraValue::String(s) => crate::limits::check_bytes(s.len()),
        _ => Ok(()),
    };
    match checked {
        Ok(()) => value,
        Err(message) => {
            super::errors::set_eval_error(message);
            LoraValue::Null
        }
    }
}
