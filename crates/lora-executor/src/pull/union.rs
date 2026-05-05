//! Read-side UNION source.

use std::collections::BTreeSet;

use crate::errors::ExecResult;
use crate::executor::GroupValueKey;
use crate::value::Row;

use super::RowSource;

/// Streaming UNION source. Pulls each branch in sequence. `UNION ALL`
/// passes rows through directly; plain `UNION` keeps a seen-key set and
/// yields the first row for each unique named column/value key.
///
/// Replaces the buffered fallback that previously sat in
/// `PullExecutor::open_compiled` for any UNION-bearing plan. The
/// consumer side is now streaming, so a write op on top of a UNION read
/// can stream its writes as the union yields.
pub struct UnionSource<'a> {
    branches: Vec<Box<dyn RowSource + 'a>>,
    branch_idx: usize,
    needs_dedup: bool,
    seen: BTreeSet<Vec<(String, GroupValueKey)>>,
}

impl<'a> UnionSource<'a> {
    pub(super) fn new(branches: Vec<Box<dyn RowSource + 'a>>, needs_dedup: bool) -> Self {
        Self {
            branches,
            branch_idx: 0,
            needs_dedup,
            seen: BTreeSet::new(),
        }
    }
}

impl<'a> RowSource for UnionSource<'a> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        while self.branch_idx < self.branches.len() {
            match self.branches[self.branch_idx].next_row()? {
                Some(row) => {
                    if self.needs_dedup {
                        let key = row
                            .iter_named()
                            .map(|(_, name, val)| {
                                (name.into_owned(), GroupValueKey::from_value(val))
                            })
                            .collect();
                        if !self.seen.insert(key) {
                            continue;
                        }
                    }
                    return Ok(Some(row));
                }
                None => {
                    self.branch_idx += 1;
                }
            }
        }
        Ok(None)
    }
}
