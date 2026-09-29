---
title: GraphQL Translation Rules
sidebar_label: Translation rules
description: How @loradb/lora-graphql compiles GraphQL operations into LoraDB Cypher, with the statement shapes it chooses and the measurements behind them.
unlisted: true
---

# Translation rules

Every read root field compiles to one Cypher statement. Every mutation
compiles to a short sequence of statements in one interactive
transaction. This page shows the shapes the compiler chooses and why. You
do not need it to use the library, but it helps when you read a plan, a
`check` finding, or the `onStatement` log.

## A read, end to end

```graphql
{
  festivals(
    where: { capacity: { gte: 1000 }, followers: { some: { key: { eq: "u1" } } } }
    sort: [{ name: ASC }]
    limit: 10
  ) {
    key
    name
    genre {
      name
    }
    followers(limit: 3) {
      name
    }
  }
}
```

compiles to:

<CypherSnippet code={String.raw`MATCH (this:Festival)
WHERE this.capacity >= $p0 AND size([(this)<-[this_followers_rel:FOLLOWS]-(this_followers:User) WHERE this_followers.key = $p2 | 1]) > 0
WITH this ORDER BY this.name ASC LIMIT $p4
CALL {
  WITH this
  MATCH (this)<-[:FOLLOWS]-(this_followers1:User)
  WITH this_followers1 ORDER BY this_followers1.key ASC LIMIT $p3
  RETURN collect(this_followers1 { .name }) AS this_followers1_list
}
RETURN this { .key, .name, genre: head([(this)-[:IN_GENRE]->(this_genre:Genre) | this_genre { .name }]), followers: this_followers1_list } AS this`} />

Things to notice:

- Every value is a parameter (`$p0` to `$p4`). Labels, relationship types
  and property names come from the model, never from the request.
- The root is sorted and limited before anything nested is read, so nested
  lists run for at most 10 festivals.
- The nested list is a `CALL` subquery, which can sort and limit per
  parent. Without a requested sort, nested lists come in `@key` order.
- The single relationship `genre` is a pattern comprehension, with no
  `OPTIONAL MATCH`.

To see the statements for your own operations, pass `onStatement` to
`new LoraGraphQL`, or run `lora.explain(query, variables)`.

## The rules

Measured on LoraDB 0.15 over 20 000 festivals and 100 000 relationships
(`yarn bench` in the package):

| Rule | Why |
| --- | --- |
| Relationship filters as `size([... \| 1])`; aggregates of related values with `reduce` | No `OPTIONAL MATCH`; `EXISTS { }` does not parse; aggregates nest safely |
| `CALL { }` for nested lists, nested connections, `@cypher` and interface members | The only way to sort, limit and aggregate per parent |
| Ordered by an always-present string: `WHERE s >= ""` | The planner walks the index in order and stops at the limit: 0.03 ms instead of 7 ms |
| Mutation statements seek the key in their own `MATCH`, then expand | The plan no longer depends on the optimizer finding the seek: 0.06 ms per connect |
| A relationship filter naming a key starts from that node | 0.07 ms instead of 9.3 ms |
| Keyset predicates written out, led by `sortKey >= $v` on non-null keys | `[a, b] > $list` silently matches nothing; the lead bound gets a range scan |
| Lists sort by the requested fields only; connections add a unique tie-breaker | Two sort keys cannot stream from an index |
| Every value a parameter; every identifier from the model, escaped | No injection, stable statement text |
| Absent filters left out, never `($p IS NULL OR ...)` | Keeps the predicate visible to the planner |

### Sorted lists walk the index

A sort on a required string field adds a predicate that is always true
for present values:

<CypherSnippet code={String.raw`MATCH (this:Festival)
WHERE this.name >= ""
WITH this ORDER BY this.name ASC LIMIT $p0
RETURN this { .key, .name } AS this`} />

With a RANGE index on `name` (inferred from `@sortable`), the planner reads
the index in order and stops after `$p0` rows instead of sorting the
whole label.

### Relationship filters can start from the related node

When the node's own filters give nothing to seek on, and a relationship
filter names a related node by key (`some` or `single` with `eq` or `in`
on the key), the statement starts from that node and expands:

<CypherSnippet code={String.raw`MATCH (this_followers_anchor:User)
WHERE this_followers_anchor.key = $p2
MATCH (this_followers_anchor)-[:FOLLOWS]->(this:Festival)
WHERE size([(this)<-[this_followers_rel:FOLLOWS]-(this_followers:User) WHERE this_followers.key = $p1 | 1]) > 0
WITH DISTINCT this
WITH this ORDER BY this.key ASC LIMIT $p3
RETURN this { .key } AS this`} />

The original filter stays in the `WHERE`, so the result is the same as the
label scan; only the access path changes. `check` records the anchor as
the expected seek.

### Connections page by keyset

Connections sort by the requested fields and then by the `@key`, and
return the sort values as the cursor:

<CypherSnippet code={String.raw`MATCH (this:Festival)
WITH this ORDER BY this.capacity DESC, this.key ASC LIMIT $p0
RETURN this { .key } AS node, [this.capacity, this.key] AS __cursor`} />

The limit is one more than `first`, to compute `hasNextPage`. With `after`,
the keyset predicate is written out field by field rather than as a list
comparison, and never uses `SKIP`.

## Statement text and caching

Statement text depends only on the shape of the input: which filters are
present, which fields are selected, and the caller's claims, which are
folded in at compile time. Values are always parameters. Two requests
with the same shape produce the same text, so LoraDB's own plan cache
serves every repeat, and the library's compile cache skips translation
for repeated field nodes. See
[the smart layer](/docs/graphql/smart-layer#s3-compile-cache).

## Engine behaviours the compiler works around

Some rules exist because of current LoraDB behaviour. Each workaround goes
away when the engine fix lands:

| Behaviour | Workaround |
| --- | --- |
| An aggregate nested in a call (`head(collect(x))`) is not aggregated | `WITH collect(x) AS c RETURN head(c)`; `reduce` for per-parent aggregates |
| `max`, `sum` and `avg` over durations are wrong | Duration aggregates are folded with `reduce` |
| Integer division returns a float; negative list slices return `[]` | `toInteger(a / b)` for `Int` fields; `l[..size(l) - n]` |
| `COUNT { ... RETURN DISTINCT x }`, `EXISTS { }` and `UNION` inside `CALL` do not parse | `reduce` for distinct counts; comprehensions; per-member subqueries |
| `[a, b] > $list` matches nothing | Keyset predicates written out |
