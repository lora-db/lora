//! Sort helpers shared by buffered execution and the pull pipeline.

use std::cmp::Ordering;

use lora_analyzer::ResolvedSortItem;
use lora_ast::SortDirection;
use lora_store::GraphStorage;

use crate::eval::{eval_expr, EvalContext};
use crate::value::{LoraValue, Row};

use super::helpers::compare_values_total;

pub(crate) fn sort_rows_with_top_k<S: GraphStorage>(
    rows: &mut Vec<Row>,
    items: &[ResolvedSortItem],
    eval_ctx: &EvalContext<'_, S>,
    top_k: Option<usize>,
) {
    retain_top_k_candidates(rows, items, eval_ctx, top_k);
    rows.sort_by(|a, b| compare_sort_items(items, a, b, eval_ctx));
}

fn retain_top_k_candidates<S: GraphStorage>(
    rows: &mut Vec<Row>,
    items: &[ResolvedSortItem],
    eval_ctx: &EvalContext<'_, S>,
    top_k: Option<usize>,
) {
    let Some(top_k) = top_k else {
        return;
    };

    if top_k == 0 {
        rows.clear();
        return;
    }

    // `select_nth_unstable_by` pays an extra comparator-heavy partition pass.
    // With expression-based sort keys that only wins when LIMIT is genuinely
    // selective; broad pagination such as SKIP 500 LIMIT 50 over 1k rows is
    // faster as a plain full sort.
    if rows.len() > top_k && top_k.saturating_mul(4) < rows.len() {
        rows.select_nth_unstable_by(top_k, |a, b| compare_sort_items(items, a, b, eval_ctx));
        rows.truncate(top_k);
    }
}

fn compare_sort_items<S: GraphStorage>(
    items: &[ResolvedSortItem],
    a: &Row,
    b: &Row,
    eval_ctx: &EvalContext<'_, S>,
) -> Ordering {
    for item in items {
        let ord = compare_sort_item(item, a, b, eval_ctx);
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

fn compare_sort_item<S: GraphStorage>(
    item: &ResolvedSortItem,
    a: &Row,
    b: &Row,
    eval_ctx: &EvalContext<'_, S>,
) -> Ordering {
    let av = eval_expr(&item.expr, a, eval_ctx);
    let bv = eval_expr(&item.expr, b, eval_ctx);

    compare_values_for_sort(&av, &bv, matches!(item.direction, SortDirection::Asc))
}

fn compare_values_for_sort(a: &LoraValue, b: &LoraValue, ascending: bool) -> Ordering {
    let ord = match (a, b) {
        (LoraValue::Null, LoraValue::Null) => Ordering::Equal,
        (LoraValue::Null, _) => Ordering::Greater,
        (_, LoraValue::Null) => Ordering::Less,
        _ => compare_values_total(a, b),
    };

    if ascending {
        ord
    } else {
        ord.reverse()
    }
}
