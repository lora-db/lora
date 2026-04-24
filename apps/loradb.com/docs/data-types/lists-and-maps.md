---
title: Lists and Maps
sidebar_label: Lists & Maps
description: LoraDB's two composite value types — ordered nestable lists and string-keyed maps — including literals, indexing, slicing, nesting rules, and Cypher operations.
---

# Lists and Maps

Lists and maps are LoraDB's two composite types. Both can nest and
both can store any supported value — including other lists, maps, and
[typed values](./overview) like temporals and points.

## Lists

Ordered, heterogeneous, nestable.

```cypher
RETURN [1, 2, 3]
RETURN ['a', 'b', 'c']
RETURN [1, 'two', true, null]        -- heterogeneous is fine
RETURN [[1, 2], [3, 4]]              -- nested
```

### Indexing

Zero-based; negatives count from the end; out-of-range → `null`.

```cypher
RETURN [10, 20, 30][0]       -- 10
RETURN [10, 20, 30][-1]      -- 30
RETURN [10, 20, 30][9]       -- null
```

### Slicing

End-exclusive. Open-ended slices work.

```cypher
RETURN [1, 2, 3, 4, 5][1..3]   -- [2, 3]
RETURN [1, 2, 3, 4, 5][..2]    -- [1, 2]
RETURN [1, 2, 3, 4, 5][3..]    -- [4, 5]
RETURN [1, 2, 3, 4, 5][-2..]   -- [4, 5]
```

### Concatenation

```cypher
RETURN [1, 2] + [3, 4]       -- [1, 2, 3, 4]
RETURN [1, 2] + 3            -- [1, 2, 3]
RETURN 0 + [1, 2]            -- [0, 1, 2]
```

### Comprehensions

```cypher
-- filter
RETURN [x IN [1, 2, 3, 4] WHERE x > 2]       -- [3, 4]
-- map
RETURN [x IN [1, 2, 3] | x * 10]             -- [10, 20, 30]
-- filter + map
RETURN [x IN [1, 2, 3, 4] WHERE x > 2 | x * 10]   -- [30, 40]
```

See more in [List Functions → list comprehension](../functions/list#list-comprehension).

### Predicates (in [`WHERE`](../queries/where))

```cypher
MATCH (n) WHERE all(x IN n.scores WHERE x >= 0)        RETURN n
MATCH (n) WHERE any(x IN n.tags   WHERE x = 'VIP')     RETURN n
MATCH (n) WHERE none(x IN n.scores WHERE x < 0)        RETURN n
MATCH (n) WHERE single(x IN n.roles WHERE x = 'owner') RETURN n
```

### More list functions

See [List Functions](../functions/list) for the full list — includes
`size`, `head`, `tail`, `range`, `reduce`, comprehensions, and pattern
comprehensions.

### Parameters

```cypher
MATCH (u:User)
WHERE u.id IN $ids
RETURN u
```

The `$ids` parameter must bind to a list. Lists in parameters may be
heterogeneous.

### Unwinding a list into rows

Combine with [`UNWIND`](../queries/unwind-merge#unwind) when you want
one row per element — the bulk-load idiom:

```cypher
UNWIND $items AS it
CREATE (:Item {sku: it.sku, price: it.price})
```

### Lists from aggregation

[`collect`](../functions/aggregation#collect) turns rows into a list:

```cypher
MATCH (u:User)-[:OWNS]->(r:Repo)
RETURN u.name, collect(r.name) AS repos
```

## Maps

String-keyed dictionaries.

```cypher
RETURN {name: 'Ada', born: 1815}
RETURN {name: 'Ada', roles: ['admin', 'user'], profile: {tier: 'gold'}}
```

### Key access

Dot access for static keys, bracket access for computed keys.

```cypher
WITH {name: 'Ada', born: 1815} AS m
RETURN m.name, m.born, m['name']
```

Accessing a missing key returns `null`.

```cypher
WITH {a: 1} AS m
RETURN m.b                   -- null
```

### Build a map from variables

```cypher
MATCH (u:User)
RETURN {id: u.id, name: u.name, active: u.active} AS user
```

### Dynamic keys

```cypher
WITH 'red' AS color
RETURN {[color]: 1}          -- {red: 1}
```

### Concatenation / merge

`+` merges two maps, right-hand keys win:

```cypher
RETURN {a: 1, b: 2} + {b: 20, c: 3}
-- {a: 1, b: 20, c: 3}
```

Useful when combining with [`SET += $patch`](../queries/set-delete#merge-properties-):

```cypher
MATCH (u:User {id: $id})
SET u += $patch
```

### Maps on entities

Property maps on nodes / relationships are the same shape as literal
maps:

```cypher
CREATE (p:Person {name: 'Ada', born: 1815, skills: ['math', 'cs']})
MATCH  (p:Person) RETURN p.name, p.skills
```

### Map projection

Project a subset of an entity's properties into a map — useful for
shaping results:

```cypher
MATCH (p:Person)
RETURN p {.name, .born}           -- pick keys
RETURN p {.*}                     -- all properties
RETURN p {.name, yob: p.born}     -- rename / computed
RETURN p {.name, friends: [(p)-[:KNOWS]->(f) | f.name]}
```

The last form embeds a [pattern comprehension](../functions/list#pattern-comprehension)
inline.

### `keys` and `properties`

```cypher
MATCH (p:Person)
RETURN keys(p), properties(p)
-- ['born', 'name'], {born: 1815, name: 'Ada'}
```

### Parameters

Maps bind directly — useful as a bulk-update pattern:

```cypher
MATCH (u:User {id: $id})
SET u += $patch
RETURN u
```

Where `$patch = {name: 'New Name', active: true}` from the host
language.

### UNWIND lists of maps

The idiomatic [bulk-load pattern](../queries/unwind-merge#unwind):

```cypher
UNWIND $rows AS row
CREATE (:Person {name: row.name, born: row.born})
```

## Serialisation

Across the HTTP boundary and in binding results:

| Value | Shape |
|---|---|
| `List` | JSON array |
| `Map` | JSON object |

Nested lists and maps round-trip cleanly. Typed values inside
(temporals, points, nodes) retain their `kind` discriminators.

## Common patterns

### Zip two lists

```cypher
WITH ['a', 'b', 'c'] AS keys, [1, 2, 3] AS vals
RETURN [i IN range(0, size(keys) - 1) | [keys[i], vals[i]]]
```

### Distinct list

No built-in `distinct(list)` helper — use `collect(DISTINCT x)` after
`UNWIND`:

```cypher
UNWIND [1, 2, 2, 3, 3, 3] AS x
RETURN collect(DISTINCT x)   -- [1, 2, 3]
```

### Top-N inside a collected list

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, p ORDER BY p.published_at DESC
WITH u, collect(p.title)[..3] AS recent_three
RETURN u.name, recent_three
```

### Map of counts

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u.region AS region, count(*) AS posts
RETURN collect({region: region, posts: posts}) AS summary
```

### Nested projection

```cypher
MATCH (u:User)
RETURN u {
  .id,
  .name,
  posts: [(u)-[:WROTE]->(p:Post) | p {.id, .title}]
}
```

One row per user; each row's `posts` is a list of small maps.

### List membership vs map key check

```cypher
-- IN on a list
RETURN 2 IN [1, 2, 3]              -- true

-- 'in keys' on a map
WITH {a: 1, b: 2} AS m
RETURN 'a' IN keys(m)              -- true
```

### Append without duplicates

No `distinct_list` helper; compose with a list predicate:

```cypher
WITH [1, 2, 3] AS xs, 2 AS new_x
RETURN CASE WHEN new_x IN xs THEN xs ELSE xs + new_x END
-- [1, 2, 3]
```

### Index of first match

```cypher
WITH ['a', 'b', 'c', 'd'] AS xs, 'c' AS needle
RETURN head([i IN range(0, size(xs) - 1) WHERE xs[i] = needle])
-- 2
```

Uses a list comprehension to filter and [`head`](../functions/list#size--head--tail--last)
to pick the first.

### Merge nested maps

`+` on maps is shallow — to deep-merge, build the merged value
explicitly:

```cypher
WITH {a: 1, nested: {x: 1, y: 2}} AS base,
     {nested: {y: 99, z: 3}, b: 2} AS patch
RETURN base + patch + {nested: base.nested + patch.nested}
-- {a: 1, b: 2, nested: {x: 1, y: 99, z: 3}}
```

## Edge cases

### Empty list / empty map

```cypher
RETURN size([]), size({})    -- 0, 0
RETURN head([])              -- null
```

`UNWIND []` emits zero rows. Watch for unbound parameters — an unset
`$rows` resolves to `null` and UNWIND of `null` also emits zero rows.
See [UNWIND → empty list](../queries/unwind-merge#empty-list).

### Heterogeneous lists

Nothing enforces uniform element types. Use
[`valueType`](../functions/overview#type-conversion-and-checking) in
`all(… WHERE …)` if you need a guarantee.

### Missing map keys

```cypher
WITH {a: 1} AS m
RETURN m.b              -- null
RETURN m['b']           -- null
```

No exception — missing keys silently return `null`. Be careful in
arithmetic: `m.b + 1` becomes `null`.

### Comparing maps

Equality checks compare all key/value pairs recursively. Ordering of
keys doesn't matter.

```cypher
RETURN {a: 1, b: 2} = {b: 2, a: 1}    -- true
```

## Limitations

- List element types are not constrained — there is no `List<Integer>`
  constraint at creation time.
- Maps don't preserve any specific key ordering in responses — rely on
  explicit projection (`RETURN m {.key}`) if order matters to your
  consumer.
- No built-in `distinct(list)` helper — use `collect(DISTINCT x)`
  after `UNWIND`.

## See also

- [**List Functions**](../functions/list) — every list helper.
- [**Aggregation → collect**](../functions/aggregation#collect).
- [**UNWIND**](../queries/unwind-merge#unwind) / [**MERGE**](../queries/unwind-merge#merge).
- [**RETURN → map projection**](../queries/return-with#map-projection).
- [**Properties**](../concepts/properties) — maps on entities.
