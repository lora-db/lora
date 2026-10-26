//! Plan-time evaluation of builtin calls on constant arguments, the
//! [`lora_compiler::FoldCall`] the database compiles with.
//!
//! Only calls whose value is the same whenever and wherever they run are
//! folded: the value constructors and parsers (`datetime('…')`,
//! `date('…')`, `duration('…')`, `point({...})`, casts), arithmetic on
//! their results, and the pure `math.*` functions. Their per-row cost is
//! the parse, so folding removes most of a filter such as
//! `n.at > datetime('2024-01-01T00:00:00Z')`. Never folded: the clock
//! (`datetime()`, `timestamp()`), random values (`rand()`,
//! `randomUUID()`), anything that reads the graph, and a `TIME`, whose
//! offset in a named zone is the zone's offset at the moment it is built.
//!
//! A call that fails on its arguments is left in the plan, so the query
//! still fails only when a row reaches it.

use std::collections::BTreeMap;

use lora_analyzer::{FunctionId, LiteralValue, ResolvedExpr};
use lora_store::PropertyValue;

use crate::value::LoraValue;

use super::errors::{clear_eval_error, set_eval_error, take_eval_error};

/// A folded string longer than this stays a call, so a plan does not
/// carry a large value from a call such as `cast.to(…, STRING)`.
const MAX_FOLDED_STRING: usize = 4096;

/// Builtins whose value depends only on their arguments.
fn is_foldable(name: &str) -> bool {
    matches!(
        name,
        "cast.to"
            | "cast.try"
            | "cast.can"
            | "geo.point"
            | "geo.distance"
            | "temporal.parse"
            | "temporal.add"
            | "temporal.truncate"
            | "temporal.between"
            | "temporal.duration_between"
            | "temporal.in_days"
            | "temporal.in_months"
            | "temporal.in_seconds"
            | "temporal.from_epoch"
            | "temporal.from_epoch_millis"
            | "math.min"
            | "math.max"
            | "math.round"
            | "math.trunc"
            | "math.sigmoid"
            | "math.tanh"
            | "math.cosh"
            | "math.sinh"
            | "math.cot"
            | "math.coth"
            | "math.atan2"
            | "math.pow"
            | "math.hypot"
            | "math.log_base"
            | "math.gcd"
            | "math.lcm"
            | "math.clamp"
            | "math.lerp"
            | "math.abs"
            | "math.ceil"
            | "math.floor"
            | "math.sqrt"
            | "math.sign"
            | "math.log"
            | "math.ln"
            | "math.log10"
            | "math.exp"
            | "math.sin"
            | "math.cos"
            | "math.tan"
            | "math.asin"
            | "math.acos"
            | "math.atan"
            | "math.degrees"
            | "math.radians"
            | "math.pi"
            | "math.e"
    )
}

/// The value of `function(args...)` as a literal, when it can be computed
/// once for every row of every run of the plan; see the module docs.
pub fn fold_call(function: FunctionId, args: &[ResolvedExpr]) -> Option<LiteralValue> {
    let FunctionId::Builtin(spec) = function else {
        return None;
    };
    if !is_foldable(spec.name) {
        return None;
    }
    let args = args.iter().map(constant).collect::<Option<Vec<_>>>()?;

    // The error slot belongs to whatever evaluation is in progress on this
    // thread; keep it as it was.
    let outer = take_eval_error();
    let value = super::builtins::dispatch_pure(spec.name, &args);
    let failed = take_eval_error().is_some();
    clear_eval_error();
    if let Some(message) = outer {
        set_eval_error(message);
    }
    if failed {
        return None;
    }
    literal(value?)
}

/// The value of a constant argument: a literal, or a list or map of them.
fn constant(expr: &ResolvedExpr) -> Option<LoraValue> {
    Some(match expr {
        ResolvedExpr::Literal(literal) => super::expr::eval_literal(literal),
        ResolvedExpr::List(items) => {
            LoraValue::List(items.iter().map(constant).collect::<Option<_>>()?)
        }
        ResolvedExpr::Map(items) => LoraValue::Map(
            items
                .iter()
                .map(|(key, value)| Some((key.clone(), constant(value)?)))
                .collect::<Option<BTreeMap<_, _>>>()?,
        ),
        _ => return None,
    })
}

fn literal(value: LoraValue) -> Option<LiteralValue> {
    Some(match value {
        LoraValue::Null => LiteralValue::Null,
        LoraValue::Bool(b) => LiteralValue::Bool(b),
        LoraValue::Int(i) => LiteralValue::Integer(i),
        LoraValue::Float(f) => LiteralValue::Float(f),
        LoraValue::String(s) if s.len() <= MAX_FOLDED_STRING => LiteralValue::String(s),
        LoraValue::Date(_)
        | LoraValue::LocalTime(_)
        | LoraValue::DateTime(_)
        | LoraValue::LocalDateTime(_)
        | LoraValue::Duration(_)
        | LoraValue::Point(_) => LiteralValue::Value(Box::new(PropertyValue::from(value))),
        // `TIME` (see the module docs), and containers and vectors, which
        // a call can make large.
        _ => return None,
    })
}
