//! Path-construction operator source.

use lora_analyzer::symbols::VarId;
use lora_store::GraphStorage;

use crate::errors::ExecResult;
use crate::executor::build_path_value;
use crate::value::{LoraValue, Row};

use super::traits::{RowSource, StreamCtx};

/// Path-building source. Ordinary path construction is one-in/one-out.
/// Shortest-path filtering still has to compare the complete path set,
/// so that mode drains internally before yielding.
pub struct PathBuildSource<'a, S: GraphStorage> {
    state: PathBuildState<'a, S>,
}

enum PathBuildState<'a, S: GraphStorage> {
    Streaming {
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        output: VarId,
        node_vars: &'a [VarId],
        rel_vars: &'a [VarId],
    },
    PendingShortest {
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        output: VarId,
        node_vars: &'a [VarId],
        rel_vars: &'a [VarId],
        all: bool,
    },
    Yielding(std::vec::IntoIter<Row>),
}

impl<'a, S: GraphStorage> PathBuildSource<'a, S> {
    pub(super) fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        output: VarId,
        node_vars: &'a [VarId],
        rel_vars: &'a [VarId],
        shortest_path_all: Option<bool>,
    ) -> Self {
        let state = match shortest_path_all {
            Some(all) => PathBuildState::PendingShortest {
                upstream,
                ctx,
                output,
                node_vars,
                rel_vars,
                all,
            },
            None => PathBuildState::Streaming {
                upstream,
                ctx,
                output,
                node_vars,
                rel_vars,
            },
        };
        Self { state }
    }

    fn attach_path(
        mut row: Row,
        ctx: &StreamCtx<'a, S>,
        output: VarId,
        node_vars: &[VarId],
        rel_vars: &[VarId],
    ) -> Row {
        let path = build_path_value(&row, node_vars, rel_vars, ctx.storage);
        row.insert(output, path);
        row
    }

    fn shortest_path_rows(
        upstream: &mut Box<dyn RowSource + 'a>,
        ctx: &StreamCtx<'a, S>,
        output: VarId,
        node_vars: &[VarId],
        rel_vars: &[VarId],
        all: bool,
    ) -> ExecResult<Vec<Row>> {
        let mut best_len: Option<usize> = None;
        let mut best_rows = Vec::new();

        while let Some(row) = upstream.next_row()? {
            let row = Self::attach_path(row, ctx, output, node_vars, rel_vars);
            let path_len = match row.get(output) {
                Some(LoraValue::Path(path)) => path.rels.len(),
                _ => usize::MAX,
            };

            match best_len {
                None => {
                    best_len = Some(path_len);
                    best_rows.push(row);
                }
                Some(current) if path_len < current => {
                    best_len = Some(path_len);
                    best_rows.clear();
                    best_rows.push(row);
                }
                Some(current) if path_len == current && all => best_rows.push(row),
                _ => {}
            }
        }

        Ok(best_rows)
    }
}

impl<'a, S: GraphStorage> RowSource for PathBuildSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            match &mut self.state {
                PathBuildState::Streaming {
                    upstream,
                    ctx,
                    output,
                    node_vars,
                    rel_vars,
                } => {
                    return Ok(upstream
                        .next_row()?
                        .map(|row| Self::attach_path(row, ctx, *output, node_vars, rel_vars)));
                }
                PathBuildState::PendingShortest {
                    upstream,
                    ctx,
                    output,
                    node_vars,
                    rel_vars,
                    all,
                } => {
                    let rows = Self::shortest_path_rows(
                        upstream, ctx, *output, node_vars, rel_vars, *all,
                    )?;
                    self.state = PathBuildState::Yielding(rows.into_iter());
                }
                PathBuildState::Yielding(it) => return Ok(it.next()),
            }
        }
    }
}
