//! `cast.*` — value conversion with explicit target types.
//!
//! Canonical casts are `cast.to(value, TYPE)`, `cast.try(value, TYPE)`,
//! and `cast.can(value, TYPE)`. Historical Cypher-style `toInteger()`
//! and the earlier Lora `type.cast()` surface resolve here in the
//! analyzer.

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "to" => super::type_ns::cast_to(args),
        "try" => super::type_ns::cast_try(args),
        "can" => super::type_ns::cast_can(args),
        _ => return None,
    })
}
