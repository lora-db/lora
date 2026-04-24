---
title: RETURN and WITH — Projecting and Piping Results
sidebar_label: RETURN / WITH
description: The RETURN and WITH clauses in LoraDB — RETURN ends a query and hands rows back to the caller, WITH pipes them into the next stage. Projections, aliasing, and DISTINCT semantics.
---

# RETURN and WITH — Projecting and Piping Results

Both clauses project rows forward. [`WITH`](#with) hands the
projected rows to the next clause; [`RETURN`](#return) ends the
query and hands them back to the caller. Rows typically come from a
preceding [`MATCH`](./match) or [`UNWIND`](./unwind-merge#unwind).

> Think of `WITH` as a pipe between stages, and `RETURN` as the
> output of the final stage.

## Overview

| Goal | Clause |
|---|---|
| Shape the final output | [<CypherCode code="RETURN" />](#return) |
| Rename a column | <CypherCode code="RETURN expr AS name" /> |
| Deduplicate rows | [<CypherCode code="DISTINCT" />](#distinct) |
| Sort / paginate | [<CypherCode code="ORDER BY" />, <CypherCode code="SKIP" />, <CypherCode code="LIMIT" />](./ordering) |
| Build a subset-map per entity | [Map projection](#map-projection) |
| Conditional per-row value | [<CypherCode code="CASE … WHEN … THEN … END" />](#case-expressions) |
| Pipe into the next stage | [<CypherCode code="WITH" />](#with) |
| HAVING-style filtering | [<CypherCode code="WITH … WHERE" />](#having-style-filtering-with) |
| Combine two result sets | [<CypherCode code="UNION" /> / <CypherCode code="UNION ALL" />](#union--union-all) |

## RETURN

### Basic projection

Return whole entities, bare properties, or any expression.

```cypher
MATCH (n) RETURN n
MATCH (n) RETURN n.name, n.age
MATCH (n) RETURN n.name AS userName
MATCH (n) RETURN n.age * 2 AS doubled_age
```

Aliases (`AS`) set the column name in the host response. Reserve them
for anything the consumer has to look up by key.

### Star

`RETURN *` projects every variable in scope. Handy for exploratory work,
noisy for production queries.

```cypher
MATCH (a)-[r]->(b) RETURN *
MATCH (a)-[r]->(b) RETURN *, a.name AS name
```

### Literal expressions

Return constants, function calls, arithmetic:

```cypher
RETURN 1 + 2 AS three
RETURN timestamp() AS now_ms
RETURN datetime() AS now, date() AS today
RETURN 'hello, ' + $name AS greeting
```

### DISTINCT

Deduplicate the output rows. Applies to the full row, not per-column.

```cypher
MATCH (n) RETURN DISTINCT n.city
MATCH (p:Person)-[:WROTE]->(:Post) RETURN DISTINCT p
```

`DISTINCT` runs **before** [`ORDER BY`](./ordering) and is expensive on
large inputs — prefer filtering with [`WHERE`](./where) first.

### ORDER BY, SKIP, LIMIT

Shape the final result set. Full reference:
[Ordering & Pagination](./ordering).

```cypher
MATCH (n) RETURN n ORDER BY n.name ASC
MATCH (n) RETURN n ORDER BY n.last ASC, n.first DESC
MATCH (n) RETURN n ORDER BY n.name DESC SKIP 5 LIMIT 10
MATCH (n) RETURN n LIMIT 1
```

### Map projection

Shape a node or relationship into a map with only the keys you want —
useful when the consumer doesn't need every property.

```cypher
-- Pick a subset
MATCH (n:User) RETURN n {.name, .age}

-- All properties (equivalent to `properties(n)`)
MATCH (n:User) RETURN n {.*}

-- Rename + compute
MATCH (n:User) RETURN n {.name, score: n.age * 2}

-- Include related data
MATCH (u:User)
RETURN u {.name, posts: [(u)-[:WROTE]->(p) | p.title]}
```

See also [Lists & Maps → Map projection](../data-types/lists-and-maps#map-projection).

### CASE expressions

`CASE` is LoraDB's conditional expression — the Cypher equivalent of
SQL's `CASE` or a ternary. It's a plain expression, so it works
anywhere a value is allowed: `RETURN`, `WITH`, `SET`, `ORDER BY`, and
inside predicates.

Two forms.

**Simple form** — match an input against successive values:

```cypher
MATCH (p:Product)
RETURN p.name,
       CASE p.tier
         WHEN 'gold'   THEN 1.2
         WHEN 'silver' THEN 1.1
         WHEN 'bronze' THEN 1.0
         ELSE                0.9
       END AS multiplier
```

**Generic form** — each branch is its own boolean expression:

```cypher
MATCH (o:Order)
RETURN o.id,
       CASE
         WHEN o.amount >= 1000 THEN 'large'
         WHEN o.amount >= 100  THEN 'medium'
         WHEN o.amount >= 10   THEN 'small'
         ELSE                       'tiny'
       END AS bucket
```

The generic form is the one you'll reach for most often — it allows
arbitrary predicates per branch, including
[null-safe](../data-types/scalars#null) checks and pattern-based
predicates.

#### ELSE is optional

Omitting `ELSE` implicitly falls through to `null`:

```cypher
MATCH (u:User)
RETURN u.name,
       CASE WHEN u.score > 100 THEN 'pro' END AS tier
-- tier is null for users at or below 100
```

#### Branches are short-circuit

Branches evaluate top-to-bottom; the first matching `WHEN` wins. Place
the narrowest condition first if branches overlap.

#### Type coercion across branches

Every branch — including the implicit `null` from a missing `ELSE` —
can return any type. Nothing forces uniformity. Most callers prefer
one type per `CASE` for predictable downstream shape:

```cypher
RETURN CASE WHEN $has_value THEN $value ELSE null END AS maybe
```

#### In predicates and filters

`CASE` is an expression, so it composes inside
[`WHERE`](./where) and [`ORDER BY`](./ordering):

```cypher
MATCH (p:Product)
WHERE CASE
        WHEN p.on_sale THEN p.sale_price
        ELSE                p.price
      END < $max
RETURN p
```

```cypher
MATCH (t:Task)
RETURN t
ORDER BY CASE t.status
           WHEN 'urgent' THEN 0
           WHEN 'open'   THEN 1
           ELSE               2
         END, t.created_at
```

That ordering pattern is how you express "custom priority order" —
ASCII/byte order on the status string would give you `open`, `urgent`,
not what you want.

#### In SET and aggregates

```cypher
MATCH (u:User)
SET u.tier = CASE WHEN u.score >= 100 THEN 'pro' ELSE 'free' END
```

```cypher
MATCH (r:Review)
RETURN r.product,
       count(CASE WHEN r.stars >= 4 THEN 1 END) AS positive,
       count(CASE WHEN r.stars <= 2 THEN 1 END) AS negative
```

Combining `CASE` with [`count(expr)`](../functions/aggregation#count)
is the idiomatic way to express "count rows that satisfy X" inside a
larger aggregation — `count` skips `null`, so the missing `ELSE`
branch is exactly what you want.

#### See also

- [`coalesce`](../functions/overview#type-conversion-and-checking) — a
  compact shorthand when you only need "first non-null".
- [WHERE → boolean operators](./where#boolean-operators) — three-valued
  logic rules that `CASE` predicates follow.
- [Ordering by computed expression](./ordering#ordering-by-computed-expression).

## WITH

`WITH` is the pipe of Cypher. Use it to split a query into stages. The
projected rows of one stage become the input rows of the next.

### Piping variables

The simplest `WITH` — pass the bindings through untouched:

```cypher
MATCH (a)-[r]->(b)
WITH a, r, b
RETURN a, r, b
```

That's pedagogical; a real query uses `WITH` to change something.

### Transforming between stages

`WITH` can rename, compute, filter, aggregate — anything `RETURN` does
at the end of the pipeline.

```cypher
MATCH (u:User)
WITH u, u.born AS year
WHERE year < 1900
RETURN u.name, year
```

### HAVING-style filtering (WITH)

Aggregates are not allowed in [`WHERE`](./where). Aggregate into a
`WITH`, then filter:

```cypher
MATCH (p:Person)-[:WORKS_AT]->(c:Company)
WITH c.name AS company, count(p) AS employees
WHERE employees > 5
RETURN company, employees
```

See [Aggregation → HAVING-style filtering](./aggregation#5-filter-after-aggregating-having-style).

### Renaming and shaping

```cypher
MATCH (n:User)
WITH n.name AS username
RETURN username
```

### Ordering inside a pipeline

`ORDER BY` and `LIMIT` attach to a `WITH` stage just like they do to a
final `RETURN`. Only surviving rows move forward.

```cypher
MATCH (n:User)
WITH n
ORDER BY n.age DESC
LIMIT 3
MATCH (n)-[:FOLLOWS]->(other)
RETURN n.name, other.name
```

### Chaining multiple WITH stages

```cypher
MATCH (o:Order)-[:CONTAINS]->(i:Item)
WITH o, sum(i.price) AS total
WHERE total > 100
WITH o, total
ORDER BY total DESC
LIMIT 20
RETURN o.id, total
```

Each stage's output columns become the next stage's bindings — any
variable not projected is dropped.

### Losing variables through WITH

A variable must be explicitly projected into `WITH` to survive. This is
a common source of `Unknown variable` errors:

```cypher
MATCH (a:User)-[r:KNOWS]->(b)
WITH a         -- r and b drop out of scope here
RETURN a, r    -- error: r is not in scope
```

Either pipe them through (`WITH a, r, b`) or don't bind them in the
first place.

## UNION / UNION ALL

Combine two result sets that share a column shape. `UNION`
deduplicates; `UNION ALL` doesn't.

```cypher
MATCH (n:User)    RETURN n.name AS name
UNION
MATCH (n:Product) RETURN n.name AS name
```

```cypher
MATCH (a:A) RETURN a.v AS v
UNION ALL
MATCH (b:B) RETURN b.v AS v
UNION ALL
MATCH (c:C) RETURN c.v AS v
```

### ORDER BY / LIMIT across UNION

Apply at the very end — they shape the combined result:

```cypher
MATCH (n:User)    RETURN n.name AS name
UNION ALL
MATCH (n:Product) RETURN n.name AS name
ORDER BY name
LIMIT 10
```

### Column shape must match

Both sides must expose the same column names in the same order:

```cypher
-- Valid
MATCH (n:User)    RETURN n.name AS name, 'user'    AS kind
UNION ALL
MATCH (n:Product) RETURN n.name AS name, 'product' AS kind

-- Invalid — column shape mismatch
MATCH (n:User)    RETURN n.name, 'user'
UNION
MATCH (n:Product) RETURN n.name, n.price, 'product'
```

## Common patterns

### Pick N, then follow

Top-3 users by age, then project their friends:

```cypher
MATCH (u:User)
WITH u ORDER BY u.age DESC LIMIT 3
MATCH (u)-[:FOLLOWS]->(f)
RETURN u.name, collect(f.name) AS following
```

### Count-and-rank

```cypher
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, count(p) AS posts
ORDER BY posts DESC
LIMIT 10
RETURN u.name, posts
```

### Project the top of a nested list

```cypher
MATCH (p:Person)
RETURN p.name,
       [(p)-[:KNOWS]->(f) | f.name][..5] AS first_five_friends
```

### Keep only rows that meet an aggregate

```cypher
MATCH (r:Review)
WITH r.product AS product, avg(r.stars) AS mean
WHERE mean >= 4.5
RETURN product, mean
ORDER BY mean DESC
```

### Using both RETURN DISTINCT and ORDER BY

`DISTINCT` runs first — you can only order by projected columns.

```cypher
MATCH (p:Person)
RETURN DISTINCT p.city AS city
ORDER BY city
```

## Edge cases

### Empty aggregation input

`RETURN count(*)` with zero matches still emits one row with value `0`.
`sum`, `avg`, `min`, `max` return `null` on empty input. See
[Aggregation → count](./aggregation#count).

### WITH without projection

Every `WITH` must project at least one thing — there's no "pass
everything" shorthand. `WITH *` works and projects every in-scope
variable:

```cypher
MATCH (a)-[r]->(b)
WITH *
RETURN a, r, b
```

### Aggregation in WITH without a group key

Aggregating with no non-aggregated column folds everything into one row:

```cypher
MATCH (o:Order)
WITH sum(o.amount) AS total
RETURN total
```

## See also

- [**MATCH**](./match) — source of rows.
- [**WHERE**](./where) — predicate filtering; also used after `WITH`.
- [**Aggregation**](./aggregation) — group-and-collapse semantics.
- [**Ordering & Pagination**](./ordering) — `ORDER BY`, `SKIP`, `LIMIT`.
- [**Lists & Maps → Map projection**](../data-types/lists-and-maps#map-projection).
- [**List Functions → Pattern comprehension**](../functions/list#pattern-comprehension) — inline nested projections.
