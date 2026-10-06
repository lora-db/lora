# Graph Architecture

## Storage engine design

The live graph is stored entirely in process memory by
`lora_store::InMemoryGraph`. The implementation is slot-indexed rather than
map-backed: node and relationship IDs are direct indexes into vectors of
optional records. Deletes leave tombstones, IDs are never reused, and a compact
`live_*_count` is maintained for catalog reads.

`lora-database` wraps this store in a `RwLock<Arc<InMemoryGraph>>` snapshot
holder (`live_store.rs`). Read-only auto-commit queries clone the `Arc` under a
brief read lock and run without a store lock. Mutating auto-commit queries take
the database writer mutex, stage changes against a cloned graph (or mutate the
live graph in place via `Arc::make_mut` for plans that cannot fail midway),
append WAL records when configured, and publish the new `Arc` under the write
lock. Explicit read-write transactions also serialize through the writer mutex.

## Core data structures

```text
InMemoryGraph
├── next_node_id:           u64
├── next_rel_id:            u64
├── nodes:                  ChunkedVec<Option<Blob>>   // encoded records
├── relationships:          ChunkedVec<Option<Blob>>
├── dicts:                  Dicts                      // label, type and key numbers
├── live_node_count:        usize
├── live_rel_count:         usize
├── outgoing:               ChunkedVec<AdjList>      // (type, neighbour, relationship id) entries
├── incoming:               ChunkedVec<AdjList>
├── nodes_by_label:         BTreeMap<String, ChunkedVec<NodeId>>
├── relationships_by_type:  BTreeMap<String, ChunkedVec<RelationshipId>>
├── indexes:                IndexBundle
│   ├── catalog:            RwLock<IndexCatalog>
│   ├── properties:         RwLock<PropertyIndexRegistry>
│   ├── text / sorted / point / fulltext / vector:
│   │                       EntityIndexStore<…> (one per entity kind)
│   └── active_* counters:  AtomicUsize
├── constraint_catalog:     RwLock<ConstraintCatalog>
├── active_constraints:     AtomicUsize
├── recorder:               Option<Arc<dyn MutationRecorder>>
└── deleted_sink:           Option<Arc<dyn DeletedRecordSink>>
```

(`crates/lora-store/src/memory/graph.rs`, `IndexBundle` in
`memory/entity_index_store.rs`.)

`ChunkedVec` (`memory/chunked_vec.rs`) is a `Vec` stored as a two-level
persistent radix tree: 512-entry leaf chunks shared by `Arc`, 128 leaves per
interior node, one shared root. Cloning it bumps one refcount, so cloning the
graph is O(#labels + #relationship types), and a write copies only the root
table, interior node and chunk on its path. The index and constraint catalogs
are shared the same way. Each record is an immutable, reference-counted byte
string, so a staged writer shares unchanged records with the current published
snapshot; a property or label change stores a new encoding of that one record.
Secondary indexes are copy-on-write too (`memory/cow.rs`),
so write cost stays flat as the graph grows.

`recorder` and `deleted_sink` are not part of the graph's identity and are
dropped on clone. `deleted_sink` sees each record just before a delete drops
it, so change feeds can report deleted entities without copying the graph.

### Stored records

A node or relationship is stored as one compact byte string (`encoded.rs`),
behind an 8-byte slot and an 8-byte header:

```text
node:         label count, label numbers...,  properties
relationship: source id, target id, type number,  properties
properties:   count, then per property in key-name order:
                key number, tag byte, payload
payload:      null / false / true   nothing
              int                   zigzag varint
              float                 8 bytes
              string                length, UTF-8 bytes
              other                 length, `codec` bytes (lists, maps,
                                    temporals, points, vectors, binary)
```

Integers are varints. Labels, relationship types and property keys are stored
as numbers from the graph's dictionaries (`dict.rs`), assigned in first-use
order and never reused. A clone shares the dictionaries until one side meets a
new name. The numbers are not persisted: snapshots and the WAL store names.

Readers never see the bytes. `with_node` and `with_relationship` pass a
`NodeRef` / `RelRef` view (`types/view.rs`) that reads labels and properties in
place: scalars by value, strings as slices, other kinds decoded when asked for.
`node()` and `relationship()` decode a whole record into the structs below,
which are also what the write API, the WAL and snapshots exchange.

### Node record

```rust
struct NodeRecord {
    id: NodeId,           // u64, auto-incremented
    labels: Labels,       // trimmed, empty labels removed, duplicates removed
    properties: PropertyMap,
}
```

### Relationship record

```rust
struct RelationshipRecord {
    id: RelationshipId,   // u64, auto-incremented
    src: NodeId,          // source node
    dst: NodeId,          // destination node
    rel_type: Name,       // trimmed, non-empty, immutable
    properties: PropertyMap,
}
```

`Name` and `Labels` (`types/name.rs`) are interned: every record with the
label `User` points at one shared copy of the string, and a node with one
label stores it inline. Both compare with `&str` and `String` and serialize
like the `String` and `Vec<String>` they replaced.

`PropertyMap` (`types/property_map.rs`) is a key-sorted `Vec` with a
`BTreeMap`-shaped API: same iteration order and serde form, much smaller
per-bag overhead.

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

### Property and catalog-backed indexes

`InMemoryGraph` has lazy exact-match property indexes for nodes and
relationships. A call to `find_nodes_by_property` or
`find_relationships_by_property` builds the index for that property key the
first time it can be indexed, then keeps the active index current on future
mutations.

The exact-match index keeps one hash map per scope (label or relationship
type) and key: value to ids. An entity is listed under each of its labels, and
a node without labels under a scope of its own. A lookup that names a label
reads that label's map. The first lookup of a key that names none builds one
more map for that key across all scopes, kept current from then on and not
rebuilt after a restart until the next such lookup. A
sorted (RANGE) index keeps the same value-to-ids entries in sorted arrays of at
most 256 entries under two levels of tables.

An entry is a 32-byte key and a 16-byte id set, which holds a single id
inline. At 2M unique integer keys that comes to about 89 bytes per entry in the
hash index and 49 in the sorted one, so a RANGE index or uniqueness constraint
costs about 138 bytes per entry.

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

The explicit index catalog is separate from the lazy hash registry. `CREATE
INDEX` / `CREATE RANGE INDEX` records a RANGE definition and activates the
matching equality and sorted-property scopes. `CREATE TEXT INDEX` activates a
trigram candidate index. `CREATE POINT INDEX` activates a grid-bucket spatial
index. `CREATE LOOKUP INDEX` is catalog-only because label and relationship
type token indexes are always maintained.

Catalog entries are user-visible through `SHOW INDEXES`, participate in the
optimizer's cost model, and are durable through snapshots and WAL/archive
mutation events. Dropping a TEXT/RANGE/POINT catalog entry releases its
catalog-backed scope; the lazy equality buckets may remain available for
ordinary exact-match lookups.

### Adjacency indexes

Each node has two adjacency lists (`memory/adjacency.rs`):

- `outgoing[node_id]`: relationships leaving the node
- `incoming[node_id]`: relationships arriving at the node

An entry is `(relationship type, neighbour, relationship id)`, so a hop
filters by type and finds the far endpoint from the list alone, without
reading a relationship record. A list is a byte string of variable-width
entries: one header byte, then the type number and the two ids in as many
bytes as each needs. That is 8 bytes per entry while ids fit in three bytes
(16M nodes and relationships) and 10 bytes up to 4 billion. Two such entries
fit inline; longer lists spill to the heap. Relationship types are numbered
per graph by the type dictionary, the same numbers relationship records use.

Deleting a relationship removes its entry from both endpoint lists. Deleting a
node clears the node's lists; the outer adjacency vectors are not shrunk. A
list stores no length, so `degree` counts entries.

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

1. Resolve the requested relationship types to their numbers. A type no
   relationship ever had matches nothing.
2. Walk the entries of `outgoing`, `incoming`, or both, keeping those of a
   requested type. An undirected walk reports a self-loop once.
3. Yield `(relationship id, neighbour id)` straight from the entries.
4. The compatibility API then reads both records and returns
   `Vec<(RelationshipRecord, NodeRecord)>`. Hot executor paths stop at the ids.

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
- **Scoped constraint/index surface** — uniqueness, existence, type, key,
  RANGE, TEXT, POINT, LOOKUP, VECTOR, and FULLTEXT surfaces exist. Composite
  RANGE definitions are cataloged, but current optimizer rewrites target one
  property at a time. Vector indexes default to an exact flat scan; an
  approximate HNSW backend (`memory/hnsw.rs`) is opt-in per index with
  `OPTIONS {indexConfig: {`vector.indexProvider`: 'hnsw'}}`.
- **Clone compatibility APIs** — bulk read helpers allocate owned records even
  though executor hot paths avoid many clones.
- **Vectors cannot be stored inside list properties** — a vector can be a direct
  property or a value inside a top-level map property, but list-of-vector
  properties are rejected to preserve future indexing options.

## Durability

Snapshots are encoded by the `lora-snapshot` columnar codec. The current file
magic is `LORACOL1`; the envelope contains an explicit binary manifest, a
BLAKE3 checksum, and an optional compressed/encrypted body. `lora-database` writes snapshots via
an atomic `<path>.tmp` + rename protocol and publishes loaded snapshots by
replacing the `Arc` in the database's `RwLock<Arc<S>>` store holder.

The WAL is built on `MutationEvent`. When WAL is enabled, `InMemoryGraph` has a
`MutationRecorder`; writes are buffered into committed batches and replayed on
recovery. Named databases use the same WAL events with a `.loradb` container
mirror.

See [Snapshots](../operations/snapshots.md) and [WAL](../operations/wal.md) for
details, and the website's [WAL and Checkpoints](../../apps/loradb.com/docs/wal.md)
for operating it.

## Next steps

- How reads and writes flow through the engine: [Data Flow](data-flow.md)
- Value representation and property types: [Value Model](../internals/value-model.md)
- Known performance trade-offs: [Performance Notes](../performance/notes.md)
- Broader limitations and mitigations: [Known Risks](../design/known-risks.md)
- Durability, snapshots, WAL, and admin routes: [Snapshots](../operations/snapshots.md)
