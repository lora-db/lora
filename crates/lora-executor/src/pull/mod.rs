//! Pull-based row pipeline.
//!
//! [`RowSource`] is the fallible row cursor; [`PullExecutor::open_compiled`]
//! and [`MutablePullExecutor::open_compiled`] return a `Box<dyn RowSource + 'a>`
//! representing a streaming query plan execution.
//!
//! ## Architecture
//!
//! The streaming-listed operators have real per-operator
//! [`RowSource`] implementations that pull from their upstream one
//! row at a time:
//!
//! * [`ArgumentSource`]
//! * [`NodeScanSource`]
//! * [`NodeByLabelScanSource`]
//! * [`ExpandSource`] (single-hop)
//! * [`VariableLengthExpandSource`]
//! * [`FilterSource`]
//! * [`ProjectionSource`]
//! * [`DistinctSource`]
//! * [`UnwindSource`]
//! * [`LimitSource`]
//! * [`SortSource`] (buffers internally, yields lazily)
//! * [`HashAggregationSource`] (buffers internally, yields lazily)
//! * [`OptionalMatchSource`] (streams outer input, buffers inner once)
//! * [`PathBuildSource`]
//!
//! Blocking internals such as sort, aggregation, and shortest-path
//! filtering still allocate where the Cypher semantics require a
//! complete input set. Deduping operators keep only their seen-key
//! state and stream rows as soon as a new key appears.
//!
//! Hydration happens once at the top of the pipeline — operator
//! sources yield raw rows so intermediate evaluations work on
//! storage-borrowed values, and the topmost [`HydratingSource`]
//! converts node / relationship references to their full hydrated
//! map form before the row leaves the cursor.
//!
//! ## Layout
//! - `traits` — the [`RowSource`] trait, [`drain`], [`StreamCtx`],
//!   [`BufferedRowSource`], [`ArgumentSource`], [`HydratingSource`] and
//!   [`hydrate_value`]; the plan walker (`is_streaming_op`,
//!   `subtree_is_fully_streaming`, `build_streaming`,
//!   `compiled_to_streaming`, `write_op_input`); the
//!   [`PullExecutor`] / [`MutablePullExecutor`] entry points and the
//!   mutable cursor machinery ([`StreamingWriteCursor`],
//!   [`MutableUnionSource`]); [`collect_compiled`]; [`StreamShape`] and
//!   [`classify_stream`]; [`plan_result_columns`] /
//!   [`compiled_result_columns`].
//! - `scan` — node scan operator sources ([`NodeScanSource`],
//!   [`NodeByLabelScanSource`], [`NodeByPropertyScanSource`]).
//! - `expand` — single-hop and variable-length expansion
//!   ([`ExpandSource`], [`VariableLengthExpandSource`]).
//! - `filter` — predicate filter ([`FilterSource`]).
//! - `projection` — projection / unwind / distinct
//!   ([`ProjectionSource`], [`UnwindSource`], [`DistinctSource`]).
//! - `sort` — sort and limit ([`SortSource`], [`LimitSource`]).
//! - `aggregate` — hash aggregation and the streamable fold-only fast
//!   path ([`HashAggregationSource`], [`StreamableAggKind`],
//!   [`AggState`]).
//! - `optional` — outer OPTIONAL MATCH ([`OptionalMatchSource`]).
//! - `path` — path construction including SHORTEST PATH filtering
//!   ([`PathBuildSource`]).
//! - `union` — read-side UNION ([`UnionSource`]).

mod aggregate;
mod expand;
mod filter;
mod optional;
mod path;
mod projection;
mod scan;
mod sort;
mod traits;
mod union;

#[cfg(test)]
mod tests;

// Public surface — these names appear in `lora_executor`'s public
// API via the explicit `pub use pull::{...}` list in `lib.rs`.
pub use traits::{
    classify_stream, collect_compiled, compiled_result_columns, drain, plan_result_columns,
    BufferedRowSource, MutablePullExecutor, PullExecutor, RowSource, StreamShape,
};

// Crate-internal re-exports used by the buffered executor in
// `crate::executor` for the streaming aggregate fast-path and for
// the `StreamingWriteCursor` plan-shape probes.
pub(crate) use aggregate::{classify_streamable_aggregates, AggState, StreamableAggSpec};
pub(crate) use traits::{build_streaming, subtree_is_fully_streaming};
