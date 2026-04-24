---
title: Aggregation Functions
sidebar_label: Aggregation
description: Aggregation functions in LoraDB — count, sum, avg, min, max, collect, stdev, and percentile family — including null handling, empty-input semantics, and implicit grouping rules.
---

# Aggregation Functions

Aggregation collapses a group of input rows into a single value per
group. For clause-level semantics (implicit `GROUP BY`,
`HAVING`-style filtering via [`WITH`](../queries/return-with#with),
where aggregates are legal) see the
[Aggregation query page](../queries/aggregation).

> All aggregates **skip `null` inputs** except `count(*)` (counts
> rows) and `collect(expr)` (keeps nulls). Empty-input semantics
> vary per function — see the [summary table](#summary-table).

## Summary table

| Function | `DISTINCT` | Empty input | Null input | Returns |
|---|---|---|---|---|
| <CypherCode code="count(*)" /> | — | `0` | counted as 1 per row | `Int` |
| <CypherCode code="count(expr)" /> | yes | `0` | skipped | `Int` |
| <CypherCode code="collect(expr)" /> | yes | `[]` | included as `null` | `List` |
| <CypherCode code="sum(expr)" /> | yes | `null` | skipped | `Int` if all-int, else `Float` |
| <CypherCode code="avg(expr)" /> | yes | `null` | skipped | `Float` |
| <CypherCode code="min(expr)" /> / <CypherCode code="max(expr)" /> | yes | `null` | skipped | same type as element |
| <CypherCode code="stdev(expr)" /> | — | `0.0` | skipped | `Float`, sample (n − 1) |
| <CypherCode code="stdevp(expr)" /> | — | `0.0` | skipped | `Float`, population (n) |
| <CypherCode code="percentileCont(expr, p)" /> | — | `null` | skipped | `Float`, linear interpolation |
| <CypherCode code="percentileDisc(expr, p)" /> | — | `null` | skipped | `Float`, nearest rank |

## count

### Row count

```cypher
MATCH (n:User)
RETURN count(*) AS users
```

### Non-null count

```cypher
UNWIND [1, 2, null, 4] AS x
RETURN count(*), count(x)
-- 4, 3
```

`count(*)` counts every input row, including rows where bound variables
are `null`; `count(expr)` skips null `expr`.

### Distinct count

```cypher
UNWIND ['a', 'a', 'b', 'c'] AS x
RETURN count(x), count(DISTINCT x)
-- 4, 3
```

### count with OPTIONAL MATCH

This is the subtlety that trips up new Cypher users:

```cypher
MATCH (u:User)
OPTIONAL MATCH (u)-[:WROTE]->(p:Post)
RETURN u.name,
       count(*) AS rows,   -- 1 per user, even if no posts
       count(p) AS posts   -- 0 if no posts
```

Always prefer `count(expr)` when you want zeros for optional matches.

### Empty graph

```cypher
MATCH (:NoSuchLabel)
RETURN count(*)       -- 0
```

One row out, with value `0` — `count(*)` never returns `null`.

## collect

### Basic collect

```cypher
MATCH (p:Person)-[:KNOWS]->(f:Person)
RETURN p.name, collect(f.name) AS friends
```

### Distinct values

```cypher
UNWIND [1, 2, null, 2, 3] AS x
RETURN collect(x),           -- [1, 2, null, 2, 3]
       collect(DISTINCT x)   -- [1, 2, null, 3]
```

### Collect keeps nulls

`collect` **keeps nulls** that survive to the aggregate. Filter before
the aggregate if you don't want them:

```cypher
UNWIND [1, 2, null, 3] AS x
WITH x WHERE x IS NOT NULL
RETURN collect(x)             -- [1, 2, 3]
```

### Collect + slice for top-N

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, p ORDER BY p.published_at DESC
WITH u, collect(p.title)[..5] AS last_five
RETURN u.name, last_five
```

Use any [list operation](../functions/list) on the resulting list.

### Collect of maps

```cypher
MATCH (p:Project)-[:HAS_TASK]->(t:Task)
RETURN p.name,
       collect({id: t.id, name: t.name, done: t.done}) AS tasks
```

One row per project, with an array of task summaries.

## sum

```cypher
UNWIND [1, 2, null, 4] AS x
RETURN sum(x)                 -- 7

UNWIND [1.0, 2.5, 3.5] AS x
RETURN sum(x)                 -- 7.0

MATCH (:Never)
RETURN sum(1)                 -- null   (empty input)
```

Return type: `Int` when every contributing element is an `Int`; `Float`
if any contributor is a `Float`.

### Distinct sum

```cypher
UNWIND [1, 1, 2, 2, 3] AS x
RETURN sum(x), sum(DISTINCT x)   -- 9, 6
```

## avg

```cypher
UNWIND [1, 2, 3, 4] AS x
RETURN avg(x)                 -- 2.5

UNWIND [1, null, 3] AS x
RETURN avg(x)                 -- 2.0

MATCH (:Never)
RETURN avg(1)                 -- null
```

Always returns `Float` (or `null` on empty input).

## min / max

Works on numbers, strings, and temporal values under their natural total
order.

```cypher
UNWIND ['banana', 'apple', 'cherry'] AS s
RETURN min(s), max(s)         -- 'apple', 'cherry'

UNWIND [date('2024-01-01'), date('2024-06-30'), date('2024-12-15')] AS d
RETURN min(d), max(d)
-- 2024-01-01, 2024-12-15

MATCH (:Never)
RETURN min(1)                 -- null
```

### Min/max with tiebreaker

`min()` / `max()` only return the value, not the node owning it. For
"the node with the maximum", sort and take one:

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, count(p) AS posts
ORDER BY posts DESC
LIMIT 1
RETURN u.name, posts
```

## stdev / stdevp

Sample vs population standard deviation.

```cypher
UNWIND [2, 4, 4, 4, 5, 5, 7, 9] AS x
RETURN stdev(x),              -- 2.1380…  (n − 1)
       stdevp(x)              -- 2.0      (n)
```

- `stdev` returns `0.0` when fewer than two non-null values are aggregated.
- `stdevp` returns `0.0` on an empty input.

```cypher
MATCH (r:Review)
WITH r.product AS product, avg(r.stars) AS mean, stdev(r.stars) AS sd
WHERE sd > 1.0
RETURN product, mean, sd
```

## percentileCont / percentileDisc

Both take the column and a percentile `p ∈ [0, 1]`.

```cypher
UNWIND [1, 2, 3, 4, 5] AS x
RETURN percentileCont(x, 0.5),  -- 3.0    (exact median for odd count)
       percentileDisc(x, 0.5)   -- 3

UNWIND [1, 2, 3, 4] AS x
RETURN percentileCont(x, 0.5),  -- 2.5    (linear interpolation)
       percentileDisc(x, 0.5)   -- 2      (nearest rank)

UNWIND [10, 20, 30, 40, 50, 60, 70, 80, 90, 100] AS x
RETURN percentileCont(x, 0.9),  -- 91.0
       percentileDisc(x, 0.9)   -- 90
```

- `percentileCont` interpolates between values.
- `percentileDisc` picks an actual input value.

Use `percentileCont` when the "true" percentile matters (latency
histograms); `percentileDisc` when you need a real observation (the
actual data point at P50).

## Typical patterns

### Group + aggregate

```cypher
MATCH (o:Order)-[:CONTAINS]->(i:Item)
RETURN o.region AS region,
       count(i)     AS items,
       sum(i.price) AS revenue
ORDER BY revenue DESC
```

### Aggregate + filter (HAVING)

```cypher
MATCH (p:Person)-[:WORKS_AT]->(c:Company)
WITH c.name AS company, count(p) AS employees
WHERE employees > 5
RETURN company, employees
```

### Multiple aggregates in one RETURN

```cypher
MATCH (r:Review)
RETURN count(*) AS n,
       avg(r.stars) AS mean,
       stdev(r.stars) AS sd,
       percentileCont(r.stars, 0.5)  AS median,
       percentileCont(r.stars, 0.95) AS p95
```

### Re-aggregate after first aggregate

```cypher
MATCH (o:Order)
WITH o.region AS region, sum(o.amount) AS revenue
RETURN count(region) AS regions,
       avg(revenue)  AS mean_regional_revenue
```

### Rolling count by date bucket

```cypher
MATCH (e:Event)
RETURN date.truncate('month', e.at) AS month,
       count(*) AS events
ORDER BY month
```

Uses [`date.truncate`](./temporal#truncation).

### Percentile per group

```cypher
MATCH (r:Review)
RETURN r.product AS product,
       percentileCont(r.stars, 0.5)  AS p50,
       percentileCont(r.stars, 0.95) AS p95
ORDER BY p95 DESC
```

### Count-if via CASE

`count(expr)` skips `null`. A `CASE` with an omitted `ELSE` returns
`null`, so the pattern cleanly expresses "count rows where condition
holds":

```cypher
MATCH (o:Order)
RETURN o.region,
       count(CASE WHEN o.status = 'paid'      THEN 1 END) AS paid,
       count(CASE WHEN o.status = 'cancelled' THEN 1 END) AS cancelled
```

See [`CASE`](../queries/return-with#case-expressions).

### Sum with a filter on the aggregated value

`WHERE` runs before aggregation. To filter aggregated values, pipe
through [`WITH`](../queries/return-with#with):

```cypher
MATCH (o:Order)
WITH o.customer AS customer, sum(o.amount) AS lifetime
WHERE lifetime > 1000
RETURN customer, lifetime
ORDER BY lifetime DESC
```

### Aggregation with ORDER BY inside collect

`collect` preserves input order. Sort the rows before the aggregate
to produce an ordered list:

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, p ORDER BY p.published_at DESC
RETURN u.handle, collect(p.title)[..3] AS latest_three
```

## Limitations

- **Aggregates are rejected inside [`WHERE`](../queries/where).** Use
  [`WITH … WHERE`](../queries/return-with#having-style-filtering-with)
  instead.
- **`stdev`, `stdevp`, `percentileCont`, `percentileDisc` don't
  support `DISTINCT`.** For a percentile of distinct values, first
  `collect(DISTINCT x)`, then [`UNWIND`](../queries/unwind-merge#unwind)
  and aggregate.
- **`count(*)` counts rows, not non-null values.** Use `count(expr)`
  when you want nulls skipped.
- **No `GROUP BY` keyword** — non-aggregated columns in the same
  projection stage form the group key implicitly.

See [Limitations](../limitations#aggregates) for the full list.

## See also

- [**Aggregation query page**](../queries/aggregation) — clause semantics and grouping.
- [**RETURN / WITH**](../queries/return-with) — HAVING-style filtering.
- [**WHERE**](../queries/where) — filter before aggregation.
- [**UNWIND**](../queries/unwind-merge#unwind) — expand a collected list back into rows.
- [**List Functions**](./list) — post-process `collect()` output.
- [**Temporal Functions**](./temporal) — date buckets for grouping.
