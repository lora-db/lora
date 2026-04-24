# Performance Considerations

## Current bottlenecks

### Global mutex serialization

All query execution holds `Arc<Mutex<InMemoryGraph>>` for the duration of analyze + compile + execute (parsing runs outside the lock). This means:

- No concurrent reads
- Write queries block all other queries
- A long-running `MATCH (n) RETURN n` on a large graph blocks everything

**Source**: `crates/lora-database/src/database.rs` (`Database::execute_with_params`)

> 🚀 **Production note** — If your workload depends on concurrent read throughput, the self-hosted core is not a fit — the mutex is a hard architectural ceiling. The [LoraDB managed platform](https://loradb.com) runs a different concurrency model designed for production traffic.

### Clone-heavy read API

The `GraphStorage` trait returns owned `Vec<NodeRecord>` and `Vec<RelationshipRecord>`. Every scan clones all matching records:

```rust
fn all_nodes(&self) -> Vec<NodeRecord>;           // clones all nodes
fn nodes_by_label(&self, label: &str) -> Vec<NodeRecord>;  // clones matching nodes
```

For a graph with 1M nodes, `MATCH (n) RETURN n` allocates and clones all 1M records.

**Source**: `crates/lora-store/src/graph.rs` (`GraphStorage::all_nodes`, `nodes_by_label`) and `crates/lora-store/src/memory.rs` (`InMemoryGraph` overrides)

### No property indexes

Property-based filters require full scans:

```cypher
MATCH (n:User {email: 'alice@example.com'}) RETURN n
```

This scans all `:User` nodes and checks each one's `email` property. The `find_nodes_by_property` helper uses this strategy:

```rust
fn find_nodes_by_property(&self, label: Option<&str>, key: &str, value: &PropertyValue) -> Vec<NodeRecord> {
    let candidates = match label { ... };
    candidates.into_iter().filter(|n| n.properties.get(key) == Some(value)).collect()
}
```

**Source**: `crates/lora-store/src/graph.rs` (`GraphStorage::find_nodes_by_property`)

### Volcano model overhead

The executor pulls rows one-at-a-time through recursive calls. Each operator processes its full input before returning. This is simple but:

- No pipelining / streaming
- Large intermediate result sets are fully materialized in memory
- Sorts and aggregations must buffer all input

### Snapshot save / load

Snapshot operations serialize against every query through the same global mutex.

- **Save.** `Database::save_snapshot_to` acquires the mutex, bincode-serializes every `NodeRecord` and `RelationshipRecord`, writes the result to `<path>.tmp`, then `fsync`s and renames. The mutex-held window covers the serialize step — it is `O(n + r)` in nodes and relationships. The `fsync` and rename happen against the tmp file but the mutex is still held until the write path completes.
- **Load.** `Database::load_snapshot_from` acquires the mutex for the entire deserialize + index-rebuild. Adjacency and label / type indexes are reconstructed from the deserialized records, which is also `O(n + r)`.

Practical rule: do not schedule a save at a cadence smaller than the measured save duration — overlapping saves means the second one waits behind the first and amplifies the stall. For large graphs, prefer a cron that calls `POST /admin/snapshot/save` at an interval larger than the measured save wall-time.

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
| Index selection | No property index to select |
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

Rough estimate: a node with 1 label and 5 string properties ≈ 200-400 bytes. A relationship with 2 properties ≈ 150-300 bytes. Adjacency indexes add ~50 bytes per relationship.

## Recommendations for improvement

1. **Read/write lock** -- replace `Mutex` with `RwLock` to allow concurrent reads
2. **Borrowing iterators** -- change `GraphStorage` to return iterators instead of owned Vecs
3. **Property indexes** -- add hash-based indexes for commonly-filtered properties
4. **Streaming execution** -- replace materialized `Vec<Row>` with iterator-based execution
5. **Query timeout** -- add deadline-based cancellation to prevent mutex starvation
6. **HashMap option** -- consider `HashMap` for primary storage when ordering is not needed

## Next steps

- See measured numbers for each area: [Benchmarks](benchmarks.md)
- Storage layout the suggestions above affect: [Graph Engine](../architecture/graph-engine.md)
- Pipeline stages and where cost accumulates: [Data Flow](../architecture/data-flow.md)
- Need indexes, concurrency, and persistence today? [LoraDB managed platform](https://loradb.com)
