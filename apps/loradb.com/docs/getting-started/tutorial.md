---
title: A Ten-Minute Tour of LoraDB
sidebar_label: Tutorial
description: A ten-minute guided tour of LoraDB in Cypher — create nodes and relationships, match patterns, filter, aggregate, walk paths, and use CASE — to learn the engine by doing.
---

# A Ten-Minute Tour of LoraDB

A guided walkthrough. Run each block in order and you'll end up with
a populated graph, the Cypher basics that shape it, and the
aggregation patterns you'll reach for most often.

> Already installed LoraDB? Good — use whichever binding you picked.
> The queries are the same in every language. If you haven't, start
> with [**Installation**](./installation).

## What you'll learn

| Step | Topic |
|---|---|
| [1](#step-1--create-some-data) | Create nodes and relationships |
| [2](#step-2--find-something) | MATCH patterns |
| [3](#step-3--filter) | WHERE, case, string matching |
| [4](#step-4--project-and-shape-results) | RETURN, aliases, map projection |
| [5](#step-5--count-and-aggregate) | count, implicit group-by |
| [6](#step-6--walk-multi-step-patterns) | Multi-hop and variable-length patterns |
| [7](#step-7--update-and-delete) | SET, MERGE, DETACH DELETE |
| [8](#step-8--conditional-values-bonus) | CASE expressions |
| [Next](#where-to-go-next) | Where to go from here |

## Step 1 — Create some data

We'll model a tiny network: a handful of people and who follows whom.

```cypher
CREATE (ada:Person   {name: 'Ada',   born: 1815})
CREATE (grace:Person {name: 'Grace', born: 1906})
CREATE (alan:Person  {name: 'Alan',  born: 1912})
CREATE (linus:Person {name: 'Linus', born: 1969})
```

Four `Person` [nodes](../concepts/nodes), each with `name` and `born`
[properties](../concepts/properties). `Person` is a **label** — a tag
you can use later to filter [`MATCH`](../queries/match) queries. The
variables (`ada`, `grace`, …) only live for the duration of this query.

Now wire them up:

```cypher
MATCH (ada:Person   {name: 'Ada'}),
      (grace:Person {name: 'Grace'}),
      (alan:Person  {name: 'Alan'}),
      (linus:Person {name: 'Linus'})
CREATE (grace)-[:FOLLOWS]->(ada),
       (alan)-[:FOLLOWS]->(ada),
       (linus)-[:FOLLOWS]->(alan),
       (linus)-[:FOLLOWS]->(grace)
```

We re-bind the nodes by `name` and [`CREATE`](../queries/create) four
directed `FOLLOWS` [relationships](../concepts/relationships). A
relationship is always between two nodes, always has a single **type**,
always has a direction, and can carry its own properties.

> **Why this matters.** In a relational schema you'd model this with
> a `follows` join table; in LoraDB the relationship is a first-class
> value you can pattern-match directly.

## Step 2 — Find something

[`MATCH`](../queries/match) describes a shape you want to find in the
graph. The simplest possible shape: "every Person":

```cypher
MATCH (p:Person)
RETURN p.name
```

You'll see four rows back — Ada, Grace, Alan, Linus.

Now a shape with a relationship:

```cypher
MATCH (follower:Person)-[:FOLLOWS]->(leader:Person)
RETURN follower.name, leader.name
```

This returns one row per edge — four rows:

| follower | leader |
|---|---|
| Grace | Ada |
| Alan | Ada |
| Linus | Alan |
| Linus | Grace |

The pattern reads left-to-right: "a `:Person` named `follower` who has
an outgoing `:FOLLOWS` relationship to another `:Person` named
`leader`." `MATCH` returns **every assignment** of variables to graph
entities that makes the pattern true.

## Step 3 — Filter

[`WHERE`](../queries/where) narrows the rows. It runs after `MATCH`
and can reference anything the match bound.

```cypher
MATCH (p:Person)
WHERE p.born >= 1900
RETURN p.name, p.born
```

Three rows back: Grace (1906), Alan (1912), Linus (1969). `WHERE` is
also where most null-safe checks and
[string matching](../functions/string#string-operators-in-where) live:

```cypher
MATCH (p:Person)
WHERE p.name STARTS WITH 'A'
RETURN p.name
```

Two rows: Ada, Alan. Run through
[`toLower`](../functions/string#tolower--toupper) if you want
case-insensitive matching:

```cypher
MATCH (p:Person)
WHERE toLower(p.name) CONTAINS 'a'
RETURN p.name
```

### Range filters

```cypher
MATCH (p:Person)
WHERE p.born >= 1900 AND p.born < 1950
RETURN p.name, p.born
```

LoraDB has no `BETWEEN` keyword — use explicit `>=` / `<=`. See
[Limitations](../limitations#operators-and-expressions).

## Step 4 — Project and shape results

[`RETURN`](../queries/return-with) can rename with `AS`, compute
expressions, and pick subsets via
[**map projection**](../data-types/lists-and-maps#map-projection):

```cypher
MATCH (p:Person)
RETURN p.name AS name,
       2026 - p.born AS approx_age,
       p {.name, .born} AS record
```

One row per person; each row has three columns. `p {.name, .born}`
builds a [map](../data-types/lists-and-maps#maps) from the node's
properties — useful for shaping results when your consumer only needs
a subset.

## Step 5 — Count and aggregate

[Aggregates](../queries/aggregation) collapse rows. The simplest is
[`count(*)`](../functions/aggregation#count):

```cypher
MATCH (p:Person)
RETURN count(*) AS total
```

One row, one column, value `4`. More interestingly: how many people
does each person follow?

```cypher
MATCH (follower:Person)-[:FOLLOWS]->(leader:Person)
RETURN follower.name AS follower,
       count(leader) AS following
ORDER BY following DESC
```

| follower | following |
|---|---|
| Linus | 2 |
| Grace | 1 |
| Alan | 1 |

> **Why this works.** `follower.name` is non-aggregated, so it becomes
> an implicit **group key**. One row per group. `count(leader)` is the
> size of each group.

And the reverse — how many followers does each person have?

```cypher
MATCH (follower:Person)-[:FOLLOWS]->(leader:Person)
RETURN leader.name AS person,
       count(follower) AS followers
ORDER BY followers DESC
```

### Filter after aggregating (HAVING-style)

Cypher has no `HAVING`. Use
[`WITH` + `WHERE`](../queries/return-with#having-style-filtering-with):

```cypher
MATCH (follower:Person)-[:FOLLOWS]->(leader:Person)
WITH leader.name AS person, count(follower) AS followers
WHERE followers >= 2
RETURN person, followers
```

## Step 6 — Walk multi-step patterns

Cypher really earns its keep on multi-hop patterns. Who follows someone
who follows Ada?

```cypher
MATCH (n:Person)-[:FOLLOWS]->(mid:Person)-[:FOLLOWS]->(ada:Person {name: 'Ada'})
RETURN n.name AS two_hops_away
```

One row: Linus, because Linus → Alan → Ada (and Linus → Grace → Ada).

You can bind the whole [path](../queries/paths) and read its length:

```cypher
MATCH p = (linus:Person {name: 'Linus'})-[:FOLLOWS*1..3]->(ada:Person {name: 'Ada'})
RETURN length(p), nodes(p)
```

`*1..3` means "between 1 and 3 hops along `:FOLLOWS` edges" — see
[variable-length patterns](../queries/paths#variable-length-relationships).
`length(p)` is the hop count; `nodes(p)` is the list of nodes on that
path.

### Shortest path

```cypher
MATCH p = shortestPath(
  (linus:Person {name: 'Linus'})-[:FOLLOWS*]->(ada:Person {name: 'Ada'})
)
RETURN length(p) AS hops, [n IN nodes(p) | n.name] AS via
```

## Step 7 — Update and delete

[`SET`](../queries/set-delete#set--properties) changes properties or
adds labels on existing nodes:

```cypher
MATCH (ada:Person {name: 'Ada'})
SET ada.field = 'Mathematics'
RETURN ada
```

```cypher
MATCH (ada:Person {name: 'Ada'})
SET ada:Pioneer
RETURN labels(ada)
-- ['Person', 'Pioneer']
```

[`MERGE`](../queries/unwind-merge#merge) is "match, or create if
missing":

```cypher
MERGE (p:Person {name: 'Ada'})
  ON MATCH  SET p.updated = timestamp()
  ON CREATE SET p.created = timestamp()
RETURN p.name, p.updated, p.created
```

And [`DETACH DELETE`](../queries/set-delete#detach-delete) removes a
node and all its relationships:

```cypher
MATCH (alan:Person {name: 'Alan'})
DETACH DELETE alan
```

## Step 8 — Conditional values (bonus)

[`CASE`](../queries/return-with#case-expressions) is LoraDB's
ternary. Two forms — match a value, or boolean-per-branch:

```cypher
MATCH (p:Person)
RETURN p.name,
       CASE
         WHEN p.born < 1900  THEN '19th century'
         WHEN p.born < 2000  THEN '20th century'
         ELSE                     '21st century'
       END AS era
```

Works anywhere a value is allowed — `RETURN`, `WITH`, `SET`,
`ORDER BY`, inside function arguments.

## Where to go next

You've now touched every common clause: `CREATE`, `MATCH`, `WHERE`,
`RETURN`, `ORDER BY`, `count`, `MERGE`, `SET`, `DETACH DELETE`, and
`CASE`. From here:

- **[Query reference](../queries)** — clause-by-clause details.
- **[Query examples](../queries/examples)** — copy-paste patterns for
  common shapes.
- **[Aggregation](../queries/aggregation)** — grouping, `collect`,
  percentiles, HAVING-style filtering.
- **[Paths](../queries/paths)** — variable-length and shortest paths.
- **[Functions](../functions/overview)** — string, math, list, temporal,
  spatial.
- **[Data Types](../data-types/overview)** — the complete value catalogue.
- **[Limitations](../limitations)** — what's intentionally not supported today.

## See also

- [**Graph Model**](../concepts/graph-model) — the underlying data model.
- [**Installation**](./installation) — pick your platform.
- [**Troubleshooting**](../troubleshooting) — common errors and fixes.
