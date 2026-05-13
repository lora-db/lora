---
title: Vector Functions (Similarity, Distance, Norms)
sidebar_label: Vector
description: Vector functions in LoraDB — cast-based vector construction, cosine and Euclidean similarity, signed distance metrics, vector.norm, dimension introspection, and vector retrieval examples.
---

# Vector Functions (Similarity, Distance, Norms)

LoraDB has a first-class [`VECTOR`](../data-types/vectors) value type
with a compact set of built-in functions for measuring similarity,
computing signed distances under standard metrics, and inspecting
shape. Vector values are constructed with casts. Every similarity / distance
computation is **exhaustive** when called directly in a query. For a
cataloged vector search surface, use
[`CREATE VECTOR INDEX`](../queries/indexes#vector-indexes) with
`db.index.vector.queryNodes` or `queryRelationships`; those procedures
currently use flat scan execution over the indexed label/type scope.

All similarity / distance math uses `f32` internally: coordinates
are converted into `f32` before accumulation, then the scalar result
widens back to `f64`. This is stable regardless of the underlying
coordinate type.

## Overview

| Goal | Function |
|---|---|
| Construct a vector | [<CypherCode code="[1, 2, 3]::VECTOR<INTEGER>(3)" />](#construction) |
| Bounded similarity (higher = closer, in `[0, 1]`) | [<CypherCode code="vector.similarity(a, b)" />, <CypherCode code="vector.similarity(a, b, 'euclidean')" />](#bounded-similarity) |
| Signed distance under a named metric (smaller = closer) | [<CypherCode code="vector.distance(a, b, METRIC)" />](#signed-distance) |
| Magnitude of a vector | [<CypherCode code="vector.norm(v, METRIC)" />](#norms) |
| Dimension | [<CypherCode code="vector.dimension(v)" />](#introspection), <CypherCode code="value.size(v)" /> |
| Runtime type tag | [<CypherCode code="type.of(v)" />](#introspection) |
| Back to a `LIST` | [<CypherCode code="vector.coordinates(v, INTEGER)" />, <CypherCode code="vector.coordinates(v, FLOAT)" />](#list-conversion) |

`vector.similarity` also accepts a plain `LIST<NUMBER>` on either
side — the list is coerced to a `FLOAT32` vector of the same length.
`vector.distance` and `vector.norm` require real `VECTOR` values.

## Construction

Construct vectors with `value::VECTOR<COORD>(DIM)` or
`CAST(value AS VECTOR<COORD>(DIM))`. See [Data Types → Vectors →
Construction](../data-types/vectors#construction) for the full rules;
the examples below cover the common shapes.

```cypher
-- Integer-backed
RETURN [1, 2, 3]::VECTOR<INTEGER>(3) AS v        -- VECTOR<INTEGER>(3)
RETURN [1, 2, 3]::VECTOR<INT8>(3)    AS v        -- VECTOR<INTEGER8>(3)
RETURN [1, 2, 3]::VECTOR<INT16>(3)   AS v        -- VECTOR<INTEGER16>(3)
RETURN [1, 2, 3]::VECTOR<INT32>(3)   AS v        -- VECTOR<INTEGER32>(3)

-- Float-backed
RETURN [0.1, 0.2, 0.3]::VECTOR<FLOAT32>(3) AS v  -- VECTOR<FLOAT32>(3)
RETURN [0.1, 0.2, 0.3]::VECTOR<FLOAT64>(3) AS v  -- VECTOR<FLOAT64>(3)
RETURN [0.1, 0.2, 0.3]::VECTOR<FLOAT>(3)   AS v  -- FLOAT is an alias for FLOAT64

-- CAST(...) form, useful when the value is already parenthesized
RETURN CAST('[1.05, 0.123, 5]' AS VECTOR<FLOAT64>(3)) AS v
RETURN CAST('[1e-2, 2e-2, 3e-2]' AS VECTOR<FLOAT32>(3)) AS v
```

### Coordinate-type tag forms

The coordinate tag appears inside the `VECTOR<...>(...)` type. Matching
is case-insensitive and accepts aliases such as `FLOAT` for `FLOAT64`
and `INT8` for `INTEGER8`.

```cypher
RETURN [1, 2, 3]::VECTOR<INTEGER>(3)
RETURN [1, 2, 3]::VECTOR<INT8>(3)
RETURN [1, 2, 3]::VECTOR<FLOAT>(3)      -- FLOAT aliases FLOAT64
```

### From a parameter

```cypher
RETURN $embedding::VECTOR<FLOAT32>(384) AS query_vec
RETURN CAST($embedding AS VECTOR<FLOAT32>(384)) AS query_vec
```

```ts
// Node / TypeScript
await db.execute(
  'RETURN $embedding::VECTOR<FLOAT32>(384) AS q',
  { embedding: myFloat32Array },
);
```

```python
# Python
db.execute(
    "RETURN $embedding::VECTOR<FLOAT32>(384) AS q",
    {"embedding": embedding_list},
)
```

### Coercion quick reference

```cypher
-- Integers promoted to float-backed vectors (exact for small magnitudes)
RETURN [1, 2, 3]::VECTOR<FLOAT32>(3)            -- [1.0, 2.0, 3.0]

-- Floats truncate toward zero into integer-backed vectors
RETURN [1.9, -1.9, 0.999, -0.999]::VECTOR<INTEGER>(4)
       -- [1, -1, 0, 0]

-- Out-of-range errors loudly (no silent saturation)
RETURN [128]::VECTOR<INT8>(1)                   -- error: value 128 overflows INTEGER8
RETURN [2e39]::VECTOR<FLOAT32>(1)               -- error: value overflows FLOAT32

-- NaN / Infinity / mixed types / nested lists all error
RETURN [1, 'two', 3]::VECTOR<FLOAT32>(3)        -- error: non-numeric coordinate
```

### Null propagation

```cypher
RETURN null::VECTOR<FLOAT32>(3)      -- null
RETURN CAST(null AS VECTOR<FLOAT32>(3)) -- null
```

## Bounded similarity

`vector.similarity(a, b)` defaults to cosine. Pass `'euclidean'` as the
third argument for bounded Euclidean similarity. Both forms return a
scalar in `[0, 1]` where **higher = more similar** and accept a `VECTOR`
or a `LIST<NUMBER>` on either side.

```cypher
-- Cosine: (1 + raw_cosine) / 2
RETURN vector.similarity([1, 0, 0], [1, 0, 0])    -- 1.0     (identical direction)
RETURN vector.similarity([1, 0, 0], [0, 1, 0])    -- 0.5     (orthogonal)
RETURN vector.similarity([1, 0, 0], [-1, 0, 0])   -- 0.0     (opposite)
RETURN vector.similarity([1, 2, 3], [2, 4, 6])    -- 1.0     (colinear)

-- Euclidean: 1 / (1 + d²)
RETURN vector.similarity([4, 5, 6], [2, 8, 3], 'euclidean') -- ≈ 0.04348
RETURN vector.similarity([0, 0, 0], [0, 0, 0], 'euclidean') -- 1.0
```

### Mixing lists and vectors

```cypher
-- Pure VECTOR on both sides
WITH [0.1, 0.2, 0.3]::VECTOR<FLOAT32>(3) AS a,
     [0.2, 0.2, 0.2]::VECTOR<FLOAT32>(3) AS b
RETURN vector.similarity(a, b) AS score

-- VECTOR vs LIST: list is coerced to FLOAT32
MATCH (d:Doc)
RETURN d.id,
       vector.similarity(d.embedding, [0.1, 0.2, 0.3]) AS score
ORDER BY score DESC
LIMIT 10

-- LIST vs LIST: both coerced, useful for ad-hoc debugging
RETURN vector.similarity([1, 2, 3], [1, 2, 3]) AS score
```

### Null / error semantics

```cypher
-- null on either side → null
RETURN vector.similarity(null, [1, 2, 3])            -- null
RETURN vector.similarity([1,2]::VECTOR<FLOAT32>(2), null)  -- null

-- zero-norm argument to cosine → null (cosine is undefined)
RETURN vector.similarity([0, 0, 0], [1, 2, 3])        -- null

-- dimension mismatch → error
RETURN vector.similarity([1, 2, 3], [1, 2])           -- error

-- empty list → error
RETURN vector.similarity([], [1, 2, 3])               -- error
```

## Signed distance

`vector.distance(a, b, METRIC)` — **smaller = more similar**. Both
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
WITH [1, 2, 3]::VECTOR<FLOAT32>(3) AS a,
     [4, 6, 8]::VECTOR<FLOAT32>(3) AS b
RETURN vector.distance(a, b, EUCLIDEAN)          AS l2,          -- ≈ 7.0711
       vector.distance(a, b, EUCLIDEAN_SQUARED)  AS l2_squared,  -- 50.0
       vector.distance(a, b, MANHATTAN)          AS l1,          -- 12.0
       vector.distance(a, b, COSINE)             AS cos_dist,
       vector.distance(a, b, DOT)                AS neg_dot,
       vector.distance(a, b, HAMMING)            AS hamming      -- 3 (all positions differ)
```

### Pick the right metric

```cypher
-- L2 / Euclidean — generic "closeness", respects magnitude.
WITH [1, 0]::VECTOR<FLOAT32>(2) AS a, [3, 0]::VECTOR<FLOAT32>(2) AS b
RETURN vector.distance(a, b, EUCLIDEAN)          -- 2.0

-- Squared L2 — same ranking as L2, cheaper (no sqrt). Use for ORDER BY.
WITH [1, 0]::VECTOR<FLOAT32>(2) AS a, [3, 0]::VECTOR<FLOAT32>(2) AS b
RETURN vector.distance(a, b, EUCLIDEAN_SQUARED)  -- 4.0

-- Cosine — magnitude-invariant; parallel vectors are "the same".
WITH [1, 2, 3]::VECTOR<FLOAT32>(3) AS a,
     [2, 4, 6]::VECTOR<FLOAT32>(3) AS b
RETURN vector.distance(a, b, COSINE)             -- ≈ 0.0   (colinear)

-- Dot — raw inner product, negated so "smaller is closer".
-- Useful when embeddings are already unit-normalised.
WITH [1, 0]::VECTOR<FLOAT32>(2) AS a, [1, 0]::VECTOR<FLOAT32>(2) AS b
RETURN vector.distance(a, b, DOT)                -- -1.0

-- Hamming — positionwise difference count. Handy for binary / quantised vectors.
WITH [1, 0, 1, 1]::VECTOR<INT8>(4) AS a,
     [1, 1, 1, 0]::VECTOR<INT8>(4) AS b
RETURN vector.distance(a, b, HAMMING)            -- 2
```

### Null / error semantics

```cypher
-- null vectors or null metric → null
RETURN vector.distance(null, [1,2,3]::VECTOR<FLOAT32>(3), EUCLIDEAN)   -- null
RETURN vector.distance([1,2,3]::VECTOR<FLOAT32>(3), null, EUCLIDEAN)   -- null
RETURN vector.distance([1,2,3]::VECTOR<FLOAT32>(3),
                       [4,5,6]::VECTOR<FLOAT32>(3), null)              -- null

-- Plain list → error (unlike vector.similarity)
RETURN vector.distance([1,2,3], [4,5,6]::VECTOR<FLOAT32>(3), EUCLIDEAN)  -- error

-- Dimension mismatch → error
RETURN vector.distance([1,2]::VECTOR<FLOAT32>(2),
                       [1,2,3]::VECTOR<FLOAT32>(3), EUCLIDEAN)          -- error

-- Unknown metric → error
RETURN vector.distance([1,2,3]::VECTOR<FLOAT32>(3),
                       [4,5,6]::VECTOR<FLOAT32>(3), 'MAHALANOBIS')      -- error
```

## Norms

`vector.norm(v, METRIC)` — magnitude of a single vector.

| Metric | Formula |
|---|---|
| `EUCLIDEAN` | `sqrt(Σ xᵢ²)` — L2 length |
| `MANHATTAN` | `Σ \|xᵢ\|` — L1 length |

```cypher
WITH [3, 4]::VECTOR<FLOAT32>(2) AS v
RETURN vector.norm(v, EUCLIDEAN)   -- 5.0    (3² + 4² = 25)
WITH [3, 4]::VECTOR<FLOAT32>(2) AS v
RETURN vector.norm(v, MANHATTAN)   -- 7.0

WITH [1, -2, 2]::VECTOR<FLOAT32>(3) AS v
RETURN vector.norm(v, EUCLIDEAN)   -- 3.0    (sqrt(9))

-- null propagates
RETURN vector.norm(null, EUCLIDEAN)                                -- null
RETURN vector.norm([1,2,3]::VECTOR<FLOAT32>(3), null)              -- null
RETURN vector.norm([1,2,3]::VECTOR<FLOAT32>(3), 'MAHALANOBIS')     -- error
```

### Unit-normalisation pattern

There's no built-in `vector.normalize` — compose with a list and cast
the rebuilt coordinates:

```cypher
WITH [3, 0, 4]::VECTOR<FLOAT32>(3) AS v
WITH v, vector.norm(v, EUCLIDEAN) AS n, vector.coordinates(v, FLOAT) AS coords
RETURN [coords[0] / n, coords[1] / n, coords[2] / n]::VECTOR<FLOAT32>(3) AS unit
       -- [0.6, 0.0, 0.8]
```

For variable-dimension unit-normalisation, this is easier to keep
host-side — the client languages all ship vector parameter helpers.

## Introspection

| Expression | Returns | Notes |
|---|---|---|
| `type.of(v)` | `String` — `"VECTOR<TYPE>(DIM)"` | Only type whose tag encodes structure |
| `value.size(v)` | `Int` — dimension | Same as `vector.dimension` |
| `vector.dimension(v)` | `Int` — dimension | Explicit name |

```cypher
WITH [1, 2, 3, 4]::VECTOR<FLOAT32>(4) AS v
RETURN type.of(v)                 AS t,   -- 'VECTOR<FLOAT32>(4)'
       value.size(v)               AS s,   -- 4
       vector.dimension(v)         AS d    -- 4

-- Coordinate type is part of the type tag
RETURN type.of([1,2,3]::VECTOR<INTEGER>(3))   -- 'VECTOR<INTEGER>(3)'
RETURN type.of([1,2,3]::VECTOR<INT8>(3))      -- 'VECTOR<INTEGER8>(3)'
```

### Guarding by shape

```cypher
MATCH (d:Doc)
WHERE type.of(d.embedding) = 'VECTOR<FLOAT32>(384)'
RETURN d.id AS id
```

```cypher
MATCH (d:Doc)
WHERE vector.dimension(d.embedding) = 384
RETURN count(*) AS docs_with_384d
```

## List conversion

| Function | From | To |
|---|---|---|
| `vector.coordinates(v, INTEGER)` | any `VECTOR` | `LIST<INTEGER>` — truncates toward zero |
| `vector.coordinates(v, FLOAT)` | any `VECTOR` | `LIST<FLOAT>` — widens exact |

```cypher
RETURN vector.coordinates([1.9, -1.9, 3.0]::VECTOR<FLOAT32>(3), INTEGER)
       -- [1, -1, 3]
RETURN vector.coordinates([1, 2, 3]::VECTOR<INT8>(3), FLOAT)
       -- [1.0, 2.0, 3.0]

-- null propagates
RETURN vector.coordinates(null, INTEGER)   -- null
RETURN vector.coordinates(null, FLOAT)     -- null

-- non-VECTOR input errors
RETURN vector.coordinates([1, 2, 3], FLOAT) -- error
```

Both converters round-trip cleanly through the binding layer — use
them when you need to hand off to a caller that wants a plain array.

## kNN and retrieval patterns

### Top-k by cosine similarity

```cypher
MATCH (d:Doc)
RETURN d.id AS id, d.title AS title
ORDER BY vector.similarity(d.embedding, $query) DESC
LIMIT 10
```

### Top-k carrying the score forward

```cypher
MATCH (d:Doc)
WITH d, vector.similarity(d.embedding, $query) AS score
RETURN d.id AS id, d.title AS title, score
ORDER BY score DESC
LIMIT 10
```

### Top-k by signed distance (smaller = closer)

```cypher
MATCH (d:Doc)
WITH d, vector.distance(d.embedding, $query, EUCLIDEAN) AS dist
RETURN d.id AS id, dist
ORDER BY dist ASC
LIMIT 10
```

### Cheaper ranking with `EUCLIDEAN_SQUARED`

The rankings are identical; `EUCLIDEAN_SQUARED` skips the `sqrt`.

```cypher
MATCH (d:Doc)
WITH d, vector.distance(d.embedding, $query, EUCLIDEAN_SQUARED) AS d2
RETURN d.id AS id
ORDER BY d2 ASC
LIMIT 20
```

### Narrow the candidate set first

Similarity is `O(n)` over matched nodes — push filters into `MATCH`
and `WHERE` before scoring.

```cypher
MATCH (d:Doc {tenant: $tenant})
WHERE d.language = 'en' AND d.published_at >= '2026-01-01'::DATE
WITH  d, vector.similarity(d.embedding, $query) AS score
RETURN d.id, score
ORDER BY score DESC
LIMIT 10
```

### Score threshold

```cypher
MATCH (d:Doc)
WITH d, vector.similarity(d.embedding, $query) AS score
WHERE score >= 0.75
RETURN d.id, score
ORDER BY score DESC
```

### Graph-filtered retrieval

The reason `VECTOR` lives next to the graph — score first, then use
relationships to explain or filter.

```cypher
MATCH (d:Doc)
WITH d, vector.similarity(d.embedding, $query) AS score
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
WITH d, vector.similarity(d.embedding, $query) AS score
ORDER BY score DESC
LIMIT 10
MATCH (d)-[:CITED_BY]->(citing:Doc)
RETURN d.id AS hit, score, collect(citing.id) AS citations
```

### Per-category nearest

```cypher
MATCH (d:Doc)
WITH d, d.category AS category,
     vector.similarity(d.embedding, $query) AS score
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
WHERE string.lower(d.title) CONTAINS string.lower($q)
   OR string.lower(d.body)  CONTAINS string.lower($q)
WITH d,
     vector.similarity(d.embedding, $query) AS vec_score,
     CASE WHEN string.lower(d.title) CONTAINS string.lower($q) THEN 0.2 ELSE 0.0 END AS title_boost
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
              vector.similarity(candidate.embedding, $query) AS score
RETURN candidate.id, score
ORDER BY score DESC
LIMIT 10
```

### Multi-vector query (best-of / max-sim)

Score each document against several query vectors and keep the best:

```cypher
UNWIND $queries AS q
MATCH (d:Doc)
WITH d, max(vector.similarity(d.embedding, q)) AS best
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
               acc + vector.similarity(d.embedding, q) / value.size(qs)) AS score
RETURN d.id, score
ORDER BY score DESC
LIMIT 10
```

### Metric comparison side-by-side

Useful during debugging — show every metric for one candidate:

```cypher
WITH [0.10, 0.20, 0.30]::VECTOR<FLOAT32>(3) AS q
MATCH (d:Doc {id: $id})
RETURN d.id,
       vector.similarity    (d.embedding, q)               AS cos_bounded,
       vector.similarity(d.embedding, q, 'euclidean')       AS euc_bounded,
       vector.distance(d.embedding, q, EUCLIDEAN)                 AS l2,
       vector.distance(d.embedding, q, EUCLIDEAN_SQUARED)         AS l2_sq,
       vector.distance(d.embedding, q, MANHATTAN)                 AS l1,
       vector.distance(d.embedding, q, COSINE)                    AS cos_dist,
       vector.distance(d.embedding, q, DOT)                       AS neg_dot
```

### Count above threshold

```cypher
MATCH (d:Doc)
WITH d, vector.similarity(d.embedding, $query) AS score
WHERE score >= $threshold
RETURN count(*) AS hits
```

### Bucket by similarity band

```cypher
MATCH (d:Doc)
WITH vector.similarity(d.embedding, $query) AS score
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
WITH d, vector.similarity(d.embedding, $query) AS score
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
  [1, 2, 3]::VECTOR<INTEGER>(3),
  [1, 2, 3]::VECTOR<INTEGER>(3),
  [1, 2, 3]::VECTOR<INTEGER8>(3)
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
ORDER BY vector.similarity(d.embedding, $query) DESC
LIMIT 5
```

### Zero vectors and cosine

Cosine on a zero-norm vector is undefined, so
`vector.similarity([0,…], anything)` returns `null`. Filter
or coalesce explicitly:

```cypher
MATCH (d:Doc)
WITH d, coalesce(vector.similarity(d.embedding, $query), 0.0) AS score
RETURN d.id, score
ORDER BY score DESC
LIMIT 10
```

### Integer-backed vectors in similarity

Integer coordinates widen to `f32` before accumulation — identical
ranking behaviour to a `FLOAT32` vector with the same values, modulo
precision loss for magnitudes that don't fit in the mantissa.

```cypher
RETURN vector.similarity([1,2,3]::VECTOR<INT8>(3),
                                [2,4,6]::VECTOR<INT8>(3))
       -- 1.0 (colinear, same result as FLOAT32)
```

### HTTP and parameters

`POST /query` does not yet accept a `params` field, so a vector cannot
ride in as a parameter over HTTP. Either embed the vector literally
in the query — using the string form makes this practical —

```bash
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query":"RETURN [0.1,0.2,0.3]::VECTOR<FLOAT32>(3) AS v"}'
```

— or use one of the in-process bindings, which all support parameters.

## Index-backed retrieval

For the supported index procedure surface, create a vector index and
query it with `CALL`:

```cypher
CREATE VECTOR INDEX doc_embedding
FOR (d:Doc)
ON (d.embedding)
OPTIONS {indexConfig: {
  `vector.dimensions`: 384,
  `vector.similarity_function`: 'cosine'
}};

CALL db.index.vector.queryNodes('doc_embedding', 10, $query)
YIELD node, score;
```

This returns the top `k` rows by descending score. `k` must be
positive, and the query vector dimension must match the index
configuration. See [Queries → Indexes → Vector indexes](../queries/indexes#vector-indexes)
for relationship indexes and option details.

## Limitations

- **No ANN structure yet** — vector index procedures are supported, but
  currently scan the indexed label/type scope linearly.
- **Direct vector function calls are exhaustive** — keep `MATCH`
  filters tight when using `ORDER BY vector.similarity(...) LIMIT k`.
- **No embedding generation** — LoraDB has no plugin surface. Produce
  embeddings host-side and pass them in.
- **No list-of-vectors as a property** — store each vector on its own
  node or relationship. Lists of vectors inside a query are fine.
- **No parameters over HTTP** — see the note above.
- **Dimension ≤ 4096** — enforced at construction.
- **Ordering by a vector column is unspecified** — order by a scalar
  score instead.

See also the [Cypher support matrix (§13b)](https://github.com/lora-db/lora/blob/main/docs/reference/cypher-support-matrix.md#13b-vector-types-and-functions)
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
