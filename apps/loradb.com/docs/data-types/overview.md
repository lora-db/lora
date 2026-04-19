---
title: Supported Data Types in LoraDB
sidebar_label: Overview
---

# Supported Data Types in LoraDB

Every value in LoraDB — stored as a [property](../concepts/properties),
projected in a [`RETURN`](../queries/return-with), or bound as a
[parameter](../queries/#parameters) — has one of the types below.

## Category pages

| Category | Pages |
|---|---|
| **Scalars** (null, boolean, integer, float, string) | [Scalars](./scalars) |
| **Collections** (lists, maps) | [Lists & Maps](./lists-and-maps) |
| **Temporal** (date, time, datetime, duration) | [Temporal](./temporal) |
| **Spatial** (points) | [Spatial](./spatial) |
| **Graph** (node, relationship, path) | [below](#graph-types) |

## At-a-glance table

| Type | Literal | `valueType` |
|---|---|---|
| `Null` | `null` | `"NULL"` |
| `Boolean` | `true`, `false` | `"BOOLEAN"` |
| `Integer` | `42`, `0xFF`, `0o17` | `"INTEGER"` |
| `Float` | `3.14`, `1.0e10` | `"FLOAT"` |
| `String` | `'hi'`, `"there"` | `"STRING"` |
| `List` | `[1, 2, 3]` | `"LIST<T>"` |
| `Map` | `{k: v}` | `"MAP"` |
| `Date`, `Time`, `DateTime`, `LocalTime`, `LocalDateTime` | via constructor | `"DATE"`, `"TIME"`, … |
| `Duration` | `duration('P30D')` | `"DURATION"` |
| `Point` | `point({x, y})` | `"POINT"` |
| `Node`, `Relationship`, `Path` | produced by queries | `"NODE"`, … |

## Where each type shows up

| Source | Lifetime |
|---|---|
| Node / relationship property | Persists in the graph until deleted |
| `RETURN` expression | One row, then gone |
| Parameter | Per-query call |
| `WITH` binding | Current pipeline stage |
| Function argument / result | Per expression |

Graph types (`Node`, `Relationship`, `Path`) are special: they appear
only in results, never as storable property values.

## Graph types

Produced by queries; not storable as properties.

| Type | Hydrated shape |
|---|---|
| `Node` | `{kind: "node", id, labels, properties}` |
| `Relationship` | `{kind: "relationship", id, src, dst, type, properties}` |
| `Path` | `{kind: "path", nodes, rels}` — alternating sequence |

```cypher
MATCH (a:Person)-[r:KNOWS]->(b:Person)
RETURN a, r, b   -- a, b are Nodes; r is a Relationship
```

Narrow graph-typed results in host code with `isNode` /
`isRelationship` / `isPath` (JS —
[Node](../getting-started/node#type-guards)) or `is_node` /
`is_relationship` / `is_path` (Python —
[guards](../getting-started/python#c-structured-result-handling)).

## Runtime type checking

Use [`valueType(x)`](../functions/overview#type-conversion-and-checking)
to discover a value's type at query time.

```cypher
RETURN valueType(1),                    -- 'INTEGER'
       valueType([1, 2, 3]),            -- 'LIST<INTEGER>'
       valueType(date('2024-01-15')),   -- 'DATE'
       valueType(point({x: 1, y: 2}))   -- 'POINT'
```

### Filter by runtime type

Useful on heterogeneous list properties:

```cypher
MATCH (n:Record)
WHERE all(x IN n.values WHERE valueType(x) = 'INTEGER')
RETURN n
```

### Distinguish graph types

```cypher
MATCH (n)
RETURN valueType(n) AS t, count(*) ORDER BY count(*) DESC
-- typically all NODE, but useful for generic projections
```

## Conversion matrix

| From → To | `toInteger` | `toFloat` | `toString` | `toBoolean` |
|---|---|---|---|---|
| `Boolean` | `1` / `0` | `1.0` / `0.0` | `'true'` / `'false'` | — |
| `Integer` | — | `Float(n)` | decimal digits | `0 → false`, non-zero → `true` |
| `Float` | truncates | — | decimal string | — |
| `String` | parses / null | parses / null | — | `'true'` / `'false'` / null |
| `Null` | `null` | `null` | `null` | `null` |

See [`toString` / `toInteger` / `toFloat` / `toBoolean`](../functions/string#type-conversion)
for the full specification.

## Parameter binding

Host language values map to LoraDB types as follows — see your
binding's "Parameters" section for specifics:

| Host | LoraDB |
|---|---|
| `null` / `None` / `undefined` | `Null` |
| `bool` / `boolean` | `Boolean` |
| `int` / integer `number` / `i64` | `Integer` |
| `float` / non-integer `number` / `f64` | `Float` |
| `str` / `String` | `String` |
| `list` / `array` / `Vec` | `List` |
| `dict` / `object` / `BTreeMap` | `Map` |
| helpers (`date()`, `wgs84()`, …) | `Date`, `Point`, etc. |

Details: [Rust](../getting-started/rust#b-parameterised-query),
[Node](../getting-started/node#b-parameterised-query),
[Python](../getting-started/python#b-parameterised-query),
[WASM](../getting-started/wasm#b-parameterised-query).

## Null across types

Every type has a single sentinel `Null` value — there's no
`Integer null` distinct from `String null`. Implications:

- `null = null` is **not** `true` — it's `null`. Use
  [`IS NULL`](../queries/where#null-checks).
- `valueType(null)` is `'NULL'`, not `'NULL<INTEGER>'`.
- Missing map keys and missing properties return `null`, so
  a null property and an absent property are indistinguishable. See
  [Properties → Missing vs null](../concepts/properties#missing-vs-null).
- Arithmetic and most functions propagate `null` — use
  [`coalesce`](../functions/overview#type-conversion-and-checking) or
  [`CASE`](../queries/return-with#case-expressions) to supply defaults.

## Equality and ordering semantics

| Type | Equality | Ordering |
|---|---|---|
| Scalars (`Boolean`, `Integer`, `Float`, `String`) | Per-type | Per-type total order |
| `Null` | Propagates to `null` | Last ASC / first DESC |
| `List` | Element-wise, same length | Lex (element-by-element) |
| `Map` | Key/value set equal | — (not ordered) |
| `Point` | All components + SRID | — (not ordered) |
| Temporals | Same type, same instant | Per-type chronological |
| `Node` / `Relationship` | By internal `id()` | — |
| `Path` | Structural equality | — |

Anything left unordered has equality only — attempting
[`ORDER BY`](../queries/ordering) on a map/point/node column doesn't
raise, but the sort is effectively a no-op.

## What isn't a type

- **Binary / byte arrays** — store base64 strings in `String`.
- **Fixed-precision decimals** — use scaled integers or strings.
- **User-defined types** — not supported.
- **Enums** — use a string or integer by convention.

See [Limitations](../limitations#data-types) for the full list.

## See also

- [**Scalars**](./scalars), [**Lists & Maps**](./lists-and-maps),
  [**Temporal**](./temporal), [**Spatial**](./spatial) — per-type reference.
- [**Functions → Overview**](../functions/overview) — `toString`, `coalesce`, `valueType`.
- [**Properties**](../concepts/properties) — how types attach to entities.
- [**Queries → Parameters**](../queries/#parameters) — binding typed values from the host.
