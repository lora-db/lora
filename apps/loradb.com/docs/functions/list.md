---
title: List Functions
sidebar_label: List
description: List functions in LoraDB — size, head, tail, last, range, reverse, reduce, and friends — with 0-based indexing, negative indices, and null-propagation semantics.
---

# List Functions

[Lists](../data-types/lists-and-maps#lists) are first-class values.
Indexing is 0-based and supports negative indices counted from the end.
List functions generally return `null` on `null` input.

## Overview

| Goal | Function / Syntax |
|---|---|
| Size | [<CypherCode code="size(list)" />](#size--head--tail--last) |
| First, rest, last | [<CypherCode code="head" />, <CypherCode code="tail" />, <CypherCode code="last" />](#size--head--tail--last) |
| Reverse | [<CypherCode code="reverse(list)" />](#reverse) |
| Range | [<CypherCode code="range(start, end[, step])" />](#range) |
| Fold | [`reduce(acc, x IN list | …)`](#reduce) |
| Index / slice | [<CypherCode code="list[i]" />, <CypherCode code="list[a..b]" />](#indexing-and-slicing) |
| Concat | [<CypherCode code="list + list" />, <CypherCode code="list + x" />, <CypherCode code="x + list" />](#concatenation) |
| Filter / map | [`[x IN list WHERE … | …]`](#list-comprehension) |
| Attach related entities | [Pattern comprehension](#pattern-comprehension) |
| Quantify | [<CypherCode code="all" />, <CypherCode code="any" />, <CypherCode code="none" />, <CypherCode code="single" />](#predicates-in-where) |
| Collect rows | [<CypherCode code="collect(expr)" />](./aggregation#collect) |
| Unwind rows | [<CypherCode code="UNWIND list AS row" />](../queries/unwind-merge#unwind) |

## size / head / tail / last

| Function | Behaviour |
|---|---|
| `size(list)` | Number of elements; `null` → `null` |
| `head(list)` | First element; empty list → `null` |
| `tail(list)` | All but first; empty list → `null` |
| `last(list)` | Last element; empty list → `null` |

```cypher
RETURN size([1, 2, 3])            -- 3
RETURN head([1, 2, 3])            -- 1
RETURN tail([1, 2, 3])            -- [2, 3]
RETURN last([1, 2, 3])            -- 3
RETURN head([])                   -- null
RETURN size([])                   -- 0
```

`size` also works on strings — see
[`String Functions → size`](./string#size--length--charlength).

## reverse

Works on lists and strings.

```cypher
RETURN reverse([1, 2, 3])         -- [3, 2, 1]
RETURN reverse('abc')             -- 'cba'
```

## range

`range(start, end[, step])` — inclusive, integers only.

```cypher
RETURN range(1, 5)                -- [1, 2, 3, 4, 5]
RETURN range(0, 10, 2)            -- [0, 2, 4, 6, 8, 10]
RETURN range(10, 1, -1)           -- [10, 9, 8, …, 1]
RETURN range(1, 5, 0)             -- null  (zero step)
```

### Common uses

```cypher
// Pagination helper: generate page numbers
RETURN range(1, toInteger(ceil($total / $size))) AS pages

// Simple synthetic data
UNWIND range(1, 100) AS i
CREATE (:Point {id: i, x: rand() * 100, y: rand() * 100})
```

## reduce

Fold over a list, carrying an accumulator.

```cypher
RETURN reduce(total = 0, x IN [1, 2, 3, 4] | total + x)    -- 10
RETURN reduce(pairs = [], x IN [1, 2, 3] | pairs + [x * 2]) -- [2, 4, 6]
```

### Word frequency with a map accumulator

```cypher
RETURN reduce(
  acc = {},
  x IN ['red', 'red', 'blue', 'green'] |
  acc + {[x]: coalesce(acc[x], 0) + 1}
)
-- {red: 2, blue: 1, green: 1}
```

`acc` starts as an empty map. Each element rebuilds `acc` by
merging in the updated count for `x` — `coalesce(acc[x], 0)`
falls back to zero on the first occurrence of a key. The
dynamic-key form `{[x]: …}` is what makes this a real histogram
rather than a single `x` column.

### Nested / layered lists

```cypher
// Flatten a list-of-lists
RETURN reduce(out = [], xs IN [[1, 2], [3, 4], [5]] | out + xs)
-- [1, 2, 3, 4, 5]
```

`reduce` has no short-circuit — the whole list is visited even if the
accumulator could stop early. Use
[`any` / `all`](#predicates-in-where) for short-circuit boolean
questions.

## Indexing and slicing

```cypher
RETURN [10, 20, 30][0]       -- 10
RETURN [10, 20, 30][-1]      -- 30
RETURN [10, 20, 30][5]       -- null (out of range)

RETURN [1, 2, 3, 4, 5][1..3]   -- [2, 3]     (end-exclusive)
RETURN [1, 2, 3, 4, 5][..2]    -- [1, 2]
RETURN [1, 2, 3, 4, 5][3..]    -- [4, 5]
RETURN [1, 2, 3, 4, 5][-2..]   -- [4, 5]
```

### Slicing recipes

```cypher
// Top 3
RETURN collect(x)[..3]

// Last 3
RETURN collect(x)[-3..]

// Second-to-fifth
RETURN collect(x)[1..5]
```

## Concatenation

```cypher
RETURN [1, 2] + [3, 4]           -- [1, 2, 3, 4]
RETURN 0 + [1, 2]                -- [0, 1, 2]
RETURN [1, 2] + 3                -- [1, 2, 3]
```

## List comprehension

```cypher
-- Filter
RETURN [x IN [1, 2, 3, 4] WHERE x > 2]       -- [3, 4]

-- Map
RETURN [x IN [1, 2, 3] | x * 10]             -- [10, 20, 30]

-- Filter + map
RETURN [x IN [1, 2, 3, 4] WHERE x > 2 | x * 10]   -- [30, 40]
```

### On node properties

```cypher
MATCH (u:User)
RETURN u.name,
       [t IN u.tags WHERE size(t) > 3] AS long_tags
```

### Nested comprehension

```cypher
UNWIND [[1, 2], [3, 4], [5, 6]] AS pair
RETURN [x IN pair WHERE x % 2 = 0 | x * 10]
```

## Pattern comprehension

Bind a pattern and collect one value per match — inline.

```cypher
MATCH (p:Person)
RETURN p.name,
       [(p)-[:KNOWS]->(f) | f.name] AS friends
```

With a filter:

```cypher
MATCH (p:Person)
RETURN p.name,
       [(p)-[:WROTE]->(post:Post) WHERE post.published | post.title] AS posts
```

Returns a list per matched `p` — ideal when you'd otherwise add a new
`MATCH` stage and an aggregation just to assemble "each owner's
items".

### Take top-N inline

```cypher
MATCH (u:User)
RETURN u.name,
       [(u)-[:WROTE]->(p) | p.title][..3] AS recent_titles
```

### Pattern comprehension vs OPTIONAL MATCH

| Want | Use |
|---|---|
| A list per outer row, even if empty | Pattern comprehension |
| A row per match, with nulls for no-match | `OPTIONAL MATCH` |
| Existence only, no values | [`EXISTS { … }`](../queries/where#pattern-existence) |

## Predicates (in [`WHERE`](../queries/where))

These are part of `WHERE` but operate over lists — included here as a
cross-reference.

| Predicate | Description |
|---|---|
| `all(x IN list WHERE pred)` | `true` if `pred` holds for every element |
| `any(x IN list WHERE pred)` | `true` if `pred` holds for at least one |
| `none(x IN list WHERE pred)` | `true` if `pred` holds for none |
| `single(x IN list WHERE pred)` | `true` if `pred` holds for exactly one |

```cypher
MATCH (n)
WHERE all(x IN n.scores WHERE x >= 0)
RETURN n

MATCH (n)
WHERE any(x IN n.tags WHERE x = 'featured')
RETURN n

MATCH (n)
WHERE single(x IN n.roles WHERE x = 'owner')
RETURN n
```

### Predicates on paths

`nodes(p)` and `relationships(p)` are lists, so path predicates work
too:

```cypher
MATCH p = (a)-[:FOLLOWS*1..3]->(b)
WHERE all(r IN relationships(p) WHERE r.active)
RETURN p
```

See [Paths](../queries/paths#path-functions).

## Common patterns

### Distinct values from a list

There's no built-in `distinct(list)`. Use
[`UNWIND`](../queries/unwind-merge#unwind) +
[`collect(DISTINCT …)`](./aggregation#collect):

```cypher
UNWIND [1, 2, 2, 3, 3, 3] AS x
RETURN collect(DISTINCT x)   -- [1, 2, 3]
```

### Sort a list

No in-place sort. Unwind, order, re-collect:

```cypher
UNWIND [3, 1, 4, 1, 5, 9, 2, 6] AS x
WITH x ORDER BY x
RETURN collect(x)   -- [1, 1, 2, 3, 4, 5, 6, 9]
```

### Zip two lists (best effort)

```cypher
WITH ['a', 'b', 'c'] AS keys, [1, 2, 3] AS vals
RETURN [i IN range(0, size(keys) - 1) | [keys[i], vals[i]]]
-- [['a', 1], ['b', 2], ['c', 3]]
```

### Reduce to a single map

```cypher
WITH [{k: 'a', v: 1}, {k: 'b', v: 2}] AS kvs
RETURN reduce(m = {}, kv IN kvs | m + {[kv.k]: kv.v})
-- {a: 1, b: 2}
```

### Running totals

```cypher
WITH [10, 20, 30, 40] AS xs
RETURN reduce(
  acc = {total: 0, running: []},
  x IN xs |
  {
    total:   acc.total + x,
    running: acc.running + [acc.total + x]
  }
).running AS running_totals
-- [10, 30, 60, 100]
```

The accumulator is a map with two fields: `total` keeps the running
sum between steps, `running` appends the new total on each step.
The trailing `.running` picks that second field off the final map
so the caller gets the sequence of totals, not the map wrapper.
`reduce` has no short-circuit, so the full list is always visited —
use it when you genuinely need every step.

### Min-by on objects

`min()` is an aggregate — for "pick the list element with the
smallest key" within a single row, use `reduce`:

```cypher
WITH [{name: 'a', score: 9},
      {name: 'b', score: 3},
      {name: 'c', score: 7}] AS rows
RETURN reduce(
  best = rows[0],
  r IN tail(rows) |
  CASE WHEN r.score < best.score THEN r ELSE best END
) AS winner
-- {name: 'b', score: 3}
```

Start with the first element as `best`, walk `tail(rows)`, and on
each step keep whichever of `best` vs. `r` has the smaller `score`.
The [`CASE`](../queries/return-with#case-expressions) expression
returns a whole map — the same shape as `best` — so the accumulator
type is stable. Swap `<` for `>` to turn this into a max-by.

### Bucket counts

```cypher
WITH [1, 7, 12, 3, 45, 9, 22] AS xs
RETURN reduce(
  buckets = {small: 0, medium: 0, large: 0},
  x IN xs |
  CASE
    WHEN x < 10 THEN buckets + {small:  buckets.small  + 1}
    WHEN x < 30 THEN buckets + {medium: buckets.medium + 1}
    ELSE             buckets + {large:  buckets.large  + 1}
  END
) AS histogram
```

### Slice the first N of each group

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, p ORDER BY p.published_at DESC
WITH u, collect(p)[..3] AS recent_three
RETURN u.name, [p IN recent_three | p.title]
```

## Edge cases

### Out-of-range index

```cypher
RETURN [1, 2, 3][99]       -- null
RETURN [1, 2, 3][-99]      -- null
```

### Slice with reversed bounds

```cypher
RETURN [1, 2, 3, 4][3..1]   -- []   (empty, not null, not an error)
```

### Operations on null lists

Every list function returns `null` on a `null` input, including `size`.

```cypher
RETURN size(null), head(null), tail(null)    -- null, null, null
```

### Heterogeneous lists

Nothing enforces element-type uniformity.

```cypher
RETURN [1, 'two', true, null]
```

Use a [`valueType`](./overview#type-conversion-and-checking) check in
`all(… WHERE …)` if you need a guarantee.

## Limitations

- List element types are not enforced — `[1, 'two', true]` is a
  perfectly valid list. Use
  `all(x IN list WHERE valueType(x) = 'INTEGER')` if you need
  homogeneity.
- `reduce` has no short-circuit — the whole list is visited even if a
  condition could stop it early.
- No built-in `distinct(list)` helper — use `collect(DISTINCT x)` on an
  unwound list.

## See also

- [**Lists & Maps**](../data-types/lists-and-maps) — the data-type side.
- [**Aggregation → collect**](./aggregation#collect) — produce lists from rows.
- [**UNWIND**](../queries/unwind-merge#unwind) — lists back into rows.
- [**Paths**](../queries/paths) — `nodes()` / `relationships()` produce lists.
- [**WHERE → list predicates**](../queries/where#list-predicates).
