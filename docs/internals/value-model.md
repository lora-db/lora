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

`LoraVector` is a first-class property and query value defined in
`lora-store/src/vector.rs`. Every binding speaks the same canonical
tagged shape on the wire; the engine carries the narrow typed storage
internally.

### Shape

```rust
pub struct LoraVector {
    pub dimension: usize,
    pub values: VectorValues,
}

pub enum VectorValues {
    Float64(Vec<f64>),
    Float32(Vec<f32>),
    Integer64(Vec<i64>),
    Integer32(Vec<i32>),
    Integer16(Vec<i16>),
    Integer8(Vec<i8>),
}
```

| Invariant | Enforced by |
|---|---|
| `dimension` is in `1..=4096` (`MAX_VECTOR_DIMENSION`) | `LoraVector::try_new` |
| `values.len() == dimension` | `LoraVector::try_new` (`DimensionMismatch`) |
| No `NaN` or `±Infinity` in float-backed coordinates | `LoraVector::try_new` |
| Float→int coercion truncates toward zero | `LoraVector::try_new` |
| Integer coord that overflows the target width errors | `LoraVector::try_new` (`OutOfRange`) |

### Coordinate-type tags

| Canonical tag (`VectorCoordinateType::as_str`) | Storage | Aliases accepted by `parse` |
|---|---|---|
| `FLOAT64` | `f64` | `FLOAT`, `FLOAT64` |
| `FLOAT32` | `f32` | `FLOAT32` |
| `INTEGER` | `i64` | `INTEGER`, `INT`, `INT64`, `INTEGER64`, `SIGNED INTEGER` |
| `INTEGER32` | `i32` | `INTEGER32`, `INT32` |
| `INTEGER16` | `i16` | `INTEGER16`, `INT16` |
| `INTEGER8` | `i8` | `INTEGER8`, `INT8` |

`VectorCoordinateType::parse` is case-insensitive and collapses
whitespace runs (`SIGNED   INTEGER` → canonicalised to `SIGNED INTEGER`
before matching). `DOUBLE` is deliberately rejected so typos surface as
`UnknownCoordinateType`.

### Wire format

Every binding serialises a vector to this exact JSON shape
(`lora-node/src/lib.rs::vector_to_json`,
`lora-wasm/src/lib.rs::vector_to_json`,
`lora-python/src/lib.rs::vector_to_py`,
`lora-ruby/src/lib.rs::vector_to_ruby`,
`lora-ffi/src/lib.rs::vector_to_json`,
`lora-executor/src/value.rs::serialize_vector` — all in lockstep):

```json
{
  "kind": "vector",
  "dimension": 3,
  "coordinateType": "FLOAT32",
  "values": [0.1, 0.2, 0.3]
}
```

Narrow storage widens for the wire: `INTEGER8` / `INTEGER16` /
`INTEGER32` widen to `i64` in the `values` array; `FLOAT32` widens to
`f64`. `INTEGER64` and `FLOAT64` are serialised at full width.

Parameter decoding is also shared — every binding calls a private
`vector_from_json_map` that rebuilds a `LoraVector` through
`LoraVector::try_new`, so the exact same validation rules apply to
host-built vectors as to Cypher-built ones.

### Conversion rules

- `VectorValues::as_f64_vec` — lossless widen to `Vec<f64>`, used by
  every math function so the f32 accumulator can run irrespective of
  the underlying storage.
- `VectorValues::to_i64_vec` — truncate-toward-zero on floats, pass
  through on ints; matches the `toIntegerList(vector)` semantics.
- `RawCoordinate::Int(i64)` / `RawCoordinate::Float(f64)` — the single
  entry point for user-supplied coordinates. The executor
  (`eval.rs::coerce_list_to_raw_coords`) and every binding funnel
  values through `RawCoordinate` so the coercion rules live in one
  place.
- `parse_string_values` — parses `vector("[1, 2, 3]", 3, INT)` style
  strings. Integer-looking tokens are preferred over float parsing so
  integer-backed vectors don't truncate unnecessarily.

### Storage semantics

`PropertyValue::Vector(LoraVector)` is the storage-layer variant, so a
`VECTOR` can live on a node or relationship property directly.

The write path goes through `lora_value_to_property` in
`lora-executor/src/value.rs`:

```rust
fn visit(value: &LoraValue, inside_list: bool) -> Result<(), PropertyConversionError> {
    match value {
        LoraValue::Vector(_) if inside_list => Err(NestedVectorInList),
        LoraValue::List(items)  => items.iter().try_for_each(|i| visit(i, true)),
        LoraValue::Map(m)       => m.values().try_for_each(|v| visit(v, inside_list)),
        _                       => Ok(()),
    }
}
```

Effect: a `VECTOR` immediately inside a `List` is rejected; so is a
`VECTOR` nested inside a `Map` that is itself inside a `List`
(`inside_list` is preserved when recursing through maps). A `VECTOR`
inside a `Map` that is itself a top-level property is allowed — a
map of vectors, or a map containing a vector, is a valid property.

Every write site — `CREATE`, `MERGE`, `SET`, `SET +=`, `SET n = {...}`
— eventually calls `lora_value_to_property`, so the check applies
uniformly.

Lists of vectors remain perfectly valid **inside queries**; the
property-storage rule is the only place the engine rejects them. A
bulk insert via `UNWIND $batch AS row CREATE (...)` works because each
row is unpacked and each `VECTOR` flows through the property converter
as a standalone value, not as a list entry.

### Equality, grouping, and ordering

- Equality is defined on `PartialEq` over `LoraVector` — coordinate
  type, dimension, and every value must match.
  `vector([1,2,3], 3, INTEGER) = vector([1,2,3], 3, INTEGER8)` is
  `false` because the coordinate types differ.
- `LoraVector::to_key_string` returns
  `"{coordinateType}|{dimension}|v0,v1,…"` (values rendered via `{:?}`
  so NaN/Inf encodings don't collide with finite values). This key
  drives `DISTINCT`, `UNION`, and grouping — DISTINCT therefore
  collapses equal vectors and preserves differences in coordinate
  type.
- `ORDER BY` on a vector column runs deterministically but the order
  is implementation-defined. Primary sort should be a scalar score
  (`vector.similarity.cosine(...)`); vector columns serve as
  tie-breakers only.

### Function surface

All analyzed in `lora-analyzer/src/analyzer.rs` (`KNOWN_FUNCTIONS`,
`function_arity`, `try_vector_enum_literal`) and executed in
`lora-executor/src/eval.rs`:

| Cypher | Arity | Notes |
|---|---|---|
| `vector(value, dimension, coordinateType)` | 3 | value: `LIST<NUMBER>` or `STRING`; dimension: integer (or whole-number float); coordinate type: string, bare identifier (rewritten), or `$param` (preserved). Null value/dimension → null; null coordinate type → error. |
| `vector.similarity.cosine(a, b)` | 2 | Accepts `VECTOR` or `LIST<NUMBER>` on either side; list is coerced to a `FLOAT32` vector of matching dimension. Zero-norm vector → `null`. Bounded to `[0, 1]`: `(1 + raw_cosine) / 2`. |
| `vector.similarity.euclidean(a, b)` | 2 | Same input acceptance. Returns `1 / (1 + d²)`. |
| `vector_distance(a, b, metric)` | 3 | Both sides must be `VECTOR`; plain list is rejected. Metric: `EUCLIDEAN`, `EUCLIDEAN_SQUARED`, `MANHATTAN`, `COSINE` (= `1 - raw_cosine`), `DOT` (= `-(a·b)`), `HAMMING`; case-insensitive; bare identifier or quoted string. |
| `vector_norm(v, metric)` | 2 | `EUCLIDEAN` or `MANHATTAN`. |
| `vector_dimension_count(v)` | 1 | Returns `dimension`. |
| `size(v)` / `length(v)` | 1 | Same as `vector_dimension_count` for a `VECTOR` input; dispatch lives in the generic `size`/`length` handler in `eval.rs`. |
| `valueType(v)` | 1 | Returns `"VECTOR<COORD>(N)"`. |
| `toIntegerList(v)` | 1 | Rejects non-vector; float coordinates truncate toward zero. |
| `toFloatList(v)` | 1 | Rejects non-vector. |

All similarity/distance math uses `f32` arithmetic internally, widened
back to `f64` for the result — this matches the reference spec and
keeps the numerics predictable across storage types.

### Parser / analyzer quirks

- **No `vector` keyword.** `vector(...)` is parsed as an ordinary
  function call; the special-casing lives entirely in the analyzer.
- **Bare-identifier enum slots.**
  `try_vector_enum_literal(fn_name, arg_idx, expr)` rewrites a bare
  `Expr::Variable` to `ResolvedExpr::Literal(LiteralValue::String(...))`
  in three places only:
  | Function | Slot |
  |---|---|
  | `vector` | argument 2 (coordinate type) |
  | `vector_distance` | argument 2 (metric) |
  | `vector_norm` | argument 1 (metric) |
  Every other slot falls through to normal variable resolution — so a
  local variable `COSINE` coming out of an `UNWIND` binds normally
  when it's not in an enum slot.
- **Parameters in enum slots are preserved.** A `$param` in an enum
  slot is **not** rewritten — it flows through as `Parameter("type")`
  so callers can pass the coordinate type / metric from host code.
- **Quoted strings pass through unchanged** in enum slots, so
  `vector([1,2,3], 3, 'SIGNED INTEGER')` works the same as
  `vector([1,2,3], 3, INTEGER)` where the alias has no space.
- **Arity is enforced.** `function_arity` pins every vector function
  to its exact count; wrong arity produces `SemanticError::WrongArity`
  at analysis time.
- **Unknown `vector.similarity.*` names are caught.** `vector.bogus(...)`
  and `vector.similarity.manhattan(...)` both fail
  `validate_function_name` with `SemanticError::UnknownFunction`.

### What's not implemented

- **Vector indexes** (`CREATE VECTOR INDEX`) and any ANN execution
  path.
- **Built-in embedding generation** — no plugin system.
- **Extra metrics** beyond those listed above. Adding one is
  mechanical: implement in `vector.rs`, wire into
  `eval_vector_distance_fn` / `eval_vector_norm_fn`, add a test in
  `crates/lora-database/tests/vectors.rs`.
- **HTTP parameter forwarding.** `lora-server` does not accept a
  `params` body — vectors passed over HTTP have to be embedded as
  `vector(...)` literals inside the query string.

## Schema validation

Lora is schema-free — labels, relationship types, and property keys are
created implicitly on write. The analyzer performs soft validation only:

| Context | Labels | Rel types | Properties |
|---|---|---|---|
| `MATCH` | Must exist in graph (unless graph is empty) | Same | Same |
| `CREATE` / `MERGE` / `SET` | Any name allowed | Any name allowed | Any name allowed |

This means the first write to an empty graph can use any names, and `MATCH`
queries against those names succeed once data exists.
