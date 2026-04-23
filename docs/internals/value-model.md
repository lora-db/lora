# Value Model

This document describes the internal representation of values in the storage
and executor layers. User-facing type documentation lives on the docs site
under **Data Types**.

## Storage-layer value type

Defined in `lora-store` as `PropertyValue`. Every property stored on a node or
relationship uses this enum:

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
    Vector(LoraVector),
}
```

Notes:

- Storage does **not** carry graph-entity references — those only exist at the
  executor level.
- `Map` is `BTreeMap` for deterministic serialisation order.
- Temporal and spatial types are defined in `lora-store/src/temporal.rs` and
  `lora-store/src/spatial.rs` respectively.

## Executor value type

`LoraValue` extends `PropertyValue` with three graph references:

```rust
enum LoraValue {
    // …all PropertyValue variants…
    Node(NodeId),
    Relationship(RelationshipId),
    Path(LoraPath),
}
```

`Node`, `Relationship`, and `Path` are hydrated on the way out of the engine:

- `Node`           → `{kind, id, labels, properties}`
- `Relationship`   → `{kind, id, startId, endId, type, properties}`
- `Path`           → alternating sequence of hydrated nodes and relationships

The JSON field names (`startId`, `endId`) come from `serde(rename)` on
the internal `HydratedRelationship` struct — the in-Rust `NodeId`
fields are still called `src` / `dst` on `RelationshipRecord`
(see below).

Hydration happens at the serialisation boundary (HTTP / FFI), so internal
planner and executor code can cheaply hand around `Node(id)` references.

## Node record

| Field | Type | Notes |
|---|---|---|
| `id` | `u64` (`NodeId`) | Auto-increment, immutable, never reused |
| `labels` | `Vec<String>` | Deduplicated on creation; empty strings stripped |
| `properties` | `BTreeMap<String, PropertyValue>` | Ordered |

## Relationship record

| Field | Type | Notes |
|---|---|---|
| `id` | `u64` (`RelationshipId`) | Auto-increment, immutable, never reused |
| `src`, `dst` | `NodeId` | Must exist at creation |
| `rel_type` | `String` | Non-empty after trimming, case-sensitive, immutable |
| `properties` | `BTreeMap<String, PropertyValue>` | Ordered |

## In-memory indexes

Maintained by `InMemoryGraph`:

| Index | Structure | Used for |
|---|---|---|
| Label index | `BTreeMap<String, BTreeSet<NodeId>>` | `MATCH (n:Label)` |
| Rel-type index | `BTreeMap<String, BTreeSet<RelationshipId>>` | Type-filtered scans / expands |
| Outgoing adjacency | `BTreeMap<NodeId, BTreeSet<RelationshipId>>` | Right / undirected expand |
| Incoming adjacency | `BTreeMap<NodeId, BTreeSet<RelationshipId>>` | Left / undirected expand |

No property index, uniqueness constraint, or composite index is implemented.
Property filters without a label are `O(n)` full scans.

## Spatial points

| SRID | System | Components |
|---|---|---|
| 7203 | Cartesian 2D | `x`, `y` |
| 9157 | Cartesian 3D | `x`, `y`, `z` |
| 4326 | WGS-84 geographic 2D | `longitude`, `latitude` |
| 4979 | WGS-84 geographic 3D | `longitude`, `latitude`, `height` |

`distance(a, b)` is Euclidean for Cartesian and Haversine (Earth radius
6 371 km) for WGS-84. Cross-SRID `distance` returns `null`. WGS-84-3D
`distance` ignores height today — great-circle only.

## Vectors

`LoraVector` is a first-class property and query value (`lora-store/src/vector.rs`):

| Field | Type | Notes |
|---|---|---|
| `dimension` | `usize` | `1..=4096` |
| `values` | typed `Vec` | `Float64` / `Float32` / `Integer64` / `Integer32` / `Integer16` / `Integer8` |

On the wire every vector is serialised as
`{kind:"vector", dimension, coordinateType, values}`. Coordinate types on
output always use the canonical tag (`FLOAT64`, `FLOAT32`, `INTEGER`,
`INTEGER32`, `INTEGER16`, `INTEGER8`); input accepts aliases (`FLOAT`,
`INT`, `INT64`, `INT32`, `INT16`, `INT8`, `SIGNED INTEGER`).

Lists containing VECTOR values cannot be stored as properties — the
write-path converts `LoraValue` → `PropertyValue` fallibly via
`lora_value_to_property` and rejects nested vectors loudly.

Vector indexes, approximate kNN, and built-in embedding/plugin
integration are **not yet implemented**. Exhaustive kNN today is
expressed with `ORDER BY vector.similarity.cosine(v, $q) DESC LIMIT k`
— it scans every candidate linearly, so it's only suitable for small
datasets until an index-backed variant lands.

## Schema validation

Lora is schema-free — labels, relationship types, and property keys are
created implicitly on write. The analyzer performs soft validation only:

| Context | Labels | Rel types | Properties |
|---|---|---|---|
| `MATCH` | Must exist in graph (unless graph is empty) | Same | Same |
| `CREATE` / `MERGE` / `SET` | Any name allowed | Any name allowed | Any name allowed |

This means the first write to an empty graph can use any names, and `MATCH`
queries against those names succeed once data exists.
