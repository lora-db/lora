//! Shared OPTIONAL MATCH row-composition helpers.

use std::collections::BTreeMap;
use std::sync::Arc;

use lora_analyzer::symbols::VarId;
use lora_compiler::physical::{PhysicalNodeId, PhysicalPlan};
use lora_store::GraphStorage;

use crate::errors::ExecResult;
use crate::value::{LoraValue, Row};

/// Whether OPTIONAL MATCH can run its inner pattern once per input row,
/// seeded with that row's bindings.
///
/// Seeding only reaches the inner plan's `Argument` leaf through the pull
/// pipeline; a subtree that falls back to buffered execution ignores the
/// seed and would redo its full, uncorrelated scan for every input row.
pub(crate) fn optional_can_correlate(plan: &PhysicalPlan, inner: PhysicalNodeId) -> bool {
    crate::pull::subtree_is_fully_streaming(plan, inner)
}

/// Correlated OPTIONAL MATCH: run the inner pattern seeded with each input
/// row, so it expands from the nodes the row already binds (as MATCH does)
/// instead of matching the pattern across the whole graph and joining.
/// For `MATCH (f {key: k}) OPTIONAL MATCH (f)<-[:FOLLOWS]-()` that is the
/// difference between touching one festival's followers per row and
/// touching every follow edge in the graph per row.
pub(crate) fn correlated_optional_match_rows<S: GraphStorage>(
    storage: &S,
    params: &BTreeMap<String, LoraValue>,
    plan: &PhysicalPlan,
    inner: PhysicalNodeId,
    input_rows: Vec<Row>,
    new_vars: &[VarId],
) -> ExecResult<Vec<Row>> {
    let params = Arc::new(params.clone());
    let mut out = Vec::with_capacity(input_rows.len());
    for input_row in input_rows {
        let mut source = crate::pull::build_streaming_seeded(
            plan,
            inner,
            storage,
            params.clone(),
            input_row.clone(),
        )?;
        let mut matched = false;
        while let Some(inner_row) = source.next_row()? {
            // Seeded rows carry the input bindings already; the check is
            // a guard should any operator rebind an outer variable.
            if !optional_rows_compatible(&input_row, &inner_row) {
                continue;
            }
            out.push(merge_optional_rows(&input_row, &inner_row));
            matched = true;
        }
        if !matched {
            out.push(null_extend_optional_row(input_row, new_vars));
        }
    }
    Ok(out)
}

pub(crate) fn optional_match_rows(
    input_rows: Vec<Row>,
    inner_rows: &[Row],
    new_vars: &[VarId],
) -> Vec<Row> {
    let mut out = Vec::with_capacity(input_rows.len());

    for input_row in input_rows {
        let mut matched = false;

        for inner_row in inner_rows {
            if !optional_rows_compatible(&input_row, inner_row) {
                continue;
            }

            out.push(merge_optional_rows(&input_row, inner_row));
            matched = true;
        }

        if !matched {
            out.push(null_extend_optional_row(input_row, new_vars));
        }
    }

    out
}

pub(crate) fn optional_rows_compatible(input_row: &Row, inner_row: &Row) -> bool {
    input_row
        .iter()
        .all(|(var, val)| match inner_row.get(*var) {
            Some(inner_val) => inner_val == val,
            None => true,
        })
}

pub(crate) fn merge_optional_rows(input_row: &Row, inner_row: &Row) -> Row {
    let mut merged = input_row.clone();
    merged.fill_missing_from(inner_row);
    merged
}

pub(crate) fn null_extend_optional_row(mut input_row: Row, new_vars: &[VarId]) -> Row {
    for &var_id in new_vars {
        if !input_row.contains_key(var_id) {
            input_row.insert(var_id, LoraValue::Null);
        }
    }
    input_row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(id: u32) -> VarId {
        VarId(id)
    }

    fn row(entries: &[(u32, &str, LoraValue)]) -> Row {
        let mut row = Row::new();
        for (id, name, value) in entries {
            row.insert_named(var(*id), *name, value.clone());
        }
        row
    }

    #[test]
    fn compatibility_requires_shared_variables_to_match() {
        let input = row(&[(0, "n", LoraValue::Int(1)), (1, "m", LoraValue::Int(2))]);
        let compatible_inner = row(&[(0, "n", LoraValue::Int(1)), (2, "x", LoraValue::Int(3))]);
        let incompatible_inner = row(&[(0, "n", LoraValue::Int(9)), (2, "x", LoraValue::Int(3))]);

        assert!(optional_rows_compatible(&input, &compatible_inner));
        assert!(!optional_rows_compatible(&input, &incompatible_inner));
    }

    #[test]
    fn merge_preserves_input_bindings_and_adds_new_inner_bindings() {
        let input = row(&[(0, "n", LoraValue::Int(1))]);
        let inner = row(&[(0, "n", LoraValue::Int(99)), (1, "m", LoraValue::Int(2))]);

        let merged = merge_optional_rows(&input, &inner);

        assert_eq!(merged.get(var(0)), Some(&LoraValue::Int(1)));
        assert_eq!(merged.get_name(var(0)).as_deref(), Some("n"));
        assert_eq!(merged.get(var(1)), Some(&LoraValue::Int(2)));
        assert_eq!(merged.get_name(var(1)).as_deref(), Some("m"));
    }

    #[test]
    fn null_extension_only_fills_missing_optional_variables() {
        let input = row(&[(0, "n", LoraValue::Int(1)), (1, "m", LoraValue::Int(2))]);

        let extended = null_extend_optional_row(input, &[var(1), var(2)]);

        assert_eq!(extended.get(var(1)), Some(&LoraValue::Int(2)));
        assert_eq!(extended.get(var(2)), Some(&LoraValue::Null));
    }

    #[test]
    fn optional_match_rows_emits_matches_or_one_null_extended_row() {
        let matched_input = row(&[(0, "n", LoraValue::Int(1))]);
        let unmatched_input = row(&[(0, "n", LoraValue::Int(2))]);
        let inner_rows = [
            row(&[(0, "n", LoraValue::Int(1)), (1, "m", LoraValue::Int(10))]),
            row(&[(0, "n", LoraValue::Int(1)), (1, "m", LoraValue::Int(11))]),
        ];

        let rows =
            optional_match_rows(vec![matched_input, unmatched_input], &inner_rows, &[var(1)]);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].get(var(1)), Some(&LoraValue::Int(10)));
        assert_eq!(rows[1].get(var(1)), Some(&LoraValue::Int(11)));
        assert_eq!(rows[2].get(var(0)), Some(&LoraValue::Int(2)));
        assert_eq!(rows[2].get(var(1)), Some(&LoraValue::Null));
    }
}
