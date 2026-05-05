# Performance Considerations

## Current bottlenecks

### Snapshot publication and writer serialization

The database stores the graph in an `ArcSwap<InMemoryGraph>`.
Auto-commit read-only queries load an `Arc` snapshot and run without holding a
store lock, so readers can overlap and keep seeing their pinned snapshot after a
writer publishes a newer graph. Mutating auto-commit queries stage a
copy-on-write clone, then serialize commit publication through the database
writer mutex. Explicit read-write transactions hold that writer mutex for the
transaction lifetime.

This means:

- Concurrent read-only queries are allowed and do not block write staging.
- Write commits serialize at publication time.
- Explicit read-write transactions block other writers until commit/rollback.
- A long-running stream pins its snapshot, which can increase memory pressure by
  keeping older `Arc` records alive.

**Source**: `crates/lora-database/src/database/mod.rs`,
`crates/lora-database/src/database/execute.rs`,
`crates/lora-database/src/database/occ.rs`, and
`crates/lora-database/src/transaction.rs`.

`execute_with_timeout` / `execute_with_params_timeout` add cooperative
deadline checks during executor work. The checks are not preemptive; very large
single operator steps can still run until they reach the next check.

### Clone-heavy read API

The compatibility surface on `GraphStorage` still returns owned
`Vec<NodeRecord>` and `Vec<RelationshipRecord>`. Callers using those helpers
clone all matching records:

```rust
fn all_nodes(&self) -> Vec<NodeRecord>;           // clones all nodes
fn nodes_by_label(&self, label: &str) -> Vec<NodeRecord>;  // clones matching nodes
```

For a graph with 1M nodes, `MATCH (n) RETURN n` allocates and clones all 1M records.
Borrow-capable backends can implement `BorrowedGraphStorage` and the
`with_node` / `with_relationship` hooks to avoid clones on hot paths, but more
bulk compatibility APIs still return owned records.

**Source**: `crates/lora-store/src/traits.rs` and
`crates/lora-store/src/memory/impls.rs`.

### Property index coverage

`InMemoryGraph` now keeps hash-based secondary indexes for equality lookups on
node and relationship properties:

```cypher
MATCH (n:User {email: 'alice@example.com'}) RETURN n
```

For indexable values, `find_nodes_by_property` and
`find_relationships_by_property` go directly through the property index and then
intersect with the label / relationship-type index when one is present. The
index currently covers `null`, booleans, integers, non-NaN floats, strings, and
nested lists/maps that contain only those values.

Temporal, spatial, vector, and NaN float values deliberately fall back to the
old scan path until the storage crate has a stable hash representation that
matches their equality semantics.

**Source**: `crates/lora-store/src/memory.rs` (`InMemoryGraph::find_nodes_by_property`, `InMemoryGraph::find_relationships_by_property`)

### Partially streaming execution

The executor now has a pull-based row pipeline for the common read path and
for auto-commit streaming writes. `MATCH`, single-hop expansion, `WHERE`,
projection, `UNWIND`, `LIMIT`, and write roots such as `CREATE`, `SET`,
`DELETE`, `REMOVE`, and `MERGE` can compose through row cursors.

Some operators still have blocking internals because their semantics require a
whole input set:

- `ORDER BY`, `DISTINCT`, `UNION`, and aggregations buffer internally, then
  yield rows lazily to downstream consumers.
- `OPTIONAL MATCH` streams its outer input but materializes the inner plan once.
- Shortest-path filtering and variable-length expansion still allocate traversal
  state.

The practical win is reduced peak memory for downstream writes: a query such as
`MATCH ... WITH ... ORDER BY ... CREATE ... RETURN ...` no longer has to hold
both the blocking operator's full output and the write operator's full output at
the same time.

### Snapshot save / load

Snapshot operations publish through the same current-store pointer but do not
use the old `RwLock` model.

- **Save.** `Database::save_snapshot_to` loads an `Arc` snapshot, encodes it with
  the `lora-snapshot` columnar codec, writes to `<path>.tmp`, `fsync`s, renames,
  and best-effort syncs the parent directory. Encoding is `O(n + r)`.
- **Load.** `Database::load_snapshot_from` decodes the file into a fresh graph,
  rebuilds adjacency and label/type/property index state, then publishes the new
  `Arc`. Decode/rebuild is also `O(n + r)`.

Practical rule: do not schedule a save at a cadence smaller than the measured
save duration — overlapping saves can amplify writer stalls. For large graphs,
prefer a cron that calls `POST /admin/snapshot/save` at an interval larger than
the measured save wall-time.

**Source**: `crates/lora-snapshot`, `crates/lora-store/src/snapshot.rs`, and
`crates/lora-database/src/snapshot/`. Round-trip coverage lives in
`crates/lora-database/tests/snapshot.rs`; there is no dedicated benchmark file
yet (potential future slot: `crates/lora-database/benches/snapshot_benchmarks.rs`).

See also [Snapshots (operator doc)](../operations/snapshots.md) and [Data Flow → Concurrency model](../architecture/data-flow.md#concurrency-model).

### Cross-product in multi-pattern MATCH

```cypher
MATCH (a:User), (b:User) CREATE (a)-[:KNOWS]->(b)
```

The planner chains two `NodeScan` operators, producing a cross-product. For `N` users, this is `N^2` rows.

## Build-time optimizations

The `.cargo/config.toml` enables aggressive optimization for release builds:

```toml
[build]
rustflags = ["-C", "target-cpu=native"]

[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"
```

- `target-cpu=native` -- uses SIMD and CPU-specific instructions
- `lto = "fat"` -- cross-crate link-time optimization
- `codegen-units = 1` -- better optimization at the cost of compile time
- `panic = "abort"` -- removes unwinding tables

## Query optimizer

The optimizer currently implements one rule:

### Filter push-down

Moves `Filter` operators below `Projection` operators when safe:

```
Before:  Filter(Projection(input))
After:   Projection(Filter(input))
```

Conditions for push-down:
- The projection is not `DISTINCT`
- The projection does not use `include_existing` (star projection)

**Source**: `crates/lora-compiler/src/optimizer.rs` (`push_filter_below_projection`)

### Not implemented

| Optimization | Description |
|-------------|-------------|
| Join ordering | No cost-based optimization for multi-pattern queries |
| Index selection | Equality lookups can use in-memory property indexes; the compiler still has no cost-based index selection |
| Predicate decomposition | Compound `AND` predicates are not split for selective push-down |
| Limit push-down | `LIMIT` is not pushed to scan operators |
| Redundant scan elimination | Rescanning the same label is not detected |
| Short-circuit evaluation | Filters evaluate all predicates regardless |
| Common subexpression elimination | Not implemented |

## Data structure choices

### Slot vectors and BTreeMap catalogs

Primary node and relationship storage uses slot-indexed vectors, so point lookup
by ID is direct. Catalog-style indexes use `BTreeMap<String, Vec<Id>>` for
deterministic label/type ordering, and properties remain `BTreeMap` for stable
map order. Lazy property indexes use hashable keys internally for exact-match
lookups.

### SmallVec for labels and types

The AST uses `SmallVec<String, 2>` for node labels and relationship types, avoiding heap allocation for the common case of 1-2 labels/types.

**Source**: `crates/lora-ast/src/ast.rs` (`NodePattern`, `RelationshipPattern`)

## Memory usage

The in-memory graph stores:
- One `NodeRecord` per node (labels Vec + properties BTreeMap)
- One `RelationshipRecord` per relationship
- Two adjacency entries per relationship (outgoing + incoming)
- One label index entry per (label, node) pair
- One type index entry per (type, relationship) pair
- One property index entry per indexable (property key, property value, node/relationship) tuple

Rough estimate: a node with 1 label and 5 string properties ≈ 200-400 bytes before
secondary index overhead. A relationship with 2 properties ≈ 150-300 bytes before
secondary index overhead. Adjacency indexes add ~50 bytes per relationship.
Property indexes trade additional memory for faster equality filtering on common
scalar and scalar-container properties.

## Recommendations for improvement

1. **Write publication windows** -- continue shrinking serialized commit/checkpoint/restore windows for write-heavy paths
2. **Borrowing APIs** -- migrate more executor internals from owned `GraphStorage` helpers onto `with_node` / `with_relationship` and other borrowed hooks
3. **Property indexes** -- extend the current hash indexes to temporal / spatial / vector values once canonical hash keys are available
4. **Streaming coverage** -- keep moving remaining blocking internals toward cursor-shaped sources where semantics allow it
5. **Query timeout coverage** -- extend deadline cancellation into streaming APIs and more fine-grained executor loops
6. **HashMap option** -- consider `HashMap` for primary storage when ordering is not needed

## Next steps

- See measured numbers for each area: [Benchmarks](benchmarks.md)
- Storage layout the suggestions above affect: [Graph Engine](../architecture/graph-engine.md)
- Pipeline stages and where cost accumulates: [Data Flow](../architecture/data-flow.md)
- Need indexes, concurrency, and persistence today? [LoraDB managed platform](https://loradb.com)
