//! Sort helpers shared by buffered execution and the pull pipeline.
//!
//! [`SortBuffer`] evaluates each row's sort keys once, when the row
//! arrives, instead of twice per comparison. With a row bound (`ORDER BY
//! … LIMIT k`, the bound being `skip + limit`) it holds at most about
//! `2k` rows: when full it keeps the best `k` (a linear-time selection)
//! and remembers the worst of them, so any later row that does not sort
//! before it is dropped after one key comparison.
//!
//! Ordering is exactly the stable sort's: keys compare with
//! [`compare_values_for_sort`] (nulls last ascending, first descending,
//! mixed types by [`compare_values_total`]) and ties keep arrival order,
//! enforced by comparing arrival sequence numbers last.

use std::cmp::Ordering;

use lora_analyzer::ResolvedSortItem;
use lora_ast::SortDirection;
use lora_compiler::SortLimit;
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
    if top_k.is_none() && rows.len() < 2 {
        return;
    }
    let taken = std::mem::take(rows);
    *rows = SortBuffer::from_rows(items, top_k, taken, eval_ctx).finish();
}

/// The row bound a Sort may apply: the static `top_k` and, for `LIMIT
/// $n`, the parent LIMIT's expressions evaluated now (the way the LIMIT
/// itself evaluates them, so both agree on the count). `None` when every
/// row must be kept.
pub(crate) fn sort_row_bound(
    top_k: Option<usize>,
    limit: Option<&SortLimit>,
    eval_ctx: &EvalContext<'_, impl GraphStorage>,
) -> Option<usize> {
    let dynamic = limit.and_then(|bound| {
        let scratch = Row::new();
        let limit = eval_expr(&bound.limit, &scratch, eval_ctx).as_i64()?.max(0) as usize;
        let skip = bound
            .skip
            .as_ref()
            .and_then(|e| eval_expr(e, &scratch, eval_ctx).as_i64())
            .unwrap_or(0)
            .max(0) as usize;
        Some(skip.saturating_add(limit))
    });
    match (top_k, dynamic) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Rows being sorted, with their keys evaluated once. See the module docs.
pub(crate) struct SortBuffer<'i> {
    items: &'i [ResolvedSortItem],
    /// Most rows to keep; `None` keeps all.
    bound: Option<usize>,
    rows: Vec<Row>,
    /// `keys[slot * items.len() + i]` is slot's key for `items[i]`.
    keys: Vec<LoraValue>,
    /// Arrival order of each slot's row, the final tie-break. Only kept
    /// under a bound; unbounded, a slot's index is its arrival order.
    seqs: Vec<u64>,
    /// Under a bound, once the buffer has been cut down: the keys of the
    /// worst row kept. A later row that does not sort strictly before it
    /// can never be in the result.
    cutoff: Option<Vec<LoraValue>>,
    next_seq: u64,
    scratch: Vec<LoraValue>,
}

impl<'i> SortBuffer<'i> {
    pub(crate) fn new(items: &'i [ResolvedSortItem], bound: Option<usize>) -> Self {
        Self {
            items,
            bound,
            rows: Vec::new(),
            keys: Vec::new(),
            seqs: Vec::new(),
            cutoff: None,
            next_seq: 0,
            scratch: Vec::with_capacity(items.len()),
        }
    }

    /// A buffer over rows already collected, keeping them in place.
    fn from_rows<S: GraphStorage>(
        items: &'i [ResolvedSortItem],
        bound: Option<usize>,
        rows: Vec<Row>,
        eval_ctx: &EvalContext<'_, S>,
    ) -> Self {
        let mut keys = Vec::with_capacity(rows.len() * items.len());
        for row in &rows {
            for item in items {
                keys.push(eval_expr(&item.expr, row, eval_ctx));
            }
        }
        let seqs = if bound.is_some() {
            (0..rows.len() as u64).collect()
        } else {
            Vec::new()
        };
        let next_seq = rows.len() as u64;
        Self {
            items,
            bound,
            rows,
            keys,
            seqs,
            cutoff: None,
            next_seq,
            scratch: Vec::new(),
        }
    }

    pub(crate) fn push<S: GraphStorage>(&mut self, row: Row, eval_ctx: &EvalContext<'_, S>) {
        let Some(bound) = self.bound else {
            for item in self.items {
                self.keys.push(eval_expr(&item.expr, &row, eval_ctx));
            }
            self.rows.push(row);
            return;
        };
        if bound == 0 {
            return;
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.scratch.clear();
        for item in self.items {
            self.scratch.push(eval_expr(&item.expr, &row, eval_ctx));
        }
        // A later row loses every tie, so equal to the cutoff is out too.
        if let Some(cutoff) = &self.cutoff {
            if compare_keys(self.items, &self.scratch, cutoff) != Ordering::Less {
                return;
            }
        }
        self.keys.append(&mut self.scratch);
        self.rows.push(row);
        self.seqs.push(seq);
        // Cut back to `bound` once twice that many are held: amortized
        // linear, and memory stays O(bound).
        if self.rows.len() >= bound.saturating_mul(2).max(64) {
            self.cut_to(bound);
        }
    }

    /// Keep only the best `bound` rows (in no particular order) and
    /// remember the worst of them as the cutoff.
    fn cut_to(&mut self, bound: usize) {
        if self.rows.len() <= bound {
            return;
        }
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        order.select_nth_unstable_by(bound - 1, |&a, &b| self.cmp_slots(a, b));
        let mut keep = vec![false; self.rows.len()];
        for &slot in &order[..bound] {
            keep[slot] = true;
        }
        let n = self.items.len();
        let worst = order[bound - 1];
        self.cutoff = Some(self.keys[worst * n..(worst + 1) * n].to_vec());

        let mut slot = 0;
        self.rows.retain(|_| {
            slot += 1;
            keep[slot - 1]
        });
        let mut slot = 0;
        self.seqs.retain(|_| {
            slot += 1;
            keep[slot - 1]
        });
        let mut i = 0;
        self.keys.retain(|_| {
            i += 1;
            keep[(i - 1) / n]
        });
    }

    /// The kept rows, in order.
    pub(crate) fn finish(mut self) -> Vec<Row> {
        if let Some(bound) = self.bound {
            if bound == 0 {
                return Vec::new();
            }
            self.cut_to(bound);
        }
        let len = self.rows.len();
        if len < 2 {
            return self.rows;
        }
        let mut order: Vec<usize> = (0..len).collect();
        order.sort_unstable_by(|&a, &b| self.cmp_slots(a, b));
        // Already in order (common when rows arrive sorted): no moves.
        if order.iter().enumerate().all(|(i, &slot)| i == slot) {
            return self.rows;
        }
        let mut rows = self.rows;
        order
            .into_iter()
            .map(|slot| std::mem::take(&mut rows[slot]))
            .collect()
    }

    /// Row order of two slots: keys, then arrival.
    fn cmp_slots(&self, a: usize, b: usize) -> Ordering {
        let n = self.items.len();
        compare_keys(
            self.items,
            &self.keys[a * n..(a + 1) * n],
            &self.keys[b * n..(b + 1) * n],
        )
        .then_with(|| {
            if self.seqs.is_empty() {
                a.cmp(&b)
            } else {
                self.seqs[a].cmp(&self.seqs[b])
            }
        })
    }
}

fn compare_keys(items: &[ResolvedSortItem], a: &[LoraValue], b: &[LoraValue]) -> Ordering {
    for ((item, av), bv) in items.iter().zip(a).zip(b) {
        let ord = compare_values_for_sort(av, bv, matches!(item.direction, SortDirection::Asc));
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
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

#[cfg(test)]
mod tests {
    use super::*;
    use lora_analyzer::symbols::VarId;
    use lora_analyzer::ResolvedExpr;
    use lora_store::InMemoryGraph;
    use std::collections::BTreeMap;

    fn item(var: u32, direction: SortDirection) -> ResolvedSortItem {
        ResolvedSortItem {
            expr: ResolvedExpr::Variable(VarId(var)),
            direction,
        }
    }

    fn row(a: LoraValue, b: i64) -> Row {
        let mut r = Row::new();
        r.insert(VarId(0), a);
        r.insert(VarId(1), LoraValue::Int(b));
        r
    }

    fn values() -> Vec<LoraValue> {
        vec![
            LoraValue::Int(3),
            LoraValue::Null,
            LoraValue::String("b".into()),
            LoraValue::Float(1.5),
            LoraValue::Int(3),
            LoraValue::Bool(true),
            LoraValue::String("a".into()),
            LoraValue::Null,
            LoraValue::Int(-7),
            LoraValue::Float(3.0),
            LoraValue::Int(1),
            LoraValue::String("b".into()),
        ]
    }

    /// The reference: a stable full sort comparing freshly evaluated keys.
    fn reference(rows: &[Row], items: &[ResolvedSortItem]) -> Vec<(LoraValue, LoraValue)> {
        let mut rows = rows.to_vec();
        rows.sort_by(|a, b| {
            let ka: Vec<LoraValue> = items
                .iter()
                .map(|i| {
                    a.get(match &i.expr {
                        ResolvedExpr::Variable(v) => *v,
                        _ => unreachable!(),
                    })
                    .cloned()
                    .unwrap()
                })
                .collect();
            let kb: Vec<LoraValue> = items
                .iter()
                .map(|i| {
                    b.get(match &i.expr {
                        ResolvedExpr::Variable(v) => *v,
                        _ => unreachable!(),
                    })
                    .cloned()
                    .unwrap()
                })
                .collect();
            compare_keys(items, &ka, &kb)
        });
        rows.iter().map(project).collect()
    }

    fn project(r: &Row) -> (LoraValue, LoraValue) {
        (
            r.get(VarId(0)).cloned().unwrap(),
            r.get(VarId(1)).cloned().unwrap(),
        )
    }

    #[test]
    fn bounded_sort_matches_stable_sort_prefix() {
        let graph = InMemoryGraph::new();
        let params = BTreeMap::new();
        let eval_ctx = EvalContext {
            storage: &graph,
            params: &params,
        };
        let rows: Vec<Row> = values()
            .into_iter()
            .enumerate()
            .map(|(i, v)| row(v, i as i64))
            .collect();
        for items in [
            vec![item(0, SortDirection::Asc)],
            vec![item(0, SortDirection::Desc)],
            vec![item(0, SortDirection::Asc), item(1, SortDirection::Desc)],
        ] {
            let expected = reference(&rows, &items);
            for k in 0..=rows.len() + 2 {
                let mut got = rows.clone();
                sort_rows_with_top_k(&mut got, &items, &eval_ctx, Some(k));
                let got: Vec<_> = got.iter().map(project).collect();
                assert_eq!(got, expected[..k.min(expected.len())], "k={k}");
            }
            let mut got = rows.clone();
            sort_rows_with_top_k(&mut got, &items, &eval_ctx, None);
            let got: Vec<_> = got.iter().map(project).collect();
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn streamed_bound_cuts_match_stable_sort_prefix() {
        let graph = InMemoryGraph::new();
        let params = BTreeMap::new();
        let eval_ctx = EvalContext {
            storage: &graph,
            params: &params,
        };
        // Many ties and some nulls, in scrambled order.
        let rows: Vec<Row> = (0..700i64)
            .map(|i| {
                let j = (i * 7919) % 700;
                let a = match j % 11 {
                    0 => LoraValue::Null,
                    1 => LoraValue::Float((j % 5) as f64 + 0.5),
                    _ => LoraValue::Int(j % 17),
                };
                row(a, i)
            })
            .collect();
        for items in [
            vec![item(0, SortDirection::Asc)],
            vec![item(0, SortDirection::Desc)],
        ] {
            let expected = reference(&rows, &items);
            for k in [0, 1, 5, 31, 32, 33, 100, 350, 699, 700, 900] {
                let mut buffer = SortBuffer::new(&items, Some(k));
                for r in rows.iter().cloned() {
                    buffer.push(r, &eval_ctx);
                }
                let got: Vec<_> = buffer.finish().iter().map(project).collect();
                assert_eq!(got, expected[..k.min(expected.len())], "streamed k={k}");
                let mut got = rows.clone();
                sort_rows_with_top_k(&mut got, &items, &eval_ctx, Some(k));
                let got: Vec<_> = got.iter().map(project).collect();
                assert_eq!(got, expected[..k.min(expected.len())], "buffered k={k}");
            }
        }
    }
}
