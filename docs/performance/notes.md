# Performance Considerations

## Current bottlenecks

### Store lock contention

The database stores the graph in `Arc<RwLock<InMemoryGraph>>`.
Auto-commit read-only queries analyze, compile, and execute under a shared read
lock, so multiple reads can overlap. Writes, explicit transactions, snapshot
loads, and WAL checkpoints still take the exclusive write side. This means:

- Concurrent read-only queries are allowed
- Write queries block reads and writes while they hold the write lock
- A long-running read stream can still delay writers until the stream is dropped

**Source**: `crates/lora-database/src/database.rs` (`Database::execute_with_params`)

`execute_with_timeout` / `execute_with_params_timeout` add cooperative
deadline checks during lock acquisition and executor work. The checks are not
preemptive; very large single operator steps can still run until they reach the
next check.

### Clone-heavy read API

The compatibility surface on `GraphStorage` still returns owned
`Vec<NodeRecord>` and `Vec<RelationshipRecord>`. Callers using those helpers
clone all matching records:

```rust
fn all_nodes(&self) -> Vec<NodeRecord>;           // clones all nodes
fn nodes_by_label(&self, label: &str) -> Vec<NodeRecord>;  // clones matching nodes
```

For a graph with 1M nodes, `MATCH (n) RETURN n` allocates and clones all 1M records.
Borrow-capable backends can now implement `BorrowedGraphStorage` iterator hooks
(`node_refs`, `node_refs_by_label`, `relationship_refs`,
`relationship_refs_by_type`) to expose borrowed scans, but more executor paths
still need to move onto those hooks before the owned helpers can be retired.

**Source**: `crates/lora-store/src/graph.rs` (`GraphStorage::all_nodes`, `nodes_by_label`) and `crates/lora-store/src/memory.rs` (`InMemoryGraph` overrides)

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

Snapshot operations coordinate through the store lock.

- **Save.** `Database::save_snapshot_to` acquires a read lock, bincode-serializes every `NodeRecord` and `RelationshipRecord`, writes the result to `<path>.tmp`, then `fsync`s and renames. The read-lock-held window covers the serialize step — it is `O(n + r)` in nodes and relationships. Other readers may proceed; writers wait.
- **Load.** `Database::load_snapshot_from` acquires the write lock for the entire deserialize + index-rebuild. Adjacency and label / type indexes are reconstructed from the deserialized records, which is also `O(n + r)`.

Practical rule: do not schedule a save at a cadence smaller than the measured
save duration — overlapping saves can amplify writer stalls. For large graphs,
prefer a cron that calls `POST /admin/snapshot/save` at an interval larger than
the measured save wall-time.

**Source**: `crates/lora-store/src/snapshot.rs`, `crates/lora-database/src/database.rs`. Round-trip coverage lives in `crates/lora-database/tests/snapshot.rs`; there is no dedicated benchmark file yet (potential future slot: `crates/lora-database/benches/snapshot_benchmarks.rs`).

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

### BTreeMap vs HashMap

The codebase uses `BTreeMap` and `BTreeSet` exclusively instead of `HashMap`/`HashSet`. This provides:

- **Deterministic iteration order** -- useful for testing and debugging
- **Slower point lookups** -- O(log n) vs O(1) amortized
- **No hashing overhead** -- avoids hash function cost

For a graph database workload, `HashMap` would likely be faster for point lookups but `BTreeMap` gives more predictable behavior.

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

1. **Read/write lock coverage** -- continue shrinking exclusive write-lock windows for admin and write-heavy paths
2. **Borrowing iterators** -- migrate more executor internals from owned `GraphStorage` helpers onto the new `BorrowedGraphStorage` iterator hooks
3. **Property indexes** -- extend the current hash indexes to temporal / spatial / vector values once canonical hash keys are available
4. **Streaming coverage** -- keep moving remaining blocking internals toward cursor-shaped sources where semantics allow it
5. **Query timeout coverage** -- extend deadline cancellation into streaming APIs and more fine-grained executor loops
6. **HashMap option** -- consider `HashMap` for primary storage when ordering is not needed

## Next steps

- See measured numbers for each area: [Benchmarks](benchmarks.md)
- Storage layout the suggestions above affect: [Graph Engine](../architecture/graph-engine.md)
- Pipeline stages and where cost accumulates: [Data Flow](../architecture/data-flow.md)
- Need indexes, concurrency, and persistence today? [LoraDB managed platform](https://loradb.com)
