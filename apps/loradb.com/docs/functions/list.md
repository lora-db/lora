---
title: List Functions
sidebar_label: List
description: List functions in LoraDB — value.size, list.first, list.rest, list.last, list.range, value.reverse, reduce, and friends — with 0-based indexing, negative indices, and null-propagation semantics.
---

# List Functions

[Lists](../data-types/lists-and-maps#lists) are first-class values.
Indexing is 0-based and supports negative indices counted from the end.
List functions generally return `null` on `null` input.

## Overview

| Goal | Function / Syntax |
|---|---|
| Size | [<CypherCode code="value.size(list)" />](#size--head--tail--last) |
| First, rest, last | [<CypherCode code="list.first" />, <CypherCode code="list.rest" />, <CypherCode code="list.last" />](#size--head--tail--last) |
| Safe index / slice | [<CypherCode code="list.at(list, i)" />, <CypherCode code="list.slice(list, from[, to])" />](#function-indexing-and-slicing) |
| Reverse | [<CypherCode code="value.reverse(list)" />](#reverse) |
| Range | [<CypherCode code="list.range(start, end[, step])" />](#range) |
| Take / drop | [<CypherCode code="list.take" />, <CypherCode code="list.drop" />, <CypherCode code="list.take_last" />, <CypherCode code="list.drop_last" />](#selection-and-reshaping) |
| Append / prepend / concat | [<CypherCode code="list.append" />, <CypherCode code="list.prepend" />, <CypherCode code="list.concat" />](#functional-concatenation) |
| Deduplicate | [<CypherCode code="list.unique(list)" />](#deduplicate-and-set-operations) |
| Per-list numeric summaries | [<CypherCode code="list.sum" />, <CypherCode code="list.avg" />, <CypherCode code="list.median" />](#numeric-summaries) |
| Fold | [`reduce(acc, x IN list | …)`](#reduce) |
| Index / slice | [<CypherCode code="list[i]" />, <CypherCode code="list[a..b]" />](#indexing-and-slicing) |
| Concat | [<CypherCode code="list + list" />, <CypherCode code="list + x" />, <CypherCode code="x + list" />](#concatenation) |
| Filter / map | [`[x IN list WHERE … | …]`](#list-comprehension) |
| Attach related entities | [Pattern comprehension](#pattern-comprehension) |
| Quantify | [<CypherCode code="all" />, <CypherCode code="any" />, <CypherCode code="none" />, <CypherCode code="single" />](#predicates-in-where) |
| Collect rows | [<CypherCode code="collect(expr)" />](./aggregation#collect) |
| Unwind rows | [<CypherCode code="UNWIND list AS row" />](../queries/unwind-merge#unwind) |

## value.size / list.first / list.rest / list.last {#size--head--tail--last}

| Function | Behaviour |
|---|---|
| `value.size(list)` | Number of elements; `null` → `null` |
| `list.first(list)` | First element; empty list → `null` |
| `list.rest(list)` | All but first; empty list → `null` |
| `list.last(list)` | Last element; empty list → `null` |

<QueryCodeBlock code={String.raw`RETURN value.size([1, 2, 3]);            // 3
RETURN list.first([1, 2, 3]);            // 1
RETURN list.rest([1, 2, 3]);            // [2, 3]
RETURN list.last([1, 2, 3]);            // 3
RETURN list.first([]);                   // null
RETURN value.size([])                   // 0`} />

`value.size` also works on strings — see
[`String Functions → size`](./string#stringlength--valuesize).

## reverse

Works on lists and strings.

<QueryCodeBlock code={String.raw`RETURN value.reverse([1, 2, 3]);         // [3, 2, 1]
RETURN value.reverse('abc')             // 'cba'`} />

## range

`list.range(start, end[, step])` — inclusive, integers only.

<QueryCodeBlock code={String.raw`RETURN list.range(1, 5);                // [1, 2, 3, 4, 5]
RETURN list.range(0, 10, 2);            // [0, 2, 4, 6, 8, 10]
RETURN list.range(10, 1, -1);           // [10, 9, 8, …, 1]
RETURN list.range(1, 5, 0)             // null  (zero step)`} />

### Common uses

<QueryCodeBlock code={String.raw`// Pagination helper: generate page numbers
RETURN list.range(1, toInteger(math.ceil($total / $size))) AS pages

;// Simple synthetic data
UNWIND list.range(1, 100) AS i
CREATE (:Point {id: i, x: math.random() * 100, y: math.random() * 100})`} />

## Selection and Reshaping

These functions transform one list value inside a row. They do not
change row cardinality; use `UNWIND` when you want one output row per
element.

| Function | Behaviour |
|---|---|
| `list.take(xs, n)` | First `n` items; negative or zero `n` returns `[]` |
| `list.drop(xs, n)` | Everything after the first `n` items; negative or zero `n` returns the original list |
| `list.take_last(xs, n)` | Last `n` items; negative or zero `n` returns `[]` |
| `list.drop_last(xs, n)` | Everything except the last `n` items; negative or zero `n` returns the original list |
| `list.flatten(xs[, depth])` | Flattens nested lists to the given depth; default `1` |
| `list.compact(xs)` | Removes `null` elements |
| `list.chunks(xs, size)` | Consecutive fixed-size chunks; the last chunk may be shorter |
| `list.windows(xs, size[, step])` | Sliding fixed-size windows |
| `list.zip(a, b)` | Pairs elements until the shorter list is exhausted |
| `list.append(xs, value)` | Returns a new list with `value` added at the end |
| `list.prepend(xs, value)` | Returns a new list with `value` added at the beginning |
| `list.concat(a, b, ...)` | Concatenates two or more lists |

<QueryCodeBlock code={String.raw`RETURN list.take([1, 2, 3, 4], 2);              // [1, 2]
RETURN list.drop([1, 2, 3, 4], 2);              // [3, 4]
RETURN list.take_last([1, 2, 3, 4], 2);         // [3, 4]
RETURN list.drop_last([1, 2, 3, 4], 1);         // [1, 2, 3]
RETURN list.flatten([[1, 2], [3, [4]]], 2);     // [1, 2, 3, 4]
RETURN list.chunks([1, 2, 3, 4, 5], 2);         // [[1, 2], [3, 4], [5]]
RETURN list.windows([1, 2, 3, 4], 2);           // [[1, 2], [2, 3], [3, 4]]
RETURN list.zip(['a', 'b'], [1, 2, 3]);         // [['a', 1], ['b', 2]]
RETURN list.append(['a', 'b'], 'c');            // ['a', 'b', 'c']
RETURN list.prepend(['b', 'c'], 'a');           // ['a', 'b', 'c']
RETURN list.concat([1, 2], [3], [4, 5])        // [1, 2, 3, 4, 5]`} />

Use `list.sample(xs[, n])` or `list.shuffle(xs)` for lightweight random
sampling. They are not cryptographically secure and are intended for
exploration, demos, and approximate picks.

## reduce

Fold over a list, carrying an accumulator.

<QueryCodeBlock code={String.raw`RETURN reduce(total = 0, x IN [1, 2, 3, 4] | total + x);    // 10
RETURN reduce(pairs = [], x IN [1, 2, 3] | pairs + [x * 2]) // [2, 4, 6]`} />

### Word frequency with a map accumulator

<QueryCodeBlock code={String.raw`RETURN reduce(
  acc = {},
  x IN ['red', 'red', 'blue', 'green'] |
  acc + {[x]: coalesce(acc[x], 0) + 1}
)
// {red: 2, blue: 1, green: 1}`} />

`acc` starts as an empty map. Each element rebuilds `acc` by
merging in the updated count for `x` — `coalesce(acc[x], 0)`
falls back to zero on the first occurrence of a key. The
dynamic-key form `{[x]: …}` is what makes this a real histogram
rather than a single `x` column.

### Nested / layered lists

<QueryCodeBlock code={String.raw`// Flatten a list-of-lists
RETURN reduce(out = [], xs IN [[1, 2], [3, 4], [5]] | out + xs)
// [1, 2, 3, 4, 5]`} />

`reduce` has no short-circuit — the whole list is visited even if the
accumulator could stop early. Use
[`any` / `all`](#predicates-in-where) for short-circuit boolean
questions.

## Indexing and slicing

<QueryCodeBlock code={String.raw`RETURN [10, 20, 30][0];       // 10
RETURN [10, 20, 30][-1];      // 30
RETURN [10, 20, 30][5];       // null (out of range)

RETURN [1, 2, 3, 4, 5][1..3];   // [2, 3]     (end-exclusive)
RETURN [1, 2, 3, 4, 5][..2];    // [1, 2]
RETURN [1, 2, 3, 4, 5][3..];    // [4, 5]
RETURN [1, 2, 3, 4, 5][-2..]   // [4, 5]`} />

## Function Indexing And Slicing

Use `list.at(list, index)` and `list.slice(list, from[, to])` when a
query builder needs regular function calls instead of postfix syntax.
Both use the same 0-based, negative-from-end convention as list
indexing.

<QueryCodeBlock code={String.raw`RETURN list.at([10, 20, 30], 1);          // 20
RETURN list.at([10, 20, 30], -1);         // 30
RETURN list.at([10, 20, 30], 99);         // null

RETURN list.slice([10, 20, 30, 40], 1, 3);   // [20, 30]
RETURN list.slice([10, 20, 30, 40], -3, -1); // [20, 30]
RETURN list.slice([10, 20, 30], 2, 1)       // []`} />

## Numeric Summaries

The `list.*` summary helpers work inside a single row. They skip
`null` values and return `null` if a non-numeric value appears where a
number is required.

| Function | Behaviour |
|---|---|
| `list.sum(list)` | Sum of numeric items; empty/all-null list returns `0` |
| `list.avg(list)` | Average of numeric items; empty/all-null list returns `null` |
| `list.min(list)` / `list.max(list)` | Smallest/largest comparable value |
| `list.product(list)` | Product of numeric items; empty/all-null list returns `1` |
| `list.stdev(list)` | Sample standard deviation; fewer than two values returns `null` |
| `list.median(list)` | Median numeric value as a float |

<QueryCodeBlock code={String.raw`RETURN list.sum([1, 2, 3]);       // 6
RETURN list.avg([1, 2, null, 5]); // 2.6666…
RETURN list.median([1, 10, 20]);  // 10.0
RETURN list.product([2, 3, 4])   // 24`} />

### Slicing recipes

<QueryCodeBlock code={String.raw`// Top 3
RETURN collect(x)[..3]

;// Last 3
RETURN collect(x)[-3..]

;// Second-to-fifth
RETURN collect(x)[1..5]`} />

## Concatenation

<QueryCodeBlock code={String.raw`RETURN [1, 2] + [3, 4];           // [1, 2, 3, 4]
RETURN 0 + [1, 2];                // [0, 1, 2]
RETURN [1, 2] + 3                // [1, 2, 3]`} />

## Functional Concatenation

Use the function forms when generated queries or nested expressions are
easier to build with regular calls than with `+`.

<QueryCodeBlock code={String.raw`RETURN list.concat([1, 2], [3], [4, 5]);   // [1, 2, 3, 4, 5]
RETURN list.append([1, 2], 3);             // [1, 2, 3]
RETURN list.prepend([2, 3], 1)            // [1, 2, 3]`} />

`list.append` and `list.prepend` can add any value, including `null`.
`list.concat` requires every argument to be a list; a non-list argument
returns `null`.

## List comprehension

<QueryCodeBlock code={String.raw`// Filter
RETURN [x IN [1, 2, 3, 4] WHERE x > 2]       // [3, 4]

;// Map
RETURN [x IN [1, 2, 3] | x * 10]             // [10, 20, 30]

;// Filter + map
RETURN [x IN [1, 2, 3, 4] WHERE x > 2 | x * 10]   // [30, 40]`} />

### On node properties

<QueryCodeBlock code={String.raw`MATCH (u:User)
RETURN u.name,
       [t IN u.tags WHERE value.size(t) > 3] AS long_tags`} />

### Nested comprehension

<QueryCodeBlock code={String.raw`UNWIND [[1, 2], [3, 4], [5, 6]] AS pair
RETURN [x IN pair WHERE x % 2 = 0 | x * 10]`} />

## Pattern comprehension

Bind a pattern and collect one value per match — inline.

<QueryCodeBlock code={String.raw`MATCH (p:Person)
RETURN p.name,
       [(p)-[:KNOWS]->(f) | f.name] AS friends`} />

With a filter:

<QueryCodeBlock code={String.raw`MATCH (p:Person)
RETURN p.name,
       [(p)-[:WROTE]->(post:Post) WHERE post.published | post.title] AS posts`} />

Returns a list per matched `p` — ideal when you'd otherwise add a new
`MATCH` stage and an aggregation just to assemble "each owner's
items".

### Take top-N inline

<QueryCodeBlock code={String.raw`MATCH (u:User)
RETURN u.name,
       [(u)-[:WROTE]->(p) | p.title][..3] AS recent_titles`} />

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

<QueryCodeBlock code={String.raw`MATCH (n)
WHERE all(x IN n.scores WHERE x >= 0)
RETURN n;

MATCH (n)
WHERE any(x IN n.tags WHERE x = 'featured')
RETURN n;

MATCH (n)
WHERE single(x IN n.roles WHERE x = 'owner')
RETURN n`} />

### Predicates on paths

`path.nodes(p)` and `path.edges(p)` are lists, so path predicates work
too:

<QueryCodeBlock code={String.raw`MATCH p = (a)-[:FOLLOWS*1..3]->(b)
WHERE all(r IN path.edges(p) WHERE r.active)
RETURN p`} />

See [Paths](../queries/paths#path-functions).

## Common patterns

### Deduplicate And Set Operations

Use `list.unique(list)` to keep the first occurrence of each value
inside one row. Use set-style helpers when comparing two lists.

<QueryCodeBlock code={String.raw`RETURN list.unique([1, 2, 2, 3, 3, 3]);          // [1, 2, 3]
RETURN list.union([1, 2], [2, 3]);               // [1, 2, 3]
RETURN list.intersect([1, 2, 3], [2, 3, 4]);     // [2, 3]
RETURN list.diff([1, 2, 3], [2])                // [1, 3]`} />

### Sort a list

Use `list.sort(list[, 'desc'])` for scalar lists in a single row. For
row-level ordering, sort rows before collecting.

<QueryCodeBlock code={String.raw`RETURN list.sort([3, 1, 4, 1, 5, 9, 2, 6])
// [1, 1, 2, 3, 4, 5, 6, 9]`} />

### Zip two lists

<QueryCodeBlock code={String.raw`RETURN list.zip(['a', 'b', 'c'], [1, 2, 3])
// [['a', 1], ['b', 2], ['c', 3]]`} />

### Reduce to a single map

<QueryCodeBlock code={String.raw`WITH [{k: 'a', v: 1}, {k: 'b', v: 2}] AS kvs
RETURN reduce(m = {}, kv IN kvs | m + {[kv.k]: kv.v})
// {a: 1, b: 2}`} />

### Running totals

<QueryCodeBlock code={String.raw`WITH [10, 20, 30, 40] AS xs
RETURN reduce(
  acc = {total: 0, running: []},
  x IN xs |
  {
    total:   acc.total + x,
    running: acc.running + [acc.total + x]
  }
).running AS running_totals
// [10, 30, 60, 100]`} />

The accumulator is a map with two fields: `total` keeps the running
sum between steps, `running` appends the new total on each step.
The trailing `.running` picks that second field off the final map
so the caller gets the sequence of totals, not the map wrapper.
`reduce` has no short-circuit, so the full list is always visited —
use it when you genuinely need every step.

### Min-by on objects

`min()` is an aggregate — for "pick the list element with the
smallest key" within a single row, use `reduce`:

<QueryCodeBlock code={String.raw`WITH [{name: 'a', score: 9},
      {name: 'b', score: 3},
      {name: 'c', score: 7}] AS rows
RETURN reduce(
  best = rows[0],
  r IN list.rest(rows) |
  CASE WHEN r.score < best.score THEN r ELSE best END
) AS winner
// {name: 'b', score: 3}`} />

Start with the first element as `best`, walk `list.rest(rows)`, and on
each step keep whichever of `best` vs. `r` has the smaller `score`.
The [`CASE`](../queries/return-with#case-expressions) expression
returns a whole map — the same shape as `best` — so the accumulator
type is stable. Swap `<` for `>` to turn this into a max-by.

### Bucket counts

<QueryCodeBlock code={String.raw`WITH [1, 7, 12, 3, 45, 9, 22] AS xs
RETURN reduce(
  buckets = {small: 0, medium: 0, large: 0},
  x IN xs |
  CASE
    WHEN x < 10 THEN buckets + {small:  buckets.small  + 1}
    WHEN x < 30 THEN buckets + {medium: buckets.medium + 1}
    ELSE             buckets + {large:  buckets.large  + 1}
  END
) AS histogram`} />

### Slice the first N of each group

<QueryCodeBlock code={String.raw`MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, p ORDER BY p.published_at DESC
WITH u, collect(p)[..3] AS recent_three
RETURN u.name, [p IN recent_three | p.title]`} />

## Edge cases

### Out-of-range index

<QueryCodeBlock code={String.raw`RETURN [1, 2, 3][99];       // null
RETURN [1, 2, 3][-99]      // null`} />

### Slice with reversed bounds

<QueryCodeBlock code={String.raw`RETURN [1, 2, 3, 4][3..1]   // []   (empty, not null, not an error)`} />

### Operations on null lists

Every list function returns `null` on a `null` input, including `value.size`.

<QueryCodeBlock code={String.raw`RETURN value.size(null), list.first(null), list.rest(null)    // null, null, null`} />

### Heterogeneous lists

Nothing enforces element-type uniformity.

<QueryCodeBlock code={String.raw`RETURN [1, 'two', true, null]`} />

Use a [`type.of`](./overview#type-conversion-and-checking) check in
`all(… WHERE …)` if you need a guarantee.

## Limitations

- List element types are not enforced — `[1, 'two', true]` is a
  perfectly valid list. Use
  `all(x IN list WHERE type.of(x) = 'INTEGER')` if you need
  homogeneity.
- `reduce` has no short-circuit — the whole list is visited even if a
  condition could stop it early.
- `list.sort` only sorts directly comparable scalar values. For maps,
  nodes, or custom sort keys, sort rows with `ORDER BY` before
  collecting.

## See also

- [**Lists & Maps**](../data-types/lists-and-maps) — the data-type side.
- [**Aggregation → collect**](./aggregation#collect) — produce lists from rows.
- [**UNWIND**](../queries/unwind-merge#unwind) — lists back into rows.
- [**Paths**](../queries/paths) — `path.nodes()` / `path.edges()` produce lists.
- [**WHERE → list predicates**](../queries/where#list-predicates).
