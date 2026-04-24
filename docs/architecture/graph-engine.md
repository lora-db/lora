# Graph Architecture

## Storage engine design

The graph is stored entirely in process memory using a `BTreeMap`-backed implementation (`InMemoryGraph`). The design prioritizes simplicity and correctness over throughput.

## Core data structures

```
InMemoryGraph
├── nodes:              BTreeMap<NodeId, NodeRecord>
├── relationships:      BTreeMap<RelationshipId, RelationshipRecord>
├── outgoing:           BTreeMap<NodeId, BTreeSet<RelationshipId>>     # adjacency out
├── incoming:           BTreeMap<NodeId, BTreeSet<RelationshipId>>     # adjacency in
├── nodes_by_label:     BTreeMap<String, BTreeSet<NodeId>>    # label index
├── relationships_by_type: BTreeMap<String, BTreeSet<RelationshipId>>  # type index
├── next_node_id:       u64                                    # monotonic counter
└── next_rel_id:        u64                                    # monotonic counter
```

### Node record

```rust
struct NodeRecord {
    id: NodeId,           // u64, auto-incremented
    labels: Vec<String>,  // zero or more (deduplicated on create)
    properties: BTreeMap<String, PropertyValue>,
}
```

### Relationship record

```rust
struct RelationshipRecord {
    id: RelationshipId,            // u64, auto-incremented
    src: NodeId,          // source node
    dst: NodeId,          // destination node
    rel_type: String,     // exactly one type (trimmed, non-empty)
    properties: BTreeMap<String, PropertyValue>,
}
```

### Property values

```rust
enum PropertyValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<PropertyValue>),
    Map(BTreeMap<String, PropertyValue>),
    Date(LoraDate),
    Time(LoraTime),
    LocalTime(LoraLocalTime),
    DateTime(LoraDateTime),
    LocalDateTime(LoraLocalDateTime),
    Duration(LoraDuration),
    Point(LoraPoint),
}
```

Temporal and spatial types are first-class property values — they can be stored on nodes and relationships, compared, and used in expressions. Definitions live in `lora-store/src/temporal.rs` and `lora-store/src/spatial.rs`.

## Index structures

### Label index

Maps label names to the set of node IDs carrying that label. Updated on node creation, `add_node_label`, `remove_node_label`, and node deletion.

```
"User"    -> {0, 1, 3, 5}
"Admin"   -> {0}
"Company" -> {2}
```

### Relationship type index

Maps type names to the set of relationship IDs with that type. Updated on relationship creation and deletion.

```
"FOLLOWS"  -> {0, 1, 2}
"KNOWS"    -> {3, 4}
```

### Adjacency indexes

Two separate indexes for directed traversal:

- **outgoing**: `BTreeMap<NodeId, BTreeSet<RelationshipId>>` -- relationships leaving a node
- **incoming**: `BTreeMap<NodeId, BTreeSet<RelationshipId>>` -- relationships arriving at a node

Both are updated on relationship creation and deletion.

## ID allocation

Node and relationship IDs are allocated sequentially from monotonic counters. IDs are never reused after deletion.

```
next_node_id: 0 -> 1 -> 2 -> ...
next_rel_id:  0 -> 1 -> 2 -> ...
```

**Implication**: after deleting node 3 and creating a new node, the new node gets ID `next_node_id` (not 3). This avoids stale reference issues but means IDs are not contiguous after deletions.

## Traversal operations

### Expand

The core traversal primitive. Given a source node, direction, and optional type filter:

1. Look up relationship IDs from the adjacency index (outgoing, incoming, or both)
2. Filter by relationship type if types are specified
3. For each matching relationship, resolve the other endpoint node
4. Return `Vec<(RelationshipRecord, NodeRecord)>`

The `InMemoryGraph` overrides the default `expand` implementation for efficiency, using `BTreeSet`-based lookups instead of scanning all relationships.

### Direction handling

```
Direction::Right     -> outgoing adjacency
Direction::Left      -> incoming adjacency
Direction::Undirected -> union of both
```

## Write operations

### Node creation

1. Allocate `NodeId`
2. Normalize labels (trim, deduplicate, remove empty)
3. Insert `NodeRecord`
4. Update label index for each label
5. Initialize empty adjacency entries

### Relationship creation

1. Validate both endpoints exist
2. Validate type is non-empty
3. Allocate `RelationshipId`
4. Insert `RelationshipRecord`
5. Update outgoing adjacency for source
6. Update incoming adjacency for destination
7. Update type index

### Node deletion

- **Plain delete** (`delete_node`): fails if the node has any incident relationships (outgoing or incoming)
- **Detach delete** (`detach_delete_node`): first deletes all incident relationships, then the node

### Property mutation

- `set_node_property` / `set_relationship_property`: insert or update a single key
- `remove_node_property` / `remove_relationship_property`: remove a single key
- `replace_node_properties`: clear all properties, then set new ones
- `merge_node_properties`: set new properties without removing existing ones
- `add_node_label` / `remove_node_label`: modify labels with index maintenance

## Storage trait hierarchy

The storage API is split into a small set of required primitives plus a large
defaulted helper surface. A new backend only has to implement the required
primitives; everything else is derived.

```rust
trait GraphStorage {
    // --- Required primitives (backend-neutral) ---
    fn contains_node(&self, id) -> bool;
    fn node(&self, id) -> Option<NodeRecord>;      // owned point lookup
    fn all_node_ids(&self) -> Vec<NodeId>;
    fn node_ids_by_label(&self, label) -> Vec<NodeId>;

    fn contains_relationship(&self, id) -> bool;
    fn relationship(&self, id) -> Option<RelationshipRecord>;
    fn all_rel_ids(&self) -> Vec<RelationshipId>;
    fn rel_ids_by_type(&self, rel_type) -> Vec<RelationshipId>;
    fn relationship_endpoints(&self, id) -> Option<(NodeId, NodeId)>;

    fn expand_ids(&self, node, direction, types) -> Vec<(RelationshipId, NodeId)>;

    fn all_labels(&self) -> Vec<String>;
    fn all_relationship_types(&self) -> Vec<String>;

    // --- Optional optimization hooks (default: clone through `node` / `relationship`) ---
    fn with_node<F, R>(&self, id, f: F) -> Option<R>           where F: FnOnce(&NodeRecord) -> R;
    fn with_relationship<F, R>(&self, id, f: F) -> Option<R>   where F: FnOnce(&RelationshipRecord) -> R;

    // --- Defaulted helpers (override for perf) ---
    //   all_nodes / nodes_by_label / all_relationships / relationships_by_type
    //   outgoing_relationships / incoming_relationships / expand / degree / ...
    //   node_has_label / node_labels / node_property / relationship_type / ...
    //   has_label_name / has_property_key / find_nodes_by_property / ...
}

trait GraphCatalog {
    // Narrow schema slice. Blanket-implemented for every `GraphStorage`.
    // The analyzer bounds on this, not on the full `GraphStorage` surface.
    fn node_count(&self) -> usize;
    fn relationship_count(&self) -> usize;
    fn has_label_name(&self, label) -> bool;
    fn has_relationship_type_name(&self, rel_type) -> bool;
    fn has_property_key(&self, key) -> bool;
}

trait BorrowedGraphStorage: GraphStorage {
    // Optional capability for backends that keep owned records in
    // long-lived, addressable storage (e.g. `InMemoryGraph`). Exposes
    // `&NodeRecord` / `&RelationshipRecord`. The executor does not require
    // this trait — it uses `with_node` / `with_relationship` on hot paths so
    // a non-borrow backend is a first-class citizen.
    fn node_ref(&self, id) -> Option<&NodeRecord>;
    fn relationship_ref(&self, id) -> Option<&RelationshipRecord>;
}

trait GraphStorageMut: GraphStorage {
    // Create / mutate / delete plus an admin `clear()` hook for resets.
    // Snapshot / WAL / restore entry points will land here next.
}
```

**Why the layering:**

1. `GraphStorage` is the single umbrella a backend implements. All
   record-returning scans (`all_nodes` etc.) default through the id-scan +
   point-lookup primitives, so a minimal backend only needs the required ones.
2. `GraphCatalog` lets the analyzer depend on a ~5-method slice instead of the
   full storage surface. Every `GraphStorage` gets `GraphCatalog` for free.
3. `BorrowedGraphStorage` is opt-in. The executor pivots on `with_node` /
   `with_relationship` closures, so backends that cannot hand out `&NodeRecord`
   (disk-backed, column-oriented, remote) are not forced to fake it.
4. `GraphStorageMut` adds write + lifecycle hooks. `clear()` is the first
   admin method — future snapshot/restore/WAL work will land here without a
   second round of restructuring.

`InMemoryGraph` implements all four traits and overrides the optimization
hooks plus the bulk-scan methods for zero-clone access.

## Limitations (observed)

- **No property indexes** -- equality lookups on properties require full scans (filtered by label when possible via `find_nodes_by_property`)
- **No uniqueness constraints** -- nothing prevents duplicate nodes with identical labels and properties
- **BTreeMap overhead** -- ordered maps are used everywhere, which has higher constant factors than `HashMap` for unordered access; provides deterministic iteration order
- **Full cloning on bulk reads** -- `all_nodes()`, `nodes_by_label()`, and the other record-returning scans still allocate a `Vec<NodeRecord>`. Hot-path executor code has been migrated to `with_node` / `with_relationship` closures, which avoid the clone on `InMemoryGraph`; bulk-scan APIs remain clone-based
- **No compaction** -- deleted IDs leave gaps in the ID space, adjacency maps may retain empty entries

> 🚀 **Production note** — These limits are fine for local development, tests, and modest embedded graphs. Property indexes, uniqueness constraints, and compaction are handled automatically in the [LoraDB managed platform](https://loradb.com) — reach for it once your workload outgrows a single in-memory process.

## Durability

`InMemoryGraph` implements the [`Snapshotable`] trait: it can serialize its
full state (nodes, relationships, ID counters) to a byte stream and restore
from one. The file format is a fixed-size header (magic + version + a
reserved `wal_lsn` field) followed by a bincode-serialized payload and a
CRC32 trailer. Adjacency and label/type indexes are rebuilt on load rather
than stored.

Alongside snapshots, every mutation fires a `MutationEvent` at an optional
`MutationRecorder`. The recorder is `None` by default, so zero-WAL
workloads pay only a null-pointer check per mutation. The enum covers every
`GraphStorageMut` method and is the vocabulary a future WAL will use. See
[Snapshots](../operations/snapshots.md) for the operator-facing
documentation.

## Next steps

- How reads and writes flow through the engine: [Data Flow](data-flow.md)
- Value representation and property types: [Value Model](../internals/value-model.md)
- Known performance trade-offs: [Performance Notes](../performance/notes.md)
- Broader limitations and mitigations: [Known Risks](../design/known-risks.md)
- Durability, snapshots, and the admin surface: [Snapshots](../operations/snapshots.md)
