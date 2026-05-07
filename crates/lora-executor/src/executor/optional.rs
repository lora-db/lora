//! Shared OPTIONAL MATCH row-composition helpers.

use lora_analyzer::symbols::VarId;

use crate::value::{LoraValue, Row};

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
    for (var, name, val) in inner_row.iter_named() {
        if !merged.contains_key(*var) {
            merged.insert_named(*var, name.into_owned(), val.clone());
        }
    }
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
