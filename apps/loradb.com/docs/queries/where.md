---
title: WHERE — Filtering Rows
sidebar_label: WHERE
description: The WHERE clause in LoraDB — boolean filters after MATCH, WITH, or OPTIONAL MATCH — including comparisons, regex, list predicates, EXISTS subqueries, and when to filter after aggregation.
---

# WHERE — Filtering Rows

`WHERE` filters rows produced by the preceding [`MATCH`](./match),
[`WITH`](./return-with#with), or
[`OPTIONAL MATCH`](./match#optional-match). Any boolean expression
is valid.

> `WHERE` runs **before** [`RETURN`](./return-with) and
> [aggregation](./aggregation). For filtering _after_ an aggregate
> (SQL `HAVING`), pipe through [`WITH`](./return-with#with).

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

<QueryCodeBlock code={String.raw`MATCH (n:User) WHERE n.age > 18                     RETURN n;
MATCH (n:User) WHERE n.age >= 18 AND n.age <= 65    RETURN n;
MATCH (n:User) WHERE n.name = 'alice'               RETURN n;
MATCH (n:User) WHERE n.name <> 'bob'                RETURN n`} />

RANGE indexes can accelerate equality and range comparisons when the
predicate is scoped to a matching label or relationship type:

<QueryCodeBlock code={String.raw`CREATE INDEX user_age FOR (u:User) ON (u.age);
MATCH (u:User) WHERE u.age >= 18 AND u.age < 65 RETURN u`} />

Comparison returns `null` (not `false`) when either operand is `null` or
when the types mismatch — see [Scalars → Null](../data-types/scalars#null)
and [Limitations](../limitations#operators-and-expressions).

No `BETWEEN` keyword — use explicit `>=` / `<=` bounds:

<QueryCodeBlock code={String.raw`MATCH (p:Product) WHERE p.price >= 10 AND p.price <= 50 RETURN p`} />

## Boolean operators

<QueryCodeBlock code={String.raw`MATCH (n) WHERE n.active AND n.age >= 18 RETURN n;
MATCH (n) WHERE n.active OR n.age < 18   RETURN n;
MATCH (n) WHERE NOT n.active             RETURN n;
MATCH (n) WHERE n.active XOR n.admin     RETURN n`} />

Three-valued logic applies — `null AND false` is `false`, `null AND
true` is `null`. See the full truth table in
[Scalars → Null](../data-types/scalars#null).

`AND` and `OR` evaluate left to right and stop as soon as the left side
decides: after `false AND` or `true OR` the right side is not evaluated,
so it cannot fail the query. That makes a guard work:

<QueryCodeBlock code={String.raw`MATCH (e:Event)
WHERE type.of(e.at) = 'DATE' AND e.at >= date('2024-01-01')
RETURN e`} />

Here an `e.at` that holds a `DATETIME` never reaches the comparison,
which would otherwise be an error (see
[Temporal types](../data-types/temporal#cross-type-comparison)). A
`null` left side decides nothing, so the right side is evaluated.

### Precedence

`NOT` binds tightest, then `AND`, then `XOR`, then `OR`. Parenthesise
freely when in doubt:

<QueryCodeBlock code={String.raw`// These are equivalent
MATCH (n) WHERE n.a OR n.b AND n.c       RETURN n;
MATCH (n) WHERE n.a OR (n.b AND n.c)     RETURN n`} />

## String matching

All string operators are **case-sensitive**. For case-insensitive
matching, normalise with [`string.lower`](../functions/string#tolower--toupper)
or `string.upper` on both sides.

<QueryCodeBlock code={String.raw`MATCH (n) WHERE n.name STARTS WITH 'a'   RETURN n;
MATCH (n) WHERE n.name ENDS   WITH 'z'   RETURN n;
MATCH (n) WHERE n.name CONTAINS 'al'     RETURN n`} />

TEXT indexes can accelerate these string predicates:

<QueryCodeBlock code={String.raw`CREATE TEXT INDEX user_name FOR (u:User) ON (u.name);
MATCH (u:User) WHERE u.name STARTS WITH 'Al' RETURN u`} />

<QueryCodeBlock code={String.raw`MATCH (u:User)
WHERE string.lower(u.name) STARTS WITH string.lower($query)
RETURN u`} />

## Regex

<QueryCodeBlock code={String.raw`MATCH (u:User) WHERE u.name  =~ 'A.*e'           RETURN u;
MATCH (u:User) WHERE u.email =~ '.*@loradb\\.com' RETURN u`} />

Uses the Rust `regex` crate — standard RE2-style syntax, no
backreferences. Anchors are implicit: `=~ 'foo'` matches only the full
string `"foo"`, not any string containing `foo`. Use `.*` to allow
prefixes/suffixes, or `CONTAINS 'foo'` for plain substring.

## Null checks

Most expressions involving `null` propagate to `null`, not to `false`.
Use `IS NULL` / `IS NOT NULL`, **not** `= null`.

<QueryCodeBlock code={String.raw`MATCH (n) WHERE n.optional IS NULL     RETURN n;
MATCH (n) WHERE n.optional IS NOT NULL RETURN n

;// Wrong — always yields zero rows
MATCH (n) WHERE n.optional = null      RETURN n`} />

Common guard: require a property to exist _and_ be non-empty:

<QueryCodeBlock code={String.raw`MATCH (u:User)
WHERE u.email IS NOT NULL AND string.length(u.email) > 0
RETURN u`} />

## IN

Membership check against a list literal or parameter.

<QueryCodeBlock code={String.raw`MATCH (n)      WHERE n.age IN [18, 21, 25]         RETURN n;
MATCH (n)      WHERE NOT n.name IN ['Alice', 'Bob'] RETURN n;
MATCH (u:User) WHERE u.id IN $ids                   RETURN u`} />

`$ids` binds to a [list](../data-types/lists-and-maps#lists) in the host
language (`[1, 2, 3]` in JS/Python, `Vec<LoraValue>` in Rust).

### IN with DISTINCT

<QueryCodeBlock code={String.raw`MATCH (u:User)-[:OWNS]->(p:Project)
WHERE p.tag IN $tags
RETURN DISTINCT u`} />

### `IN` over an empty list

`x IN []` is always `false`. Empty-list parameters drop every row —
validate on the host side if that's a likely accident.

## Arithmetic in WHERE

Any expression that produces a boolean is allowed.

<QueryCodeBlock code={String.raw`MATCH (n) WHERE n.age + 5 > 30                 RETURN n;
MATCH (n) WHERE n.price * n.quantity > 1000    RETURN n;
MATCH (n) WHERE (n.end - n.start).seconds > 60 RETURN n`} />

## Cross-variable comparison

Predicates can reference multiple bindings from the `MATCH`:

<QueryCodeBlock code={String.raw`MATCH (a:User)-[:FOLLOWS]->(b:User)
WHERE a.age > b.age
RETURN a.name AS older, b.name AS younger`} />

<QueryCodeBlock code={String.raw`MATCH (a:User)-[:FOLLOWS]->(b)
WHERE a.country = b.country
RETURN a, b`} />

## Pattern existence

Use `EXISTS { pattern }` to filter rows by whether a pattern matches —
without adding extra rows to the output.

<QueryCodeBlock code={String.raw`MATCH (u:User)
WHERE EXISTS { (u)-[:FOLLOWS]->() }
RETURN u`} />

<QueryCodeBlock code={String.raw`// Users who have never posted
MATCH (u:User)
WHERE NOT EXISTS { (u)-[:WROTE]->(:Post) }
RETURN u.name`} />

This is the anti-join pattern — cheaper than
`OPTIONAL MATCH … WHERE other IS NULL` when you don't need the optional
result.

## List predicates

Ask a question about the elements of a list. Covered fully in
[List Functions → Predicates](../functions/list#predicates-in-where).

<QueryCodeBlock code={String.raw`MATCH (n) WHERE all(x IN n.scores WHERE x > 0)      RETURN n;
MATCH (n) WHERE any(x IN n.tags   WHERE x = 'VIP')  RETURN n;
MATCH (n) WHERE none(x IN n.scores WHERE x < 0)     RETURN n;
MATCH (n) WHERE single(x IN n.scores WHERE x = 100) RETURN n`} />

## CASE in predicates

[`CASE`](./return-with#case-expressions) is an expression, so it
composes inside `WHERE` wherever you'd write a scalar. Useful when the
comparison value itself depends on a per-row condition:

<QueryCodeBlock code={String.raw`MATCH (p:Product)
WHERE CASE
        WHEN p.on_sale THEN p.sale_price
        ELSE                p.price
      END <= $budget
RETURN p`} />

Equivalent with [`coalesce`](../functions/overview#type-conversion-and-checking)
when you only need a "first non-null" fallback:

<QueryCodeBlock code={String.raw`MATCH (p:Product)
WHERE coalesce(p.sale_price, p.price) <= $budget
RETURN p`} />

See [`RETURN → CASE expressions`](./return-with#case-expressions) for
the full syntax.

## Common patterns

### Safe prefix search

<QueryCodeBlock code={String.raw`MATCH (u:User)
WHERE string.lower(u.name) STARTS WITH string.lower($query)
RETURN u
ORDER BY u.name
LIMIT 20`} />

### Tag filtering

<QueryCodeBlock code={String.raw`MATCH (p:Product)
WHERE any(t IN p.tags WHERE t IN $tags)
RETURN p`} />

### Date range

<QueryCodeBlock code={String.raw`MATCH (e:Event)
WHERE e.at >= '2024-01-01'::DATE AND e.at < '2025-01-01'::DATE
RETURN e
ORDER BY e.at`} />

See [Temporal Functions](../functions/temporal) for cast-based
temporal construction and arithmetic.

### "Has at least one of each"

<QueryCodeBlock code={String.raw`MATCH (u:User)
WHERE EXISTS { (u)-[:OWNS]->(:Repo) }
  AND EXISTS { (u)-[:WROTE]->(:Post) }
RETURN u`} />

### "Has none of these"

<QueryCodeBlock code={String.raw`MATCH (p:Post)
WHERE none(t IN ['spam', 'nsfw', 'flagged']
           WHERE EXISTS { (p)-[:TAGGED]->(:Tag {name: t}) })
RETURN p`} />

### Chained optional predicates

Break a complex predicate into `WITH` stages for readability. Each
stage only sees what it needs:

<QueryCodeBlock code={String.raw`MATCH (u:User)
WITH u, coalesce(u.score, 0) AS s
WHERE s >= 50
WITH u, s, (u.last_seen >= temporal.now() - 'P30D'::DURATION) AS recent
WHERE recent
RETURN u.handle, s`} />

Same meaning as one giant `WHERE`, but each stage is narrower and
easier to trace.

### Default a missing value

Use [`coalesce`](../functions/overview#type-conversion-and-checking) to
substitute a fallback:

<QueryCodeBlock code={String.raw`MATCH (p:Person)
WHERE coalesce(p.score, 0) >= 50
RETURN p`} />

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

<QueryCodeBlock code={String.raw`// If n.a is null, the whole expression is null → row dropped
MATCH (n) WHERE n.a = 1 AND n.b > 2 RETURN n`} />

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
