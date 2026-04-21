---
title: Query Parameters
sidebar_label: Parameters
---

# Query Parameters

Parameters are the **only** safe way to mix host-side values into a
query. Every binding accepts them; the HTTP transport does not yet
forward them (see [caveat](#http-api-doesnt-forward-params)).

```cypher
MATCH (u:User) WHERE u.id = $id RETURN u
```

`$id` is a placeholder. The host supplies its value at call time.

## Why parameters

- **Safety.** Parameters cannot rewrite the query shape — no
  injection. Inlining untrusted values is unsafe.
- **Cacheability.** The parser and analyzer can reuse the same plan
  across calls with different `$id` values.
- **Type fidelity.** Typed values (dates, points, bigints) survive the
  trip — no string round-trip through the query text.

## Binding parameters by host

### Rust

```rust
use std::collections::BTreeMap;
use lora_database::{Database, LoraValue};

let db = Database::in_memory();

let mut params: BTreeMap<String, LoraValue> = BTreeMap::new();
params.insert("name".into(), LoraValue::String("Ada".into()));
params.insert("min".into(),  LoraValue::Int(1800));

db.execute_with_params(
    "MATCH (p:Person)
     WHERE p.name = $name AND p.born >= $min
     RETURN p.name AS name",
    None,
    params,
)?;
```

More detail: [Rust → Parameterised query](../getting-started/rust#parameterised-query).

### Node / TypeScript

```ts
await db.execute(
  "MATCH (p:Person) WHERE p.name = $name RETURN p",
  { name: 'Ada' }
);
```

More detail: [Node → Parameterised query](../getting-started/node#parameterised-query).

### Python

```python
db.execute(
    "MATCH (p:Person) WHERE p.name = $name RETURN p",
    {"name": "Ada"},
)
```

More detail: [Python → Parameterised query](../getting-started/python#parameterised-query).

### WASM

```ts
await db.execute(
  "MATCH (u:User) WHERE u.handle = $handle RETURN u",
  { handle: 'alice' }
);
```

More detail: [WASM → Parameterised query](../getting-started/wasm#parameterised-query).

## Host → LoraDB type mapping

| Host value | LoraDB type |
|---|---|
| `null` / `None` / `undefined` | [`Null`](../data-types/scalars#null) |
| `bool` / `boolean` | [`Boolean`](../data-types/scalars#boolean) |
| `int` / integer `number` / `i64` | [`Integer`](../data-types/scalars#integer) |
| `float` / non-integer `number` / `f64` | [`Float`](../data-types/scalars#float) |
| `str` / `String` | [`String`](../data-types/scalars#string) |
| list / array / `Vec` | [`List`](../data-types/lists-and-maps#lists) |
| dict / object / `BTreeMap` | [`Map`](../data-types/lists-and-maps#maps) |
| `date()` helper | [`Date`](../data-types/temporal) |
| `datetime()` / `localdatetime()` helper | [`DateTime`](../data-types/temporal) / [`LocalDateTime`](../data-types/temporal) |
| `duration()` helper | [`Duration`](../data-types/temporal) |
| `wgs84()` / `cartesian()` helper | [`Point`](../data-types/spatial) |

Missing entries resolve to `null`. The engine doesn't raise on an
unbound parameter — it silently filters everything out. Audit bindings
when a query returns no rows. See
[Troubleshooting → Silent filter from an unbound parameter](../troubleshooting#silent-filter-from-an-unbound-parameter).

## Where parameters can appear

| Position | Supported |
|---|---|
| Expression / literal (`p.age = $age`) | ✓ |
| Inline map property (`{id: $id}`) | ✓ |
| List expression (`$ids`) | ✓ |
| `UNWIND $rows AS row` | ✓ |
| Pattern label / relationship type | ✗ — see [Limitations](../limitations#parameters) |
| Property key name | ✗ |

The unsupported positions would let a parameter rewrite the query
shape. If you genuinely need a dynamic label, compose the query
string host-side from a trusted allow-list — never from raw input.

## Common patterns

### Bulk load from a list

```cypher
UNWIND $rows AS row
CREATE (:User {id: row.id, name: row.name})
```

```ts
await db.execute(
  "UNWIND $rows AS row CREATE (:User {id: row.id, name: row.name})",
  { rows: [{ id: 1, name: 'Ada' }, { id: 2, name: 'Grace' }] }
);
```

See [`UNWIND` → bulk load](./unwind-merge#bulk-load-from-parameter).

### Dynamic `IN`-style filter

```cypher
MATCH (u:User) WHERE u.id IN $ids RETURN u
```

```python
db.execute(
    "MATCH (u:User) WHERE u.id IN $ids RETURN u",
    {"ids": [1, 2, 3, 4]},
)
```

### Pass-through typed values

```ts
import { wgs84, duration } from 'lora-node';

await db.execute(
  "CREATE (:Trip {origin: $here, span: $span})",
  { here: wgs84(4.89, 52.37), span: duration('PT90M') }
);
```

### Default a missing value host-side

LoraDB doesn't have "default parameter values" — bind `null`
explicitly (or use [`coalesce`](../functions/overview#type-conversion-and-checking)
in the query) when the caller omits a field:

```ts
await db.execute(
  "MATCH (u:User) WHERE u.tier = coalesce($tier, u.tier) RETURN u",
  { tier: opts?.tier ?? null }
);
```

## HTTP API doesn't forward params

:::caution

`POST /query` currently ignores any `params` body field. Bind via the
Rust / Node / Python / WASM bindings, or build the literal into the
query string when values are trusted and encoded. Parameters over
HTTP are on the roadmap — see
[Limitations → Parameters](../limitations#parameters).

:::

If you must use HTTP today with dynamic values, serialise the value
into the query yourself via a trusted encoder:

```bash
NAME='Ada'
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  --data-binary "$(jq -n --arg q "MATCH (p:Person {name: '$NAME'}) RETURN p" \
                   '{query: $q}')"
```

For anything user-supplied, run against a local binding with real
parameters and expose a narrower API on top.

## Common mistakes

### Unbound parameter

The query parses, runs, returns zero rows. Cause: the host didn't
bind `$id` at all. Fix: audit the params map, or validate inputs
before executing.

### Wrong type

`{id: $id}` with host value `"1"` (a string) won't match an integer
property. Use the right host type, or coerce inside the query:

```cypher
MATCH (n:User) WHERE toString(n.id) = $id RETURN n
```

### Inlining untrusted input

```ts
// Don't do this
await db.execute(`MATCH (u:User {name: '${req.query.name}'}) RETURN u`);
```

Use `$name` and pass `{ name: req.query.name }` instead. Parameters
are the only supported safe mixing mechanism.

## See also

- [Queries → Overview](./) — pipeline and clauses.
- [Data Types → Overview](../data-types/overview) — every value the
  engine understands.
- [UNWIND + MERGE](./unwind-merge) — bulk load and upserts with
  parameters.
- [Troubleshooting → Parameters](../troubleshooting#parameters) —
  silent filtering, HTTP ignore, integer precision.
- [Limitations → Parameters](../limitations#parameters) — HTTP and
  identifier positions.
