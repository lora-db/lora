---
title: ORDER BY, SKIP, LIMIT — Ordering and Pagination
sidebar_label: Ordering & Pagination
description: Order and paginate LoraDB query results with ORDER BY, SKIP, and LIMIT — evaluation order, stable sorting rules, and their interaction with projection and aggregation.
---

# ORDER BY, SKIP, LIMIT — Ordering and Pagination

`ORDER BY`, `SKIP`, and `LIMIT` shape the final result set of a
query, or the output of a [`WITH`](./return-with#with) stage. They
are **evaluated after** projection and
[aggregation](./aggregation).

## Overview

| Goal | Clause |
|---|---|
| Sort ascending | <CypherCode code="ORDER BY expr ASC" /> (default) |
| Sort descending | <CypherCode code="ORDER BY expr DESC" /> |
| Sort by multiple keys | <CypherCode code="ORDER BY a ASC, b DESC" /> |
| Skip rows | <CypherCode code="SKIP n" /> |
| Limit rows | <CypherCode code="LIMIT n" /> |
| Top-N | <CypherCode code="ORDER BY expr LIMIT n" /> |
| Pagination | <CypherCode code="ORDER BY key SKIP $offset LIMIT $size" /> |

## Syntax

```text
<RETURN | WITH> expr [, expr]
  [ORDER BY expr [ASC | DESC] [, expr [ASC | DESC]]]
  [SKIP n]
  [LIMIT n]
```

`n` must be a non-negative integer literal or a parameter that resolves
to one. Negative or non-integer `SKIP` / `LIMIT` is a semantic error.

## Order a single column

```cypher
MATCH (n:User)
RETURN n.name
ORDER BY n.name ASC
```

`ASC` is the default; `ORDER BY n.name` is equivalent.

### Direction comparison

| Type | Ordering |
|---|---|
| `Int`, `Float` | Numeric (`NaN` is incomparable) |
| `String` | Byte-lexicographic |
| `Boolean` | `false < true` |
| `Date`, `DateTime`, `Time`, `LocalTime`, `LocalDateTime` | Chronological |
| `Duration` | By total length (calendar-aware) |
| `Point` | Not orderable — equality only |
| `Null` | See [Nulls in ordering](#nulls-in-ordering) |

## Multi-key ordering

Later keys break ties in earlier keys.

```cypher
MATCH (p:Person)
RETURN p
ORDER BY p.last_name ASC, p.first_name ASC, p.id ASC
```

Mix directions freely:

```cypher
MATCH (u:User)
RETURN u
ORDER BY u.country ASC, u.age DESC
```

## Ordering by computed expression

You can order on anything that evaluates to a comparable value.

```cypher
MATCH (p:Person)
RETURN p.name, p.age
ORDER BY p.age * -1 DESC            -- youngest first
```

You can also order by an alias defined in the same `RETURN`:

```cypher
MATCH (u:User)-[:WROTE]->(:Post)
RETURN u.name AS author, count(*) AS posts
ORDER BY posts DESC, author ASC
```

## Pagination — SKIP + LIMIT

```cypher
MATCH (n:User)
RETURN n
ORDER BY n.id
SKIP  20
LIMIT 10
```

- `SKIP 0` / no `SKIP` — start at the first row.
- `LIMIT 0` — return zero rows.
- `LIMIT n` without `ORDER BY` — the "first n" rows are **undefined**
  without a tiebreaker. Always pair `LIMIT` with `ORDER BY` when the
  order matters.

Parameters work identically:

```cypher
MATCH (n:User)
RETURN n
ORDER BY n.id
SKIP $offset
LIMIT $page_size
```

### Stable pagination

`SKIP + LIMIT` is offset-based and can miss / repeat rows if the
underlying data changes between pages. For stable pagination, sort by an
immutable key and filter by "last seen":

```cypher
MATCH (n:User)
WHERE n.id > $after
RETURN n
ORDER BY n.id
LIMIT $page_size
```

Then use the last row's id as the next `$after`.

## Ordering inside a pipeline

`ORDER BY` and `LIMIT` can attach to a [`WITH`](./return-with#with)
stage. Only the surviving rows move forward.

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, count(p) AS posts
ORDER BY posts DESC
LIMIT 10
MATCH (u)-[:FOLLOWS]->(other)
RETURN u.name, count(other) AS following
```

This is how you express "top 10 posters, then each of their followings".

### Custom sort order with CASE

Use [`CASE`](./return-with#case-expressions) to project a sort key
that doesn't match the data's natural ordering. Typical for ordering
strings by business meaning rather than alphabet:

```cypher
MATCH (t:Task)
RETURN t.title, t.status
ORDER BY CASE t.status
           WHEN 'urgent' THEN 0
           WHEN 'open'   THEN 1
           WHEN 'review' THEN 2
           ELSE               3
         END, t.created_at DESC
```

One row per task, sorted urgent-first then newest-within-tier.

## DISTINCT + ORDER BY

[`DISTINCT`](./return-with#distinct) runs before ordering. A column used
to sort must either be a projected column or a deterministic expression
over projected columns.

```cypher
MATCH (p:Person)
RETURN DISTINCT p.city
ORDER BY p.city
```

## UNION + ORDER BY / LIMIT

For [`UNION` / `UNION ALL`](./return-with#union--union-all),
`ORDER BY` and `LIMIT` apply to the combined result:

```cypher
MATCH (n:User)    RETURN n.name AS name
UNION ALL
MATCH (n:Product) RETURN n.name AS name
ORDER BY name
LIMIT 20
```

## Nulls in ordering

`null` values sort **last** in ascending order and **first** in
descending order. There is no `NULLS FIRST` / `NULLS LAST` keyword —
reverse the sort direction, or guard with
[`coalesce`](../functions/overview#type-conversion-and-checking) to
change placement.

```cypher
-- Nulls to the end of a DESC sort
MATCH (p:Person)
RETURN p.name, p.rank
ORDER BY coalesce(p.rank, -2147483648) DESC

-- Nulls to the start of an ASC sort
MATCH (p:Person)
RETURN p.name, p.rank
ORDER BY coalesce(p.rank, -2147483648) ASC
```

## Common patterns

### Top-N

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
RETURN u.name, count(p) AS posts
ORDER BY posts DESC
LIMIT 10
```

### First row only

```cypher
MATCH (u:User {email: $email})
RETURN u
ORDER BY u.created ASC
LIMIT 1
```

### Bottom-N (with tiebreaker)

```cypher
MATCH (p:Product)
RETURN p
ORDER BY p.price ASC, p.id ASC
LIMIT 5
```

### Page N

```cypher
MATCH (n:Post)
RETURN n
ORDER BY n.published_at DESC, n.id DESC
SKIP ($page - 1) * $size
LIMIT $size
```

### Random sample (unstable)

```cypher
MATCH (n)
RETURN n
ORDER BY rand()
LIMIT 10
```

[`rand()`](../functions/math#random) is re-evaluated per row — good for
a rough sample, but don't rely on it for cryptographic randomness.

## Edge cases

### Ordering by a nullable column with NULL present

```cypher
MATCH (p:Person)
RETURN p.name, p.rank
ORDER BY p.rank ASC
-- Rows where p.rank IS NULL appear at the end
```

### Ordering by a type-mixed column

If `p.score` is sometimes `Int` and sometimes `String`, ordering is
well-defined but unlikely to match your intent. Cast with
[`toString`](../functions/string#type-conversion) or
[`toInteger`](../functions/overview#type-conversion-and-checking) first.

### LIMIT in the middle of a pipeline

`LIMIT` on a `WITH` trims rows for downstream stages — subsequent
`MATCH` clauses only run for the surviving rows. Use it to keep a
multi-stage query bounded:

```cypher
MATCH (u:User)
WITH u ORDER BY u.created DESC LIMIT 100
MATCH (u)-[:WROTE]->(p)
RETURN u.name, count(p)
```

### SKIP larger than result count

Returns zero rows — never an error.

## Notes on performance

- `ORDER BY` sorts the full projected result set in memory. Combine with
  [`LIMIT`](./ordering) when the input is large.
- There are **no property indexes** (see
  [Limitations](../limitations#storage)), so `ORDER BY n.prop` walks
  every matched row, not a pre-sorted index.
- Pair `LIMIT` with a stable key (like an id) so re-running the same
  query yields the same rows.

## See also

- [**RETURN / WITH**](./return-with) — where ordering attaches.
- [**Aggregation**](./aggregation) — aggregation runs before ordering.
- [**Query Examples**](./examples) — copy-paste Top-N / pagination.
- [**Math → rand**](../functions/math#random) — random sampling.
- [**Temporal Functions**](../functions/temporal) — ordering dates.
