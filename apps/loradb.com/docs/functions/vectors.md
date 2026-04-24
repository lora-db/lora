---
title: Vector Functions (Similarity, Distance, Norms)
sidebar_label: Vector
description: Vector functions in LoraDB — vector() construction, cosine and Euclidean similarity, signed distance metrics (EUCLIDEAN, EUCLIDEAN_SQUARED, MANHATTAN, COSINE, DOT, HAMMING), vector_norm, dimension introspection, and many kNN / graph-filtered retrieval examples.
---

# Vector Functions (Similarity, Distance, Norms)

LoraDB has a first-class [`VECTOR`](../data-types/vectors) value type
with a compact set of built-in functions for constructing vectors,
measuring similarity, computing signed distances under standard
metrics, and inspecting shape. Every similarity / distance
computation is **exhaustive** — vector indexes are not yet
implemented — so retrieval is expressed as `ORDER BY … LIMIT k` over
the matched candidate set.

All similarity / distance math uses `f32` internally: coordinates
are converted into `f32` before accumulation, then the scalar result
widens back to `f64`. This is stable regardless of the underlying
coordinate type.

## Overview

| Goal | Function |
|---|---|
| Construct a vector | [<CypherCode code="vector(values, dim, type)" />](#constructor) |
| Bounded similarity (higher = closer, in `[0, 1]`) | [<CypherCode code="vector.similarity.cosine(a, b)" />, <CypherCode code="vector.similarity.euclidean(a, b)" />](#bounded-similarity) |
| Signed distance under a named metric (smaller = closer) | [<CypherCode code="vector_distance(a, b, METRIC)" />](#signed-distance) |
| Magnitude of a vector | [<CypherCode code="vector_norm(v, METRIC)" />](#norms) |
| Dimension | [<CypherCode code="vector_dimension_count(v)" />](#introspection), <CypherCode code="size(v)" />, <CypherCode code="length(v)" /> |
| Runtime type tag | [<CypherCode code="valueType(v)" />](#introspection) |
| Back to a `LIST` | [<CypherCode code="toIntegerList(v)" />, <CypherCode code="toFloatList(v)" />](#list-conversion) |

`vector.similarity.*` also accepts a plain `LIST<NUMBER>` on either
side — the list is coerced to a `FLOAT32` vector of the same length.
`vector_distance` and `vector_norm` require real `VECTOR` values.

## Constructor

`vector(values, dimension, coordinateType)` — three arguments, no
more, no less. See [Data Types → Vectors → Construction](../data-types/vectors#construction)
for the full rules; the examples below cover the common shapes.

```cypher
-- Integer-backed
RETURN vector([1, 2, 3], 3, INTEGER) AS v        -- VECTOR<INTEGER>(3)
RETURN vector([1, 2, 3], 3, INT8)    AS v        -- VECTOR<INTEGER8>(3)
RETURN vector([1, 2, 3], 3, INT16)   AS v        -- VECTOR<INTEGER16>(3)
RETURN vector([1, 2, 3], 3, INT32)   AS v        -- VECTOR<INTEGER32>(3)

-- Float-backed
RETURN vector([0.1, 0.2, 0.3], 3, FLOAT32) AS v  -- VECTOR<FLOAT32>(3)
RETURN vector([0.1, 0.2, 0.3], 3, FLOAT64) AS v  -- VECTOR<FLOAT64>(3)
RETURN vector([0.1, 0.2, 0.3], 3, FLOAT)   AS v  -- FLOAT is an alias for FLOAT64

-- String form (useful for HTTP where parameters aren't yet forwarded)
RETURN vector('[1.05, 0.123, 5]', 3, FLOAT64) AS v
RETURN vector('[1e-2, 2e-2, 3e-2]', 3, FLOAT32) AS v
```

### Coordinate-type tag forms

The third argument accepts a **bare identifier**, a **quoted string**,
or a **parameter**. Matching is case-insensitive and collapses
internal whitespace, so `"signed integer"`, `"SIGNED   INTEGER"`,
and `"Signed Integer"` all resolve to `INTEGER`.

```cypher
RETURN vector([1, 2, 3], 3, INTEGER)           -- bare identifier
RETURN vector([1, 2, 3], 3, 'INTEGER')         -- quoted string
RETURN vector([1, 2, 3], 3, 'SIGNED INTEGER')  -- multi-word alias → must quote
RETURN vector($values, 3, $type)               -- host-provided tag
```

### From a parameter

```cypher
RETURN vector($embedding, 384, FLOAT32) AS query_vec
```

```ts
// Node / TypeScript
await db.execute(
  'RETURN vector($embedding, 384, FLOAT32) AS q',
  { embedding: myFloat32Array },
);
```

```python
# Python
db.execute(
    "RETURN vector($embedding, 384, FLOAT32) AS q",
    {"embedding": embedding_list},
)
```

### Coercion quick reference

```cypher
-- Integers promoted to float-backed vectors (exact for small magnitudes)
RETURN vector([1, 2, 3], 3, FLOAT32)            -- [1.0, 2.0, 3.0]

-- Floats truncate toward zero into integer-backed vectors
RETURN vector([1.9, -1.9, 0.999, -0.999], 4, INTEGER)
       -- [1, -1, 0, 0]

-- Out-of-range errors loudly (no silent saturation)
RETURN vector([128], 1, INT8)                   -- error: value 128 overflows INTEGER8
RETURN vector([2e39], 1, FLOAT32)               -- error: value overflows FLOAT32

-- NaN / Infinity / mixed types / nested lists all error
RETURN vector([1, 'two', 3], 3, FLOAT32)        -- error: non-numeric coordinate
```

### Null propagation

```cypher
RETURN vector(null, 3, FLOAT32)      -- null
RETURN vector([1,2,3], null, FLOAT32) -- null
RETURN vector([1], 1, null)           -- error: coordinate-type null is rejected
```

## Bounded similarity

Both `vector.similarity.cosine` and `vector.similarity.euclidean`
return a scalar in `[0, 1]` where **higher = more similar**. Both
accept a `VECTOR` **or** a `LIST<NUMBER>` on either side; lists are
coerced to a `FLOAT32` vector of matching length.

```cypher
-- Cosine: (1 + raw_cosine) / 2
RETURN vector.similarity.cosine([1, 0, 0], [1, 0, 0])    -- 1.0     (identical direction)
RETURN vector.similarity.cosine([1, 0, 0], [0, 1, 0])    -- 0.5     (orthogonal)
RETURN vector.similarity.cosine([1, 0, 0], [-1, 0, 0])   -- 0.0     (opposite)
RETURN vector.similarity.cosine([1, 2, 3], [2, 4, 6])    -- 1.0     (colinear)

-- Euclidean: 1 / (1 + d²)
RETURN vector.similarity.euclidean([4, 5, 6], [2, 8, 3]) -- ≈ 0.04348  (d² = 22)
RETURN vector.similarity.euclidean([0, 0, 0], [0, 0, 0]) -- 1.0         (identical)
```

### Mixing lists and vectors

```cypher
-- Pure VECTOR on both sides
WITH vector([0.1, 0.2, 0.3], 3, FLOAT32) AS a,
     vector([0.2, 0.2, 0.2], 3, FLOAT32) AS b
RETURN vector.similarity.cosine(a, b) AS score

-- VECTOR vs LIST: list is coerced to FLOAT32
MATCH (d:Doc)
RETURN d.id,
       vector.similarity.cosine(d.embedding, [0.1, 0.2, 0.3]) AS score
ORDER BY score DESC
LIMIT 10

-- LIST vs LIST: both coerced, useful for ad-hoc debugging
RETURN vector.similarity.cosine([1, 2, 3], [1, 2, 3]) AS score
```

### Null / error semantics

```cypher
-- null on either side → null
RETURN vector.similarity.cosine(null, [1, 2, 3])            -- null
RETURN vector.similarity.euclidean(vector([1,2], 2, FLOAT32), null)  -- null

-- zero-norm argument to cosine → null (cosine is undefined)
RETURN vector.similarity.cosine([0, 0, 0], [1, 2, 3])        -- null

-- dimension mismatch → error
RETURN vector.similarity.cosine([1, 2, 3], [1, 2])           -- error

-- empty list → error
RETURN vector.similarity.cosine([], [1, 2, 3])               -- error
```

## Signed distance

`vector_distance(a, b, METRIC)` — **smaller = more similar**. Both
arguments must be real `VECTOR` values with matching dimensions; a
plain list is rejected here (unlike the bounded-similarity functions).

| Metric | Formula | Range |
|---|---|---|
| `EUCLIDEAN` | `sqrt(Σ (aᵢ - bᵢ)²)` | `[0, ∞)` |
| `EUCLIDEAN_SQUARED` | `Σ (aᵢ - bᵢ)²` | `[0, ∞)` — skip the `sqrt` for pure ranking |
| `MANHATTAN` | `Σ \|aᵢ - bᵢ\|` | `[0, ∞)` |
| `COSINE` | `1 - raw_cosine(a, b)` (raw, not bounded) | `[0, 2]` — identical = `0`, opposite = `2` |
| `DOT` | `-(a · b)` — negated so smaller = closer | `(-∞, ∞)` |
| `HAMMING` | count of positions where `aᵢ ≠ bᵢ` (`f32` compare) | `[0, dim]` |

Metric names are case-insensitive and may be passed as bare
identifiers or quoted strings.

```cypher
WITH vector([1, 2, 3], 3, FLOAT32) AS a,
     vector([4, 6, 8], 3, FLOAT32) AS b
RETURN vector_distance(a, b, EUCLIDEAN)          AS l2,          -- ≈ 7.0711
       vector_distance(a, b, EUCLIDEAN_SQUARED)  AS l2_squared,  -- 50.0
       vector_distance(a, b, MANHATTAN)          AS l1,          -- 12.0
       vector_distance(a, b, COSINE)             AS cos_dist,
       vector_distance(a, b, DOT)                AS neg_dot,
       vector_distance(a, b, HAMMING)            AS hamming      -- 3 (all positions differ)
```

### Pick the right metric

```cypher
-- L2 / Euclidean — generic "closeness", respects magnitude.
WITH vector([1, 0], 2, FLOAT32) AS a, vector([3, 0], 2, FLOAT32) AS b
RETURN vector_distance(a, b, EUCLIDEAN)          -- 2.0

-- Squared L2 — same ranking as L2, cheaper (no sqrt). Use for ORDER BY.
WITH vector([1, 0], 2, FLOAT32) AS a, vector([3, 0], 2, FLOAT32) AS b
RETURN vector_distance(a, b, EUCLIDEAN_SQUARED)  -- 4.0

-- Cosine — magnitude-invariant; parallel vectors are "the same".
WITH vector([1, 2, 3], 3, FLOAT32) AS a,
     vector([2, 4, 6], 3, FLOAT32) AS b
RETURN vector_distance(a, b, COSINE)             -- ≈ 0.0   (colinear)

-- Dot — raw inner product, negated so "smaller is closer".
-- Useful when embeddings are already unit-normalised.
WITH vector([1, 0], 2, FLOAT32) AS a, vector([1, 0], 2, FLOAT32) AS b
RETURN vector_distance(a, b, DOT)                -- -1.0

-- Hamming — positionwise difference count. Handy for binary / quantised vectors.
WITH vector([1, 0, 1, 1], 4, INT8) AS a,
     vector([1, 1, 1, 0], 4, INT8) AS b
RETURN vector_distance(a, b, HAMMING)            -- 2
```

### Null / error semantics

```cypher
-- null vectors or null metric → null
RETURN vector_distance(null, vector([1,2,3], 3, FLOAT32), EUCLIDEAN)   -- null
RETURN vector_distance(vector([1,2,3], 3, FLOAT32), null, EUCLIDEAN)   -- null
RETURN vector_distance(vector([1,2,3], 3, FLOAT32),
                       vector([4,5,6], 3, FLOAT32), null)              -- null

-- Plain list → error (unlike vector.similarity.*)
RETURN vector_distance([1,2,3], vector([4,5,6], 3, FLOAT32), EUCLIDEAN)  -- error

-- Dimension mismatch → error
RETURN vector_distance(vector([1,2], 2, FLOAT32),
                       vector([1,2,3], 3, FLOAT32), EUCLIDEAN)          -- error

-- Unknown metric → error
RETURN vector_distance(vector([1,2,3], 3, FLOAT32),
                       vector([4,5,6], 3, FLOAT32), 'MAHALANOBIS')      -- error
```

## Norms

`vector_norm(v, METRIC)` — magnitude of a single vector.

| Metric | Formula |
|---|---|
| `EUCLIDEAN` | `sqrt(Σ xᵢ²)` — L2 length |
| `MANHATTAN` | `Σ \|xᵢ\|` — L1 length |

```cypher
WITH vector([3, 4], 2, FLOAT32) AS v
RETURN vector_norm(v, EUCLIDEAN)   -- 5.0    (3² + 4² = 25)
WITH vector([3, 4], 2, FLOAT32) AS v
RETURN vector_norm(v, MANHATTAN)   -- 7.0

WITH vector([1, -2, 2], 3, FLOAT32) AS v
RETURN vector_norm(v, EUCLIDEAN)   -- 3.0    (sqrt(9))

-- null propagates
RETURN vector_norm(null, EUCLIDEAN)                                -- null
RETURN vector_norm(vector([1,2,3], 3, FLOAT32), null)              -- null
RETURN vector_norm(vector([1,2,3], 3, FLOAT32), 'MAHALANOBIS')     -- error
```

### Unit-normalisation pattern

There's no built-in `vector_normalize` — compose with a list and
`vector()` re-construction:

```cypher
WITH vector([3, 0, 4], 3, FLOAT32) AS v
WITH v, vector_norm(v, EUCLIDEAN) AS n, toFloatList(v) AS coords
RETURN vector([coords[0] / n, coords[1] / n, coords[2] / n], 3, FLOAT32) AS unit
       -- [0.6, 0.0, 0.8]
```

For variable-dimension unit-normalisation, this is easier to keep
host-side — the client languages all ship a vector constructor.

## Introspection

| Expression | Returns | Notes |
|---|---|---|
| `valueType(v)` | `String` — `"VECTOR<TYPE>(DIM)"` | Only type whose tag encodes structure |
| `size(v)` | `Int` — dimension | Same as `vector_dimension_count` |
| `length(v)` | `Int` — dimension | Alias of `size` on vectors |
| `vector_dimension_count(v)` | `Int` — dimension | Explicit name |

```cypher
WITH vector([1, 2, 3, 4], 4, FLOAT32) AS v
RETURN valueType(v)                 AS t,   -- 'VECTOR<FLOAT32>(4)'
       size(v)                      AS s,   -- 4
       length(v)                    AS l,   -- 4
       vector_dimension_count(v)    AS d    -- 4

-- Coordinate type is part of the valueType tag
RETURN valueType(vector([1,2,3], 3, INTEGER))   -- 'VECTOR<INTEGER>(3)'
RETURN valueType(vector([1,2,3], 3, INT8))      -- 'VECTOR<INTEGER8>(3)'
```

### Guarding by shape

```cypher
MATCH (d:Doc)
WHERE valueType(d.embedding) = 'VECTOR<FLOAT32>(384)'
RETURN d.id AS id
```

```cypher
MATCH (d:Doc)
WHERE vector_dimension_count(d.embedding) = 384
RETURN count(*) AS docs_with_384d
```

## List conversion

| Function | From | To |
|---|---|---|
| `toIntegerList(v)` | any `VECTOR` | `LIST<INTEGER>` — truncates toward zero |
| `toFloatList(v)` | any `VECTOR` | `LIST<FLOAT>` — widens exact |

```cypher
RETURN toIntegerList(vector([1.9, -1.9, 3.0], 3, FLOAT32))  -- [1, -1, 3]
RETURN toFloatList  (vector([1, 2, 3],        3, INT8))     -- [1.0, 2.0, 3.0]

-- null propagates
RETURN toIntegerList(null)         -- null
RETURN toFloatList(null)           -- null

-- non-VECTOR input errors
RETURN toIntegerList([1, 2, 3])    -- error
```

Both converters round-trip cleanly through the binding layer — use
them when you need to hand off to a caller that wants a plain array.

## kNN and retrieval patterns

### Top-k by cosine similarity

```cypher
MATCH (d:Doc)
RETURN d.id AS id, d.title AS title
ORDER BY vector.similarity.cosine(d.embedding, $query) DESC
LIMIT 10
```

### Top-k carrying the score forward

```cypher
MATCH (d:Doc)
WITH d, vector.similarity.cosine(d.embedding, $query) AS score
RETURN d.id AS id, d.title AS title, score
ORDER BY score DESC
LIMIT 10
```

### Top-k by signed distance (smaller = closer)

```cypher
MATCH (d:Doc)
WITH d, vector_distance(d.embedding, $query, EUCLIDEAN) AS dist
RETURN d.id AS id, dist
ORDER BY dist ASC
LIMIT 10
```

### Cheaper ranking with `EUCLIDEAN_SQUARED`

The rankings are identical; `EUCLIDEAN_SQUARED` skips the `sqrt`.

```cypher
MATCH (d:Doc)
WITH d, vector_distance(d.embedding, $query, EUCLIDEAN_SQUARED) AS d2
RETURN d.id AS id
ORDER BY d2 ASC
LIMIT 20
```

### Narrow the candidate set first

Similarity is `O(n)` over matched nodes — push filters into `MATCH`
and `WHERE` before scoring.

```cypher
MATCH (d:Doc {tenant: $tenant})
WHERE d.language = 'en' AND d.published_at >= date('2026-01-01')
WITH  d, vector.similarity.cosine(d.embedding, $query) AS score
RETURN d.id, score
ORDER BY score DESC
LIMIT 10
```

### Score threshold

```cypher
MATCH (d:Doc)
WITH d, vector.similarity.cosine(d.embedding, $query) AS score
WHERE score >= 0.75
RETURN d.id, score
ORDER BY score DESC
```

### Graph-filtered retrieval

The reason `VECTOR` lives next to the graph — score first, then use
relationships to explain or filter.

```cypher
MATCH (d:Doc)
WITH d, vector.similarity.cosine(d.embedding, $query) AS score
MATCH (d)-[:MENTIONS]->(e:Entity)
WHERE e.type = $entity_type
RETURN d.id, d.title, score, collect(e.name) AS entities
ORDER BY score DESC
LIMIT 5
```

### Neighbour-expansion after retrieval

Pull the local graph context around each hit:

```cypher
MATCH (d:Doc)
WITH d, vector.similarity.cosine(d.embedding, $query) AS score
ORDER BY score DESC
LIMIT 10
MATCH (d)-[:CITED_BY]->(citing:Doc)
RETURN d.id AS hit, score, collect(citing.id) AS citations
```

### Per-category nearest

```cypher
MATCH (d:Doc)
WITH d, d.category AS category,
     vector.similarity.cosine(d.embedding, $query) AS score
ORDER BY score DESC
WITH category, collect({d: d, score: score})[0] AS top
RETURN category, top.d.id AS id, top.score AS score
ORDER BY score DESC
```

### Hybrid: keyword + vector

Blend a lexical boost into a vector score — straight arithmetic, no
special function required:

```cypher
MATCH (d:Doc)
WHERE toLower(d.title) CONTAINS toLower($q)
   OR toLower(d.body)  CONTAINS toLower($q)
WITH d,
     vector.similarity.cosine(d.embedding, $query) AS vec_score,
     CASE WHEN toLower(d.title) CONTAINS toLower($q) THEN 0.2 ELSE 0.0 END AS title_boost
RETURN d.id AS id,
       vec_score + title_boost AS score
ORDER BY score DESC
LIMIT 10
```

### Expand candidates via relationships, then rank

Start from a seed node, hop to candidates through the graph, then
rank by similarity to the query vector:

```cypher
MATCH (seed:Doc {id: $seed_id})-[:SIMILAR_TO*1..2]-(candidate:Doc)
WHERE candidate.id <> $seed_id
WITH DISTINCT candidate,
              vector.similarity.cosine(candidate.embedding, $query) AS score
RETURN candidate.id, score
ORDER BY score DESC
LIMIT 10
```

### Multi-vector query (best-of / max-sim)

Score each document against several query vectors and keep the best:

```cypher
UNWIND $queries AS q
MATCH (d:Doc)
WITH d, max(vector.similarity.cosine(d.embedding, q)) AS best
RETURN d.id, best
ORDER BY best DESC
LIMIT 10
```

### Average query (query centroid)

If you have several positive examples, a host-side average is usually
clearer than a Cypher fold. For a small fixed number, you can also
stay in Cypher:

```cypher
WITH [$q1, $q2, $q3] AS qs
MATCH (d:Doc)
WITH d, reduce(acc = 0.0,
               q IN qs |
               acc + vector.similarity.cosine(d.embedding, q) / size(qs)) AS score
RETURN d.id, score
ORDER BY score DESC
LIMIT 10
```

### Metric comparison side-by-side

Useful during debugging — show every metric for one candidate:

```cypher
WITH vector([0.10, 0.20, 0.30], 3, FLOAT32) AS q
MATCH (d:Doc {id: $id})
RETURN d.id,
       vector.similarity.cosine    (d.embedding, q)               AS cos_bounded,
       vector.similarity.euclidean (d.embedding, q)               AS euc_bounded,
       vector_distance(d.embedding, q, EUCLIDEAN)                 AS l2,
       vector_distance(d.embedding, q, EUCLIDEAN_SQUARED)         AS l2_sq,
       vector_distance(d.embedding, q, MANHATTAN)                 AS l1,
       vector_distance(d.embedding, q, COSINE)                    AS cos_dist,
       vector_distance(d.embedding, q, DOT)                       AS neg_dot
```

### Count above threshold

```cypher
MATCH (d:Doc)
WITH d, vector.similarity.cosine(d.embedding, $query) AS score
WHERE score >= $threshold
RETURN count(*) AS hits
```

### Bucket by similarity band

```cypher
MATCH (d:Doc)
WITH vector.similarity.cosine(d.embedding, $query) AS score
WITH CASE
       WHEN score >= 0.9 THEN 'very-close'
       WHEN score >= 0.7 THEN 'close'
       WHEN score >= 0.5 THEN 'related'
       ELSE                   'distant'
     END AS band
RETURN band, count(*) AS n
ORDER BY n DESC
```

### Dedup by identity and keep the best match

```cypher
MATCH (d:Doc)
WITH d, vector.similarity.cosine(d.embedding, $query) AS score
ORDER BY score DESC
WITH d.fingerprint AS fp, collect({d: d, score: score})[0] AS best
RETURN best.d.id AS id, best.score AS score
ORDER BY score DESC
LIMIT 10
```

## Bulk insert

Vectors load efficiently through a single `UNWIND` over a parameter
list. Each row becomes a standalone `CREATE`, so each vector flows
through property conversion as a *top-level* property.

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

## Edge cases

### DISTINCT keys coordinate type

`DISTINCT` collapses duplicates by coordinate type + dimension +
values; vectors of different coord types never dedup to each other
even with numerically identical values:

```cypher
UNWIND [
  vector([1, 2, 3], 3, INTEGER),
  vector([1, 2, 3], 3, INTEGER),
  vector([1, 2, 3], 3, INTEGER8)
] AS v
RETURN DISTINCT v
-- returns two rows: one INTEGER, one INTEGER8
```

### Ordering by a vector column

`ORDER BY some_vector_column` is accepted and stable, but the order
is **implementation-defined**. Order by a scalar score when intent
matters:

```cypher
-- Works, but meaningless as a primary sort.
MATCH (d:Doc) RETURN d ORDER BY d.embedding LIMIT 5

-- Use this instead.
MATCH (d:Doc)
RETURN d
ORDER BY vector.similarity.cosine(d.embedding, $query) DESC
LIMIT 5
```

### Zero vectors and cosine

Cosine on a zero-norm vector is undefined, so
`vector.similarity.cosine([0,…], anything)` returns `null`. Filter
or coalesce explicitly:

```cypher
MATCH (d:Doc)
WITH d, coalesce(vector.similarity.cosine(d.embedding, $query), 0.0) AS score
RETURN d.id, score
ORDER BY score DESC
LIMIT 10
```

### Integer-backed vectors in similarity

Integer coordinates widen to `f32` before accumulation — identical
ranking behaviour to a `FLOAT32` vector with the same values, modulo
precision loss for magnitudes that don't fit in the mantissa.

```cypher
RETURN vector.similarity.cosine(vector([1,2,3], 3, INT8),
                                vector([2,4,6], 3, INT8))
       -- 1.0 (colinear, same result as FLOAT32)
```

### HTTP and parameters

`POST /query` does not yet accept a `params` field, so a vector cannot
ride in as a parameter over HTTP. Either embed the vector literally
in the query — using the string form makes this practical —

```bash
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query":"RETURN vector([0.1,0.2,0.3], 3, FLOAT32) AS v"}'
```

— or use one of the in-process bindings, which all support parameters.

## Limitations

- **No vector indexes yet** — every call scans the matched candidate
  set linearly. Keep `MATCH` filters tight.
- **No approximate nearest neighbour (ANN)** — a direct consequence of
  the above.
- **No embedding generation** — LoraDB has no plugin surface. Produce
  embeddings host-side and pass them in.
- **No list-of-vectors as a property** — store each vector on its own
  node or relationship. Lists of vectors inside a query are fine.
- **No parameters over HTTP** — see the note above.
- **Dimension ≤ 4096** — enforced at construction.
- **Ordering by a vector column is unspecified** — order by a scalar
  score instead.

See also the [Cypher support matrix (§13a)](https://github.com/lora-db/lora/blob/main/docs/reference/cypher-support-matrix.md#13a-vector-types-and-functions)
for the engine-side behaviour grid.

## See also

- [**Vectors (data type)**](../data-types/vectors) — full reference for
  the `VECTOR` value type: storage, coercion, parameter binding.
- [**Cookbook → Vector-retrieval patterns**](../cookbook#vector-retrieval-patterns)
  — top-k and graph-filtered retrieval recipes.
- [**Queries → Parameters**](../queries/parameters#semantic-retrieval-with-a-vector-parameter)
  — passing vectors as parameters.
- [**Math**](./math) — scalar arithmetic used alongside vector scores.
- [**Aggregation**](./aggregation) — `max`, `min`, `collect`, used
  after ranking.

### Background reading

- [**Vectors belong next to relationships**](/blog/vectors-belong-next-to-relationships)
  — why similarity lives as a value type instead of in a sidecar store.
- [**LoraDB v0.2: vector values for connected AI context**](/blog/loradb-v0-2-vectors)
  — the release that introduced `VECTOR`.
