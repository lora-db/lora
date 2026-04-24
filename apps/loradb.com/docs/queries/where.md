---
title: WHERE — Filtering Rows
sidebar_label: WHERE
description: The WHERE clause in LoraDB — boolean filters after MATCH, WITH, or OPTIONAL MATCH — including comparisons, regex, list predicates, EXISTS subqueries, and when to filter after aggregation.
---

# WHERE — Filtering Rows

`WHERE` filters rows produced by the preceding [`MATCH`](./match),
[`WITH`](./return-with#with), or [`OPTIONAL MATCH`](./match#optional-match).
Any boolean expression is valid.

> `WHERE` runs **before** [`RETURN`](./return-with) and
> [aggregation](./aggregation). For filtering _after_ an aggregate (SQL
> `HAVING`), pipe through [`WITH`](./return-with#with) first.

## Overview

| Goal | Operator / Keyword |
|---|---|
| Compare scalars | <CypherCode code="=" />, <CypherCode code="<>" />, <CypherCode code="<" />, <CypherCode code="<=" />, <CypherCode code=">" />, <CypherCode code=">=" /> |
| Boolean combinators | <CypherCode code="AND" />, <CypherCode code="OR" />, <CypherCode code="NOT" />, <CypherCode code="XOR" /> |
| Match a prefix / suffix / substring | <CypherCode code="STARTS WITH" />, <CypherCode code="ENDS WITH" />, <CypherCode code="CONTAINS" /> |
| Regex | <CypherCode code="=~" /> |
| Null-safe check | <CypherCode code="IS NULL" />, <CypherCode code="IS NOT NULL" /> |
| Membership | <CypherCode code="IN [...]" /> / <CypherCode code="IN $param" /> |
| List quantifiers | <CypherCode code="all" />, <CypherCode code="any" />, <CypherCode code="none" />, <CypherCode code="single" /> |
| Pattern existence | <CypherCode code="EXISTS { (...)-[...]->(...) }" /> |
| Conditional branch | [<CypherCode code="CASE WHEN … THEN … END" />](./return-with#case-expressions) |

## Comparison

```cypher
MATCH (n:User) WHERE n.age > 18                     RETURN n
MATCH (n:User) WHERE n.age >= 18 AND n.age <= 65    RETURN n
MATCH (n:User) WHERE n.name = 'alice'               RETURN n
MATCH (n:User) WHERE n.name <> 'bob'                RETURN n
```

Comparison returns `null` (not `false`) when either operand is `null` or
when the types mismatch — see [Scalars → Null](../data-types/scalars#null)
and [Limitations](../limitations#operators-and-expressions).

No `BETWEEN` keyword — use explicit `>=` / `<=` bounds:

```cypher
MATCH (p:Product) WHERE p.price >= 10 AND p.price <= 50 RETURN p
```

## Boolean operators

```cypher
MATCH (n) WHERE n.active AND n.age >= 18 RETURN n
MATCH (n) WHERE n.active OR n.age < 18   RETURN n
MATCH (n) WHERE NOT n.active             RETURN n
MATCH (n) WHERE n.active XOR n.admin     RETURN n
```

Three-valued logic applies — `null AND false` is `false`, `null AND
true` is `null`. See the full truth table in
[Scalars → Null](../data-types/scalars#null).

### Precedence

`NOT` binds tightest, then `AND`, then `XOR`, then `OR`. Parenthesise
freely when in doubt:

```cypher
-- These are equivalent
MATCH (n) WHERE n.a OR n.b AND n.c       RETURN n
MATCH (n) WHERE n.a OR (n.b AND n.c)     RETURN n
```

## String matching

All string operators are **case-sensitive**. For case-insensitive
matching, normalise with [`toLower`](../functions/string#tolower--toupper)
or `toUpper` on both sides.

```cypher
MATCH (n) WHERE n.name STARTS WITH 'a'   RETURN n
MATCH (n) WHERE n.name ENDS   WITH 'z'   RETURN n
MATCH (n) WHERE n.name CONTAINS 'al'     RETURN n
```

```cypher
MATCH (u:User)
WHERE toLower(u.name) STARTS WITH toLower($query)
RETURN u
```

## Regex

```cypher
MATCH (u:User) WHERE u.name  =~ 'A.*e'           RETURN u
MATCH (u:User) WHERE u.email =~ '.*@loradb\\.com' RETURN u
```

Uses the Rust `regex` crate — standard RE2-style syntax, no
backreferences. Anchors are implicit: `=~ 'foo'` matches only the full
string `"foo"`, not any string containing `foo`. Use `.*` to allow
prefixes/suffixes, or `CONTAINS 'foo'` for plain substring.

## Null checks

Most expressions involving `null` propagate to `null`, not to `false`.
Use `IS NULL` / `IS NOT NULL`, **not** `= null`.

```cypher
MATCH (n) WHERE n.optional IS NULL     RETURN n
MATCH (n) WHERE n.optional IS NOT NULL RETURN n

-- Wrong — always yields zero rows
MATCH (n) WHERE n.optional = null      RETURN n
```

Common guard: require a property to exist _and_ be non-empty:

```cypher
MATCH (u:User)
WHERE u.email IS NOT NULL AND size(u.email) > 0
RETURN u
```

## IN

Membership check against a list literal or parameter.

```cypher
MATCH (n)      WHERE n.age IN [18, 21, 25]         RETURN n
MATCH (n)      WHERE NOT n.name IN ['Alice', 'Bob'] RETURN n
MATCH (u:User) WHERE u.id IN $ids                   RETURN u
```

`$ids` binds to a [list](../data-types/lists-and-maps#lists) in the host
language (`[1, 2, 3]` in JS/Python, `Vec<LoraValue>` in Rust).

### IN with DISTINCT

```cypher
MATCH (u:User)-[:OWNS]->(p:Project)
WHERE p.tag IN $tags
RETURN DISTINCT u
```

### `IN` over an empty list

`x IN []` is always `false`. Empty-list parameters drop every row —
validate on the host side if that's a likely accident.

## Arithmetic in WHERE

Any expression that produces a boolean is allowed.

```cypher
MATCH (n) WHERE n.age + 5 > 30                 RETURN n
MATCH (n) WHERE n.price * n.quantity > 1000    RETURN n
MATCH (n) WHERE (n.end - n.start).seconds > 60 RETURN n
```

## Cross-variable comparison

Predicates can reference multiple bindings from the `MATCH`:

```cypher
MATCH (a:User)-[:FOLLOWS]->(b:User)
WHERE a.age > b.age
RETURN a.name AS older, b.name AS younger
```

```cypher
MATCH (a:User)-[:FOLLOWS]->(b)
WHERE a.country = b.country
RETURN a, b
```

## Pattern existence

Use `EXISTS { pattern }` to filter rows by whether a pattern matches —
without adding extra rows to the output.

```cypher
MATCH (u:User)
WHERE EXISTS { (u)-[:FOLLOWS]->() }
RETURN u
```

```cypher
-- Users who have never posted
MATCH (u:User)
WHERE NOT EXISTS { (u)-[:WROTE]->(:Post) }
RETURN u.name
```

This is the anti-join pattern — cheaper than
`OPTIONAL MATCH … WHERE other IS NULL` when you don't need the optional
result.

## List predicates

Ask a question about the elements of a list. Covered fully in
[List Functions → Predicates](../functions/list#predicates-in-where).

```cypher
MATCH (n) WHERE all(x IN n.scores WHERE x > 0)      RETURN n
MATCH (n) WHERE any(x IN n.tags   WHERE x = 'VIP')  RETURN n
MATCH (n) WHERE none(x IN n.scores WHERE x < 0)     RETURN n
MATCH (n) WHERE single(x IN n.scores WHERE x = 100) RETURN n
```

## CASE in predicates

[`CASE`](./return-with#case-expressions) is an expression, so it
composes inside `WHERE` wherever you'd write a scalar. Useful when the
comparison value itself depends on a per-row condition:

```cypher
MATCH (p:Product)
WHERE CASE
        WHEN p.on_sale THEN p.sale_price
        ELSE                p.price
      END <= $budget
RETURN p
```

Equivalent with [`coalesce`](../functions/overview#type-conversion-and-checking)
when you only need a "first non-null" fallback:

```cypher
MATCH (p:Product)
WHERE coalesce(p.sale_price, p.price) <= $budget
RETURN p
```

See [`RETURN → CASE expressions`](./return-with#case-expressions) for
the full syntax.

## Common patterns

### Safe prefix search

```cypher
MATCH (u:User)
WHERE toLower(u.name) STARTS WITH toLower($query)
RETURN u
ORDER BY u.name
LIMIT 20
```

### Tag filtering

```cypher
MATCH (p:Product)
WHERE any(t IN p.tags WHERE t IN $tags)
RETURN p
```

### Date range

```cypher
MATCH (e:Event)
WHERE e.at >= date('2024-01-01') AND e.at < date('2025-01-01')
RETURN e
ORDER BY e.at
```

See [Temporal Functions](../functions/temporal) for constructors and
arithmetic.

### "Has at least one of each"

```cypher
MATCH (u:User)
WHERE EXISTS { (u)-[:OWNS]->(:Repo) }
  AND EXISTS { (u)-[:WROTE]->(:Post) }
RETURN u
```

### "Has none of these"

```cypher
MATCH (p:Post)
WHERE none(t IN ['spam', 'nsfw', 'flagged']
           WHERE EXISTS { (p)-[:TAGGED]->(:Tag {name: t}) })
RETURN p
```

### Chained optional predicates

Break a complex predicate into `WITH` stages for readability. Each
stage only sees what it needs:

```cypher
MATCH (u:User)
WITH u, coalesce(u.score, 0) AS s
WHERE s >= 50
WITH u, s, (u.last_seen >= datetime() - duration('P30D')) AS recent
WHERE recent
RETURN u.handle, s
```

Same meaning as one giant `WHERE`, but each stage is narrower and
easier to trace.

### Default a missing value

Use [`coalesce`](../functions/overview#type-conversion-and-checking) to
substitute a fallback:

```cypher
MATCH (p:Person)
WHERE coalesce(p.score, 0) >= 50
RETURN p
```

## Edge cases

### Aggregates in WHERE

Aggregates are **not** allowed in `WHERE`. Attempting `WHERE count(x) > 5`
is a semantic error — use
[`WITH … WHERE`](./aggregation#5-filter-after-aggregating-having-style)
instead.

### Missing property vs null property

A missing property and a property set to `null` are indistinguishable —
both return `null` on access. `SET n.prop = null` is the idiomatic way
to remove a property (see [SET → computed expressions](./set-delete#computed-expressions)).

### Comparison across types

Cross-type comparisons (e.g. `Int < String`) return `null`, not `false`
— see [Limitations](../limitations#operators-and-expressions). Cast
explicitly with [`toString`](../functions/string#type-conversion) /
`toInteger` / `toFloat` first.

### `null` propagation in compound predicates

```cypher
-- If n.a is null, the whole expression is null → row dropped
MATCH (n) WHERE n.a = 1 AND n.b > 2 RETURN n
```

Guard null-prone properties with `IS NOT NULL` or `coalesce` when you
need to reason about three-valued logic.

## See also

- [**MATCH**](./match) — what feeds `WHERE`.
- [**RETURN / WITH**](./return-with) — projection and HAVING-style filtering.
- [**Aggregation**](./aggregation) — group, then filter via `WITH`.
- [**String Functions**](../functions/string) — `toLower`, `replace`, regex.
- [**List Functions**](../functions/list) — `all`, `any`, `none`, `single`.
- [**Scalars → Null**](../data-types/scalars#null) — three-valued logic.
- [**Temporal Functions**](../functions/temporal) — date/time predicates.
