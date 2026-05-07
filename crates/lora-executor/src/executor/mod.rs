//! Buffered execution: lower a [`PhysicalPlan`] into a `Vec<Row>` by
//! recursively materializing each operator. The pull-based streaming
//! pipeline lives in `crate::pull` and uses these helpers as its
//! buffered fallback for non-streamable subtrees.
//!
//! Layout:
//! - `immutable` — [`ExecutionContext`] and the read-only [`Executor`]
//!   plus its `exec_*` operator methods (NodeScan / Expand / Filter /
//!   Projection / Sort / Limit / OptionalMatch / PathBuild /
//!   HashAggregation, including the streaming-fold fast path that
//!   reuses [`crate::pull`]'s `StreamableAggSpec` machinery).
//! - `mutable` — [`MutableExecutionContext`], the read-write
//!   [`MutableExecutor`], and every write operator implementation
//!   (Create / Merge / Delete / Set / Remove). Owns the per-row
//!   pattern materialization helpers (`apply_create_pattern_*`,
//!   `try_match_merge_pattern`, `materialize_node_pattern`,
//!   `materialize_relationship_pattern`) plus the SET / REMOVE
//!   `EntityTarget` plumbing.
//! - `aggregation` — buffered hash aggregation shared by both executors,
//!   including the fold-only fast path reused from `crate::pull`.
//! - `optional` — OPTIONAL MATCH row compatibility, merge, and
//!   null-extension helpers shared by buffered and streaming execution.
//! - `helpers` — the cross-cutting helpers used by both executors and
//!   re-exported across the crate: structural property comparison
//!   ([`value_matches_property_value`]), label-group / property-index
//!   probes ([`indexed_node_property_candidates`],
//!   [`scan_node_ids_for_label_groups`],
//!   [`node_matches_label_groups`], [`node_matches_property_filter`],
//!   [`label_group_candidates_prefiltered`]),
//!   path construction ([`build_path_value`]), variable-length
//!   expansion (`variable_length_expand`) and shortest-path
//!   filtering (`filter_shortest_paths`),
//!   aggregate / dedup primitives
//!   ([`compute_aggregate_expr`],
//!   `dedup_rows`, `dedup_rows_by_vars`),
//!   record hydration
//!   ([`hydrate_node_record`], [`hydrate_relationship_record`]),
//!   the [`GroupValueKey`] dedup-key / aggregate-key wrapper, and
//!   variable-length range resolution ([`resolve_range`],
//!   `VarLenResult`).
//! - `sort` — buffered sort comparison and top-k candidate retention
//!   shared by buffered execution and `crate::pull::SortSource`.

mod aggregation;
mod helpers;
mod immutable;
mod mutable;
mod optional;
mod sort;

pub use helpers::value_matches_property_value;
pub use immutable::{ExecutionContext, Executor};
pub use mutable::{MutableExecutionContext, MutableExecutor};

// Crate-internal re-exports needed by callers in `crate::pull` and
// `crate::eval` (and by the buffered executor's own siblings reaching
// each other through `super::helpers::*`). The names that remain here
// are exactly the ones referenced as `crate::executor::*` from
// outside `crate::executor`.
pub(crate) use aggregation::aggregate_rows;
pub(crate) use helpers::{
    bound_node_id_for_expand, bound_relationship_id_for_expand, build_path_value,
    compute_aggregate_expr, hydrate_node_record, hydrate_relationship_record,
    indexed_node_property_candidates, label_group_candidates_prefiltered,
    node_matches_label_groups, node_matches_property_filter, resolve_range,
    scan_node_ids_for_label_groups, GroupValueKey,
};
pub(crate) use optional::{
    merge_optional_rows, null_extend_optional_row, optional_match_rows, optional_rows_compatible,
};
pub(crate) use sort::sort_rows_with_top_k;
