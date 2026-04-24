---
title: Vector Values
sidebar_label: Vectors
description: First-class VECTOR values in LoraDB — construction in Cypher, property storage, binding round-trips, and the built-in vector math functions for exhaustive kNN retrieval.
---

# Vector Values

LoraDB supports `VECTOR` as a first-class value type. Vectors can be
constructed in Cypher, stored as node or relationship properties,
returned through every binding, and used as input to built-in vector
math functions for exhaustive kNN-style retrieval.

:::info Scope
Vector **indexes** and approximate-nearest-neighbour search are **not yet
implemented**. Vector functions are exhaustive — they scan every
candidate linearly. This is fine for small datasets (demos, tests,
unit-level experiments) but not for production-scale semantic search
until an index-backed variant ships.

LoraDB also has no plugin system today, so there is no built-in
embedding/plugin integration. Generate embeddings in your application
and pass them in as parameters.
:::

## Construction

```cypher
RETURN vector([1, 2, 3], 3, INTEGER) AS v
RETURN vector([1.05, 0.123, 5], 3, FLOAT32) AS v
RETURN vector($embedding, 384, FLOAT32) AS v
RETURN vector('[1.05e+00, 0.123, 5]', 3, FLOAT) AS v
```

`vector(value, dimension, coordinateType)` takes:

- **value** — `LIST<INTEGER | FLOAT>` *or* a string like `"[1, 2, 3]"`
  (numbers separated by commas, decimal or scientific notation).
- **dimension** — integer in `1..=4096`.
- **coordinateType** — one of the canonical tags or an alias:

| Tag | Aliases accepted on input |
|---|---|
| `FLOAT64` | `FLOAT` |
| `FLOAT32` | — |
| `INTEGER` | `INT`, `INT64`, `INTEGER64`, `SIGNED INTEGER` *(as a string)* |
| `INTEGER32` | `INT32` |
| `INTEGER16` | `INT16` |
| `INTEGER8` | `INT8` |

`DOUBLE` is **not** accepted; typos surface as a clear
"unknown coordinate type" error rather than silently mapping to
`FLOAT64`.

The third argument can be written as a bare identifier
(`vector([1,2,3], 3, INTEGER)`) or as a string literal
(`vector([1,2,3], 3, 'INTEGER')`). Because the multi-word alias
`SIGNED INTEGER` contains a space, pass it as a string:
`vector([1,2,3], 3, 'SIGNED INTEGER')`.

### Coercion rules

- Integer inputs go into float-typed vectors unchanged (possible
  precision loss at very large magnitudes).
- Float inputs go into integer-typed vectors with truncation towards
  zero (`1.9 → 1`, `-1.9 → -1`).
- Out-of-range values error loudly —
  `vector([128], 1, INT8)` is an error because `128` does not fit in
  `INTEGER8`.
- `NaN`, `Infinity`, nested lists, and `null` coordinates are all errors.
- An unknown `coordinateType` is an error.

### Null propagation

- `vector(null, 3, FLOAT32)` returns `null`.
- `vector([1,2,3], null, INTEGER)` returns `null`.
- Anything else that fails validation raises a query error (`null`
  would hide the bug).

## Storage

Vectors can be stored directly as node or relationship properties:

```cypher
CREATE (:Doc {id: 1, embedding: vector([1, 2, 3], 3, INTEGER)})
MATCH (d:Doc {id: 1}) SET d.embedding = vector([0.1, 0.2], 2, FLOAT32)
```

**Restriction:** lists stored as properties cannot contain vectors.
Store each vector as its own property or hang them off separate nodes;
a list of vectors is rejected at write time.

## Introspection

| Expression | Returns |
|---|---|
| `valueType(vector([1,2,3], 3, INTEGER))` | `"VECTOR<INTEGER>(3)"` |
| `size(vector([1,2,3,4], 4, FLOAT32))` | `4` |
| `vector_dimension_count(vector([1,2,3], 3, INTEGER8))` | `3` |
| `toIntegerList(vector([1.9, -1.9, 3], 3, FLOAT32))` | `[1, -1, 3]` |
| `toFloatList(vector([1, 2, 3], 3, INT8))` | `[1.0, 2.0, 3.0]` |

## Similarity and distance

All similarity / distance functions use `float32` arithmetic internally
to match the reference spec — results are returned as `FLOAT` (f64).

### Bounded similarity in `[0, 1]`

```cypher
vector.similarity.cosine(a, b)
vector.similarity.euclidean(a, b)
```

Both accept a `VECTOR` **or** a `LIST<INTEGER | FLOAT>` on either side
(lists are coerced to `FLOAT32` vectors). Higher = more similar.

- `cosine`: `(1 + raw_cosine) / 2`, so `1.0` is identical direction,
  `0.5` is orthogonal, `0.0` is opposite. Zero vectors return `null`
  (cosine is undefined).
- `euclidean`: `1 / (1 + d²)`, where `d²` is the squared L2 distance.
  Documented example:
  `vector.similarity.euclidean([4,5,6], [2,8,3]) ≈ 0.04348`.

### Signed distance metrics

```cypher
vector_distance(a, b, EUCLIDEAN)
vector_distance(a, b, EUCLIDEAN_SQUARED)
vector_distance(a, b, MANHATTAN)
vector_distance(a, b, COSINE)
vector_distance(a, b, DOT)
vector_distance(a, b, HAMMING)
```

Both operands must be `VECTOR` values with matching dimensions. Smaller
= more similar. Metric tokens may be passed as identifiers or strings.

### Vector norms

```cypher
vector_norm(v, EUCLIDEAN)   -- sqrt(Σ xᵢ²)
vector_norm(v, MANHATTAN)   -- Σ |xᵢ|
```

## Exhaustive kNN

Until vector indexes land, approximate nearest-neighbour is expressed as
an `ORDER BY … LIMIT k` over the full candidate set:

```cypher
MATCH (d:Doc)
RETURN d.id AS id
ORDER BY vector.similarity.cosine(d.embedding, $query) DESC
LIMIT 10
```

Or, using `WITH` to carry the score forward:

```cypher
MATCH (node:Node)
WITH node, vector.similarity.euclidean($query, node.vector) AS score
RETURN node.id AS id, score
ORDER BY score DESC
LIMIT 2
```

Every `MATCH` candidate is scored, so cost is `O(n)` in the number of
matched nodes. For small datasets this is fine; for large corpora you'll
want a proper vector index (not yet implemented in LoraDB).

## Wire shape

Every binding serialises a vector as the same tagged object:

```json
{
  "kind": "vector",
  "dimension": 3,
  "coordinateType": "INTEGER",
  "values": [1, 2, 3]
}
```

The same shape is accepted on the parameter side — build one via the
helper for your language, or pass the literal object.

| Language | Helper |
|---|---|
| TypeScript / JS | `vector(values, dimension, coordinateType)`, `isVector(v)` |
| Python | `vector(values, dimension, coordinate_type)`, `is_vector(v)` |
| Go | `lora.Vector(values, dimension, coordinateType)`, `lora.IsVector(v)` |
| Ruby | `LoraRuby.vector(values, dimension, coordinate_type)`, `LoraRuby.vector?(v)` |

## See also

- [Functions → Overview](../functions/overview) — includes vector functions.
- [Queries → Parameters](../queries/parameters) — passing vectors as parameters.
- Internal value model (`docs/internals/value-model.md` in the repo) — engine-side shape.
