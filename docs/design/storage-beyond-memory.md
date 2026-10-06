# Storage beyond memory

Status: **research / proposal**, not scheduled. Written 2026-10-05 against
LoraDB v0.22.3 (`93dc8ff`). Nothing here is implemented.

**Question:** how could LoraDB store graphs larger than RAM while the hot
working set stays almost as fast as today's in-memory engine? The in-memory
default, the public API and every binding (including wasm, which has no
filesystem) must keep working throughout.

**Short answer.** Make the graph *smaller* before making it *pageable*, and
make it pageable along the seam LoraDB already has: the immutable,
`Arc`-shared 512-entry chunks that give it snapshot isolation today.

- **Stages 0 and 1 are in-memory only.** They should come first whether or
  not a disk tier ever ships. Fixing the O(N) staged write, the invisible
  property indexes and the per-record overheads would cut memory per element
  about 3× (247 → roughly 70–90 B), make writes independent of graph size,
  and push the 32 GB ceiling from ~100M to ~300M elements.
- **Stage 3 adds a disk tier.** Chunks become evictable and are checkpointed
  incrementally to a chunk file in shadow-paging style. The WAL stays
  logical. A full snapshot is replaced by "write the dirty chunks, then flip a
  root", and recovery becomes "open the root, replay the WAL tail".
- **Rejected:** a key-value backend (LiveGraph measured traversals 5.6–22×
  slower), `mmap` as the buffer manager, and a LeanStore/ARIES buffer pool.
  The pool is the fastest design in the literature, but it means rewriting
  LoraDB's transaction and recovery model.

The decisions this needs from you are collected in [§9](#9-decisions-and-open-questions).

## Implementation status

**Stage 0** shipped in v0.23.0. The distinct-count sketches, the streaming
id scans and the first slice of Stage 1 (interned names, typed adjacency)
are implemented on branch `storage/stage1`, not yet merged; see
[Stage 1, first slice](#stage-1-first-slice-interned-names-and-typed-adjacency).

Stage 0's before/after numbers come from three interleaved runs of
`storage_baseline lat 2000000` (v0.22.3 binary against the branch binary) on
the §2 machine and shape: 2M nodes / 8M relationships (10M elements), with a
RANGE index on `Person.id` and one UNIQUE constraint. Values are medians.

| Change | Before | After |
|---|---|---|
| Whole-graph clone: `ChunkedVec` becomes a two-level radix tree (leaves of 512, interior fan-out 128); index catalog, constraint catalog and property-index registry are `Arc` copy-on-write | 219–229 µs | **0.29 µs**, flat in N |
| Copy-on-write index containers: `CowMap` becomes a persistent hash trie (32-way, leaves ≤448), `CowOrdMap` three levels, `IdSet` and `CowIdMap` grouped chunks. A write copies a bounded path. | O(N/256) per indexed insert | O(log N) |
| Staged `SET` (any constraint exists) | 237–240 µs | **10.9–11.8 µs** |
| Staged `CREATE` on a constrained label | 244–245 µs | **15.2–15.5 µs** |
| `graph_create_node` of an indexed `:Person` (Rust API, staged) | 418–429 µs | **15.0–16.5 µs** (12.7–14.2 µs at 200k) |
| Staged relationship `CREATE` | 282–302 µs | **53–62 µs** |
| `id(n) = $x` (new `NodeByIdSeek` / `RelByIdSeek` operators) | 205–221 ms | **1.21–1.33 µs** |
| Snapshot load (10M el., uncompressed) | 17.4 s | **7.9 s** (no eager hash-indexing of every key) |
| Live heap after snapshot load / WAL replay vs the writing process (1M el., 1 RANGE + 1 UNIQUE) | +28% / +18% | **equal** |
| Live bytes per element, graph only | 247.2 | 247.2 |
| …with a hash index / a RANGE index | 315.1 / 342.2 | 305.8 / 333.2 |
| Cypher index seek, 1-hop, range seek; fast-path writes | — | unchanged (within run-to-run noise) |
| Cypher 2-hop | 16.3 µs | 16.1 µs (an earlier +6–10% reading was noise: 11 interleaved process pairs, ±10% per process; the extra tree level costs ~1–2 ns per lookup) |
| Raw `with_node`, find-by-property, 1-hop | — | unchanged |

`tests/write_scaling.rs` now checks that the clone, staged writes and
indexed creates are independent of graph size. Its 1M-node case is
`#[ignore]`d; run it with `cargo test --release -p lora-database --test
write_scaling -- --include-ignored`. The old code fails that test.

**Behaviour change (resolved).** Stage 0 left the planner with distinct-value
counts only for keys whose hash index was active, so a restarted process
planned some equalities on undeclared keys differently from the writer (e.g.
`MATCH (p:Person)-[:KNOWS]->(q:Person {name:$n})` became a label scan from
`p`), and a running process planned differently depending on which lookups
had run before. The Stage 4 statistics item is now done: distinct counts
come from a sketch kept for every `(label or type, property key)`
(`memory/distinct_stats.rs`), not from the hash indexes.

- **Sketch.** A counting multi-resolution bitmap: the value's hash picks a
  level (trailing zero bits) and one of 64 buckets; each bucket counts the
  entities hashed there, and the estimate linear-counts the unsaturated top
  levels. Unlike HyperLogLog it decrements, so after any mix of creates,
  updates, label changes and deletes the sketch equals one built from the
  surviving data. Writer, snapshot restart, WAL replay and a process with
  any lookup history therefore get identical `GraphStats` and plans
  (`tests/planner_stats.rs`, `tests/restart_index_state.rs`).
- **Accuracy.** Exact up to about 12 distinct values. Past that, 6–16% RMS
  error, worst seen about 36%, unbiased, from 100 to 1.5M values. Not
  clamped to the row count, because a clamp would move a unique key's
  estimate on every insert and defeat the stats cache. A unique key can
  therefore read above its row count.
- **Cost.** O(1) per property write: a hash and a counter. Sketches are
  `Arc` copy-on-write, so a graph clone shares them. The first write to a
  sketch after a clone copies it: two allocations, ≤3 KB `memcpy`, no
  refcount storm. `graph_stats` reuses its last map while the estimates
  don't change. Sketches are not persisted: snapshot load rebuilds them
  with a bulk builder, and WAL replay through the normal create path.
- **Memory.** 128 B per level up to the highest occupied one, plus ~80 B per
  sketch; counters widen to `u32` once a bucket passes 65 535 entities.
  `MemoryReport::distinct_stats_bytes` reports the total.
- **Plans.** With 600 `:Person`s, the pattern above starts with a seek on
  `q.name`, and `WHERE p.city = $a AND p.team = $b` seeks on `team`
  (150 values) rather than `city` (3), in either conjunct order. Plans don't
  depend on restart or prior lookups. EXPLAIN `estimated_rows` for an
  undeclared key is rows / distinct, no longer the label count.
- **Consequence.** A seek the planner now picks on an undeclared key builds
  that key's implicit hash index on first execution. That is the existing
  lazy behaviour, and it matches pre-0.23 restart planning. At 200k
  `:Person` with unique 40-byte `bio` strings, that first execution takes
  ~134 ms and retains ~56 MB, the same before and after this change
  (`storage_baseline lat 200000`, `cy_eq_unindexed_bio first_call`).
- **API.** `GraphStats::{node,relationship}_distinct_values` are now
  `Arc<DistinctValues>`, the same `BTreeMap<(String, String), usize>` behind
  an `Arc`.

Before/after numbers are in [Distinct-count sketches: measurements](#distinct-count-sketches-measurements).

**Festimap P2-5.** On v0.22.3, the native probe (`heap_probe --festimap`)
measures 336 B per element live with no indexes and 439 B with the schema,
and the process peaks at 102 MB RSS. P2-5 measured ~2.7 / 3.5 KB per element
on 0.15 through Node. A like-for-like Node RSS measurement is still to do.

### Distinct-count sketches: measurements

All numbers are medians of interleaved runs of the v0.23.0 binary and the
branch binary on the §2 machine: five runs each for `lat`/`restart`, two
for `mem`.

| Measure | Before | After |
|---|---|---|
| Retained heap, `mem relprops 2000000` (10M elements; 5 sketches: `Person.{id,name,score,bio}`, `KNOWS.w`) | 2,471,576,026 B | 2,471,595,882 B (**+19.9 KB total, ~4 KB per sketch**, +0.0008%) |
| `cy_create_node(fast path)`, 200k | 5.38 µs | 5.21 µs |
| `cy_set_prop(fast path)` | 4.58 µs | 4.46 µs |
| `cy_create_rel(staged)` | 40.8 µs | 40.8 µs |
| `cy_create_unique(staged)` | 14.75 µs | 14.83 µs |
| `cy_set_prop(staged: constraint)` | 9.50 µs | 9.50 µs |
| `db.graph_create_node(api)` | 12.58 µs | 12.79 µs (+1.7%) |
| Snapshot load (`restart 200000`, ~1M elements) | 0.790 s | 0.818 s (+3.5%) |
| WAL replay (`restart 200000`) | 1.124 s | 1.146 s (+2.0%) |
| Direct-API bulk build, 10M elements (no Cypher) | 4.96 s | 5.39 s (+8.7%) |
| Reads (seek, 1-/2-hop, range, scans) | — | unchanged (within noise) |

The stats cache matters. Without it, a Cypher write followed by a re-plan
paid ~450 ns to rebuild the distinct map (two `String`s per key). An
earlier version that clamped estimates to the row count missed the cache on
every insert of a unique key.

### Stage 1, first slice: interned names and typed adjacency

Three changes, all in memory, none visible in Cypher, the WAL or the
snapshot format.

- **Interned labels and relationship types.** `NodeRecord::labels` is a
  `Labels` and `RelationshipRecord::rel_type` a `Name` (`types/name.rs`):
  shared `Arc<str>`s from the process-wide intern table, with one label
  stored inline. A record no longer owns a `Vec<String>` and a `String`.
  This is a source change for Rust users of `lora-store`: both types
  compare with `&str` and `String`, deref to `str` / `[Name]` and
  serialize as before, but code that moved the `String`s out needs
  `to_string()` / `to_strings()`.
- **Typed adjacency** (`memory/adjacency.rs`). An adjacency entry is
  `(type number, neighbour, relationship id)`, so a hop filters by type and
  finds the far node from the list alone. Before, it dereferenced every
  incident `Arc<RelationshipRecord>` and compared type `String`s. Entries
  are variable-width (a header byte, then each field in the bytes it
  needs): 8 B while ids fit in three bytes, 10 B up to 4 billion, against 8
  B for the bare id before. Types are numbered by a per-graph dictionary
  that clones share until one adds a type. The numbers are not persisted.
- **Streaming id scans.** `GraphStorage::scan_node_ids` hands out node ids
  a page at a time from a cursor. The pull pipeline's all-node scan and
  single-label scan use it with pages of 1024 ids when the query is
  read-only. A scan that feeds a streaming write still copies its ids
  before the first row, because the write changes the list under it
  (`tests/streaming_scans.rs`). Multi-label scans and the materializing
  executor still build a `Vec`.

Medians of interleaved runs of the previous branch state against this one,
same machine and shape as above (2M nodes / 8M relationships): three
`hop` processes of three rounds each, three `scan`, three `lat`, one `mem`
per variant. Another build was running on the machine for part of this, so
the write rows are given as ranges.

| Measure | Before | After |
|---|---|---|
| Live bytes per element, whole graph (`mem relprops`) | 247.2 | **230.8** (−6.6%) |
| RSS per element, same run | 272.5 | 237.1 (−13%) |
| Node, one label, no properties (`mem bare`) | 166.4 B | **136.4 B** |
| Relationship, including both adjacency entries | ≈127 B | ≈114 B |
| Raw `expand_ids` 1-hop, typed | 813 ns | **325 ns** (2.5×) |
| Raw 2-hop, typed | 3.95 µs | **1.64 µs** (2.4×) |
| Cypher 1-hop from an index anchor | 5.67 µs | 4.58 µs (−19%) |
| Cypher 2-hop | 18.3 µs | 13.9 µs (−24%) |
| Cypher index seek | 1.88 µs | 1.62 µs |
| Staged relationship `CREATE` | 57–66 µs | 57–68 µs (+3% to +6% across three sets of runs) |
| `graph_create_node` / staged `SET` | 16.3 / 12.4 µs | 16.0 / 11.8 µs |
| `MATCH (p:Person) RETURN p.id LIMIT 10` (`scan` mode) | 1.29 ms | **2.4 µs** |
| `MATCH (p) RETURN p.id LIMIT 10` | 6.41 ms | **3.4 µs** |
| `MATCH (p:Person) WHERE p.score > 0.5 RETURN p.id LIMIT 10` | 1.29 ms | 3.3 µs |
| `MATCH (p:Person) RETURN p.id SKIP 5000 LIMIT 10` | 1.52 ms | 0.22 ms |
| Full label scan with a filter, 2M nodes | 220.9 ms | 220.8 ms |
| Fast-path `CREATE` / `SET` | 11.5–12.8 / 6.0–7.2 µs | 7.1–14.0 / 4.7–5.4 µs (run-to-run noise exceeds the difference) |
| Direct-API bulk build, 10M elements | 6.14–6.31 s | 6.15–6.20 s |
| Snapshot load, 10M elements | 11.2–11.7 s | 11.6–12.1 s (+3%) |
| Festimap shape, live bytes per element (`heap_probe --festimap`), plain / with schema | 336 / 439 (v0.22.3) | **316 / 384** |

What this slice does not do. The memory target of Stage 1 (T4, ≤ 100 B per
element) needs records and properties encoded into chunk arenas, read
through `NodeView` / `RelView`. A property entry is still 64 B and a record
still an `Arc` allocation. Of the 231 B per element here, about 113 B is
property entries, 91 B relationship records and their adjacency, and 27 B
node records.

Known costs:
- `degree()` walks the list (one header byte per entry) where it read a
  length. Only the `graph_degree` API calls it.
- Removing a relationship shifts the bytes after its entry, where the old
  list swapped the last id into the hole. Both first scan for the entry.
- Replacing `AdjList`'s derived `Clone` with one `memcpy`
  (`SmallVec::from_slice_copy`) made a staged relationship `CREATE` slower
  (79–83 µs), not faster. Not understood; reverted.
- Label and type names now live in the process-wide intern table, which
  never frees. D2's per-graph dictionary would bound that.

### Stage 1, second slice: read views

`GraphStorage::with_node` and `with_relationship` now pass a `NodeRef` /
`RelRef` (`types/view.rs`) instead of `&NodeRecord` / `&RelationshipRecord`.
A view exposes `id()`, `labels()`, `rel_type()`, endpoints and
`properties()`, and a property reads as a `ValueRef`: scalars by value,
strings by slice, every other kind through `ValueRef::Other`. The executor's
readers (about 70 closures), the trait's default methods and the label,
property and hydration helpers go through views. Storage is unchanged: a
view wraps the record it was made from, and `NodeRef::from(&record)` serves
backends that only have records.

This differs from the §6 sketch in one way: the views are concrete types,
not `NodeView` / `RelView` traits with associated types on `GraphStorage`.
One type keeps the trait object-safe where it was and needs no defaulted
associated type for backends that only implement `node()`. When the
in-memory store encodes records, the encoded form becomes a second
representation inside the same types.

No measurable cost: over three interleaved pairs of `hop` and `scan` runs
against the previous commit, reads moved between −5% and +3%.

Still on records, to port when the encoding lands: index maintenance and
constraint checks inside `lora-store`, the snapshot bridge, and the change
feed (`BorrowedGraphStorage`). The owned `node()` / `relationship()` and
their scan variants remain as the compatibility API.

**Not done yet from Stage 0.**
- Removing the lazy implicit indexes (D3 options b and c).

---

## Contents

1. [What exists today](#1-what-exists-today)
2. [Measurements](#2-measurements)
3. [What the numbers say](#3-what-the-numbers-say)
4. [Design space](#4-design-space)
5. [Recommendation](#5-recommendation)
6. [Trait changes](#6-trait-changes)
7. [Transactions, atomicity, WAL and snapshots in the new design](#7-transactions-atomicity-wal-and-snapshots-in-the-new-design)
8. [Benchmark plan](#8-benchmark-plan)
9. [Decisions and open questions](#9-decisions-and-open-questions)
10. [Sources](#10-sources)

Paths are relative to the repo root, and line numbers are at `93dc8ff`.
Claims about other systems are tagged **[V]** when checked against the paper,
documentation or source, **[S]** when taken from a secondary source or vendor
claim, and **[I]** when they are our own inference.

---

## 1. What exists today

### 1.1 Layout

`InMemoryGraph` (`crates/lora-store/src/memory/graph.rs:46-122`, `size_of` 856 B):

| Field | Type | Notes |
|---|---|---|
| `nodes` | `ChunkedVec<Option<Arc<NodeRecord>>>` | indexed by id; `None` is a tombstone and is never compacted (`graph.rs:51-66`) |
| `relationships` | `ChunkedVec<Option<Arc<RelationshipRecord>>>` | same (`graph.rs:67`) |
| `outgoing`, `incoming` | `ChunkedVec<SmallVec<[RelationshipId; 2]>>` | **relationship ids only**; 2 inline, more spill to the heap (`graph.rs:38-44, 74-80`) |
| `nodes_by_label`, `relationships_by_type` | `BTreeMap<String, ChunkedVec<u64>>` | label and type scans; removal is a linear `position` + `swap_remove` (`graph.rs:637-669`) |
| `indexes` | `IndexBundle` | every secondary index plus the catalog (`entity_index_store.rs:162-173`) |
| `constraint_catalog` | `RwLock<ConstraintCatalog>` | |

- **`ChunkedVec<T>`** is `Vec<Arc<Vec<T>>>` with 512 entries per chunk
  (`memory/chunked_vec.rs:17-24`). Cloning it bumps one refcount per chunk
  (`:35-43`). `get_mut` copies a shared chunk once (`:125-130`). Chunks
  are 4 KiB for slabs and label indexes and 12 KiB for adjacency.
- **Records** (`crates/lora-store/src/types/graph.rs:32-56`):
  - `NodeRecord { id, labels: Vec<String>, properties }` is 56 B, and each
    label is a separate heap `String`.
  - `RelationshipRecord { id, src, dst, rel_type: String, properties }` is
    72 B, with one heap `String` for the type **per relationship**.
  - `PropertyMap` is a key-sorted `Vec<(Arc<str>, PropertyValue)>`, 64 B
    per entry (`types/property_map.rs:33-39`).
  - `PropertyValue` is a 48 B owned enum (`types/property_value.rs:21-39`).
    No part of a value is shared, so copying a record deep-copies it.
- **Interning** (`crates/lora-store/src/intern.rs`):
  - One process-wide `RwLock<HashSet<Arc<str>>>` that is never freed
    (`:34-37`), with an unbounded per-thread `HashMap<String, Arc<str>>`
    cache in front (`:29-32`).
  - **Only property keys are interned.** Labels, relationship types, string
    values, map keys and index keys are all owned `String`s.
- **Executor values.** `LoraValue` is 48 B, and rows carry
  `Node(NodeId)` / `Relationship(RelationshipId)`, not records
  (`crates/lora-executor/src/value.rs:19-40`). `SlotValue` (`:363-444`) is
  `Inline(LoraValue) | Shared(Arc<LoraValue>)`; a value becomes `Shared`
  at ≥256 B, ≥8 entries, or when it is binary or a vector, so copying rows
  stays cheap. Property reads go through
  `with_node(id, |n| n.properties.get(key).map(LoraValue::from))`
  (`eval/expr.rs:1049-1087`): the borrow lasts only for the closure, and the
  value is copied out.
- **Adjacency stores no endpoints and no types.** `expand_ids`
  (`memory/impls.rs:492-536`) dereferences every incident
  `Arc<RelationshipRecord>` to read `dst` and compare `rel_type` as a
  `String`. That is a pointer chase per edge, and a large part of why a raw
  1-hop at 2M nodes costs 1.6 µs (§2.3). The rest of that cost is in the
  surrounding call path (§2.5).

### 1.2 The trait surface

`crates/lora-store/src/traits.rs`:

- **`GraphStorage`** (`:37-961`):
  - 11 required methods. All return **owned** values: `node -> Option<NodeRecord>`, `node_ids_by_label -> Vec<NodeId>`, `expand_ids -> Vec<(RelId, NodeId)>`, and so on.
  - Closure hooks for zero-copy reads: `with_node`, `with_relationship`, `try_for_each_expand_id`.
  - About 90 defaulted methods: scans, traversal, attributes, schema, property lookup, the index catalog, 17 constraint `check_*` methods, `graph_stats`, and index-candidate seeks.
- **`BorrowedGraphStorage`** (`:1006-1044`) returns real `&'a NodeRecord`. Its only non-test consumer is the change feed (`crates/lora-database/src/changes/build.rs`).
- **`GraphStorageMut`** (`:1055-1322`) has 13 required mutators. Its index and constraint DDL methods are defaulted.
- **`GraphCatalog`** (`:970-994`) is the analyzer's view.

The executor (generic over `S: GraphStorage[Mut]`), the compiler (which uses
`GraphStats` only) and the analyzer (which uses `GraphCatalog` only) never
name `InMemoryGraph` outside tests.

The coupling is concentrated in `lora-database`:

| Site | What it assumes |
|---|---|
| `database/execute.rs:220` | A `TypeId` check against `InMemoryGraph` selects the dual-path write dispatcher. Other backends get the pessimistic path. |
| `database/write_guard.rs:181-183, 201` | Downcast for managed snapshots and recorder install. |
| `database/replay.rs:28-63` | The recorder and deleted-sink are installed by downcast. Replay uses the inherent `replay_create_*`. |
| `transaction.rs` (23 refs) | `Arc<InMemoryGraph>` snapshot, `staged: Option<InMemoryGraph>` built by full clone (`:561`), a savepoint clone per mutating statement (`:640-647, 1202-1216`), and raw-pointer `&'static mut` (`:483-495, 1077-1099`). |
| `stream.rs:127-128` | `transmute` of `&InMemoryGraph` to `'static`. |
| `snapshot/*`, `changes/*`, `io.rs:190`, `wal/admin.rs`, `builder.rs:37-240` | Inherent `snapshot_payload` / `load_snapshot_payload`, change-feed post-images, and the durable constructors are all on `Database<InMemoryGraph>`. |
| Bindings and `lora-server` | All instantiate `Database<InMemoryGraph>`. This is a type-level dependency only. |

The shared data types (`IndexDefinition`, `GraphStats`, constraint types) live
in the private `memory` module and are re-exported (`traits.rs:12-16`,
`mutation.rs:22`). They would need to move.

**Hard parts for a paged backend:**

1. `with_node` / `with_relationship` hand out a materialised
   `&NodeRecord`. There are 39 + 21 executor call sites. They can be paged,
   because the borrow lasts only for the closure, but a compact on-disk
   encoding needs a *view* type rather than a struct reference.
2. Scans materialise the full id list (`node_ids_by_label -> Vec`,
   `pull/scan.rs:108`, `executor/helpers.rs:2201`).
3. Two `unsafe` lifetime extensions assume the snapshot is an
   `Arc<InMemoryGraph>` that never moves.
4. None of the read methods can return an I/O error.

### 1.3 Indexes, constraints, statistics

**Indexes** all live in `IndexBundle`. Most are copy-on-write containers that
are already chunked:

| Index | Structure | Notes |
|---|---|---|
| Hash property | `CowMap` with 256 `Arc` shards → `IdSet` posting lists (inline / up to 64 / 512-id chunks) | Every active key is indexed twice: unscoped and per label (`property_index.rs:23-25, 132-151`). |
| RANGE | Hash index plus a sorted `CowOrdMap` (partitions of ≤1024 keys) | `sorted_property_index.rs:39-52` |
| TEXT | Trigram `CowMap<[u8;3], CowIdMap>` | |
| FULLTEXT | Postings `CowOrdMap` plus an entity→terms map | |
| POINT | Uniform grid (cell 100) | |
| VECTOR | `BTreeMap` (flat) or hand-written HNSW (m=16) | Keeps a second copy of every vector (`vector_index.rs:278-281`). Not internally copy-on-write. |

- **Persistence:** only HNSW persists its data, as a JSON trailer in the
  snapshot. Every other index is rebuilt on load.
- **The implicit hash index.** The first equality lookup on *any* property
  key builds a hash index over every entity that has that key, and keeps it
  for good (`ensure_node_property_index`, `graph.rs:780-808`, called from
  `impls.rs:813, 848, 879, 917`). After a restart it is worse:
  - snapshot load (`rebuild_property_indexes`, `memory/snapshot.rs:111` →
    `graph.rs:1733-1771`) and WAL replay (`index_node_properties_eager`,
    `graph.rs:1787-1797`) **eagerly hash-index every indexable property of
    every entity, both unscoped and per label**;
  - so a restarted process uses more memory than the process that wrote the
    data. The measured cost is in §2.

**Constraints** (`constraint_catalog.rs`, `constraint_enforce.rs`):

- The kinds are Unique, Existence, NodeKey, RelationshipKey and
  PropertyType. Unique and the key kinds own a backing RANGE index.
- Every mutation walks every catalog entry (`constraint_enforce.rs:491-514`).
- A uniqueness check probes the label-scoped hash index on the *first*
  property and compares the rest of the tuple per candidate (`:659-714`).
- `@uniqueTogether` is not an engine constraint. It is a set of follow-up
  Cypher queries issued by lora-graphql (`packages/lora-graphql/src/execute/mutate.ts:1611-1673`).

**Statistics:**

- `GraphStats` (`stats.rs`, built by `InMemoryGraph::graph_stats`) holds exact
  label and type counts. Its distinct counts are estimates from the
  per-`(label or type, key)` sketches in `memory/distinct_stats.rs`, which
  exist for every key and depend only on the live data. (Until the Stage 4
  stats change they were **the bucket count of the scoped hash index**, so
  they existed only for keys whose hash index happened to be active; see
  the resolved behaviour change under Implementation status.)
- Stats are rebuilt on a plan-cache miss (`database/compile.rs:62-87`). The
  cache key includes the write epoch, so every write invalidates every
  cached plan.

### 1.4 Concurrency, atomicity and durability

**Readers and writers.**

- `LiveStore<S>` is `RwLock<Arc<S>>` plus an epoch (`live_store.rs:34-73`).
  Readers clone the `Arc` and run without locks on that version.
- **Writers are serialised by one mutex** (`database/mod.rs:109-112`). The
  `LockTable` exists but is unused (`mod.rs:113-118`).
- **There is no optimistic concurrency control today.** `occ.rs` is a
  dispatcher between two paths (`occ.rs:1-12, 37-76`). The names are a seam
  for a future write-set/CAS commit (`execute.rs:212-216`).

**The two write paths:**

- **Live fast path.** Edits the live graph in place via `Arc::make_mut`, and
  so clones the graph if a reader still pins the old `Arc`. It is used only
  when all of the following hold:
  - the plan is a single write clause that cannot fail midway;
  - there is no deadline;
  - `constraints_may_reject` is false (`occ.rs:84-99`). Since 0.22.3 that
    means *any* `SET` while *any* constraint exists takes the staged path
    (`109d0c5`).
- **Staged path.** `let staged: S = (*snapshot).clone();` at
  `write_guard.rs:115` (also `:144`, `:154`). The statement runs on the copy,
  which is published on success and dropped on failure. That is how
  statement atomicity is implemented. Explicit transactions clone once on
  their first write (`transaction.rs:561`) and take a savepoint clone before
  every later mutating statement.

**Durability:**

- **The WAL is logical.** It logs `MutationEvent`s (`lora-store/src/mutation.rs:34-104`). Each commit is one TxBegin + MutationBatch + TxCommit, written to the OS with no fsync. A `GroupSync` thread fsyncs every 50 ms (Rust) or every 1 s (Node and named databases) (`lora-wal/src/config.rs:9-20`, `wal/wal.rs:424-472`). Segments are 8 MiB.
- **Snapshots** use the `LORACOL1` columnar format with a BLAKE3 checksum, optional gzip and optional ChaCha20-Poly1305.
  - Saving deep-clones every record, then builds columns, then the bytes, then a compressed copy, all in memory (`memory/snapshot.rs:25-42`, `lora-snapshot/src/codec.rs:62-72`). Peak memory is several times the graph [I].
  - Loading reads the whole file and rebuilds every index.
  - A zero-copy `SnapshotView` exists but has no callers.
- **Checkpoints** (`snapshot/store.rs:105-133`) hold the writer mutex from encode through fsync and WAL truncation. With `checkpoint_every_commits` set they run *inside* a user's commit (`write_guard.rs:173-186`).
- **Recovery** loads the snapshot, then replays the WAL from the fence (`builder.rs:53-226`). Replay buffers every committed event in one `Vec` (`lora-wal/src/replay.rs:31-38`).

### 1.5 Bindings

| Binding | Constructors |
|---|---|
| Node | `createDatabase()` (in-memory), `createDatabase(name)` (named `.loradb`), `openWalDatabase({walDir, snapshotDir, …})` |
| Python, Ruby | `create` / `open_wal` |
| FFI / Go | `lora_db_new[_with_wal[_snapshots]|_named]` |
| wasm | `Database::in_memory()` plus snapshot to/from bytes only |

- No Cargo feature gates filesystem use. `lora-database` always depends on
  `lora-wal`, and its only feature is `parallel`. wasm compiles the WAL code
  but never calls it; fsync is a `#[cfg]` no-op on wasm32
  (`lora-wal/src/io.rs:13-31`).
- **Constraint for this design:** a disk tier has to sit behind a Cargo
  feature that is off for wasm, and behind *new, optional* open options
  everywhere else.

### 1.6 What already exists for measuring

- **Workflows.**
  - `benchmarks.yml`: release-tag `cargo bench`.
  - `perf-smoke.yml`: 19 benches at 1k nodes, 2× gate.
  - `memory-bench.yml`: `MemoryReport` estimates gated at 10% against `crates/lora-database/benches/memory_baseline.json`.
- **Probes.** `heap_probe` (a counting allocator, with `--festimap`), `keyset_probe`, `bulk_probe`.
- **Missing.** Nothing measures anything above ~1M elements, and there is no snapshot benchmark (`docs/TODO.md:38`).
- **Related TODO items.**
  - `docs/TODO.md` has no larger-than-memory item.
  - The related items are streaming bulk-scan iterators (`:36`) and concurrency phases 1–7 (`:44-50`). The phases include a "write attempt" object and moving checkpoints off the commit path.
  - `docs/architecture/graph-engine.md:264-267` says plainly that there is "no disk-backed buffer pool".
  - ADR 0003 calls snapshots "a whole-graph artifact, not an incremental storage engine".

**Festimap P2-5** (`festimap/docs/LORADB_REQUESTS.md:62-71`) is the only open
request from the main consumer:

> Measured (on 0.15.0): 20k nodes + 5k nodes + 100k relationships raised RSS
> by ~332 MB without indexes and ~435 MB with two uniqueness constraints, a
> full-text and a point index — ~2.7 KB / ~3.5 KB per element […] Expected: a
> documented per-element memory model, and ideally a lower footprint; index
> maintenance that doesn't dominate bulk loads.

The documented model is now in `docs/design/known-risks.md:95-113`, at about
320 B per element without indexes and about 430 B with them, measured as live
heap on 0.22. Nobody has re-measured RSS on 0.22 for the Festimap shape, so
P2-5 cannot yet be closed.

---

## 2. Measurements

All numbers are for v0.22.3 on an Apple M1 Max (10 cores, 32 GB, macOS 25.6,
rustc 1.95, `--release`, default features).

### 2.1 How to reproduce

The harness is `crates/lora-database/examples/storage_baseline.rs` (§2 numbers
were taken with it at `93dc8ff`, before it was checked in). To rerun:

```bash
cargo build --release -p lora-database --example storage_baseline
B=target/release/examples/storage_baseline
$B mem relprops 2000000          # one variant: bare|props|p_int|p_short|p_float|p_long|rels|relprops|range_idx|range_idx_name|hash_idx|unique
BENCH_TMP=/tmp/x $B lat 2000000  # latency (SAMPLES, WRITE_SAMPLES, SCAN_SAMPLES env)
BENCH_TMP=/tmp/x $B wal 2000000  # WAL ingest + replay, snapshot save/load
/usr/bin/time -l $B mem relprops 10000000   # 50M-element memory check
$B restart 200000                # memory + time of snapshot load / WAL replay vs the writer
$B idlat 2000000                 # id() lookups only
```

**Dataset.**
- N `:Person` nodes, each with four properties: `id` (Int), `name` (8-byte
  string), `score` (Float) and `bio` (a unique 40-byte string).
- Each node has 4 outgoing `:KNOWS` relationships to pseudo-random targets
  (splitmix64), and each relationship has one Int property `w`.
- So there are 5N elements, with an average total degree of 8.

| Scale | Nodes | Relationships | Elements |
|---|---|---|---|
| S | 20k | 80k | 100k |
| M | 200k | 800k | 1M |
| L | 2M | 8M | 10M |
| XL (memory only) | 10M | 40M | 50M |

**Method.**
- **Memory.** One process per variant. A counting `#[global_allocator]` (the
  `heap_probe` technique) gives live requested bytes, and the `ps` RSS delta
  is reported alongside. Each per-element cost is the difference between two
  variants that differ by one feature.
- **Latency.** End-to-end Cypher through `execute_rows_with_params` with a
  warm plan cache, plus direct `GraphStorage` calls. Operations under 1 µs
  are timed in batches of 64 because macOS timer resolution is about 42 ns.

### 2.2 Memory

Per-element costs below are live heap bytes at L. They stay within ±1 B
across S, M, L and XL.

| Component | Bytes |
|---|---|
| Node, 1 label, no properties | **166.4** |
| + Int or Float property | 64.0 |
| + 8-byte String property | 72.0 |
| + 40-byte String property | 107.0 |
| Relationship, no properties (record, type `String`, both adjacency pushes, type index) | **126.6** |
| + Int relationship property | 64.0 |
| **Whole graph** (the dataset above) | **247.2 per element**: 2.47 GB live / 2.72 GB RSS at 10M elements; 12.36 GB live at 50M |
| Implicit hash index entry (Int key) | 213–340, depending on where the table is in its capacity doubling |
| Declared RANGE index entry (hash part + ≈136 B sorted part) | 348–475 |
| UNIQUE constraint entry | same as RANGE |
| Implicit hash index on the unique 40-byte `bio` | 404 |

- **RSS overhead.** RSS is 10–15% above live heap for the core store, and
  15–25% above when indexes are built after loading. At XL, macOS memory
  compression distorts RSS, so only the live-heap number is meaningful
  there.
- **Snapshot size.** The uncompressed snapshot of the same graph is **71
  B/element**, 3.5× smaller than its RAM form. This roughly shows what a
  compact encoding of the same data takes.
- **Accuracy of `MemoryReport`.** It matches the allocator within 0.01% for
  the core store, but under-reports indexes by 4–11%.
- **The implicit index trap, measured.** At L, a single ad-hoc
  `MATCH (p:Person) WHERE p.bio = 'nope'` took **2.79 s** and kept
  **808 MB** (+33% of the whole graph) as an index nobody declared, which
  `SHOW INDEXES` does not show.

### 2.3 Latency

Values are median / p99. Cypher rows are end-to-end with a warm plan cache.

| Operation | S (100k el.) | M (1M) | L (10M) |
|---|---|---|---|
| Cypher index seek `MATCH (p:Person {id:$id}) RETURN p.name` | 1.00 / 1.21 µs | 1.50 / 1.96 µs | 1.67 / 2.62 µs |
| Cypher 1-hop from an index anchor (≈4 rows) | 2.04 / 2.50 µs | 3.79 / 4.46 µs | 4.88 / 8.96 µs |
| Cypher 2-hop (16 rows) | 5.33 / 6.54 µs | 11.2 / 13.7 µs | 15.4 / 20.4 µs |
| Cypher range seek (10 rows) | 4.42 / 5.04 µs | 5.04 / 5.83 µs | 5.79 / 8.92 µs |
| Cypher `MATCH (p) WHERE id(p) = $id` | **2.63 ms** | **15.7 ms** | **232 ms** (a full scan: there is no id-seek operator) |
| Cypher label scan + Float filter, per node | 96 ns | 171 ns | 194–275 ns (±30% between runs) |
| Raw `with_node` (borrow) | 3 ns | 11 ns | 21 ns |
| Raw `node(id)` (clones the record) | 169 ns | 451 ns | 730 ns |
| Raw `find_node_ids_by_property` | 132 ns | 181 ns | 335 ns |
| Raw `expand_ids` 1-hop | 158 ns | 424 ns | **1.60 µs** |
| Raw label scan reading one property, per node | 11.3 ns | 12.2 ns | 15.2 ns |
| `parse()` alone | 7.9 µs | 7.9 µs | 7.9 µs |
| Plan-cache miss, total | 14.8 µs | 15.6 µs | 21.0 µs |

| Write | Path | S | M | L |
|---|---|---|---|---|
| `CREATE (:Person {4 props})` | live fast path | 5.7 µs | 6.2 µs | 12.6 µs |
| `SET p.score = $v` (no constraints in the DB) | live fast path | 4.5 µs | 5.0 µs | 5.4 µs |
| The same `SET` once *any* constraint exists | staged | 17.8 µs | **37.2 µs** | **244 µs** |
| `CREATE` on a label with a UNIQUE constraint | staged | 28.5 µs | **48.0 µs** | **254 µs** |
| `MATCH (a),(b) CREATE (a)-[:KNOWS]->(b)` | staged | 45.2 µs | 67.1 µs | 295 µs |
| `db.graph_create_node` (Rust API) | staged | 23.2 µs | 53.8 µs | 422 µs |
| `InMemoryGraph::clone()` alone | — | 2.1 µs | 20.3 µs | **218 µs** |

The "~40 µs per constrained write at 200k nodes" figure is confirmed (37–48 µs
at M). It **grows linearly with graph size**, and extrapolates to about
1.2 ms at 50M elements. `docs/design/known-risks.md:93` and
`tests/write_scaling.rs` claim "tens of microseconds regardless of graph
size"; that is wrong past roughly 1M elements. The test only compares 2k
with 40k nodes.

**Why the clone grows linearly.** At L the clone touches the chunk tables of
six `ChunkedVec`s:

| Chunk table | Chunks |
|---|---|
| nodes | 3.9k |
| relationships | 15.6k |
| outgoing | 3.9k |
| incoming | 3.9k |
| label index | 3.9k |
| type index | 15.6k |
| **Total** | **≈47k** |

That is ≈47k refcount increments, roughly 380 KB of pointer tables to copy,
and the matching decrements when the old version drops. At ≈4–5 ns per
refcount bump this explains the 218 µs [I]. Adding or removing a hash index
barely changed the clone time (218 vs 220 µs), so the deep-cloned
property-index registry is *not* the dominant term at this scale. The clone
is 55–90% of each staged write.

### 2.4 Load and recovery (L, 10M elements)

| | Value |
|---|---|
| Snapshot size, uncompressed / gzip-1 | 713 MB (71 B/el) / 100 MB (synthetic data, so the ratio is optimistic) |
| Snapshot save | 2.9–3.6 s |
| Snapshot load | **17.3–18.1 s** (≈0.56M el/s); building the same graph directly takes 4.7 s |
| WAL size after Cypher ingest | 856 MB (86 B/el) |
| WAL replay, no snapshot | **17.7 s** (564k el/s) |
| Cypher ingest with WAL: nodes / relationships | 516k/s / 125k/s (relationship ingest falls from 442k/s at S) |
| `CREATE INDEX` backfill on `Person.id` | 2.5 s |

### 2.5 Paging spike

This is a throwaway standalone crate (~775 lines, smallvec + libc), not in
the repo. It answers three questions that only measurement can settle:

1. How much does a compact layout gain?
2. What does paging cost when every page is resident (hot)?
3. What does a miss cost (cold), and does an O(1) clone work?

**Shape.** 2M nodes with 8M relationships (and 200k / 800k), out-degree 4,
random targets, 4 relationship types.

**Timing.**
- `batch=16` times 16 random expands together and divides by 16. Their
  cache misses overlap, so this is closer to throughput than latency.
- `batch=1` is single-operation latency, rounded to the 42 ns timer step.
- Each benchmark ran twice. Medians repeated within 5–10%; p99 and p99.9
  varied up to 2–3×.

```bash
# crate lives in the research scratchpad (spike-paging/); to rebuild:
cargo run --release -- all    # see spike-results.md for the per-section flags
```

**Layouts compared.**

| Layout | What it is |
|---|---|
| A, current | Replica of today: `SmallVec` of rel ids → `Arc<RelationshipRecord>` with a `String` type and one property. |
| B, compact | Per-chunk CSR of inline `(type u32, neighbour, rel id)`, sorted by type. |
| C, paged | B's bytes in 4 KiB or 16 KiB pages of a vmcache-style pool: an anonymous `MAP_NORESERVE` region, a per-page `AtomicU64` state/version word, optimistic reads, and a node→page table. |
| D, paged cold | C with pages evicted and refilled by `pread` with `F_NOCACHE`, which bypasses the OS page cache. Every sample was confirmed to miss. |

**Bytes per relationship (adjacency only).**

| Layout | Bytes |
|---|---|
| A | 16 B of adjacency, plus 184 B of record that each expand also touches (≈200 B) |
| B | ≈21 B (25 B as actually allocated) |
| C | 20.1 B in pages (99.6% full), plus 8 B per node of page table |

**Hot expand at 2M nodes / 8M rels** (ns per operation; two runs shown where
they differed):

| Layout | 1-hop typed, batch=16, p50/p99 | 2-hop typed, batch=16 | 2-hop untyped, batch=16 | 1-hop, batch=1, p50/p99 |
|---|---|---|---|---|
| A current | 250–268 / ~300 | 612–628 | 1170 | 375–416 / 1380–1620 |
| B compact | **44 / 60** | **112–117** | 305–310 | 84–125 / ~300 |
| C pool, 4 KiB, all hot | **60–62 / 76–78** | **161–172** | 344–365 | 125 / 750–833 |
| C pool, 16 KiB, all hot | 62 / 76–78 | 167–177 | 362–404 | 125 / 1120–1250 |

At 200k nodes, where most of the graph fits in cache: A is 164–193 ns,
B ~30 ns, C ~40 ns.

**Cold expand at 2M nodes** (every sample misses one page; µs):

| Page size | QD1, p50 / p99 / p99.9 | 8 threads, p50 / p99, throughput |
|---|---|---|
| 4 KiB | **85 / 113–120 / 356–807** | 88–89 / 167–181, ≈85k/s |
| 16 KiB | 99–100 / 125–129 / 259–426 | 103–105 / 201–204, ≈72k/s |

Raw random `pread` with `F_NOCACHE` on this SSD:

| Read size | QD1 | 8 threads | 32 threads |
|---|---|---|---|
| 4 KiB | 77–84 µs (12–13k IOPS) | 79–85 µs (90–97k IOPS) | 205 µs (148k IOPS) |
| 16 KiB | 90–100 µs | — | — |

A re-read of the same block takes 18.6 µs (the SSD's own cache) against
0.46 µs through the OS cache, which confirms that `F_NOCACHE` bypasses the
OS cache.

**Eviction on macOS** (512 MiB anonymous region, same result in both runs):

| Call | RSS | Footprint | Contents |
|---|---|---|---|
| `MADV_DONTNEED` | unchanged | unchanged | kept |
| `MADV_FREE` | unchanged | unchanged | kept |
| `MADV_FREE_REUSABLE` | unchanged | 513 → 1 MB | kept (reclaimed lazily) |
| `mmap(MAP_FIXED)` over the range | 514 → 2 MB | 513 → 1 MB | zeroed |

**Clone: flat chunk table versus a 2-level radix tree** (fan-out 512, u64
leaves, p50):

| | 2M entries | 20M entries |
|---|---|---|
| Flat `Vec<Arc<chunk>>` clone | 9.3–9.7 µs | 97 µs |
| Dropping that version later | 8.6–8.8 µs | 85–86 µs |
| 2-level tree clone (+drop) | **9–10 ns** | **9–10 ns** |
| Tree: first write per new path | 1.04–1.08 µs | 1.42–1.46 µs |
| Tree: further writes to an already-copied leaf | 7–8 ns | 7–8 ns |
| Random read, flat / tree | 1 / 2 ns | 1 / 2 ns |

**What the spike settles.**

1. **The compact layout is the big hot-path win.**
   - Inline `(type, neighbour, rel id)` is **4–6× faster on 1-hop and ~5× on
     2-hop** than a faithful replica of today's layout, and touches 8–9×
     fewer bytes per expand.
   - The replica runs at 0.25–0.4 µs, while LoraDB's raw `expand_ids`
     measured 1.6 µs at the same scale (§2.3). So a large part of today's
     1.6 µs is *above* the layout: the per-call `Vec`, the `node_at` check,
     and `&[String]` type matching. **[I] uncertain:** that path was not
     profiled. Stage 1 should profile `expand_ids` before claiming a 4–6×
     end-to-end gain.
2. **Hot paging costs about +18 ns per 1-hop and +35–90 ns per 2-hop.** That
   is one extra cache miss through the node→page table and the page state
   word. Paged-and-hot (C) is still **~4× faster than today's in-RAM
   layout** (A).
   - Against §2.3's Cypher 1-hop of 4.9 µs, +18 ns is 0.4%, well inside the
     10–20% budget (T9).
   - This backs the claim that residency checks are affordable. A
     `ChunkCell` check should cost the same as or less than C, since it
     needs no optimistic version recheck for immutable chunks [I].
3. **A cold miss costs one SSD read plus about 1 µs.**
   - That is ≈85 µs for 4 KiB and ≈100 µs for 16 KiB on this Apple SSD,
     about 1,400× a hot expand.
   - Eight concurrent misses see the same per-read latency, so a cold
     traversal must issue each hop's page reads **in parallel**. Hence
     `prefetch_nodes` and batched faults in Stage 3. Done serially, a cold
     2-hop over d neighbours costs (1+d) × 85 µs.
   - Linux server NVMe with `O_DIRECT` is typically 10–20 µs per read; that
     was not tested here.
   - **4 KiB is the better I/O unit for cold point access** (see Q2).
4. **macOS does not free anonymous pages on `madvise`.** `MADV_DONTNEED` and
   `MADV_FREE` have no visible effect, and `MADV_FREE_REUSABLE` only changes
   the footprint figure. A vmcache-style pool would need per-OS eviction
   (`MAP_FIXED` remap on macOS).
   - The recommended design avoids the issue: it evicts by dropping an
     `Arc<ChunkData>` (`free`). This is also why vmcache stays a Stage 5
     Linux-only option.
5. **An O(1) version clone is cheap.**
   - A flat chunk table costs ~2.4 ns per chunk to clone, plus about the
     same again to drop. Across LoraDB's six chunk tables that accounts for
     the 218 µs.
   - A 2-level radix tree clones in ~10 ns. A write then pays ~1–1.5 µs per
     distinct path copied, and ~7 ns per further write to an already-copied
     leaf.
   - A typical single-row write touches about 4–6 paths (node slab,
     adjacency out and in, relationship slab, label and type index), for an
     estimated **~6–10 µs** of path copies, independent of N. A real leaf of
     `Option<Arc<…>>` adds ~512 refcount bumps per leaf copy, the same as
     today.
   - This supports T1 (≤ 25 µs, flat in N). Narrower interior nodes (fan-out
     64) would make each path copy cheaper and are worth trying.

**Not covered by the spike:**
- concurrent writers (the optimistic-read retry path never ran under
  contention);
- hub nodes spanning pages;
- properties and in-adjacency in layouts B–D;
- Linux NVMe;
- `MADV_FREE_REUSABLE` under real memory pressure.

---

## 3. What the numbers say

1. **Memory is mostly fixed overhead per record, not data.**
   - An empty labelled node costs 166 B, an empty relationship 127 B, and
     each property entry 64 B before its value.
   - The same graph serialises to 71 B per element. A compact in-memory
     encoding could plausibly hold 3× more data in the same RAM with no disk
     at all (§4.7).
2. **At 32 GB, today's ceiling is about 100M elements without indexes**
   (≈247 B/el live, plus 10–15% RSS slack), or about 70M with one index.
   - Snapshot save and load briefly need a multiple of that (deep-cloned
     payload, then encoded and compressed buffers).
   - The implicit hash indexes added on restart can take another large
     fraction.
3. **The Cypher layer dominates hot point reads, not storage.**
   - An index seek is 1.0–1.7 µs end to end, of which 0.1–0.3 µs is the
     store. Storage can be about 1 µs slower on a hit before users see more
     than a 2× change, and about 0.1–0.2 µs slower and stay inside the 10–20%
     budget.
   - A cold SSD read (tens to ~100 µs) costs 30–60× a whole query, so cold
     access must be rare and batched.
4. **Traversal is the weakest hot path.**
   - Raw 1-hop costs 1.6 µs at L because adjacency stores ids only and every
     edge dereferences a separately allocated record.
   - A layout that stores `(type, neighbour, rel id)` inline would make
     traversal *faster than today* while also shrinking it.
5. **The staged write is O(N), and any disk design inherits this unless it
   is fixed first.** Every SET in a database with any constraint, every
   relationship CREATE, and every `graph_*` API write pays the whole-graph
   clone.
6. **Restart is O(data), in two places.**
   - Snapshot load (0.56M el/s) is 3.8× slower than a direct build, so it
     has headroom on its own.
   - A store that is mostly on disk must not have to load everything at
     start.
7. **Two features are missing that a disk tier would make painful:**
   - there is no id-seek operator (`id(n) = $x` is a full scan);
   - every scan materialises the whole id list.

---

## 4. Design space

Each option below is described for LoraDB specifically: what changes, what it
costs, and how it behaves for hot and for cold data. The latency figures are
estimates unless they cite a measurement.

### 4.1 Buffer-managed pages with pointer swizzling (LeanStore, Umbra)

**How it works.**
- **LeanStore** (ICDE 2018) **[V]**:
  - A page reference is a "swip": either a raw pointer (hot) or a page id
    (cold). A hot access costs one branch.
  - Eviction moves random pages into a FIFO cooling stage of about 10% of the
    pool. Touching a cooling page brings it back without I/O.
  - Readers use optimistic version latches. Freed pages are reclaimed by epoch.
- **Umbra** (CIDR 2020) **[V]**:
  - Adds variable-size pages in size classes starting at 64 KiB. Each class
    reserves its own virtual address range inside one shared physical budget,
    using `MADV_DONTNEED` for eviction.
  - Uses a versioned latch with a shared mode.

**Measured in the papers.**
- LeanStore ran single-threaded TPC-C at **67K vs 69K txn/s** for an
  in-memory B-tree, against 16K for WiredTiger and 10K for BerkeleyDB. It
  reached 845K txn/s on 60 threads **[V]**.
- Umbra stayed within 6% of a version with no buffer manager on JOB and
  TPC-H, which are OLAP workloads **[V]**.

**Why it fits a graph badly.**
- Swizzling needs every page to have **exactly one owning reference**, so
  stored structures must be trees **[V]**. Adjacency is the opposite: many
  parents and cycles.
- LeanStore works around this by keeping everything in B-trees keyed by
  logical id. A hop then becomes a B-tree descent, not a pointer.
- The vmcache paper names graph data as the case swizzling cannot handle
  **[V]**.

**What it would cost LoraDB.**
- A new storage engine:
  - B-trees for records, adjacency and indexes;
  - a page latch protocol;
  - a buffer pool with eviction;
  - physical or physiological logging. The current logical WAL cannot redo
    onto fuzzy, partially flushed pages.
  - checkpointing in the style of LeanStore's 2020 follow-up
    (Haubenschild et al., SIGMOD 2020) **[V]**.
- `InMemoryGraph`'s copy-on-write snapshot model does not carry over, so
  isolation would need MVCC.

**Assessment.**
- **Latency:** hot ≈ in-memory B-tree, but slower than today's direct slab
  index for point access. Cold is one or more page reads.
- **Write amplification:** a full page per dirty record at checkpoint.
- **Recovery:** ARIES-like.
- **Complexity:** very high.
- **Risk:** high. In effect it is a second database.
- **Verdict:** the gold standard for B-tree OLTP, but the wrong shape for a
  graph and the wrong cost for LoraDB.

### 4.2 Virtual-memory-assisted buffer management (vmcache, exmap)

**How it works.** vmcache (Leis et al., SIGMOD 2023) **[V]**:
- The whole database is reserved as one anonymous `MAP_NORESERVE` virtual
  range, and a page id *is* an offset into it. The MMU does the translation,
  so no swizzling is needed and pages can have any number of parents.
- A miss is `pread` into place. Eviction is `pwrite` if dirty, then
  `madvise(MADV_DONTNEED)`. The DBMS, not the OS, decides what stays
  resident.
- Each page has a 64-bit state word (lock bits + version) for optimistic reads.

**Measured in the paper** **[V]**:
- Hot reads cost **under 8% more than raw memory**.
- An uncontended exclusive lock and unlock takes 238 ns.
- Overhead is about 16 B of RAM per 4 KiB on disk.
- Out of memory, Linux page-table manipulation becomes the bottleneck. The
  **exmap** kernel module fixes that, and adds about 60% throughput on random
  lookups.
- The reference buffer manager is about 800 lines.
- Kùzu's last buffer manager was "inspired by vmcache" **[V]**: anonymous
  mmap, `MADV_DONTNEED`, optimistic reads, second-chance eviction.

**Risks.**
- exmap is an out-of-tree Linux module, which is unusable in containers and
  managed hosts **[I]**.
- The setup assumes `vm.overcommit_memory=1`.
- **macOS `MADV_DONTNEED` semantics differ from Linux:** the page may not be
  zeroed or freed right away. See §2.5.
- In Rust, optimistic reads of memory a writer may change concurrently have
  to go through atomics or volatile copies to avoid UB **[I]**.
- It is a page-level design, so it needs the same logging answer as §4.1,
  unless pages are never updated in place (see §5).

**Verdict:** the best *page* mechanism for graph data. LoraDB would only need
the translation trick if pages were the unit of sharing; §5 uses chunk ids
instead, which gives the same multi-parent property in safe Rust.

### 4.3 File-backed `mmap` (LMDB-style or custom)

**How it works.** Map the database file and let the kernel page it.

**Problems** (Crotty, Leis and Pavlo, CIDR 2022) **[V]**:
- **Transactional safety.** The OS may write dirty pages back before commit,
  and `mlock` does not stop it. The workarounds are OS copy-on-write
  (MongoDB MMAPv1), user-space copy-on-write (SQLite, MonetDB, RavenDB), or
  shadow paging with a single writer (LMDB).
- **I/O stalls.** A page fault blocks the thread, with no asynchronous
  alternative.
- **Error handling.** I/O errors arrive as `SIGBUS` at any memory access, and
  checksums would have to be verified on every access.
- **Performance.** Page-table contention, the single `kswapd` eviction
  thread, and TLB shootdowns. Their fio comparison held for 27 s, then
  throughput **dropped to about zero for about 5 s** and settled at roughly
  half. Sequential scans across 10 SSDs ran **about 20× slower**. Overall
  "2–20× worse".

**Counterpoints.**
- LMDB works well with a **read-only** map, copy-on-write B+trees, a single
  writer and no recovery step, all of which LoraDB already has in spirit.
  But it is kernel-bound once the data exceeds memory, and writers cannot
  scale **[V]**.
- LiveGraph used mmap and listed replacing it as future work **[V]**.

**What it would cost LoraDB.**
- Cheap to prototype: map an immutable columnar file (for example the
  existing `SnapshotView`) and read cold chunks through it.
- An in-RAM delta overlay would handle writes.

**Verdict.** Acceptable only for **immutable, read-only** cold segments, and
even then the stall and `SIGBUS` behaviour is uncontrolled. Not recommended
as the primary mechanism. It stays a possible later optimisation for a
read-mostly archive tier.

### 4.4 Embedded key-value backends

| | redb 4.3 | LMDB (heed 0.22) | fjall 3.1 | RocksDB (rust-rocksdb 0.25) | sled |
|---|---|---|---|---|---|
| Structure | copy-on-write B+tree | copy-on-write B+tree, mmap | LSM, pure Rust | LSM, C++ | Bw-tree-ish |
| Writers | 1 | 1 | 1 or optimistic | many | many |
| Readers | MVCC, never blocked | lock-free; long readers grow the file | snapshot | snapshot | — |
| Durability | 1 fsync + checksums, or 2-phase commit | fsync at commit | **OS buffers by default** | WAL | — |
| Recovery | full repair walks every tree after an unclean shutdown without quick-repair | none needed | LSM replay | WAL replay | — |
| wasm | none (pluggable `StorageBackend` trait; one fork claims wasm32 **[S]**) | will not build (heed#162) | no | no | no |
| Status (2026-10) | stable format | stable; OpenLDAP licence | 3.0 released 2026-01 | mature | beta, format unstable, last release 2024 → **not viable** |

All facts in this table are **[V]** against the crates' docs and repos unless
tagged otherwise.

**How graph databases on KV stores lay out adjacency.**
- JanusGraph stores one row per vertex holding its edges, sorted by label and
  sort key, and stores every edge twice **[V]**.
- Dgraph keys `<predicate, uid>` → a posting list compressed in blocks of 256
  **[S]**.
- SurrealDB writes 4 keys per `RELATE`, embedding the far endpoint in the key,
  so a hop is a prefix scan **[V, source]**.
- Memgraph's on-disk mode (RocksDB) is labelled **experimental**. It offers
  only snapshot isolation and no replication, a "single transaction must fit
  into the memory", and it publishes no comparison with its in-memory mode
  **[V]**.

**The key data point.** LiveGraph (VLDB 2020) **[V]** measured in-memory
adjacency scans **22× slower on RocksDB** and **5.6× slower on LMDB** than a
sequential per-vertex edge log, with 11× and 7× more cache misses. An LSM has
to probe SST files on every seek because only the source half of an edge key
is known.

**What it would cost LoraDB.**
- A `KvGraph: GraphStorage` with a key schema like
  `n/<id>`, `a/<src>/<dir>/<type>/<dst>/<rel>`, `p/<id>/<key>`, `i/<index>/<value>/<id>`.
- Every read would decode bytes.
- The copy-on-write snapshot model would be replaced by the KV store's own
  MVCC. That gives a single writer for redb and LMDB, which is the same as
  today.
- The logical WAL would be redundant with the KV store's own log.

**Assessment.**
- **Latency:** hot reads go through B-tree or LSM lookups, a large
  regression from 21 ns borrows and slab indexing. Cold reads are reasonable.
- **Write amplification:** about 33 on RocksDB with leveled compaction
  **[V, tuning guide]**; copy-on-write path copies for redb and LMDB.
- **Complexity:** moderate to build, but LoraDB would lose control over
  layout and residency.

**Verdict.**
- Rejected as the primary store: it gives up the hot path, which is what
  LoraDB is for.
- Acceptable later as an *archive* backend for chunks (§5 stores opaque
  chunk blobs, and a KV store could hold them). That is a deployment
  decision, not an architecture.

### 4.5 Graph-native on-disk layouts

**Neo4j record store** **[V, 4.4 source]**.
- Fixed-size records: node **15 B**, relationship **34 B**, property **41 B**
  (four 8 B blocks), relationship group **25 B**.
- Because records are fixed-size, the id *is* the file offset.
- Each relationship sits in two doubly-linked chains, one per endpoint.
  Nodes with ≥50 relationships get per-type groups **[S]**.
- Linked chains are fine while cached but poor on disk, because every hop is
  a random record.
- **Block format** (5.14, GA in 5.16, Enterprise default since 5.22) **[V]**
  co-locates a node's properties and relationships in 128 B blocks. Neo4j
  claims it is ~40% faster with the graph fully in memory and ~70% faster
  when a third fits (a vendor figure; the internals are not public)
  **[S]**.
- **Lesson:** inline what a hop needs, and co-locate a node's edges. That is
  exactly what LoraDB's adjacency does not do today.

**Kùzu** (CIDR 2023; final source tree) **[V]**.
- Columnar node properties. Edges are stored twice, in **forward and backward
  CSR**, with edge properties in parallel columns.
- Node groups of 2^17 rows, packed CSR at density 0.8.
- Compression: bitpacking, constant, ALP and dictionary.
- A buffer manager based on vmcache with GClock / second-chance eviction.
- Factorised execution turns random property lookups into sequential scans.
- On LDBC SF100 multi-hop queries it beat DuckDB and Umbra by large factors.
  The paper publishes no in-memory vs on-disk comparison.
- The repo was archived on 2025-10-10. Reports that Apple acquired Kùzu cite
  EU filings, but there was no official announcement **[S]**. A community
  fork continues.
- **Lesson:** CSR plus columnar storage plus a vmcache-style pool gives
  on-disk graph traversal that stays fast. Updates to packed CSR are its
  weak point.

**Memgraph** **[V, docs]**.
- In-memory transactional mode costs about **204 B per vertex and 154 B per
  edge**, including a 56 B MVCC delta. That is the closest yardstick to
  LoraDB's 166 / 127 B.
- `ON_DISK_TRANSACTIONAL` stores everything in RocksDB, with skip-list caches
  of the transaction's working set. It is experimental, gives snapshot
  isolation only, does not support replication, and has no published
  numbers.

**Others** (detail in the research notes):
- **LiveGraph** **[V]**: a per-vertex append-only edge log with version
  timestamps gives sequential adjacency scans even under MVCC.
- **Aspen** (PLDI 2019) **[V]**: purely functional compressed trees with
  structural sharing. This is LoraDB's model taken further.
  - On the Twitter graph (2.4B edges): 73.5 GB uncompressed, 15.6 GB with
    chunked edges, **9.42 GB (≈3.9 B/edge)** with delta-encoded chunks.
    Across graphs that is 4.7–11.3× smaller, BFS gets 2.5–2.8× faster, and
    snapshots are O(1).
  - **PaC-trees** cut a further 1.3–2.6×.
- **Sortledton and Teseo** **[V]**: vectors for small neighbour lists, blocked
  structures for hubs.
- **TuGraph** **[S]**: packs each vertex's edges into page-sized LMDB values.
- **FalkorDB** **[V]**: one sparse matrix per type, all in RAM.
- **GraphChi and X-Stream:** whole-graph analytics only, not OLTP.

### 4.6 Hybrid tiering: hot in RAM, cold paged out

**Prior art.**
- **Anti-caching** (H-Store, VLDB 2013) **[V]**:
  - Cold tuples are evicted in blocks. Indexes stay in RAM.
  - A transaction that touches evicted data runs a pre-pass, aborts,
    fetches the data, and restarts.
  - Recovery is snapshot + command log, with delta snapshots of the immutable
    block table. That is **essentially LoraDB's model**.
  - At 8× the size of RAM it ran 8–17× faster than MySQL.
  - Weakness for a graph: "all indexes stay in RAM", and in a graph the
    adjacency *is* the index.
- **Siberia** (Hekaton, ICDE 2013) **[V]**:
  - Moves single records to a cold store, with Bloom and range filters and an
    update memo.
  - Tracking LRU inline cost **+25% per access and +16 B per record**, which
    the authors rejected.
  - Instead it logs a 10% sample of accesses and classifies them offline with
    exponential smoothing. The sample costs 2.5% in accuracy, and 1B accesses
    are classified in under a second.
  - Throughput loss was 7–14% at 5–10% cold access.
- **LeanStore cooling**: random selection plus a FIFO second chance. Hit rates
  are within 1% of LRU and 2Q with no bookkeeping on the hit path **[V]**.

**Design choices for LoraDB.**

- **Granularity.**
  - Per record (Siberia) has too much metadata per item and loses locality.
  - Per page (LeanStore, vmcache) matches the I/O unit.
  - **Per chunk** (512 entities) is LoraDB's existing copy-on-write unit,
    which makes it the natural fit [I].
- **What stays resident.** In order of eviction priority, last to go first:
  1. the chunk tables, catalogs and stats (always resident);
  2. the label index and adjacency (topology);
  3. index interiors;
  4. properties;
  5. large values.

  Topology is about 30–40% of the compact footprint (§4.7), so a budget of
  roughly half the compact size keeps every traversal in RAM and pages only
  properties [I].
- **Interaction with copy-on-write.** A chunk that versions share is
  *immutable*. Evicting it cannot conflict with a writer, because the writer
  makes a new chunk, so no latches are needed for eviction [I]. Only *clean*
  chunks (ones already in a checkpoint) can be evicted. Dirty chunks stay
  pinned until the next incremental checkpoint, which bounds dirty memory by
  checkpoint frequency.
- **Interaction with the staged clone.** It must stop being O(N) first. Once
  the chunk table is a radix tree, a clone is O(1) (one root `Arc`) and a
  write path-copies O(log₅₁₂ N) nodes (§5).
- **Interaction with the WAL.** Residency changes are not logical changes and
  are never logged. Checkpoints write chunks from a *published, immutable
  version*, so the on-disk image is always transaction-consistent at a known
  LSN. The logical WAL can replay on top of it unchanged. This is the problem
  that page-level designs (§4.1, §4.2) cannot get around without physical
  logging.

### 4.7 Cheaper wins: how far do they stretch RAM?

Estimates for the benchmark shape. Today's cost is measured; the compact
column is **[I]**, with the snapshot's 71 B/element as a sanity bound.

| Item | Today (B) | Compact (B) | How |
|---|---|---|---|
| Node record + label | 166 | ~16–24 | Slot holds an offset into a chunk-local byte arena. Labels become an interned `u16`/`u32` set (one id for the common single-label case). No `Arc` and no `String` per record. |
| 4 node properties (int, 8 B str, float, 40 B str) | 307 | ~80 | Key id (`u16`/`u32`) + tag byte + value. Ints and floats are 8 B (or varint). Strings are a length plus bytes inline; small strings have no separate heap allocation. |
| Relationship record | 127 | ~20 | `(src, dst, type_id)` in a chunked column (8+8+4, or 5+5+2 with 40-bit ids). No `Arc`, no `String`. |
| Adjacency per relationship (both directions) | 16 + record chase | ~24–32 | `(type_id, neighbour, rel_id)` inline in a per-chunk CSR, sorted by type. Delta + varint on neighbour ids halves it again (Ligra+ reports 49–56% of the original size and about 8% *faster* **[V]**). |
| Relationship int property | 64 | ~11 | As for nodes. |
| **Per element, whole graph** | **247** | **~70–90** | That is **2.7–3.5×**, consistent with the 71 B/el snapshot. |
| Implicit hash index | 213–404 per entry | 0 unless declared | Stop eagerly indexing on restart; make implicit indexes bounded and visible, or drop them. |
| Declared RANGE index | 348–475 per entry | ~40–60 | Sorted `(key bytes, id)` leaf pages with no parallel hash index. Uniqueness is checked against the sorted index. |
| Large values (>256 B text, vectors) | inline plus a second copy in the vector index | 8 B handle | Out-of-line blob store (TOAST-like, Postgres at ~2 kB **[V]**). Vectors are stored once and shared with the index. |

**Capacity at 32 GB** (≈28 GB usable):

| Stage | Without indexes | With one RANGE index per node |
|---|---|---|
| Today | ≈100M elements | ≈70M |
| After compaction | ≈300–400M | ≈250M |

Compaction also improves locality: 1-hop stops chasing a pointer per edge
(§2.5).

**Costs.**
- `with_node` can no longer hand out `&NodeRecord`; it must hand out a view
  (§6).
- An update rewrites the record's bytes within a copy-on-write chunk, instead
  of mutating a struct.

**Aspen's lesson.** Compressed, chunked, functionally shared adjacency is
both smaller *and* faster, and keeps O(1) snapshots.

---

## 5. Recommendation

### 5.1 The architecture: a versioned chunk store

Keep LoraDB's model, which is immutable `Arc`-shared chunks under a single
writer with copy-on-write versions published atomically, and make three
changes to the chunk:

1. **Compact.** A chunk holds a byte-encoded column or arena for 512 entities,
   not 512 `Arc<Record>` pointers.
2. **Tree-indexed.** Chunk tables become a persistent radix tree (fan-out
   512), so a version clone is one `Arc` bump and a write path-copies two or
   three interior nodes.
3. **Resident or not.** Each chunk is a `ChunkCell` that is either resident
   (`Arc<ChunkData>`) or on disk (`ChunkAddr`). Because a chunk is immutable
   once published, eviction never conflicts with a writer, and faulting it
   back in is a once-only fill. No page latches are needed.

```
  Version (Arc)                                 chunk file (append/shadow)
  ├─ nodes:      RadixTree ─► [ChunkCell]  ── resident Arc<NodeChunk>  │ addr ──► [...]
  ├─ props:      RadixTree ─► [ChunkCell]  ── resident Arc<PropChunk>  │ addr ──► [...]
  ├─ out / in:   RadixTree ─► [ChunkCell]  ── resident Arc<AdjChunk>   │ addr ──► [...]
  ├─ rels:       RadixTree ─► [ChunkCell]  ...
  ├─ label/type index, secondary indexes: same cells (CowOrdMap partitions, IdSet chunks)
  └─ catalogs, stats, dictionaries (always resident)
```

**Data layout.**
- **Topology and properties live in separate chunks.** Properties can then be
  cold while traversal stays hot.
- **Adjacency is a CSR block per 512-node chunk.** Entries are
  `(type_id, neighbour, rel_id)`, sorted by type, with optional delta + varint
  compression. A "big neighbour list" spill handles hub nodes, as Sortledton
  does.

**Residency.**
- **Hot-path cost.** A chunk access pays one extra pointer hop and one
  branch: the cell's state.
- **Eviction.** Random or second-chance (CLOCK). A reference bit is set by a
  sampled store, Siberia style, so the hit path does no shared-cache-line
  write on most accesses.
- **Order.** Large values first, then properties, then index leaves, then
  topology. Catalogs, dictionaries, radix interiors and stats never leave.

**Checkpoints.**
- **Unit.** A checkpoint takes a published version and writes every
  dirty chunk reachable from it to the chunk file, then the radix interiors,
  then a root record carrying the LSN. A `fsync` and an atomic root-pointer
  flip follow (shadow paging, as in LMDB and redb, at chunk granularity).
- **Write cost.** Only what changed is written. Write amplification is one
  chunk per dirty chunk, plus a few interiors.
- **Space reclamation.** Old chunk locations are freed once no checkpoint
  root and no pinned version references them (epoch-based free list).
- **Recovery.** Open the newest valid root, which makes the store usable at
  once with everything cold. Then replay the logical WAL tail from the root's
  LSN, exactly as replay works on top of a snapshot today.

**The in-memory engine is the same code with a backend that never evicts.**
Its `ChunkStore` has no disk address, so wasm, `createDatabase()` and every
existing test run as before. The residency branch disappears through
monomorphisation (`S: ChunkBackend`) or stays as a predictable branch.

**Why this design, and not the others:**

| Requirement | Versioned chunk store | Buffer pool (4.1/4.2) | KV (4.4) | mmap (4.3) |
|---|---|---|---|---|
| Hot traversal ≈ today | yes, *faster* after compaction | B-tree descent per hop | 5–22× slower | yes until eviction, then stalls |
| Keeps copy-on-write snapshot isolation and statement atomicity | yes, unchanged model | needs MVCC + undo | via KV MVCC | needs copy-on-write anyway |
| Keeps the logical WAL | yes (transaction-consistent checkpoints) | no (physical logging) | redundant | yes, with shadow paging |
| wasm and the in-memory default unchanged | yes (never-evict backend) | separate engine | separate engine | separate engine |
| Incremental checkpoint / instant restart | yes | yes | yes | yes |
| Can ship in stages | yes (stages 0–2 are in-memory only) | no | no | partly |

### 5.2 Staged roadmap

Each stage ships on its own, keeps in-memory as the default, and leaves the
public API and bindings unchanged. Where a stage adds something, it is an
optional open option.

**Stage 0: stop paying for what nobody asked for.** In-memory only. Small,
independent PRs.

| Change | Where | Expected effect |
|---|---|---|
| Radix chunk table (two levels, 512×512) for `ChunkedVec`; `Arc` the property-index registry and constraint catalog | `memory/chunked_vec.rs`, `entity_index_store.rs:175-206`, `graph.rs:163-190` | Staged write becomes an O(1) clone (spike: ~10 ns) plus ~1–1.5 µs per touched path: 254 µs → ~10–20 µs at L, flat in N (§2.5) |
| Stop eager hash-indexing on snapshot load and WAL replay; implicit indexes become explicit or capped, and show up in `SHOW INDEXES` and `MemoryReport` | `graph.rs:1733-1797`, `graph.rs:780-838` | Restart memory = runtime memory; no hidden 800 MB per ad-hoc lookup. Needs a decision (§9, D3). |
| Id-seek operator for `id(n) = $x` / `IN $list` | compiler + executor | 232 ms → ~1 µs |
| Streaming id scans (chunk iterators instead of `Vec`) | `traits.rs:49-63`, `pull/scan.rs:108` | O(1) memory per scan (TODO :36) |
| Fix `known-risks.md:93`, add a 10M-element case to `write_scaling.rs`, check `storage_baseline` in as a bench | docs, tests | Honest model; regressions are caught |
| Re-measure Festimap P2-5 RSS on 0.22 with `heap_probe --festimap` | — | Close or update P2-5 |

**Stage 1: compact in-memory encoding.** In-memory only, the biggest
user-visible win.
- Intern labels and relationship types into a per-graph dictionary
  (`u16`/`u32`). This also fixes the per-thread intern cache duplication.
- Encode records as bytes in chunk arenas. Inline `(type, neighbour, rel_id)`
  in CSR adjacency.
- Encode properties as key id + tag + value.
- Introduce the `NodeView` / `RelView` read API (§6) and port the 60
  `with_node` / `with_relationship` call sites.
- Shrink RANGE and UNIQUE to a sorted index only. The hash half is redundant
  for equality seeks once a sorted seek is cheap. Uncertain: measure first.
- **Targets:** ≤ 100 B/element on the benchmark shape, 1-hop faster than
  today, every other hot operation within 10% (§8).
- **Snapshot format:** could stay as is, but the chunk encoding makes a
  chunk-native format (Stage 3) almost free.

**Stage 2: large-value offload.** Optional, needs a filesystem.
- Values above a threshold (256 B? configurable) and all vectors go to a
  blob file, with an 8 B handle in the record and an LRU byte-budgeted cache.
- The vector index shares the stored vector instead of copying it.
- This is the easiest first use of disk. Reads of large values are rare and
  already `SlotValue::Shared`. It is off for wasm.

**Stage 3: chunk residency and incremental checkpoints.** The disk tier.
- `ChunkCell` residency, the chunk file, shadow-paged incremental
  checkpoints, a memory budget, and eviction (properties first).
- Disk-backed databases (`openWalDatabase`, named `.loradb`) gain an optional
  `memoryBudget`. Without it, behaviour is today's: everything resident, and
  only the checkpoint becomes incremental.
- `checkpoint` moves off the commit path (TODO phase 6 falls out for free).
- Restart is instant plus WAL tail.
- `lora-snapshot` stays as the portable export format and for wasm.
- Gated behind a `disk` Cargo feature. It is off for wasm.
- Traversal prefetch: an expand over a cold adjacency chunk batches the fault
  of its neighbours' chunks, with several reads in flight on a small I/O pool.

**Stage 4: index residency and statistics.**
- Index leaves (`CowOrdMap` partitions, `IdSet` chunks, trigram shards)
  become `ChunkCell`s too.
- ~~Distinct counts move to HyperLogLog, so stats no longer depend on which
  hash indexes are active.~~ Done, with deletable counting sketches instead
  of HyperLogLog (see Implementation status).
- HNSW stays resident or uses its own quantised in-RAM graph. That is a
  separate decision.

**Stage 5: only if measurements demand it.**
- Locality reordering of node ids (BFS or Rabbit order) at checkpoint.
- Linux-only `io_uring` fault batching.
- A vmcache/exmap mapping on Linux.
- A KV or object-store backend for chunk blobs.

### 5.3 What this does not try to do

- **Multi-writer concurrency.** Unchanged. The concurrency plan's phases 1–7
  apply on top. The chunk store is compatible with them because writers
  already work on private copy-on-write versions.
- **Transactions larger than memory.** A single transaction's dirty chunks
  must fit in memory, the same limit Memgraph's on-disk mode has. Huge bulk
  loads need a dedicated path that checkpoints mid-load (§9, Q4).
- **Graph analytics over everything.** Whole-graph algorithms on a dataset 10×
  RAM will be I/O-bound. That is acceptable.

---

## 6. Trait changes

Sketched Rust, not final.

```rust
// lora-store: move IndexDefinition, GraphStats, constraint types out of
// `memory` into `crate::schema` (pure data). No behaviour change.

/// Read errors become possible once a chunk can be on disk.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("i/o error reading chunk {chunk}: {source}")]
    Io { chunk: ChunkAddr, source: std::io::Error },
    #[error("checksum mismatch in chunk {0}")]
    Corrupt(ChunkAddr),
}
pub type StoreResult<T> = Result<T, StorageError>;

/// Borrowed, encoding-agnostic view of a node. Lives no longer than the
/// closure; a paged backend pins the chunk for that long.
pub trait NodeView {
    fn id(&self) -> NodeId;
    fn labels(&self) -> impl Iterator<Item = LabelId> + '_;
    fn has_label(&self, label: LabelId) -> bool;
    fn property(&self, key: KeyId) -> Option<ValueRef<'_>>;   // zero-copy
    fn properties(&self) -> impl Iterator<Item = (KeyId, ValueRef<'_>)> + '_;
}
pub trait RelView { /* id, src, dst, rel_type: TypeId, property, properties */ }

/// Borrowed value: scalars by value, strings/bytes by slice, large values
/// by handle (Stage 2 blob store resolves lazily).
pub enum ValueRef<'a> { Null, Bool(bool), Int(i64), Float(f64), Str(&'a str),
                        Bytes(&'a [u8]), Temporal(..), Point(..), Blob(BlobHandle),
                        Encoded(&'a [u8]) /* list/map, decoded on demand */ }

pub trait GraphStorage {
    type Node<'a>: NodeView where Self: 'a;
    type Rel<'a>: RelView where Self: 'a;

    // dictionaries (always resident)
    fn label_id(&self, name: &str) -> Option<LabelId>;
    fn type_id(&self, name: &str) -> Option<TypeId>;
    fn key_id(&self, name: &str) -> Option<KeyId>;

    // point access: the existing closure shape, now with a view and an error
    fn with_node<R>(&self, id: NodeId, f: impl FnOnce(Self::Node<'_>) -> R) -> StoreResult<Option<R>>;
    fn with_relationship<R>(&self, id: RelationshipId, f: impl FnOnce(Self::Rel<'_>) -> R) -> StoreResult<Option<R>>;

    // traversal: visitor over inline (rel, type, neighbour) entries
    fn try_for_each_expand<E: From<StorageError>>(
        &self, node: NodeId, dir: Direction, types: &[TypeId],
        f: impl FnMut(RelationshipId, TypeId, NodeId) -> Result<(), E>,
    ) -> Result<(), E>;

    // scans: chunked, streaming; never a full Vec
    fn for_each_node_id_chunk<E: From<StorageError>>(
        &self, label: Option<LabelId>, f: impl FnMut(&[NodeId]) -> Result<ControlFlow<()>, E>,
    ) -> Result<(), E>;

    /// Hint: these ids will be read soon (batches cold faults). No-op in memory.
    fn prefetch_nodes(&self, _ids: &[NodeId]) {}

    // index seeks, constraint checks, stats: as today, returning StoreResult
    // ...
}

/// What `lora-database` reaches around the trait for today (downcasts,
/// inherent methods). Making it a trait removes every TypeId check.
pub trait StorageEngine: GraphStorage + GraphStorageMut + Clone + Send + Sync + 'static {
    fn set_mutation_recorder(&mut self, r: Option<Arc<dyn MutationRecorder>>);
    fn set_deleted_record_sink(&mut self, s: Option<Arc<dyn DeletedRecordSink>>);
    fn apply_replayed(&mut self, ev: &MutationEvent) -> Result<(), ReplayError>;
    fn supports_live_fast_path(&self) -> bool { false }
    fn export_snapshot(&self, w: &mut dyn SnapshotSink) -> Result<SnapshotMeta, SnapshotError>;
    fn import_snapshot(src: &mut dyn SnapshotSource) -> Result<Self, SnapshotError> where Self: Sized;
    fn memory_report(&self) -> MemoryReport;
    /// Durable backends only: write dirty chunks of *this* version, return the root.
    fn checkpoint(&self, lsn: Lsn) -> Result<Option<CheckpointRoot>, StorageError> { Ok(None) }
}
```

**Migration notes.**
- **Compatibility.** Keep the old owned methods (`node() -> NodeRecord` and
  the rest) as defaulted wrappers over the views, so the change is additive
  for the executor. Port hot paths first, cold paths whenever.
- **Error handling** is the most invasive change, because executor
  read paths cannot fail on storage today. The options are in §9 (D4).
- **`unsafe` lifetime extension** in `stream.rs` and `transaction.rs` becomes
  `Arc<Version>` pinning. It is already logically that. The `'static` borrow
  turns into an owned `Arc` held by the cursor, with no `unsafe`.

---

## 7. Transactions, atomicity, WAL and snapshots in the new design

| Concern | Today | Versioned chunk store |
|---|---|---|
| Reader isolation | Pin `Arc<InMemoryGraph>` | Pin `Arc<Version>`. Faulting a cold chunk under a pinned version is safe: the chunk is immutable and its disk address stays valid while any version references it (epoch-protected free list). |
| Writers | One mutex | One mutex (unchanged). Concurrency phases 1–7 still apply later. |
| Staged write (statement atomicity) | O(N/512) clone, then publish or drop | O(1) root clone. A write path-copies the touched leaf chunk plus ≤2 interior nodes. On failure the staged root is dropped; its private chunks were never visible and never written. |
| Live fast path | `Arc::make_mut` in place | Kept as an optimisation, or retired. With O(1) staging its only benefit is skipping ~2 path copies (§9, Q5). |
| Writing to a cold chunk | n/a | The writer faults it in, copies it on write, and the new chunk is dirty and pinned. |
| Transactions and savepoints | Full clone at first write, plus a savepoint clone per statement | Same calls, each O(1). A savepoint is just an older root `Arc`. |
| WAL | Logical `MutationEvent`s, GroupSync | **Unchanged.** Residency and checkpoints are not logical events. |
| Checkpoint | Encode the whole graph under the writer mutex, often inside a commit | Background: pin a published version, write its dirty chunks and interiors, fsync, write the root (LSN = the version's commit LSN), flip the root, truncate the WAL to that LSN. The writer is never blocked. Dirty memory is bounded by checkpoint frequency and a dirty-byte trigger. |
| Recovery | Load the whole snapshot (17 s at 10M elements), rebuild every index, replay the WAL | Open the newest root whose checksum verifies (O(1)), replay the WAL tail. Indexes are persisted as chunks in Stage 4; until then they are rebuilt lazily or eagerly by policy (§9, Q6). |
| Snapshot export (`save_snapshot`, wasm) | `LORACOL1` whole-graph file | Unchanged format, now written as a *streaming* walk over a pinned version (no deep clone). A chunk-file root can also be copied as a "hot backup". |
| Torn writes and corruption | BLAKE3 per snapshot | Checksum per chunk, verified on fault. A bad chunk is a `StorageError::Corrupt` for the queries that touch it, not a `SIGBUS`. |
| Change feed / CDC | Post-image from `&InMemoryGraph` | Post-image from the published version via views. |

**How durability ordering works.**
- Commit order is unchanged: the WAL record is written before the version is
  published.
- A checkpoint never contains an uncommitted version, because it only
  pins published ones.
- A crash between "chunks written" and "root flipped" leaves orphan chunks,
  which the free-list rebuild at open discards.

This is the same argument LMDB and redb use for crash safety, one level up
**[V for LMDB/redb design]**.

---

## 8. Benchmark plan

### 8.1 Dataset generator

Extend `storage_baseline` into a checked-in example plus a CI bench:
`crates/lora-database/examples/storage_bench.rs` and
`benches/storage_scale.rs`.

| Dimension | Values |
|---|---|
| Shape | `uniform` (today's), `powerlaw` (R-MAT a=.57 b=.19 c=.19, so hubs exist), `festimap` (the `heap_probe --festimap` schema scaled up: festivals, artists, users, follows, attendances, constraints, fulltext, point) |
| Scale | 1M, 10M, 100M elements; 500M+ only for the disk tier |
| Properties | `small` (4 scalars), `mixed` (+ 1 KiB text on 5% of nodes, + a 128-d vector on 1%) |
| Access pattern | `uniform`, `zipf(0.99)` over node ids, `temporal` (a sliding hot window: recent ids hot) |
| Budget (Stage 3) | resident fraction ∈ {100%, 50%, 25%, 10%} of the compact size |
| Determinism | splitmix64 seed; print the dataset hash with the results |

Measure everything in §2, plus:
- p99.9;
- throughput with 8 reader threads;
- write throughput with GroupSync (and with per-commit fsync, once one exists);
- checkpoint duration and the largest writer stall during a checkpoint;
- time to first query after open;
- RSS high-water mark during save, load and checkpoint.

### 8.2 Pass/fail targets

"Today" means the v0.22.3 numbers in §2 on the same machine. Compact-stage
targets are on the uniform 10M-element shape.

| # | Stage | Metric | Target |
|---|---|---|---|
| T1 | 0 | Staged write (constrained CREATE, any-constraint SET, rel CREATE) at 10M and at 100M elements | median ≤ 25 µs and **independent of N** (≤ 1.3× between 1M and 100M) |
| T2 | 0 | `id(n) = $x` | ≤ 3 µs median |
| T3 | 0 | Memory after restart vs before (same data, no ad-hoc lookups) | ≤ 1.05× |
| T4 | 1 | Live bytes per element, benchmark shape, no indexes | **≤ 100 B** (today 247) |
| T5 | 1 | RANGE index bytes per entry | ≤ 80 B (today 348–475) |
| T6 | 1 | Cypher index seek, 2-hop, range seek, label scan per node | within **+10%** of today |
| T7 | 1 | Raw 1-hop at 10M | ≤ today (1.6 µs). Expected well under. |
| T8 | 1 | Fast-path write latency | within +20% |
| T9 | 3 | Hot working set (100% resident) through the tiered build, vs Stage 1 | within **+10%** on T6/T7 ops, within +20% vs today |
| T10 | 3 | Cold point lookup (property chunk not resident), NVMe, queue depth 1 | p50 ≤ 150 µs, **p99 ≤ 1 ms**, p99.9 ≤ 5 ms |
| T11 | 3 | Cold 1-hop (adjacency chunk + neighbour chunks cold) with prefetch | p99 ≤ 2 ms |
| T12 | 3 | zipf(0.99) mixed read workload, 25% budget | throughput ≥ **70%** of all-resident |
| T13 | 3 | Open-to-first-query at 100M elements | ≤ 1 s + WAL-tail replay (today 17 s at 10M) |
| T14 | 3 | Checkpoint | no writer stall > 5 ms; write volume ∝ dirty chunks (≤ 2× dirty bytes) |
| T15 | 3 | Memory | RSS ≤ budget + 10% + dirty bytes |
| T16 | all | wasm and in-memory builds | perf-smoke and memory-bench gates unchanged or better; wasm bundle size ≤ +5% |
| T17 | all | Crash safety | kill -9 fuzz (as Festimap's 3-round test, ×1000 iterations with random kill points during checkpoint): zero lost acknowledged-and-synced commits, zero corrupt opens |

T10–T11 assume an NVMe SSD with a 4–16 KiB random-read latency of
~20–100 µs. On the reference machine (§2.5) a 4 KiB read measured 77–84 µs
at QD1, and a cold paged expand 85 µs p50 / 113–120 µs p99, so T10 has about
8× headroom at p99. T11 is achievable only if the reads for a hop are issued
in parallel. A cloud
deployment on network storage (EBS gp3 ≈ 0.5–1 ms) needs its own targets.

---

## 9. Decisions and open questions

### Decisions for you

- **D1: Is RAM capacity the actual problem, or is it cost?**
  - Festimap fits in RAM by orders of magnitude.
  - If the goal is "10× more graph per GB", Stages 0–1 deliver about 3× with
    no new failure modes.
  - If the goal is "graphs of hundreds of GB on one box", Stage 3 is
    required.
  - **Recommendation:** commit to Stages 0–1 now. Decide on Stage 3 after
    Stage 1's numbers.
- **D2: Accept a per-graph dictionary for labels, types and keys?** It
  replaces the global intern table and lets records store `u16`/`u32` ids.
  This affects the WAL and snapshot codecs, which keep writing names, so the
  wire formats stay unchanged.
  - **Recommendation:** yes.
- **D3: What happens to implicit hash indexes?** Options:
  - (a) keep them lazy at runtime and stop eager rebuilding on restart;
  - (b) cap them by memory and evict;
  - (c) remove them and require declared indexes. The planner would fall back
    to a scan and EXPLAIN would say so.
  - **Recommendation:** (a) now. Then (c) behind a config flag, with
    lora-graphql's inferred indexes made explicit (the TODO :72 already
    wants a warning on them).
  - Distinct-count stats no longer depend on these indexes (sketches per
    `(label, key)`, see Implementation status), so nothing in the planner
    blocks (c).
- **D4: How do storage I/O errors reach queries?**
  - (a) `StoreResult` through every executor read path. This is the correct
    option and the most churn.
  - (b) Reads stay infallible. A fault failure poisons the query through a
    side channel (a context error slot checked at operator boundaries) and
    returns a sentinel.
  - **Recommendation:** (b) for Stage 3, migrating to (a) incrementally. It
    keeps the executor diff small, and LoraDB already uses cooperative
    deadline checks with the same shape.
- **D5: The disk-tier file format.**
  - A new chunk file, with `LORACOL1` kept as the export and interchange
    format, or
  - make the chunk file *the* snapshot format, so wasm reads and writes
    single-file chunk images in memory.
  - **Recommendation:** a new format, keeping `LORACOL1` for compatibility.
- **D6: Platforms for the disk tier.** Linux and macOS are first-class.
  Windows? The design uses only `pread`/`pwrite`/`fsync` (no `madvise`
  tricks), so portability is mostly free.
- **D7: Whether to keep the live fast path after Stage 0.** Measure first.
  If staged writes cost ≤ 1.3× fast-path writes, deleting the fast path
  removes the WAL-poisoning failure mode (`write_guard.rs:310-315`) and a
  whole class of `constraints_may_reject` edge cases.

### Open questions (measure or prototype)

- **Q1: Does compact encoding cost anything on the hot path?**
  Decoding a property from bytes versus reading a `PropertyValue`. The
  expectation is neutral or faster thanks to locality, but the Stage 1
  prototype must show it (T6).
- **Q2: Chunk size per kind.** 512 entities is the copy-on-write unit today.
  Compact property chunks might be 20–80 KiB, too large an I/O unit for point
  reads and too small for scans. Possible answers:
  - split property chunks into 4–16 KiB sub-pages, or
  - use smaller chunks for properties (64 entities).
- **Q3: Eviction policy.** Random + CLOCK with sampled reference bits is the
  starting point. Is a Siberia-style offline classification worth it for
  temporal workloads (Festimap: old festivals cold, upcoming hot)?
- **Q4: Bulk loads bigger than memory.** Single-transaction dirty data must
  fit in RAM. Options:
  - an `IMPORT` path that commits in batches and checkpoints as it goes;
  - an offline builder that writes the chunk file directly, as Kùzu's COPY
    does.
- **Q5: Explicit transactions** pin their staged version and its dirty
  chunks. A long transaction's dirty set is bounded only by memory. Should
  there be a configurable cap that aborts it?
- **Q6: Index persistence before Stage 4.** Until index leaves are chunks,
  every open rebuilds declared indexes from (possibly cold) data, which is a
  full scan from disk. Should Stage 3 already persist RANGE indexes as
  chunks? Probably yes for RANGE and uniqueness; HNSW can stay as it is.
- **Q7: macOS eviction semantics.** Stage 3 frees memory by dropping the
  `Arc<ChunkData>`, which is ordinary `free`, so it does not depend on
  `madvise`. Only a vmcache-style variant (Stage 5) would. §2.5 shows that
  macOS ignores `MADV_DONTNEED` and `MADV_FREE` for anonymous memory; only a
  `MAP_FIXED` remap frees it. So vmcache stays Linux-first.
- **Q8: Hub nodes.** A node with 1M relationships spans many adjacency pages.
  How do hub lists split, and how does a typed expand on a hub stay
  O(matching edges)? Sortledton and Teseo give two answers.

---

## 10. Sources

**LoraDB code.** Cited inline at `93dc8ff`. Survey notes and raw benchmark
output are in the research scratchpad (not committed); the harness is
`crates/lora-database/examples/storage_baseline.rs`.

**Papers and documentation.**

- **Buffer management**
  - Leis et al., *LeanStore: In-Memory Data Management Beyond Main Memory*, ICDE 2018. <https://db.in.tum.de/~leis/papers/leanstore.pdf>
  - Haubenschild et al., *Rethinking Logging, Checkpoints, and Recovery for High-Performance Storage Engines*, SIGMOD 2020. <https://db.in.tum.de/~leis/papers/rethinkingLogging.pdf>
  - Neumann & Freitag, *Umbra: A Disk-Based System with In-Memory Performance*, CIDR 2020. <https://www.cidrdb.org/cidr2020/papers/p29-neumann-cidr20.pdf>
  - Leis et al., *Virtual-Memory Assisted Buffer Management* (vmcache, exmap), SIGMOD 2023. <https://www.cs.cit.tum.de/fileadmin/w00cfj/dis/_my_direct_uploads/vmcache.pdf>; code: <https://github.com/viktorleis/vmcache>, <https://github.com/tuhhosg/exmap>
  - Crotty, Leis & Pavlo, *Are You Sure You Want to Use MMAP in Your Database Management System?*, CIDR 2022. <https://db.cs.cmu.edu/papers/2022/cidr2022-p13-crotty.pdf>
- **Hot and cold data**
  - DeBrabant et al., *Anti-Caching*, VLDB 2013. <https://www.vldb.org/pvldb/vol6/p1942-debrabant.pdf>
  - Levandoski et al., *Identifying Hot and Cold Data in Main-Memory Databases* (Siberia), ICDE 2013. <https://www.microsoft.com/en-us/research/wp-content/uploads/2013/04/ColdDataClassification-icde2013-cr.pdf>
  - Jiang et al., *CLOCK-Pro*, USENIX ATC 2005. <https://www.usenix.org/legacy/events/usenix05/tech/general/full_papers/jiang/jiang.pdf>
- **Graph systems**
  - Zhu et al., *LiveGraph*, VLDB 2020. <https://www.vldb.org/pvldb/vol13/p1020-zhu.pdf>
  - Jin et al., *KÙZU Graph Database Management System*, CIDR 2023. <https://www.cidrdb.org/cidr2023/papers/p48-jin.pdf>
  - Dhulipala et al., *Low-Latency Graph Streaming Using Compressed Purely-Functional Trees* (Aspen), PLDI 2019. <https://arxiv.org/abs/1904.08380>; PaC-trees <https://arxiv.org/abs/2204.06077>
  - Fuchs et al., *Sortledton*, VLDB 2022. <https://www.vldb.org/pvldb/vol15/p1173-fuchs.pdf>; Teseo, VLDB 2021. <http://vldb.org/pvldb/vol14/p1053-leo.pdf>
  - Shun et al., *Ligra+: Smaller and Faster*, DCC 2015. <https://www.cs.umd.edu/~laxman/papers/Ligra+.pdf>
  - Boldi & Vigna, *WebGraph I*. <https://vigna.di.unimi.it/ftp/papers/WebGraphI.pdf>; LLP <https://vigna.di.unimi.it/ftp/papers/LayeredLabelPropagation.pdf>
  - Kyrola et al., *GraphChi*, OSDI 2012. <https://www.usenix.org/system/files/conference/osdi12/osdi12-final-126.pdf>
  - Roy et al., *X-Stream*, SOSP 2013. <https://sigops.org/s/conferences/sosp/2013/papers/p472-roy.pdf>
- **Product documentation**
  - Neo4j store formats. <https://neo4j.com/docs/operations-manual/current/database-internals/store-formats/>; block format <https://neo4j.com/blog/developer/neo4j-graph-native-store-format/>; record sizes from the 4.4 source `NodeRecordFormat.java` and siblings.
  - Memgraph storage modes and memory usage. <https://memgraph.com/docs/fundamentals/storage-memory-usage>, <https://memgraph.com/blog/memgraph-storage-modes-explained>
  - redb design. <https://github.com/cberner/redb/blob/master/docs/design.md>
  - LMDB `lmdb.h`; heed wasm issue. <https://github.com/meilisearch/heed/issues/162>
  - fjall 3. <https://fjall-rs.github.io/post/fjall-3/>
  - RocksDB tuning and prefix seek. <https://github.com/facebook/rocksdb/wiki/RocksDB-Tuning-Guide>, <https://github.com/facebook/rocksdb/wiki/Prefix-Seek>
  - sled. <https://github.com/spacejam/sled>
  - JanusGraph data model. <https://docs.janusgraph.org/advanced-topics/data-model/>
  - Dgraph posting lists. <https://docs.dgraph.io/design-concepts/posting-list-concept/>
  - SurrealDB graph keys (source). <https://github.com/surrealdb/surrealdb/blob/main/surrealdb/core/src/key/graph/mod.rs>
  - FalkorDB design. <https://docs.falkordb.com/design>
  - TuGraph. <https://tugraph-db.readthedocs.io/>
  - Postgres TOAST. <https://www.postgresql.org/docs/current/storage-toast.html>

**Uncertain or unverified.**
- Neo4j block-format internals; its performance figures are vendor claims.
- Kùzu's acquisition.
- Dgraph and ArangoDB internals (taken from documentation summaries).
- Whether redb builds for wasm with a custom backend.
- Every **[I]** estimate in §4.7, which the Stage 1 prototype must confirm.
