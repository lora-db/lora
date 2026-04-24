---
title: Vector Values
sidebar_label: Vectors
description: First-class VECTOR values in LoraDB — construction in Cypher, property storage, binding round-trips, and the built-in vector math functions for exhaustive kNN retrieval.
---

# Vector Values

LoraDB treats `VECTOR` as a first-class value type. Vectors can be
constructed in Cypher, stored as node or relationship properties,
returned through every binding, and used as input to built-in vector
math functions for exhaustive kNN-style retrieval.

A `VECTOR` has three fixed attributes:

- a **dimension** in the range `1..=4096`;
- a **coordinate type** drawn from six canonical tags;
- a **values** array whose length always equals `dimension`.

:::info Scope
Vector **indexes** and approximate-nearest-neighbour search are **not
yet implemented**. Similarity and distance functions are exhaustive —
they score every matched candidate linearly. This is fine for demos,
tests, small corpora, and internal tools; it is not yet a substitute
for an index-backed vector search service at scale.

LoraDB also has no plugin system today, so there is no built-in
embedding generation. Produce embeddings in your application (hosted
API, local model, batch job) and pass them in as parameters.
:::

## Construction

```cypher
RETURN vector([1, 2, 3], 3, INTEGER) AS v
RETURN vector([1.05, 0.123, 5], 3, FLOAT32) AS v
RETURN vector($embedding, 384, FLOAT32) AS v
RETURN vector('[1.05e+00, 0.123, 5]', 3, FLOAT) AS v
```

`vector(value, dimension, coordinateType)` takes exactly three
arguments:

- **value** — `LIST<INTEGER | FLOAT>` **or** a string like
  `"[1, 2, 3]"` (numbers separated by commas, decimal or scientific
  notation). An empty string list `"[]"` parses to zero
  coordinates, which then has to match a zero-dimension declaration —
  but dimension `0` is rejected, so an empty vector is never
  constructible.
- **dimension** — an integer in `1..=4096`. Whole-number floats
  (`3.0`) are accepted for convenience; fractional floats error.
- **coordinateType** — one of the six canonical tags below, or any
  accepted alias.

Wrong arity is rejected at analysis time; anything outside the valid
ranges is rejected at evaluation time with a clear error.

### Coordinate types

| Canonical tag | Aliases accepted on input |
|---|---|
| `FLOAT64` | `FLOAT` |
| `FLOAT32` | — |
| `INTEGER` | `INT`, `INT64`, `INTEGER64`, `SIGNED INTEGER` *(as a string)* |
| `INTEGER32` | `INT32` |
| `INTEGER16` | `INT16` |
| `INTEGER8` | `INT8` |

Alias matching is case-insensitive and collapses runs of whitespace, so
`signed integer`, `SIGNED   INTEGER`, and `Signed  Integer` all
resolve to `INTEGER`. Output always emits the canonical tag.

`DOUBLE` is **not** accepted; typos surface as a clear
`unknown vector coordinate type '…'` error rather than silently
mapping to `FLOAT64`.

The third argument can be written as a bare identifier
(`vector([1,2,3], 3, INTEGER)`) or as a string literal
(`vector([1,2,3], 3, 'INTEGER')`). The analyzer rewrites a bare
identifier in this specific slot to a string literal, so normal
variable resolution is unaffected elsewhere in the query — a local
variable named `COSINE` or `INTEGER` outside the enum slot continues
to resolve normally. Because the multi-word alias `SIGNED INTEGER`
contains a space, it only works as a string:
`vector([1,2,3], 3, 'SIGNED INTEGER')`.

A parameter in the coordinate-type slot is preserved verbatim — the
analyzer does **not** rewrite `$type` to a string, so callers can
pass the coordinate type from host code:

```cypher
RETURN vector($values, 3, $type) AS v
```

### Coercion rules

- Integer inputs go into float-typed vectors unchanged (precision can
  degrade for very large magnitudes that don't fit in the float
  mantissa).
- Float inputs go into integer-typed vectors with truncation **toward
  zero** (`1.9 → 1`, `-1.9 → -1`, `0.999 → 0`, `-0.999 → 0`).
- Out-of-range values error loudly —
  `vector([128], 1, INT8)` is an error because `128` does not fit in
  `INTEGER8`; `vector([2e39], 1, FLOAT32)` is rejected because the
  value overflows `f32`; a float bigger than `i64::MAX` for an
  integer-backed vector errors rather than saturating.
- `NaN`, `Infinity`, and `-Infinity` coordinates are errors.
- Nested-list coordinates are errors (`vector([[1,2]], 1, INTEGER)`).
- Non-numeric coordinates are errors (`vector([1, 'two', 3], …)`).
- An unknown `coordinateType` is an error.

### Null propagation

- `vector(null, 3, FLOAT32)` returns `null`.
- `vector([1,2,3], null, INTEGER)` returns `null`.
- `vector([1], 1, null)` is an **error** — coordinate-type `null` is
  never ambiguous, so it's rejected loudly rather than hidden as
  `null`.

## Storage

Vectors can be stored directly as node or relationship properties:

```cypher
CREATE (:Doc {id: 1, embedding: vector([1, 2, 3], 3, INTEGER)})

MATCH (a:Doc {id: 1}), (b:Doc {id: 2})
CREATE (a)-[:SIM {score: vector([0.9, 0.1], 2, FLOAT32)}]->(b)

MATCH (d:Doc {id: 1}) SET d.embedding = vector([0.1, 0.2], 2, FLOAT32)
MATCH (d:Doc {id: 1}) SET d += {embedding: vector([1, 2], 2, FLOAT32)}
```

A vector is also a legal map value — a property map containing a
vector is stored intact:

```cypher
CREATE (:Doc {id: 1, meta: {embedding: vector([1, 2, 3], 3, INTEGER)}})
```

### Restriction: no list-of-vectors as a property

A list that contains a `VECTOR` (at any depth under the list, including
inside a map nested under the list) cannot be stored as a property.
The engine rejects the write at property-conversion time — this is a
shape decision, not an oversight:

```cypher
-- Rejected:
CREATE (:Doc {embeddings: [vector([1,2,3], 3, INTEGER)]})
CREATE (:Doc {meta: {embeddings: [vector([1,2,3], 3, INTEGER)]}})
```

If you need many embeddings per document, hang them off separate
nodes connected by a relationship, each with its own vector property
— that is also the shape a future vector index would be built on.

Lists of vectors are still perfectly legal **inside a query** (in a
`RETURN`, `WITH`, `UNWIND`, or `collect(...)`). The restriction applies
only to the write path.

## Exhaustive kNN

Until vector indexes land, approximate nearest-neighbour is expressed
as `ORDER BY … LIMIT k` over the full candidate set:

```cypher
MATCH (d:Doc)
RETURN d.id AS id
ORDER BY vector.similarity.cosine(d.embedding, $query) DESC
LIMIT 10
```

Or, using `WITH` to carry the score forward:

```cypher
MATCH (n:Node)
WITH n, vector.similarity.euclidean($query, n.vector) AS score
RETURN n.id AS id, score
ORDER BY score DESC
LIMIT 2
```

Every `MATCH` candidate is scored, so cost is `O(n)` in the number of
matched nodes. Fine for small datasets; for large corpora you'll want
a proper vector index (not yet implemented in LoraDB).

### Graph-filtered retrieval

The reason VECTOR lives next to the graph — score candidates by
similarity, then use relationships to explain or filter them:

```cypher
MATCH (d:Doc)
WITH d, vector.similarity.cosine(d.embedding, $query) AS score
MATCH (d)-[:MENTIONS]->(e:Entity)
WHERE e.type = $entity_type
RETURN d.id, d.title, score, collect(e.name) AS entities
ORDER BY score DESC
LIMIT 5
```

### Bulk insert

Vectors load efficiently through a single `UNWIND` over a parameter
list of maps. Each row becomes a standalone `CREATE`, so each vector
flows through property conversion as a *top-level* property (not a
list entry), and the property rule is satisfied by construction:

```cypher
UNWIND $batch AS row
CREATE (:Doc {id: row.id, title: row.title, embedding: row.embedding})
```

```ts
import { vector } from "@loradb/lora-node";

await db.execute(
  `UNWIND $batch AS row
   CREATE (:Doc {id: row.id, title: row.title, embedding: row.embedding})`,
  { batch: docs.map(d => ({
      id:        d.id,
      title:     d.title,
      embedding: vector(d.embedding, 384, "FLOAT32"),
    })) },
);
```

## Introspection

| Expression | Returns |
|---|---|
| `valueType(vector([1,2,3], 3, INTEGER))` | `"VECTOR<INTEGER>(3)"` |
| `size(vector([1,2,3,4], 4, FLOAT32))` | `4` (dimension) |
| `length(vector([1,2,3], 3, INTEGER))` | `3` (dimension) |
| `vector_dimension_count(vector([1,2,3], 3, INTEGER8))` | `3` |
| `toIntegerList(vector([1.9, -1.9, 3], 3, FLOAT32))` | `[1, -1, 3]` |
| `toFloatList(vector([1, 2, 3], 3, INT8))` | `[1.0, 2.0, 3.0]` |

`size` on a `VECTOR` returns its dimension — identical to
`vector_dimension_count` for convenience. `valueType` returns the
parameterised tag `VECTOR<TYPE>(DIMENSION)` so runtime inspection can
see both the coordinate type and the size.

`toIntegerList` on a float vector truncates toward zero (same rule as
`vector()` construction). Both converters propagate `null` and error
on non-`VECTOR` inputs.

## Similarity and distance

All similarity / distance functions use `f32` arithmetic internally
(values are converted from the underlying coordinate type into `f32`
before accumulation, then widened back to `f64` for the result).

### Bounded similarity in `[0, 1]`

```cypher
vector.similarity.cosine(a, b)
vector.similarity.euclidean(a, b)
```

Both accept a `VECTOR` **or** a `LIST<NUMBER>` on either side. Lists
are coerced on the fly to a `FLOAT32` vector whose dimension equals
the list's length. Higher = more similar.

- `cosine`: `(1 + raw_cosine) / 2`, so `1.0` is identical direction,
  `0.5` is orthogonal, `0.0` is opposite. A zero-norm vector returns
  `null` (cosine is undefined).
- `euclidean`: `1 / (1 + d²)`, where `d²` is the squared L2 distance.
  Documented example:
  `vector.similarity.euclidean([4,5,6], [2,8,3]) ≈ 0.04348`
  (because `d² = 2² + 3² + 3² = 22`, so `1 / 23 ≈ 0.0435`).

Both functions `null`-propagate: any `null` argument returns `null`.
A dimension mismatch is an error. An empty list on either side is an
error.

### Signed distance metrics

```cypher
vector_distance(a, b, EUCLIDEAN)
vector_distance(a, b, EUCLIDEAN_SQUARED)
vector_distance(a, b, MANHATTAN)
vector_distance(a, b, COSINE)
vector_distance(a, b, DOT)
vector_distance(a, b, HAMMING)
```

Both operands must be `VECTOR` values with matching dimensions — a
plain list is **rejected** here (unlike the similarity functions).
Smaller = more similar. Metric tokens may be passed as bare
identifiers or quoted strings; matching is case-insensitive.

| Metric | Result |
|---|---|
| `EUCLIDEAN` | `sqrt(Σ (aᵢ - bᵢ)²)` |
| `EUCLIDEAN_SQUARED` | `Σ (aᵢ - bᵢ)²` |
| `MANHATTAN` | `Σ |aᵢ - bᵢ|` |
| `COSINE` | `1 - raw_cosine(a, b)` (raw cosine, **not** the bounded variant) |
| `DOT` | `-(a · b)` — negated so "smaller = more similar" holds |
| `HAMMING` | count of positions where `aᵢ` and `bᵢ` differ (`f32` comparison) |

`null` on either vector or the metric returns `null`. An unknown
metric name is an error. A dimension mismatch is an error.

### Vector norms

```cypher
vector_norm(v, EUCLIDEAN)   -- sqrt(Σ xᵢ²)
vector_norm(v, MANHATTAN)   -- Σ |xᵢ|
```

Metric matching is case-insensitive; identifiers and quoted strings
both work. `null` vector or `null` metric returns `null`. Unknown
metric errors.

## Equality, DISTINCT, and ordering

- **Equality** compares coordinate type, dimension, and every value.
  `vector([1,2,3], 3, INTEGER) = vector([1,2,3], 3, INTEGER8)` is
  `false` — different coordinate types are never equal, even with
  numerically identical values.
- **`DISTINCT`** uses a stable key (coordinate type + dimension +
  stringified values) so duplicates collapse across projection and
  pipeline stages. Vectors of different coord types never collapse.
- **`ORDER BY`** on a vector column is accepted and runs without
  panicking, but the ordering is implementation-defined — use it for
  tie-breaking, not primary sort. Order by a scalar score
  (`vector.similarity.cosine(...)`) when intent matters.

## Passing vectors as parameters

Every binding accepts the same canonical tagged shape on input and
emits it on output. Build one via the helper for your language, or
pass the literal object directly. The **HTTP transport does not yet
forward parameters** — see the limitations section below.

### Wire shape

```json
{
  "kind": "vector",
  "dimension": 3,
  "coordinateType": "FLOAT32",
  "values": [0.1, 0.2, 0.3]
}
```

Integer-backed vectors deserialise to integers in the `values` array;
float-backed vectors deserialise to numbers that may be fractional.
`INTEGER8` / `INTEGER16` / `INTEGER32` / `FLOAT32` all widen to a
wider native type on the wire — the underlying storage stays narrow,
the JSON form uses the nearest lossless widening.

### Binding helpers

| Language | Constructor | Guard |
|---|---|---|
| TypeScript / Node | `vector(values, dimension, coordinateType)` | `isVector(v)` |
| TypeScript / WASM | `vector(values, dimension, coordinateType)` | `isVector(v)` |
| Python | `vector(values, dimension, coordinate_type)` | `is_vector(v)` |
| Go | `lora.Vector(values, dimension, coordinateType)` | `lora.IsVector(v)` |
| Ruby | `LoraRuby.vector(values, dimension, coordinate_type)` | `LoraRuby.vector?(v)` |
| Rust | `LoraVector::try_new(raw, dimension, coordinateType)` → `LoraValue::Vector(_)` | match on `LoraValue::Vector(_)` |

The Go, Ruby, and Python helpers additionally ship coordinate-type
constants (`VectorCoordTypeFloat32`, `VECTOR_COORD_TYPES`,
`LoraVectorCoordinateType`) to avoid typing the string literals by
hand.

### Node / TypeScript

```ts
import { vector } from "@loradb/lora-node";

const query = vector([0.1, 0.2, 0.3], 3, "FLOAT32");

await db.execute(
  `MATCH (d:Doc)
   RETURN d.id AS id
   ORDER BY vector.similarity.cosine(d.embedding, $q) DESC
   LIMIT 10`,
  { q: query },
);
```

### Python

```python
from lora_python import Database, vector

db = Database.create()
q = vector([0.1, 0.2, 0.3], 3, "FLOAT32")

db.execute(
    "RETURN vector.similarity.cosine($a, $b) AS s",
    {"a": q, "b": [0.1, 0.2, 0.3]},   # list is coerced to FLOAT32
)
```

### Go

```go
q := lora.Vector(
    []any{0.1, 0.2, 0.3},
    3,
    lora.VectorCoordTypeFloat32,
)
db.Execute(
    "MATCH (d:Doc) RETURN d.id ORDER BY vector.similarity.cosine(d.embedding, $q) DESC LIMIT 10",
    lora.Params{"q": q},
)
```

### Ruby

```ruby
require "lora_ruby"

q = LoraRuby.vector([0.1, 0.2, 0.3], 3, "FLOAT32")
db.execute(
  "MATCH (d:Doc) RETURN d.id ORDER BY vector.similarity.cosine(d.embedding, $q) DESC LIMIT 10",
  { q: q },
)
```

### Rust

```rust
use lora_database::Database;
use lora_store::{LoraVector, RawCoordinate, VectorCoordinateType};
use lora_database::LoraValue;
use std::collections::BTreeMap;

let mut params = BTreeMap::new();
params.insert(
    "q".into(),
    LoraValue::Vector(
        LoraVector::try_new(
            vec![
                RawCoordinate::Float(0.1),
                RawCoordinate::Float(0.2),
                RawCoordinate::Float(0.3),
            ],
            3,
            VectorCoordinateType::Float32,
        )?,
    ),
);

db.execute_with_params(
    "MATCH (d:Doc) RETURN d.id ORDER BY vector.similarity.cosine(d.embedding, $q) DESC LIMIT 10",
    None,
    params,
)?;
```

### HTTP

`POST /query` does **not** yet accept a `params` field, so a vector
cannot be passed through as a parameter over HTTP. Either embed the
vector literally in the query string —

```bash
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query":"RETURN vector([0.1,0.2,0.3], 3, FLOAT32) AS v"}'
```

— or run the engine through one of the in-process bindings above
when you need host-side vectors. See
[Queries → Parameters → HTTP API doesn't forward params](../queries/parameters#http-api-doesnt-forward-params).

## Limitations

- **Vector indexes — not yet supported.** Every similarity /
  distance call scans every candidate linearly.
- **Approximate nearest-neighbour (ANN) — not yet supported.**
  Implied by the above: retrieval is exhaustive today.
- **Embedding generation — not supported.** LoraDB has no plugin
  surface; generate embeddings in application code.
- **List-of-vectors as a property — not supported.** See
  [Storage](#restriction-no-list-of-vectors-as-a-property).
- **Parameters over HTTP — not yet supported.** `POST /query`
  ignores `params`; embed the vector in the query text or use an
  in-process binding.
- **Dimension ≤ 4096.** Hard cap at construction time.
- **Ordering by a vector column is unspecified.** Order by a scalar
  score instead.

See also the [Cypher support matrix (§13a)](https://github.com/lora-db/lora/blob/main/docs/reference/cypher-support-matrix.md#13a-vector-types-and-functions)
for the engine-side behaviour grid.

## See also

- [Functions → Overview](../functions/overview) — includes vector functions.
- [Queries → Parameters](../queries/parameters#semantic-retrieval-with-a-vector-parameter)
  — passing vectors as parameters.
- [Cookbook → Vector-retrieval patterns](../cookbook#vector-retrieval-patterns)
  — top-k and graph-filtered retrieval recipes.
- [Limitations → Vectors](../limitations#vectors) — what's not implemented yet.
- Internal [value model](https://github.com/lora-db/lora/blob/main/docs/internals/value-model.md#vectors)
  — engine-side shape and conversion rules for future contributors.

### Background reading

- [**Vectors belong next to relationships**](/blog/vectors-belong-next-to-relationships)
  — why similarity lives as a value type instead of in a sidecar store.
- [**LoraDB v0.2: vector values for connected AI context**](/blog/loradb-v0-2-vectors)
  — the release that introduced `VECTOR`.
