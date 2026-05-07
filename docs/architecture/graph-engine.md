# Graph Architecture

## Storage engine design

The live graph is stored entirely in process memory by
`lora_store::InMemoryGraph`. The implementation is slot-indexed rather than
map-backed: node and relationship IDs are direct indexes into vectors of
optional records. Deletes leave tombstones, IDs are never reused, and a compact
`live_*_count` is maintained for catalog reads.

`lora-database` wraps this store in an `ArcSwap` snapshot holder. Read-only
auto-commit queries load an `Arc<InMemoryGraph>` and run without a store lock.
Mutating auto-commit queries stage changes against a cloned snapshot, append WAL
records when configured, and publish the new `Arc` atomically. Explicit
read-write transactions still serialize through the database writer mutex.

## Core data structures

```text
InMemoryGraph
├── nodes:                  Vec<Option<Arc<NodeRecord>>>
├── relationships:          Vec<Option<Arc<RelationshipRecord>>>
├── outgoing:               Vec<Vec<RelationshipId>>
├── incoming:               Vec<Vec<RelationshipId>>
├── nodes_by_label:         BTreeMap<String, Vec<NodeId>>
├── relationships_by_type:  BTreeMap<String, Vec<RelationshipId>>
├── indexes:                RwLock<PropertyIndexRegistry>
├── next_node_id:           u64
├── next_rel_id:            u64
├── live_node_count:        usize
├── live_rel_count:         usize
└── recorder:               Option<Arc<dyn MutationRecorder>>
```

Records are held behind `Arc` so a staged writer can share unchanged records
with the current published snapshot. Property, label, and relationship changes
use `Arc::make_mut`, so only touched records are cloned.

### Node record

```rust
struct NodeRecord {
    id: NodeId,           // u64, auto-incremented
    labels: Vec<String>,  // trimmed, empty labels removed, duplicates removed
    properties: BTreeMap<String, PropertyValue>,
}
```

### Relationship record

```rust
struct RelationshipRecord {
    id: RelationshipId,   // u64, auto-incremented
    src: NodeId,          // source node
    dst: NodeId,          // destination node
    rel_type: String,     // trimmed, non-empty, immutable
    properties: BTreeMap<String, PropertyValue>,
}
```

Relationship creation fails if either endpoint is missing or the trimmed type is
empty.

### Property values

```rust
enum PropertyValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Binary(LoraBinary),
    List(Vec<PropertyValue>),
    Map(BTreeMap<String, PropertyValue>),
    Date(LoraDate),
    Time(LoraTime),
    LocalTime(LoraLocalTime),
    DateTime(LoraDateTime),
    LocalDateTime(LoraLocalDateTime),
    Duration(LoraDuration),
    Point(LoraPoint),
    Vector(LoraVector),
}
```

Temporal, spatial, binary, and vector types are first-class property values.
Definitions live under `crates/lora-store/src/types/`.

## Index structures

### Label and relationship-type indexes

Labels and relationship types map to vectors of IDs:

```text
"User"    -> [0, 1, 3, 5]
"Admin"   -> [0]
"FOLLOWS" -> [0, 1, 2]
```

The indexes are maintained on create, label add/remove, relationship create,
relationship delete, node delete, snapshot load, and WAL replay. They preserve
deterministic key ordering through `BTreeMap`; the ID lists may contain gaps only
when the corresponding records have been deleted and filtered out by the read
helpers.

### Property indexes

`InMemoryGraph` has lazy exact-match property indexes for nodes and
relationships. A call to `find_nodes_by_property` or
`find_relationships_by_property` builds the index for that property key the
first time it can be indexed, then keeps the active index current on future
mutations.

Indexed values:

- `null`, booleans, integers, strings, binary values
- finite floats (`NaN` is not indexed; `-0.0` and `+0.0` normalize together)
- lists and maps whose nested values are all indexable

Scan fallback:

- temporal values
- spatial points
- vectors
- `NaN` floats
- nested lists/maps containing any non-indexable value

The index is internal only. There is no Cypher DDL such as `CREATE INDEX`, no
composite/range/full-text/vector index, and no user-visible index catalog.

### Adjacency indexes

Outgoing and incoming relationship IDs are stored in two per-node vectors:

- `outgoing[node_id]` — relationships leaving the node
- `incoming[node_id]` — relationships arriving at the node

Deleting a relationship removes its ID from both endpoint vectors. Deleting a
node clears the node's adjacency vectors; the outer adjacency vectors are not
shrunk.

## ID allocation

Node and relationship IDs are allocated sequentially from monotonic counters and
are never reused after deletion.

```text
next_node_id: 0 -> 1 -> 2 -> ...
next_rel_id:  0 -> 1 -> 2 -> ...
```

This avoids stale-reference reuse but means IDs are not contiguous after
deletions and slot vectors may contain tombstones.

## Traversal operations

### Expand

The core traversal primitive takes a source node, a direction, and an optional
relationship type filter:

1. Read relationship IDs from `outgoing`, `incoming`, or both.
2. Filter by relationship type when types were supplied.
3. Resolve each relationship and the other endpoint node.
4. Return `Vec<(RelationshipRecord, NodeRecord)>` for the compatibility API, or
   use borrow hooks on hot executor paths to avoid record clones.

```text
Direction::Right      -> outgoing adjacency
Direction::Left       -> incoming adjacency
Direction::Undirected -> outgoing + incoming
```

## Write operations

### Node creation

1. Allocate `NodeId`.
2. Normalize labels: trim, drop empty strings, deduplicate while preserving first
   occurrence.
3. Insert `NodeRecord` at the ID slot.
4. Update active label and property indexes.
5. Initialize empty adjacency vectors for that slot.

### Relationship creation

1. Validate both endpoints exist.
2. Validate type is non-empty after trimming.
3. Allocate `RelationshipId`.
4. Insert `RelationshipRecord` at the ID slot.
5. Update outgoing, incoming, type, and active property indexes.

### Node deletion

- `delete_node` fails if the node has any incident relationships.
- `detach_delete_node` deletes all incident relationships first, then deletes the
  node.

### Property and label mutation

- `set_node_property` / `set_relationship_property`: insert or update one key.
- `remove_node_property` / `remove_relationship_property`: remove one key.
- `replace_node_properties`: replace the complete property map.
- `merge_node_properties`: merge keys without removing existing properties.
- `add_node_label` / `remove_node_label` / `set_node_labels`: modify labels with
  index maintenance.

Each primitive mutation emits a `MutationEvent` when a recorder is installed.

## Storage trait hierarchy

The storage API is split into read, catalog, borrow, and mutation traits:

- `GraphStorage` — point lookups, ID scans, label/type scans, expansion, and
  default helpers.
- `GraphCatalog` — a narrow analyzer-facing slice for counts, labels, types, and
  property-key existence.
- `BorrowedGraphStorage` — optional `&NodeRecord` / `&RelationshipRecord`
  access for backends that can hand out references.
- `GraphStorageMut` — create, mutate, delete, `clear`, and property/label helper
  methods.

`InMemoryGraph` implements all four traits and overrides the hot paths. Bulk
record-returning APIs such as `all_nodes()` still allocate owned record vectors;
the executor uses `with_node` / `with_relationship` closures where possible.

## Limitations

- **Single-process memory store** — there is no disk-backed buffer pool or remote
  storage engine.
- **Tombstones, no compaction** — deleted IDs leave gaps in the slot vectors.
- **No uniqueness constraints** — duplicate labels/properties are allowed.
- **Internal exact-match property indexes only** — no DDL, composite, range,
  full-text, or vector indexes.
- **Clone compatibility APIs** — bulk read helpers allocate owned records even
  though executor hot paths avoid many clones.
- **Vectors cannot be stored inside list properties** — a vector can be a direct
  property or a value inside a top-level map property, but list-of-vector
  properties are rejected to preserve future indexing options.

## Durability

Snapshots are encoded by the `lora-snapshot` columnar codec. The current file
magic is `LORACOL1`; the envelope contains a bincode manifest, a BLAKE3 checksum,
and an optional compressed/encrypted body. `lora-database` writes snapshots via
an atomic `<path>.tmp` + rename protocol and publishes loaded snapshots by
swapping the database's `ArcSwap` store pointer.

The WAL is built on `MutationEvent`. When WAL is enabled, `InMemoryGraph` has a
`MutationRecorder`; writes are buffered into committed batches and replayed on
recovery. Named databases use the same WAL events with a `.loradb` container
mirror.

See [Snapshots](../operations/snapshots.md) and [WAL](../operations/wal.md) for
operator-facing details.

## Next steps

- How reads and writes flow through the engine: [Data Flow](data-flow.md)
- Value representation and property types: [Value Model](../internals/value-model.md)
- Known performance trade-offs: [Performance Notes](../performance/notes.md)
- Broader limitations and mitigations: [Known Risks](../design/known-risks.md)
- Durability, snapshots, WAL, and admin routes: [Snapshots](../operations/snapshots.md)
