---
title: Aggregating Query Results
sidebar_label: Aggregation
description: How aggregation works as a clause in LoraDB — implicit GROUP BY from non-aggregated columns, HAVING-style filtering via WITH, and distinct aggregation.
---

# Aggregating Query Results

Aggregations collapse input rows into fewer rows. Non-aggregated
columns in the same [`RETURN`](./return-with) or
[`WITH`](./return-with#with) act as an **implicit group-by** —
Cypher has no explicit `GROUP BY` keyword.

> This page covers clause-level semantics. For each function's exact
> behaviour (empty input, null handling, return type) see
> [Aggregation Functions](../functions/aggregation).

## Overview

| Goal | Clause |
|---|---|
| Count rows | <CypherCode code="count(*)" /> |
| Count non-nulls | <CypherCode code="count(expr)" /> |
| Sum a property | <CypherCode code="sum(expr)" /> |
| Mean | <CypherCode code="avg(expr)" /> |
| Extremes | <CypherCode code="min(expr)" />, <CypherCode code="max(expr)" /> |
| Collect into a list | <CypherCode code="collect(expr)" /> |
| Distinct values | <CypherCode code="collect(DISTINCT expr)" /> |
| Standard deviation | <CypherCode code="stdev(expr)" />, <CypherCode code="stdevp(expr)" /> |
| Percentiles | <CypherCode code="percentileCont(expr, p)" />, <CypherCode code="percentileDisc(expr, p)" /> |
| Group by | Any non-aggregated column in the same <CypherCode code="RETURN" /> |
| HAVING-style filter | [<CypherCode code="WITH … WHERE" />](./return-with#having-style-filtering-with) |

## A five-step walkthrough

Rather than list functions first, let's build up an aggregation query
in stages. Assume a graph seeded with:

```cypher
UNWIND [
  {region: 'EU',   amount: 50,  status: 'paid'},
  {region: 'EU',   amount: 75,  status: 'paid'},
  {region: 'EU',   amount: 20,  status: 'cancelled'},
  {region: 'US',   amount: 200, status: 'paid'},
  {region: 'US',   amount: 120, status: 'paid'},
  {region: 'APAC', amount: 90,  status: 'paid'}
] AS row
CREATE (:Order {region: row.region, amount: row.amount, status: row.status})
```

### 1. Count

Start with the simplest aggregate. One row, one column.

```cypher
MATCH (o:Order)
RETURN count(*) AS orders
-- orders: 6
```

### 2. Group by region

Add a non-aggregated column to group on. Now one row per region.

```cypher
MATCH (o:Order)
RETURN o.region AS region, count(*) AS orders
-- EU: 3, US: 2, APAC: 1
```

### 3. Add a second aggregate

Every aggregate operates on the same group. Mix as many as you like in
one `RETURN`.

```cypher
MATCH (o:Order)
RETURN o.region       AS region,
       count(*)       AS orders,
       sum(o.amount)  AS revenue,
       avg(o.amount)  AS avg_ticket
ORDER BY revenue DESC
```

### 4. Filter before aggregating

Filter with [`WHERE`](./where) before the `RETURN` — this narrows the
input rows that reach the aggregate.

```cypher
MATCH (o:Order)
WHERE o.status = 'paid'
RETURN o.region AS region, sum(o.amount) AS paid_revenue
ORDER BY paid_revenue DESC
```

### 5. Filter after aggregating (HAVING-style)

Cypher has no `HAVING`. Aggregate into a [`WITH`](./return-with#with),
then filter:

```cypher
MATCH (o:Order)
WHERE o.status = 'paid'
WITH o.region AS region, sum(o.amount) AS paid_revenue
WHERE paid_revenue > 100
RETURN region, paid_revenue
```

> **Why `WITH` not `WHERE` directly?** Aggregates are not permitted
> inside [`WHERE`](./where). `WITH` promotes aggregated values into
> projected columns so the next `WHERE` can filter on them.

You now have the shape of most real aggregation queries: group, aggregate,
optionally filter. The rest of the page is a reference for every
aggregate and its edge cases.

## Built-in aggregates

| Function | Supports `DISTINCT` | Empty input | Null input | Return type |
|---|---|---|---|---|
| `count(*)` | — | `0` | counted as 1 per row | `Int` |
| `count(expr)` | yes | `0` | skipped | `Int` |
| `collect(expr)` | yes | `[]` | included as `null` | `List` |
| `sum(expr)` | yes | `null` | skipped | `Int` if all-int, else `Float` |
| `avg(expr)` | yes | `null` | skipped | `Float` |
| `min(expr)` | yes | `null` | skipped | type of min element |
| `max(expr)` | yes | `null` | skipped | type of max element |
| `stdev(expr)` | — | `0.0` | skipped | `Float`, sample (n − 1) |
| `stdevp(expr)` | — | `0.0` | skipped | `Float`, population (n) |
| `percentileCont(expr, p)` | — | `null` | skipped | `Float`, linear interpolation |
| `percentileDisc(expr, p)` | — | `null` | skipped | `Float`, nearest rank |

## count

### Row count

```cypher
MATCH (n:User)
RETURN count(*) AS users
```

`count(*)` counts every input row, including rows where bound variables
are `null`.

### Non-null count

```cypher
MATCH (n:User)
RETURN count(n.email) AS users_with_email
```

`count(expr)` skips rows where `expr` is `null`.

### Distinct

```cypher
MATCH (p:Person)
RETURN count(DISTINCT p.city) AS distinct_cities
```

### count(\*) vs count(expr) after OPTIONAL MATCH

This distinction matters when a row survives the left-join but has a
null binding:

```cypher
MATCH (u:User)
OPTIONAL MATCH (u)-[:WROTE]->(p:Post)
RETURN u.name,
       count(*)        AS rows,         -- 1 even for users without posts
       count(p)        AS posts,        -- 0 for users without posts
       count(DISTINCT p) AS unique_posts -- same as count(p) when p is 1-1 per row
```

Use `count(expr)` when you want zeros for empty optional matches; see
[`OPTIONAL MATCH with aggregation`](./match#optional-match-with-aggregation).

### Zero input

```cypher
MATCH (:LabelThatDoesNotExist)
RETURN count(*)   -- 0
```

An empty match still produces one row from `count(*)` with the value `0`.

## collect

`collect` turns rows into a list — useful for attaching one-to-many
results to each group.

```cypher
MATCH (p:Person)-[:KNOWS]->(f:Person)
RETURN p.name AS person, collect(f.name) AS friends
```

### Distinct values

```cypher
MATCH (p:Person)-[:VISITED]->(c:City)
RETURN p.name, collect(DISTINCT c.name) AS unique_cities
```

### Empty input

```cypher
MATCH (:Never)
RETURN collect(1)   -- []
```

### Collect keeps nulls

Unlike `count(expr)` / `sum` / `avg`, `collect` **keeps** `null` values
that make it through the pipeline. Filter first if you don't want them:

```cypher
UNWIND [1, null, 2, null, 3] AS x
RETURN collect(x)           -- [1, null, 2, null, 3]
RETURN collect(DISTINCT x)  -- [1, null, 2, 3]  (distinct still includes null)

-- To drop nulls before collecting:
UNWIND [1, null, 2, null, 3] AS x
WITH x WHERE x IS NOT NULL
RETURN collect(x)           -- [1, 2, 3]
```

## sum, avg

```cypher
MATCH (o:Order)
RETURN sum(o.amount) AS revenue,
       avg(o.amount) AS avg_ticket
```

- `sum` returns an `Int` when every input is an `Int`; otherwise `Float`.
- `avg` is always `Float`.
- Both **return `null` on empty input** (nothing to average / sum).
- Nulls in input are skipped — they don't affect the result.

```cypher
UNWIND [1, 2, null, 4] AS x
RETURN sum(x), avg(x)   -- 7, 2.333…
```

## min, max

```cypher
MATCH (e:Event)
RETURN min(e.when) AS earliest,
       max(e.when) AS latest
```

- Works on numbers, strings, and [temporal values](../data-types/temporal)
  under each type's native total order.
- Nulls skipped.
- Returns `null` on empty input.

```cypher
UNWIND ['banana', 'apple', 'cherry'] AS s
RETURN min(s), max(s)   -- 'apple', 'cherry'
```

## Grouping

Any non-aggregated column in the same `RETURN` or `WITH` becomes part of
the implicit group key.

```cypher
MATCH (o:Order)
RETURN o.region AS region,
       count(*)      AS orders,
       sum(o.amount) AS revenue
ORDER BY revenue DESC
```

Equivalent in SQL:

```sql
SELECT region, COUNT(*), SUM(amount)
FROM Order
GROUP BY region
ORDER BY SUM(amount) DESC;
```

### Two-level grouping

Every non-aggregated column participates in the key:

```cypher
MATCH (o:Order)
RETURN o.region AS region,
       o.status AS status,
       count(*) AS orders
ORDER BY region, status
```

### Grouping by a computed expression

The group key can be any expression, not just a property access:

```cypher
MATCH (p:Person)
RETURN p.born / 100 * 100 AS century, count(*) AS n
ORDER BY century
```

## HAVING-style filtering

Cypher has no `HAVING`. Aggregate in a `WITH`, then filter:

```cypher
MATCH (p:Person)-[:WORKS_AT]->(c:Company)
WITH c.name AS company, count(p) AS employees
WHERE employees > 5
RETURN company, employees
```

Aggregates are **not** permitted in `WHERE`. Attempting
`WHERE count(x) > 5` is a semantic error — always pipe through `WITH`.

## Aggregation after filtering

Filter before aggregating:

```cypher
MATCH (o:Order)
WHERE o.amount > 100 AND o.status = 'paid'
RETURN count(*) AS big_paid_orders
```

## Duplicates vs DISTINCT

The gap between `count(x)` and `count(DISTINCT x)` reveals repeats.

```cypher
MATCH (p:Person)-[:BOUGHT]->(b:Book)
RETURN p.name,
       count(b)          AS total_purchases,
       count(DISTINCT b) AS distinct_books
```

## Multiple aggregates

Any number of aggregates may appear in one `RETURN`:

```cypher
MATCH (r:Review)
RETURN count(*)                       AS n,
       avg(r.stars)                   AS mean,
       stdev(r.stars)                 AS sd,
       percentileCont(r.stars, 0.5)   AS median,
       percentileCont(r.stars, 0.95)  AS p95,
       min(r.stars)                   AS worst,
       max(r.stars)                   AS best
```

## percentileCont vs percentileDisc

- `percentileCont(expr, p)` — linear interpolation between ordered
  values. Returns a value that may not appear in the input.
- `percentileDisc(expr, p)` — nearest-rank. Returns an input value.

```cypher
UNWIND [1, 2, 3, 4, 5] AS x
RETURN percentileCont(x, 0.5),  -- 3.0
       percentileDisc(x, 0.5)   -- 3

UNWIND [1, 2, 3, 4] AS x
RETURN percentileCont(x, 0.5),  -- 2.5
       percentileDisc(x, 0.5)   -- 2
```

Both accept `p ∈ [0, 1]`.

## Pipeline aggregation

You can aggregate at one stage and then re-aggregate downstream:

```cypher
MATCH (u:User)-[:PLACED]->(o:Order)
WITH u.region AS region, o.amount AS amount
WITH region, sum(amount) AS revenue
WITH avg(revenue) AS avg_region_revenue,
     collect({region: region, revenue: revenue}) AS rows
RETURN avg_region_revenue, rows
```

## Common patterns

### Top-N per group

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, count(p) AS posts
ORDER BY posts DESC
LIMIT 10
RETURN u.name, posts
```

### Collect top-N related

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, p ORDER BY p.published_at DESC
WITH u, collect(p.title)[..3] AS last_three
RETURN u.name, last_three
```

### Rolling window — bucket by day

```cypher
MATCH (e:Event)
RETURN date.truncate('month', e.at) AS month, count(*) AS events
ORDER BY month
```

Uses [`date.truncate`](../functions/temporal#truncation).

### Average per category, HAVING filter

```cypher
MATCH (p:Product)
WITH p.category AS category, avg(p.rating) AS rating
WHERE rating >= 4.0
RETURN category, rating
ORDER BY rating DESC
```

### Dedup with DISTINCT inside collect

```cypher
MATCH (a:Author)-[:WROTE]->(:Book)-[:IN]->(g:Genre)
RETURN a.name, collect(DISTINCT g.name) AS genres
```

### Conditional count (count-if)

`count(expr)` skips `null`. Combined with
[`CASE`](./return-with#case-expressions) whose `ELSE` branch is
implicitly `null`, this gives you a clean "count rows matching X":

```cypher
MATCH (o:Order)
RETURN count(*)                                              AS total,
       count(CASE WHEN o.status = 'paid'      THEN 1 END)   AS paid,
       count(CASE WHEN o.status = 'cancelled' THEN 1 END)   AS cancelled,
       count(CASE WHEN o.amount >= 1000       THEN 1 END)   AS large
```

### Percent share per group

```cypher
MATCH (o:Order)
WITH o.region AS region, sum(o.amount) AS revenue
WITH collect({region: region, revenue: revenue}) AS rows,
     sum(revenue)                                 AS total
UNWIND rows AS r
RETURN r.region                              AS region,
       r.revenue                             AS revenue,
       toFloat(r.revenue) / total            AS share
ORDER BY share DESC
```

Two-pass aggregate: first compute each region's total, then divide by
the grand total from the same pipeline.

### Running metrics over time buckets

```cypher
MATCH (e:Event)
WHERE e.at >= datetime() - duration('P1Y')
RETURN date.truncate('month', e.at)  AS month,
       count(*)                       AS events,
       count(DISTINCT e.user_id)      AS unique_users,
       avg(e.duration_ms) / 1000.0    AS avg_seconds
ORDER BY month
```

### Top contributor per group (pipeline trick)

LoraDB has no window functions. Express "top-N per group" as
aggregate-then-filter:

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, p ORDER BY p.views DESC
WITH u, collect(p)[..1] AS top
UNWIND top AS best_post
RETURN u.handle, best_post.title, best_post.views
```

One row per user with their highest-viewed post. The `collect(…)[..1]`
slice picks the first element of the sort-ordered `collect`.

## Limitations

- **No `GROUP BY` keyword.** Grouping is always implicit on
  non-aggregated columns.
- **No `HAVING` keyword.** Filter post-aggregate through
  `WITH … WHERE`.
- **Aggregates rejected in `WHERE`.** Analysis-time error.
- `stdev`, `stdevp`, `percentileCont`, `percentileDisc` do **not**
  support `DISTINCT`. Use `collect(DISTINCT x)` +
  [`UNWIND`](./unwind-merge#unwind) + aggregate if you need percentile
  of distinct values.

See [Limitations](../limitations#aggregates) for the full list.

## See also

- [**Aggregation Functions**](../functions/aggregation) — per-function reference.
- [**RETURN / WITH**](./return-with) — projection, grouping, HAVING.
- [**WHERE**](./where) — filtering before aggregation.
- [**UNWIND**](./unwind-merge#unwind) — turn an aggregated list back into rows.
- [**OPTIONAL MATCH with aggregation**](./match#optional-match-with-aggregation) —
  `count(*)` vs `count(expr)` subtleties.
- [**Query Examples → Aggregation**](./examples#aggregation) — copy-paste recipes.
