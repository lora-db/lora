//! Namespaced builtin functions.
//!
//! A coherent set of two-segment, snake_case, namespace-grouped functions.
//! Canonical lora names live in a namespace; bare historical
//! names like `head()` / `toLower()` / `coalesce()` are analyzer aliases
//! that resolve into this tree before evaluation.
//!
//! Design rules — enforced module-wide, not per-function:
//! 1. Two segments only: `<namespace>.<operation>`.
//! 2. `snake_case` for the operation segment.
//! 3. Namespaces are usually the noun of the primary input (`list.*`,
//!    `string.*`, `node.*`). Runtime type questions live under `type.*`;
//!    conversions live under `cast.*`, not mixed into `value.*`.
//! 4. One operation per concept; behaviour varies via named-or-trailing
//!    arguments, not by suffix (no `sortMaps` / `sortNodes` / `sortText`).
//! 5. Predicates return `BOOL` and start with `is_`, `has_`, `contains`,
//!    `equal`, `all_`, or otherwise read as a question.
//! 6. Pure functions only. Mutating procedures live in
//!    [`crate::executor`] / [`crate::pull`] dispatch, not here.

use crate::value::LoraValue;
use lora_store::GraphStorage;

use super::expr::EvalContext;

mod bits_ns;
mod bytes_ns;
mod cast_ns;
mod crypto;
mod edge;
mod geo;
mod json_ns;
mod list;
mod map_ns;
mod math_ns;
mod node;
mod number;
mod path;
mod string_ns;
mod temporal;
mod text;
mod type_ns;
mod uuid_ns;
mod value;
mod vector_ns;

/// Dispatch a `<namespace>.<operation>` call.
///
/// Returns `None` for any name that doesn't belong to a known namespace —
/// the caller falls through to the "unknown function" path.
pub(super) fn dispatch<S: GraphStorage>(
    name: &str,
    args: &[LoraValue],
    ctx: &EvalContext<'_, S>,
) -> Option<LoraValue> {
    let (ns, op) = name.split_once('.')?;
    match ns {
        // Pure (no storage access)
        "list" => list::dispatch(op, args),
        "string" => string_ns::dispatch(op, args),
        "text" => text::dispatch(op, args),
        "map" => map_ns::dispatch(op, args),
        "number" => number::dispatch(op, args),
        "math" => math_ns::dispatch(op, args),
        "temporal" => temporal::dispatch(op, args),
        "bytes" => bytes_ns::dispatch(op, args),
        "bits" => bits_ns::dispatch(op, args),
        "cast" => cast_ns::dispatch(op, args),
        "crypto" => crypto::dispatch(op, args),
        "uuid" => uuid_ns::dispatch(op, args),
        "json" => json_ns::dispatch(op, args),
        "geo" => geo::dispatch(op, args),
        "vector" => vector_ns::dispatch(op, args),
        "type" => type_ns::dispatch(op, args),
        // Storage-aware
        "node" => node::dispatch(op, args, ctx),
        "edge" => edge::dispatch(op, args, ctx),
        "path" => path::dispatch(op, args, ctx),
        "value" => value::dispatch(op, args, ctx),
        _ => None,
    }
}

/// Pure name lookup — same dispatch tree as [`dispatch`] but no args
/// and no storage. Used by drift-safety tests to assert every signature
/// has an executor entry.
#[cfg(test)]
fn name_is_known(name: &str) -> bool {
    let Some((ns, op)) = name.split_once('.') else {
        return false;
    };
    let probe = &[];
    let pure_known = match ns {
        "list" => list::dispatch(op, probe).is_some(),
        "string" => string_ns::dispatch(op, probe).is_some(),
        "text" => text::dispatch(op, probe).is_some(),
        "map" => map_ns::dispatch(op, probe).is_some(),
        "number" => number::dispatch(op, probe).is_some(),
        "math" => math_ns::dispatch(op, probe).is_some(),
        "temporal" => temporal::dispatch(op, probe).is_some(),
        "bytes" => bytes_ns::dispatch(op, probe).is_some(),
        "bits" => bits_ns::dispatch(op, probe).is_some(),
        "cast" => cast_ns::dispatch(op, probe).is_some(),
        "crypto" => crypto::dispatch(op, probe).is_some(),
        "uuid" => uuid_ns::dispatch(op, probe).is_some(),
        "json" => json_ns::dispatch(op, probe).is_some(),
        "geo" => geo::dispatch(op, probe).is_some(),
        "vector" => vector_ns::dispatch(op, probe).is_some(),
        "type" => type_ns::known(op).is_some(),
        "node" => node::known(op).is_some(),
        "edge" => edge::known(op).is_some(),
        "path" => path::known(op).is_some(),
        "value" => value::known(op).is_some(),
        _ => false,
    };
    pure_known
}

#[cfg(test)]
mod drift_tests {
    use super::name_is_known;
    use lora_analyzer::BUILTIN_SPECS;

    #[test]
    fn every_signature_has_a_dispatch_arm() {
        let mut missing: Vec<&str> = Vec::new();
        for spec in BUILTIN_SPECS {
            if !name_is_known(spec.name) {
                missing.push(spec.name);
            }
        }
        assert!(
            missing.is_empty(),
            "the following signatures have no executor dispatch arm: {missing:?}"
        );
    }

    #[test]
    fn signature_table_has_no_duplicates() {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for spec in BUILTIN_SPECS {
            assert!(
                seen.insert(spec.name),
                "duplicate signature entry: {}",
                spec.name
            );
        }
    }

    #[test]
    fn every_signature_is_two_segments_and_snake_case() {
        for spec in BUILTIN_SPECS {
            let name = spec.name;
            let parts: Vec<&str> = name.split('.').collect();
            assert_eq!(parts.len(), 2, "expected two segments for '{name}'");
            for part in parts {
                assert!(!part.is_empty(), "empty segment in '{name}'");
                for ch in part.chars() {
                    assert!(
                        ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_',
                        "non-snake_case character {ch:?} in '{name}'"
                    );
                }
            }
        }
    }

    #[test]
    fn arity_max_is_at_least_arity_min() {
        for spec in BUILTIN_SPECS {
            let name = spec.name;
            let min = spec.arity.min;
            let max = spec.arity.max;
            if let Some(mx) = max {
                assert!(
                    mx >= min,
                    "arity max < min for '{name}': min={min} max={mx}"
                );
            }
        }
    }
}
