---
title: UNWIND and MERGE — Iterating and Upserting
sidebar_label: UNWIND / MERGE
description: The UNWIND and MERGE clauses in LoraDB — UNWIND turns a list into rows for batch processing, MERGE finds a pattern or creates it, with ON MATCH and ON CREATE upsert semantics.
---

# UNWIND and MERGE — Iterating and Upserting

Two clauses that don't fit neatly into read or write:

- [`UNWIND`](#unwind) turns a list into rows, one per element.
- [`MERGE`](#merge) finds a pattern in the graph, or creates it
  if missing (the Cypher upsert).

## Overview

| Goal | Clause |
|---|---|
| Turn a list into rows | [<CypherCode code="UNWIND" />](#unwind) |
| Bulk-load from a parameter | [<CypherCode code="UNWIND $rows AS row CREATE …" />](#bulk-load-from-parameter) |
| Upsert a node | [<CypherCode code="MERGE (n:L {key: value})" />](#basic-merge) |
| Different side-effect on insert vs update | [<CypherCode code="ON CREATE" /> / <CypherCode code="ON MATCH" />](#on-match--on-create) |
| Ensure a relationship exists | [<CypherCode code="MERGE (a)-[:R]->(b)" />](#relationship-merge) |

## UNWIND

`UNWIND` takes a [list](../data-types/lists-and-maps#lists) and emits
one row per element.

```cypher
UNWIND [1, 2, 3] AS n RETURN n
-- 1, 2, 3

UNWIND range(1, 5) AS n RETURN n
-- 1, 2, 3, 4, 5

UNWIND [] AS n RETURN n
-- zero rows
```

### Unwind a list of maps

Each element becomes a row; the variable holds the map.

```cypher
UNWIND [
  {name: 'Ada',   born: 1815},
  {name: 'Grace', born: 1906}
] AS row
RETURN row.name, row.born
```

### Bulk load from parameter

The idiomatic large-ingest pattern:

```cypher
UNWIND $people AS p
CREATE (:Person {name: p.name, born: p.born})
```

Where `$people` is a list of maps bound from the host language. One
parse, one execution plan, N inserts.

### Bulk relationships

Resolve endpoints per row with [`MATCH`](./match):

```cypher
UNWIND $edges AS e
MATCH (a:User {id: e.from}), (b:User {id: e.to})
CREATE (a)-[:FOLLOWS {since: e.since}]->(b)
```

### Deduplicate a list

Combine with [`collect(DISTINCT …)`](../queries/aggregation#collect):

```cypher
UNWIND [1, 2, 2, 3, 3, 3] AS x
RETURN collect(DISTINCT x)
-- [1, 2, 3]
```

### Chain UNWINDs

```cypher
UNWIND $groups AS g
UNWIND g.members AS m
CREATE (:Member {group: g.name, name: m})
```

### Unwind after aggregation

```cypher
MATCH (p:Person)-[:KNOWS]->(f)
WITH p, collect(f.name) AS friends
UNWIND friends AS friend
RETURN p.name, friend
```

Re-expands an aggregated list back into rows — useful when you want to
post-process each element after collecting.

### Empty list

`UNWIND []` emits **zero rows**. Any downstream clause runs zero times
— an empty list short-circuits the pipeline:

```cypher
UNWIND [] AS x
CREATE (:Should_Not_Exist)
-- no-op, no nodes created
```

### Null vs empty

`UNWIND null` emits zero rows (same as empty). This is easy to miss:

```cypher
UNWIND $maybe_list AS x    -- $maybe_list not bound → null → 0 rows
```

Pass an explicit `[]` rather than `null` when you want to express
"nothing to do" from the host.

## MERGE

`MERGE` finds a pattern in the graph. If a match exists, it's bound
to the variables. If not, the pattern is created — **exactly** as
written, labels and properties and all.

```cypher
MERGE (n:User {id: 1001}) RETURN n
```

Running that query twice produces one `:User {id: 1001}`, not two.
Running the equivalent [`CREATE`](./create) twice produces two distinct
nodes.

### Basic merge

Shape the pattern around the fields that uniquely identify the entity.

```cypher
MERGE (u:User {id: $id}) RETURN u
```

### ON MATCH / ON CREATE

Run different side-effects depending on whether the match existed or
had to be created.

```cypher
MERGE (n:User {id: 1002})
  ON MATCH  SET n.updated = timestamp()
  ON CREATE SET n.created = timestamp()
RETURN n
```

Both clauses are optional. You can have neither, one, or both.

### Merge + unconditional SET

A very common upsert pattern: `MERGE` on the unique key, then `SET` the
fields that always change:

```cypher
MERGE (u:User {id: $id})
  ON CREATE SET u.created = timestamp()
  SET u.name = $name, u.updated = timestamp()
RETURN u
```

The trailing `SET` runs on both `ON MATCH` and `ON CREATE` branches.

### Relationship merge

```cypher
MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'})
MERGE (a)-[r:FOLLOWS]->(b)
RETURN r
```

If an `(a)-[:FOLLOWS]->(b)` edge between those exact two nodes already
exists, it is reused. Otherwise a new one is created.

### Merge the full pattern

`MERGE` matches on the **whole pattern**. Properties inside the node
pattern participate in the match.

```cypher
-- Matches only the exact pattern
MERGE (u:User {id: 1, status: 'active'})
```

If a `:User {id: 1}` exists with no `status` property, this `MERGE`
**won't** match — it'll create a second node.

### Pattern caveats

- Keep the identifying properties inside the `MERGE` pattern.
- Set everything else with a trailing [`SET`](./set-delete#set--properties).

```cypher
-- Bad — 'updated_at' is part of the match key
MERGE (u:User {id: $id, updated_at: datetime()})

-- Good — identity inside, payload via SET
MERGE (u:User {id: $id})
  SET u.updated_at = datetime()
RETURN u
```

### Merge missing endpoints

If the relationship endpoints aren't bound yet, `MERGE` will create them
too — usually not what you want.

```cypher
-- Risky: creates Alice and Bob if either is missing
MERGE (a:User {name: 'Alice'})-[:FOLLOWS]->(b:User {name: 'Bob'})
```

Prefer a safer two-step: match both nodes, then merge the edge.

```cypher
MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'})
MERGE (a)-[:FOLLOWS]->(b)
```

## Common patterns

### Idempotent bulk upsert

```cypher
UNWIND $rows AS row
MERGE (u:User {id: row.id})
  ON CREATE SET u.created = timestamp()
  SET u += row.fields, u.updated = timestamp()
```

Every row becomes a "find-or-create then update" pass.

### Tag-or-create relationship

```cypher
UNWIND $tags AS t
MERGE (tag:Tag {name: t})
WITH tag
MATCH (p:Post {id: $post_id})
MERGE (p)-[:TAGGED]->(tag)
```

### Counter (monotonic increment)

```cypher
MERGE (c:Counter {name: 'views'})
  ON CREATE SET c.value = 1
  ON MATCH  SET c.value = c.value + 1
RETURN c.value
```

### "Ensure this edge"

```cypher
MATCH (u:User {id: $user}), (r:Role {name: $role})
MERGE (u)-[:HAS_ROLE]->(r)
```

Run it twenty times; at most one edge exists between that user and that role.

### Histogram from a parameter list

```cypher
UNWIND $readings AS v
WITH (v / 10) * 10 AS bucket, count(*) AS n
RETURN bucket, n
ORDER BY bucket
```

No graph data involved — the pipeline starts from a host-supplied
list. Useful for analytics queries that use LoraDB as a compute
environment.

### Unwind a list, filter, aggregate

```cypher
UNWIND $scores AS s
WITH s WHERE s IS NOT NULL AND s > 0
RETURN avg(s) AS mean, min(s) AS worst, max(s) AS best, count(*) AS n
```

### Upsert relationship with payload on first-sight

```cypher
UNWIND $edges AS e
MATCH (a:User {id: e.from}), (b:User {id: e.to})
MERGE (a)-[r:FOLLOWS]->(b)
  ON CREATE SET r.since = coalesce(e.since, timestamp())
  SET r.last_activity = timestamp()
```

### Conditional MERGE via CASE

```cypher
UNWIND $rows AS row
MERGE (u:User {id: row.id})
SET u.tier = CASE
               WHEN row.amount >= 1000 THEN 'platinum'
               WHEN row.amount >=  100 THEN 'gold'
               ELSE                         coalesce(u.tier, 'bronze')
             END
```

The trailing `SET` runs on both match and create. See
[`CASE`](./return-with#case-expressions).

## Edge cases

### Whitespace in pattern vs data

`MERGE (:User {name: 'Alice'})` will not match `:User {name: 'Alice '}`.
String equality is exact — trim on the host if inputs are dirty.

### MERGE on multi-label pattern

Labels in the `MERGE` pattern are also part of the match key:

```cypher
-- Won't match a plain :User {id: 1}
MERGE (u:User:Admin {id: 1})
```

### UNWIND + MERGE race

There is no concurrent write safety concern because LoraDB runs queries
serially — see
[Queries → Execution model](./#execution-model).

### Empty parameters

`UNWIND $rows AS row MERGE (:User {id: row.id})` is a no-op when
`$rows = []`. The `MERGE` doesn't run, so no accidental writes from a
stray empty list.

## See also

- [**CREATE**](./create) — write-only alternative.
- [**SET / REMOVE / DELETE**](./set-delete) — mutations applied after `MERGE`.
- [**MATCH**](./match) — look up endpoints before `MERGE (a)-[…]->(b)`.
- [**Parameters**](./parameters) — bind `$rows`, `$patch`, …
- [**Lists & Maps**](../data-types/lists-and-maps) — list input for `UNWIND`.
- [**Aggregation → collect**](./aggregation#collect) — produce lists you can later unwind.
