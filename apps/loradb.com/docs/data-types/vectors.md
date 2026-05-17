---
title: Vector Values
sidebar_label: Vectors
description: First-class VECTOR values in LoraDB — construction in Cypher, property storage, binding round-trips, vector indexes, and built-in vector math functions.
---

# Vector Values

LoraDB treats `VECTOR` as a first-class value type. Vectors can be
constructed in Cypher, stored as node or relationship properties,
returned through every binding, indexed through `CREATE VECTOR INDEX`,
and used as input to built-in vector math functions.

A `VECTOR` has three fixed attributes:

- a **dimension** in the range `1..=4096`;
- a **coordinate type** drawn from six canonical tags;
- a **values** array whose length always equals `dimension`.

:::info Scope
`CREATE VECTOR INDEX` and `db.index.vector.queryNodes` /
`queryRelationships` are supported. The current query procedure uses
the index definition for scope, dimensions, and scoring, then performs
a flat scan over matching entities. Approximate nearest-neighbour
structures such as HNSW are not implemented yet.

LoraDB also has no plugin system today, so there is no built-in
embedding generation. Produce embeddings in your application (hosted
API, local model, batch job) and pass them in as parameters.
:::

## Construction

<QueryCodeBlock code={String.raw`RETURN [1, 2, 3]::VECTOR<INTEGER>(3) AS v;
RETURN [1.05, 0.123, 5]::VECTOR<FLOAT32>(3) AS v;
RETURN $embedding::VECTOR<FLOAT32>(384) AS v;
RETURN CAST('[1.05e+00, 0.123, 5]' AS VECTOR<FLOAT>(3)) AS v`} />

Use `value::VECTOR<COORD>(DIM)` for compact handwritten Cypher, or
`CAST(value AS VECTOR<COORD>(DIM))` when that reads better in generated
or parenthesized expressions. `TRY_CAST(value AS
VECTOR<COORD>(DIM))` returns `null` instead of reporting a conversion
error. The cast input may be:

- **value** — `LIST<INTEGER | FLOAT>` **or** a string like
  `"[1, 2, 3]"` (numbers separated by commas, decimal or scientific
  notation). An empty string list `"[]"` parses to zero
  coordinates, which then has to match a zero-dimension declaration —
  but dimension `0` is rejected, so an empty vector is never
  constructible.
The dimension is part of the target type and must be in `1..=4096`.
Anything outside the valid ranges is rejected at evaluation time with a
clear error.

### Coordinate types

| Canonical tag | Aliases accepted on input |
|---|---|
| `FLOAT64` | `FLOAT` |
| `FLOAT32` | — |
| `INTEGER` | `INT`, `INT64`, `INTEGER64` |
| `INTEGER32` | `INT32` |
| `INTEGER16` | `INT16` |
| `INTEGER8` | `INT8` |

Alias matching is case-insensitive. Output always emits the canonical
tag.

`DOUBLE` is **not** accepted; typos surface as a clear
`unknown vector coordinate type '…'` error rather than silently
mapping to `FLOAT64`.

The coordinate type is written inside `VECTOR<...>`. If the coordinate
type or dimension must come from host code dynamically, build a type
string and use the lower-level dynamic helper:

<QueryCodeBlock code={String.raw`RETURN cast.to($values, $type_name) AS v`} />

### Coercion rules

- Integer inputs go into float-typed vectors unchanged (precision can
  degrade for very large magnitudes that don't fit in the float
  mantissa).
- Float inputs go into integer-typed vectors with truncation **toward
  zero** (`1.9 → 1`, `-1.9 → -1`, `0.999 → 0`, `-0.999 → 0`).
- Out-of-range values error loudly —
  `[128]::VECTOR<INT8>(1)` is an error because `128` does not fit in
  `INTEGER8`; `[2e39]::VECTOR<FLOAT32>(1)` is rejected because the
  value overflows `f32`; a float bigger than `i64::MAX` for an
  integer-backed vector errors rather than saturating.
- `NaN`, `Infinity`, and `-Infinity` coordinates are errors.
- Nested-list coordinates are errors (`[[1,2]]::VECTOR<INTEGER>(1)`).
- Non-numeric coordinates are errors (`[1, 'two', 3]::VECTOR<FLOAT32>(3)`).
- An unknown `coordinateType` is an error.

### Null propagation

- `null::VECTOR<FLOAT32>(3)` returns `null`.
- `CAST(null AS VECTOR<FLOAT32>(3))` returns `null`.

## Storage

Vectors can be stored directly as node or relationship properties:

<QueryCodeBlock code={String.raw`CREATE (:Doc {id: 1, embedding: [1, 2, 3]::VECTOR<INTEGER>(3)});

MATCH (a:Doc {id: 1}), (b:Doc {id: 2})
CREATE (a)-[:SIM {score: [0.9, 0.1]::VECTOR<FLOAT32>(2)}]->(b);

MATCH (d:Doc {id: 1}) SET d.embedding = [0.1, 0.2]::VECTOR<FLOAT32>(2);
MATCH (d:Doc {id: 1}) SET d += {embedding: [1, 2]::VECTOR<FLOAT32>(2)}`} />

A vector is also a legal map value — a property map containing a
vector is stored intact:

<QueryCodeBlock code={String.raw`CREATE (:Doc {id: 1, meta: {embedding: [1, 2, 3]::VECTOR<INTEGER>(3)}})`} />

### Restriction: no list-of-vectors as a property

A list that contains a `VECTOR` (at any depth under the list, including
inside a map nested under the list) cannot be stored as a property.
The engine rejects the write at property-conversion time — this is a
shape decision, not an oversight:

<QueryCodeBlock code={String.raw`// Rejected:
CREATE (:Doc {embeddings: [[1,2,3]::VECTOR<INTEGER>(3)]})
CREATE (:Doc {meta: {embeddings: [[1,2,3]::VECTOR<INTEGER>(3)]}})`} />

If you need many embeddings per document, hang them off separate
nodes connected by a relationship, each with its own vector property
— that is also the shape vector indexes expect.

Lists of vectors are still perfectly legal **inside a query** (in a
`RETURN`, `WITH`, `UNWIND`, or `collect(...)`). The restriction applies
only to the write path.

## Vector Index Search

Create a vector index on a single node or relationship property:

<QueryCodeBlock code={String.raw`CREATE VECTOR INDEX doc_embedding
FOR (d:Doc)
ON (d.embedding)
OPTIONS {indexConfig: {
  \`vector.dimensions\`: 384,
  \`vector.similarity_function\`: 'cosine'
}};`} />

Required options:

- `vector.dimensions` - integer dimension in `1..=4096`;
- `vector.similarity_function` - `'cosine'` or `'euclidean'`.

Query the indexed node scope with `db.index.vector.queryNodes`:

<QueryCodeBlock code={String.raw`CALL db.index.vector.queryNodes('doc_embedding', 10, $query)
YIELD node, score;`} />

Relationship vector indexes use the relationship procedure and yield
`relationship`:

<QueryCodeBlock code={String.raw`CALL db.index.vector.queryRelationships('rel_embedding', 10, [1, 0, 0]::VECTOR<FLOAT32>(3))
YIELD relationship, score;`} />

The procedure returns the yielded columns directly, sorted by
descending score. The query vector can be a `VECTOR`, a numeric list,
or a parameter containing a vector. Numeric lists
are coerced to `FLOAT32` vectors. `k` must be positive, and the query
dimension must match the configured index dimension.

The current implementation still scans the indexed label/type scope
linearly. Use selective labels or relationship types while the ANN
structure is future work.

## Exhaustive kNN

You can also express similarity directly with `ORDER BY … LIMIT k`
over the full candidate set:

<QueryCodeBlock code={String.raw`MATCH (d:Doc)
RETURN d.id AS id
ORDER BY vector.similarity(d.embedding, $query) DESC
LIMIT 10`} />

Or, using `WITH` to carry the score forward:

<QueryCodeBlock code={String.raw`MATCH (n:Node)
WITH n, vector.similarity($query, n.vector, 'euclidean') AS score
RETURN n.id AS id, score
ORDER BY score DESC
LIMIT 2`} />

Every `MATCH` candidate is scored, so cost is `O(n)` in the number of
matched nodes.

### Graph-filtered retrieval

The reason VECTOR lives next to the graph — score candidates by
similarity, then use relationships to explain or filter them:

<QueryCodeBlock code={String.raw`MATCH (d:Doc)
WITH d, vector.similarity(d.embedding, $query) AS score
MATCH (d)-[:MENTIONS]->(e:Entity)
WHERE e.type = $entity_type
RETURN d.id, d.title, score, collect(e.name) AS entities
ORDER BY score DESC
LIMIT 5`} />

### Bulk insert

Vectors load efficiently through a single `UNWIND` over a parameter
list of maps. Each row becomes a standalone `CREATE`, so each vector
flows through property conversion as a *top-level* property (not a
list entry), and the property rule is satisfied by construction:

<QueryCodeBlock code={String.raw`UNWIND $batch AS row
CREATE (:Doc {id: row.id, title: row.title, embedding: row.embedding})`} />

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
| `type.of([1,2,3]::VECTOR<INTEGER>(3))` | `"VECTOR<INTEGER>(3)"` |
| `value.size([1,2,3,4]::VECTOR<FLOAT32>(4))` | `4` (dimension) |
| `vector.dimension([1,2,3]::VECTOR<INTEGER8>(3))` | `3` |
| `vector.coordinates([1.9, -1.9, 3]::VECTOR<FLOAT32>(3), INTEGER)` | `[1, -1, 3]` |
| `vector.coordinates([1, 2, 3]::VECTOR<INT8>(3), FLOAT)` | `[1.0, 2.0, 3.0]` |

`value.size` on a `VECTOR` returns its dimension — identical to
`vector.dimension` for convenience. `type.of` returns the
parameterised tag `VECTOR<TYPE>(DIMENSION)` so runtime inspection can
see both the coordinate type and the size.

`vector.coordinates` on a float vector truncates toward zero (same rule as
cast-based vector construction). Both converters propagate `null` and error
on non-`VECTOR` inputs.

## Similarity and distance

All similarity / distance functions use `f32` arithmetic internally
(values are converted from the underlying coordinate type into `f32`
before accumulation, then widened back to `f64` for the result).

### Bounded similarity in `[0, 1]`

<QueryCodeBlock code={String.raw`vector.similarity(a, b)
vector.similarity(a, b, 'euclidean')`} />

Both accept a `VECTOR` **or** a `LIST<NUMBER>` on either side. Lists
are coerced on the fly to a `FLOAT32` vector whose dimension equals
the list's length. Higher = more similar.

- `cosine`: `(1 + raw_cosine) / 2`, so `1.0` is identical direction,
  `0.5` is orthogonal, `0.0` is opposite. A zero-norm vector returns
  `null` (cosine is undefined).
- `euclidean`: `1 / (1 + d²)`, where `d²` is the squared L2 distance.
  Documented example:
  `vector.similarity([4,5,6], [2,8,3], 'euclidean') ≈ 0.04348`
  (because `d² = 2² + 3² + 3² = 22`, so `1 / 23 ≈ 0.0435`).

Both functions `null`-propagate: any `null` argument returns `null`.
A dimension mismatch is an error. An empty list on either side is an
error.

### Signed distance metrics

<QueryCodeBlock code={String.raw`vector.distance(a, b, EUCLIDEAN)
vector.distance(a, b, EUCLIDEAN_SQUARED)
vector.distance(a, b, MANHATTAN)
vector.distance(a, b, COSINE)
vector.distance(a, b, DOT)
vector.distance(a, b, HAMMING)`} />

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

<QueryCodeBlock code={String.raw`vector.norm(v, EUCLIDEAN)   // sqrt(Σ xᵢ²)
vector.norm(v, MANHATTAN)   // Σ |xᵢ|`} />

Metric matching is case-insensitive; identifiers and quoted strings
both work. `null` vector or `null` metric returns `null`. Unknown
metric errors.

## Equality, DISTINCT, and ordering

- **Equality** compares coordinate type, dimension, and every value.
  `[1,2,3]::VECTOR<INTEGER>(3) = [1,2,3]::VECTOR<INTEGER8>(3)` is
  `false` — different coordinate types are never equal, even with
  numerically identical values.
- **`DISTINCT`** uses a stable key (coordinate type + dimension +
  stringified values) so duplicates collapse across projection and
  pipeline stages. Vectors of different coord types never collapse.
- **`ORDER BY`** on a vector column is accepted and runs without
  panicking, but the ordering is implementation-defined — use it for
  tie-breaking, not primary sort. Order by a scalar score
  (`vector.similarity(...)`) when intent matters.

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
   ORDER BY vector.similarity(d.embedding, $q) DESC
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
    "RETURN vector.similarity($a, $b) AS s",
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
    "MATCH (d:Doc) RETURN d.id ORDER BY vector.similarity(d.embedding, $q) DESC LIMIT 10",
    lora.Params{"q": q},
)
```

### Ruby

```ruby
require "lora_ruby"

q = LoraRuby.vector([0.1, 0.2, 0.3], 3, "FLOAT32")
db.execute(
  "MATCH (d:Doc) RETURN d.id ORDER BY vector.similarity(d.embedding, $q) DESC LIMIT 10",
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
    "MATCH (d:Doc) RETURN d.id ORDER BY vector.similarity(d.embedding, $q) DESC LIMIT 10",
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
  -d '{"query":"RETURN [0.1,0.2,0.3]::VECTOR<FLOAT32>(3) AS v"}'
```

— or run the engine through one of the in-process bindings above
when you need host-side vectors. See
[Queries → Parameters → HTTP API doesn't forward params](../queries/parameters#http-api-doesnt-forward-params).

## Limitations

- **ANN structures — not yet supported.** Vector indexes are cataloged
  and queryable, but `db.index.vector.*` performs a flat scan today.
- **Similarity / distance functions are exhaustive.** Direct
  `vector.similarity(...)` and `vector.distance(...)` calls score every
  candidate matched by the query.
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

See also the [Cypher support matrix (§13b)](https://github.com/lora-db/lora/blob/main/docs/reference/cypher-support-matrix.md#13b-vector-types-and-functions)
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
