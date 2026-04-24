---
title: Built-in Functions in LoraDB
sidebar_label: Overview
description: The built-in function library — string, math, list, aggregation, temporal, and spatial — with naming rules, null propagation, arity checking, and links to each per-category reference.
---

# Built-in Functions in LoraDB

Function names are **case-insensitive**; canonical camelCase is
shown on each page. Unknown names and wrong arity are rejected at
analysis time (`Unknown function 'foo'`, `WrongArity`).

Most functions **propagate `null`** — any `null` argument makes the
result `null`. The exceptions are the aggregates, `coalesce`,
`timestamp`, `pi`, `e`, and `rand`.

## Categories

| Category | Examples | Reference |
|---|---|---|
| **Aggregation** | <CypherCode code="count" />, <CypherCode code="sum" />, <CypherCode code="collect" />, <CypherCode code="percentileCont" /> | [Aggregation](./aggregation) |
| **String** | <CypherCode code="toLower" />, <CypherCode code="split" />, <CypherCode code="substring" />, <CypherCode code="replace" /> | [String](./string) |
| **Math** | <CypherCode code="abs" />, <CypherCode code="sqrt" />, <CypherCode code="sin" />, <CypherCode code="log" />, <CypherCode code="rand" /> | [Math](./math) |
| **List** | <CypherCode code="size" />, <CypherCode code="head" />, <CypherCode code="range" />, <CypherCode code="reduce" /> | [List](./list) |
| **Temporal** | <CypherCode code="date" />, <CypherCode code="datetime" />, <CypherCode code="duration.between" /> | [Temporal](./temporal) |
| **Spatial** | <CypherCode code="point" />, <CypherCode code="distance" /> | [Spatial](./spatial) |
| **Vector** | <CypherCode code="vector" />, <CypherCode code="vector.similarity.cosine" />, <CypherCode code="vector.similarity.euclidean" />, <CypherCode code="vector_distance" />, <CypherCode code="vector_norm" />, <CypherCode code="vector_dimension_count" />, <CypherCode code="toIntegerList" />, <CypherCode code="toFloatList" /> | [Vector](./vectors) |
| **Path** | <CypherCode code="length" />, <CypherCode code="nodes" />, <CypherCode code="relationships" /> | [Paths](../queries/paths) |

## Entity introspection

| Function | Takes | Returns |
|---|---|---|
| <CypherCode code="id(x)" /> | node \| relationship | `Int` — internal id |
| <CypherCode code="labels(n)" /> | node | `List<String>` |
| <CypherCode code="type(r)" /> | relationship | `String` — rel type |
| <CypherCode code="keys(x)" /> | node \| rel \| map | `List<String>` |
| <CypherCode code="properties(x)" /> | node \| rel \| map | `Map` |

```cypher
MATCH (u:User)-[r:FOLLOWS]->(v:User)
RETURN id(u), labels(u), type(r), keys(u), properties(u)
```

### Common uses

```cypher
// Dump every property on a node as a map
MATCH (u:User {id: $id}) RETURN properties(u)

// Discover which labels a node carries
MATCH (n) WHERE id(n) = $raw_id RETURN labels(n)

// Inspect the type of a matched edge
MATCH (a)-[r]->(b) RETURN type(r), count(*) ORDER BY count(*) DESC

// Avoid duplicate pair rows
MATCH (a)-[:KNOWS]-(b) WHERE id(a) < id(b) RETURN a, b
```

See [**Graph model → Identity**](../concepts/graph-model#identity) for
why `id()` is opaque.

## Type conversion and checking

| Function | Behaviour |
|---|---|
| <CypherCode code="toString(x)" /> | any → `String`; `null` → `null` |
| <CypherCode code="toInteger(x)" /> / <CypherCode code="toInt(x)" /> | `Int`/`Float`/`String`/`Bool` → `Int` or `null` |
| <CypherCode code="toFloat(x)" /> | `Int`/`Float`/`String` → `Float` or `null` |
| <CypherCode code="toBoolean(x)" /> / <CypherCode code="toBooleanOrNull(x)" /> | `Bool`/`String`/`Int` → `Bool` or `null` |
| <CypherCode code="valueType(x)" /> | name of the value's type, e.g. `"INTEGER"`, `"LIST<T>"` |
| <CypherCode code="coalesce(a, b, …)" /> | first non-null argument |
| <CypherCode code="timestamp()" /> | current Unix time in milliseconds |

```cypher
RETURN toInteger('42'),                         -- 42
       toInteger('abc'),                        -- null
       toFloat(42),                             -- 42.0
       coalesce(null, null, 'fallback'),        -- 'fallback'
       valueType(1),                            -- 'INTEGER'
       valueType([1, 2, 3]),                    -- 'LIST<INTEGER>'
       valueType(date('2024-01-15'))            -- 'DATE'
```

### coalesce recipes

```cypher
// Default a missing property
MATCH (p:Person) RETURN p.name, coalesce(p.nickname, p.name) AS display

// Cascade through several optional fields
RETURN coalesce($phone, $email, 'unknown') AS contact

// Replace null in ordering
MATCH (p:Person)
RETURN p.name, coalesce(p.rank, 999999) AS rank_for_sort
ORDER BY rank_for_sort
```

For multi-branch logic with arbitrary predicates per branch (not just
"first non-null"), use [`CASE`](../queries/return-with#case-expressions).

### valueType recipes

```cypher
// Filter a heterogeneous list to numbers only
MATCH (n)
WHERE all(x IN n.values WHERE valueType(x) = 'INTEGER')
RETURN n

// Group by runtime type
UNWIND [1, 'two', 3.0, true, null] AS x
RETURN valueType(x) AS t, count(*) AS n
ORDER BY t
```

### timestamp

Wall-clock milliseconds since the Unix epoch.

```cypher
MERGE (c:Counter {name: 'events'})
  ON CREATE SET c.first_seen = timestamp()
  SET c.last_seen = timestamp()
```

See [Data Types](../data-types/overview) for every `valueType` return
value and for how each type maps between LoraDB and host languages.

## Null propagation — the common thread

Most functions return `null` when any argument is `null`. A small
handful don't, so they're worth memorising:

- [Aggregates](./aggregation) (`count`, `sum`, …) skip null inputs
  (except `count(*)`, which counts rows).
- `coalesce(a, b, …)` — returns the first non-null argument.
- `timestamp()`, `pi()`, `e()`, `rand()` — take no arguments.

Everywhere else, expect `null` in → `null` out. This is what makes
[`IS NULL` / `IS NOT NULL`](../queries/where#null-checks) essential over
`= null`.

## Quick lookup

Finding the right function for a task:

| I want to… | Reach for |
|---|---|
| Pick the first non-null value | [<CypherCode code="coalesce(a, b, …)" />](#type-conversion-and-checking) |
| Branch on arbitrary conditions | [<CypherCode code="CASE WHEN … THEN … END" />](../queries/return-with#case-expressions) |
| Count rows matching a condition | [<CypherCode code="count(CASE WHEN … THEN 1 END)" />](./aggregation#count) |
| Concatenate a list into a string | [<CypherCode code="reduce" />](./list#reduce) over <CypherCode code="split" /> / <CypherCode code="collect" /> |
| Current time (ms) | [<CypherCode code="timestamp()" />](#timestamp) |
| Current calendar day | [<CypherCode code="date()" />](./temporal#date) |
| Name of a value's type | [<CypherCode code="valueType(x)" />](#type-conversion-and-checking) |
| Internal id of a node / rel | [<CypherCode code="id(x)" />](#entity-introspection) |
| Total order over temporal values | <CypherCode code="<" />, <CypherCode code="<=" />, <CypherCode code=">" />, <CypherCode code=">=" /> — see [Ordering](../queries/ordering) |
| Cartesian or geodesic distance | [<CypherCode code="distance(a, b)" />](./spatial#distance) |
| Score a VECTOR against a query vector | [<CypherCode code="vector.similarity.cosine(v, $q)" />](../data-types/vectors#bounded-similarity-in-0-1) |
| Signed distance under a metric | [<CypherCode code="vector_distance(a, b, EUCLIDEAN)" />](../data-types/vectors#signed-distance-metrics) |
| Magnitude of a VECTOR | [<CypherCode code="vector_norm(v, EUCLIDEAN)" />](../data-types/vectors#vector-norms) |
| Dimension of a VECTOR | [<CypherCode code="vector_dimension_count(v)" />](../data-types/vectors#introspection) or <CypherCode code="size(v)" /> |
| Convert VECTOR coordinates back to a LIST | [<CypherCode code="toIntegerList(v)" /> / <CypherCode code="toFloatList(v)" />](../data-types/vectors#introspection) |

## Not supported

- **APOC-style utilities** (`apoc.*`) — no compatibility layer.
- **Procedures** (`CALL db.labels()` etc.) — rejected at analysis time.
- **User-defined functions** — no registration surface.

Full list in [Limitations](../limitations).

## See also

- [**Aggregation Functions**](./aggregation) — `count`, `collect`, percentiles.
- [**String**](./string), [**Math**](./math), [**List**](./list) — everyday helpers.
- [**Temporal**](./temporal), [**Spatial**](./spatial) — typed domains.
- [**Data Types Overview**](../data-types/overview) — value shapes.
- [**Queries → Parameters**](../queries/parameters) — binding typed values from the host.
