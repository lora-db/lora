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
//!   sort / aggregate / dedup primitives
//!   ([`compute_aggregate_expr`], [`compare_sort_item`],
//!   `dedup_rows`, `dedup_rows_by_vars`),
//!   record hydration
//!   ([`hydrate_node_record`], [`hydrate_relationship_record`]),
//!   the [`GroupValueKey`] dedup-key / aggregate-key wrapper, and
//!   variable-length range resolution ([`resolve_range`],
//!   `VarLenResult`).

mod helpers;
mod immutable;
mod mutable;

pub use helpers::value_matches_property_value;
pub use immutable::{ExecutionContext, Executor};
pub use mutable::{MutableExecutionContext, MutableExecutor};

// Crate-internal re-exports needed by callers in `crate::pull` and
// `crate::eval` (and by the buffered executor's own siblings reaching
// each other through `super::helpers::*`). The names that remain here
// are exactly the ones referenced as `crate::executor::*` from
// outside `crate::executor`.
pub(crate) use helpers::{
    build_path_value, compare_sort_item, compute_aggregate_expr, hydrate_node_record,
    hydrate_relationship_record, indexed_node_property_candidates,
    label_group_candidates_prefiltered, node_matches_label_groups, node_matches_property_filter,
    resolve_range, scan_node_ids_for_label_groups, GroupValueKey,
};
