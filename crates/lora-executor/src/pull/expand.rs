//! Expand operator sources.
//!
//! - [`ExpandSource`] — single-hop edge expansion.
//! - [`VariableLengthExpandSource`] — BFS variable-length expansion
//!   yielding one matching path at a time, sharing path prefixes via
//!   the [`PathSegment`] singly-linked structure.

use std::sync::Arc;

use lora_analyzer::symbols::VarId;
use lora_analyzer::ResolvedExpr;
use lora_ast::{Direction, RangeLiteral};
use lora_store::{GraphStorage, NodeId, RelationshipId};

use crate::errors::{value_kind, ExecResult, ExecutorError};
use crate::eval::eval_expr;
use crate::executor::{resolve_range, value_matches_property_value};
use crate::value::{LoraValue, Row};

use super::{RowSource, StreamCtx};

/// Single-hop expansion. For each input row, walks edges from `src`
/// through the configured `direction` and `types` and emits one row
/// per matching `(rel, dst)` pair, optionally filtering by relationship
/// properties.
pub struct ExpandSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    src: VarId,
    rel: Option<VarId>,
    dst: VarId,
    types: &'a [String],
    direction: Direction,
    rel_properties: Option<&'a ResolvedExpr>,
    cur_row: Option<Row>,
    cur_edges: Vec<(u64, NodeId)>,
    cur_idx: usize,
}

impl<'a, S: GraphStorage> ExpandSource<'a, S> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        src: VarId,
        rel: Option<VarId>,
        dst: VarId,
        types: &'a [String],
        direction: Direction,
        rel_properties: Option<&'a ResolvedExpr>,
    ) -> Self {
        Self {
            upstream,
            ctx,
            src,
            rel,
            dst,
            types,
            direction,
            rel_properties,
            cur_row: None,
            cur_edges: Vec::new(),
            cur_idx: 0,
        }
    }
}

impl<'a, S: GraphStorage> RowSource for ExpandSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if self.cur_row.is_none() {
                match self.upstream.next_row()? {
                    Some(row) => {
                        // Resolve src now so we can `continue` out of
                        // rows that don't bind it.
                        let src_id = match row.get(self.src) {
                            Some(LoraValue::Node(id)) => *id,
                            Some(other) => {
                                return Err(ExecutorError::ExpectedNodeForExpand {
                                    var: format!("{:?}", self.src),
                                    found: value_kind(other),
                                });
                            }
                            None => continue,
                        };
                        self.cur_edges =
                            self.ctx
                                .storage
                                .expand_ids(src_id, self.direction, self.types);
                        self.cur_idx = 0;
                        self.cur_row = Some(row);
                    }
                    None => return Ok(None),
                }
            }

            let row_ref = self.cur_row.as_ref().unwrap();

            while self.cur_idx < self.cur_edges.len() {
                let (rel_id, dst_id) = self.cur_edges[self.cur_idx];
                self.cur_idx += 1;

                // Relationship-property prefilter.
                if let Some(expr) = self.rel_properties {
                    let actual = self
                        .ctx
                        .storage
                        .with_relationship(rel_id, |rel| rel.properties.clone());
                    let matches = match actual {
                        Some(props) => {
                            let eval_ctx = self.ctx.eval_ctx();
                            let expected = eval_expr(expr, row_ref, &eval_ctx);
                            let LoraValue::Map(map) = expected else {
                                return Err(ExecutorError::ExpectedPropertyMap {
                                    found: value_kind(&expected),
                                });
                            };
                            map.iter().all(|(k, v)| {
                                props
                                    .get(k)
                                    .map(|actual| value_matches_property_value(v, actual))
                                    .unwrap_or(false)
                            })
                        }
                        None => false,
                    };
                    if !matches {
                        continue;
                    }
                }

                // Existing-binding cross-checks for dst and rel.
                if let Some(existing_dst) = row_ref.get(self.dst) {
                    match existing_dst {
                        LoraValue::Node(id) if *id == dst_id => {}
                        LoraValue::Node(_) => continue,
                        other => {
                            return Err(ExecutorError::ExpectedNodeForExpand {
                                var: format!("{:?}", self.dst),
                                found: value_kind(other),
                            });
                        }
                    }
                }
                if let Some(rel_var) = self.rel {
                    if let Some(existing_rel) = row_ref.get(rel_var) {
                        match existing_rel {
                            LoraValue::Relationship(id) if *id == rel_id => {}
                            LoraValue::Relationship(_) => continue,
                            other => {
                                return Err(ExecutorError::ExpectedRelationshipForExpand {
                                    var: format!("{:?}", rel_var),
                                    found: value_kind(other),
                                });
                            }
                        }
                    }
                }

                let mut new_row = row_ref.clone();
                if !new_row.contains_key(self.dst) {
                    new_row.insert(self.dst, LoraValue::Node(dst_id));
                }
                if let Some(rel_var) = self.rel {
                    if !new_row.contains_key(rel_var) {
                        new_row.insert(rel_var, LoraValue::Relationship(rel_id));
                    }
                }
                return Ok(Some(new_row));
            }

            // Edges for the current input row exhausted.
            self.cur_row = None;
            self.cur_edges.clear();
            self.cur_idx = 0;
        }
    }
}

/// Singly-linked path segment used by [`VariableLengthExpandSource`] to share
/// path prefixes between BFS frontier entries. Each new path adds one
/// `Arc<PathSegment>` allocation regardless of depth, and cloning is a
/// refcount bump — vs. the previous `Vec<RelationshipId>`-per-frontier-entry
/// which copied the entire prefix on every step (O(branching^depth × depth)
/// at worst).
#[derive(Debug)]
struct PathSegment {
    rel: RelationshipId,
    parent: Option<Arc<PathSegment>>,
}

impl PathSegment {
    fn contains(self_: &Option<Arc<Self>>, rel_id: RelationshipId) -> bool {
        let mut cur = self_.as_ref();
        while let Some(node) = cur {
            if node.rel == rel_id {
                return true;
            }
            cur = node.parent.as_ref();
        }
        false
    }

    fn len(self_: &Option<Arc<Self>>) -> usize {
        let mut cur = self_.as_ref();
        let mut n = 0;
        while let Some(node) = cur {
            n += 1;
            cur = node.parent.as_ref();
        }
        n
    }

    /// Materialize the path into a flat `Vec<RelationshipId>`. Reverses
    /// in place because segments link tip → root.
    fn to_vec(self_: &Option<Arc<Self>>) -> Vec<RelationshipId> {
        let len = Self::len(self_);
        let mut out = Vec::with_capacity(len);
        let mut cur = self_.as_ref();
        while let Some(node) = cur {
            out.push(node.rel);
            cur = node.parent.as_ref();
        }
        out.reverse();
        out
    }
}

/// Variable-length expansion streams its upstream and walks one input row's BFS
/// frontier incrementally, yielding each matching path as it is discovered.
pub struct VariableLengthExpandSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    ctx: StreamCtx<'a, S>,
    src: VarId,
    rel: Option<VarId>,
    dst: VarId,
    types: &'a [String],
    direction: Direction,
    min_hops: u64,
    max_hops: u64,
    cur_row: Option<Row>,
    pending_zero_hop: bool,
    /// Each frontier entry is `(node, path-from-source)`. The `Option<Arc>`
    /// is `None` for the seed (zero-length path); subsequent steps share
    /// the entire prefix via the Arc chain.
    frontier: Vec<(NodeId, Option<Arc<PathSegment>>)>,
    frontier_idx: usize,
    next_frontier: Vec<(NodeId, Option<Arc<PathSegment>>)>,
    depth: u64,
    cur_path_node: Option<NodeId>,
    cur_path: Option<Arc<PathSegment>>,
    cur_edges: Vec<(u64, NodeId)>,
    cur_edge_idx: usize,
}

impl<'a, S: GraphStorage> VariableLengthExpandSource<'a, S> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        src: VarId,
        rel: Option<VarId>,
        dst: VarId,
        types: &'a [String],
        direction: Direction,
        range: &'a RangeLiteral,
    ) -> Self {
        let (min_hops, max_hops) = resolve_range(range);
        Self {
            upstream,
            ctx,
            src,
            rel,
            dst,
            types,
            direction,
            min_hops,
            max_hops,
            cur_row: None,
            pending_zero_hop: false,
            frontier: Vec::new(),
            frontier_idx: 0,
            next_frontier: Vec::new(),
            depth: 1,
            cur_path_node: None,
            cur_path: None,
            cur_edges: Vec::new(),
            cur_edge_idx: 0,
        }
    }

    fn start_row(&mut self, row: Row, src_id: NodeId) {
        self.cur_row = Some(row);
        self.pending_zero_hop = self.min_hops == 0;
        self.frontier.clear();
        self.frontier.push((src_id, None));
        self.frontier_idx = 0;
        self.next_frontier.clear();
        self.depth = 1;
        self.cur_path_node = None;
        self.cur_path = None;
        self.cur_edges.clear();
        self.cur_edge_idx = 0;
    }

    fn clear_current_row(&mut self) {
        self.cur_row = None;
        self.pending_zero_hop = false;
        self.frontier.clear();
        self.frontier_idx = 0;
        self.next_frontier.clear();
        self.depth = 1;
        self.cur_path_node = None;
        self.cur_path = None;
        self.cur_edges.clear();
        self.cur_edge_idx = 0;
    }

    fn row_for_path(&self, dst_node_id: NodeId, rel_ids: &[u64]) -> Row {
        let mut new_row = self
            .cur_row
            .as_ref()
            .expect("cur_row is set while yielding variable-length results")
            .clone();
        new_row.insert(self.dst, LoraValue::Node(dst_node_id));

        if let Some(rel_var) = self.rel {
            let rels = rel_ids
                .iter()
                .copied()
                .map(LoraValue::Relationship)
                .collect();
            new_row.insert(rel_var, LoraValue::List(rels));
        }

        new_row
    }

    fn advance_frontier(&mut self) -> bool {
        if self.next_frontier.is_empty() || self.depth >= self.max_hops {
            return false;
        }

        std::mem::swap(&mut self.frontier, &mut self.next_frontier);
        self.next_frontier.clear();
        self.frontier_idx = 0;
        self.depth += 1;
        true
    }
}

impl<'a, S: GraphStorage> RowSource for VariableLengthExpandSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            if self.cur_row.is_none() {
                match self.upstream.next_row()? {
                    Some(row) => {
                        let src_id = match row.get(self.src) {
                            Some(LoraValue::Node(id)) => *id,
                            Some(other) => {
                                return Err(ExecutorError::ExpectedNodeForExpand {
                                    var: format!("{:?}", self.src),
                                    found: value_kind(other),
                                });
                            }
                            None => continue,
                        };
                        self.start_row(row, src_id);
                    }
                    None => return Ok(None),
                }
            }

            if self.pending_zero_hop {
                self.pending_zero_hop = false;
                if let Some(LoraValue::Node(src_id)) = self
                    .cur_row
                    .as_ref()
                    .expect("cur_row is initialized before zero-hop yield")
                    .get(self.src)
                {
                    return Ok(Some(self.row_for_path(*src_id, &[])));
                }
            }

            loop {
                if self.depth > self.max_hops {
                    self.clear_current_row();
                    break;
                }

                if self.cur_path_node.is_none() {
                    if self.frontier_idx >= self.frontier.len() {
                        if self.advance_frontier() {
                            continue;
                        }
                        self.clear_current_row();
                        break;
                    }

                    let (node_id, path) = &self.frontier[self.frontier_idx];
                    self.frontier_idx += 1;
                    self.cur_path_node = Some(*node_id);
                    self.cur_path = path.clone();
                    self.cur_edges =
                        self.ctx
                            .storage
                            .expand_ids(*node_id, self.direction, self.types);
                    self.cur_edge_idx = 0;
                }

                while self.cur_edge_idx < self.cur_edges.len() {
                    let (rel_id, neighbor_id) = self.cur_edges[self.cur_edge_idx];
                    self.cur_edge_idx += 1;

                    if PathSegment::contains(&self.cur_path, rel_id) {
                        continue;
                    }

                    // One Arc allocation per new path. The prefix is shared
                    // structurally — `cur_path` becomes this new segment's
                    // parent via a refcount bump, no copy of the prefix.
                    let new_path = Arc::new(PathSegment {
                        rel: rel_id,
                        parent: self.cur_path.clone(),
                    });

                    if self.depth < self.max_hops {
                        self.next_frontier
                            .push((neighbor_id, Some(new_path.clone())));
                    }

                    if self.depth >= self.min_hops {
                        // Materialize the path to a flat Vec only when we
                        // actually emit a row — most edges visited during
                        // BFS never reach this branch.
                        let rel_ids = PathSegment::to_vec(&Some(new_path));
                        return Ok(Some(self.row_for_path(neighbor_id, &rel_ids)));
                    }
                }

                self.cur_path_node = None;
                self.cur_path = None;
                self.cur_edges.clear();
                self.cur_edge_idx = 0;
            }
        }
    }
}
