---
title: CREATE — Writing Nodes and Relationships
sidebar_label: CREATE
description: The CREATE clause in LoraDB — always inserts new nodes and relationships, with property literals, multi-pattern writes, and the difference from MERGE's upsert semantics.
---

# CREATE — Writing Nodes and Relationships

`CREATE` writes new [nodes](../concepts/nodes) and
[relationships](../concepts/relationships) into the graph. Every pattern
element on the create side becomes a new entity — `CREATE` never
deduplicates. For create-if-not-exists semantics use
[`MERGE`](./unwind-merge#merge).

> A quick guided walkthrough lives in the
> [**Ten-Minute Tour → Create some data**](../getting-started/tutorial#step-1--create-some-data).

## Overview

| You want to… | Clause |
|---|---|
| Add a node | <CypherCode code="CREATE (n:Label {k: v})" /> |
| Add multiple nodes | <CypherCode code="CREATE (a:L), (b:L)" /> |
| Add an edge between existing nodes | <CypherCode code="MATCH (a), (b) CREATE (a)-[:R]->(b)" /> |
| Add nodes + edge in one shot | <CypherCode code="CREATE (a:L)-[:R]->(b:L)" /> |
| Bulk import from a list | [<CypherCode code="UNWIND $rows AS row CREATE (…)" />](./unwind-merge#unwind) |
| Upsert | [<CypherCode code="MERGE" />](./unwind-merge#merge) |

## Nodes

The simplest `CREATE` — one node, optional labels, optional property map.

```cypher
CREATE (n:User {name: 'Alice', age: 32}) RETURN n
CREATE (n:User:Admin {name: 'Bob'})       RETURN n
CREATE (n:TempOnly)                       -- no properties, no return
```

Labels are added to the node as-is. Property values follow the standard
[data-type rules](../data-types/overview).

### Multiple nodes in one query

Comma-separated patterns:

```cypher
CREATE
  (ada:Person   {name: 'Ada',   born: 1815}),
  (grace:Person {name: 'Grace', born: 1906}),
  (alan:Person  {name: 'Alan',  born: 1912})
RETURN ada, grace, alan
```

Variables (`ada`, `grace`, …) stay in scope for the rest of the query —
useful if you want to wire them up with a relationship immediately
afterwards.

### Typed property values

Properties accept every
[supported data type](../data-types/overview), including
[temporals](../data-types/temporal) and [points](../data-types/spatial):

```cypher
CREATE (c:City {
  name:       'Amsterdam',
  population: 918000,
  tags:       ['capital', 'port'],
  founded:    date('1275-10-27'),
  location:   point({latitude: 52.37, longitude: 4.89})
})
```

## Relationships

`CREATE` for a relationship requires both endpoints to exist — either
bound by an earlier [`MATCH`](./match), or created inline in the same
`CREATE`.

### Match-then-create

Look up both endpoints, then add the edge:

```cypher
MATCH (a:User {name: 'Alice'}), (b:User {name: 'Bob'})
CREATE (a)-[r:FOLLOWS {since: 2020}]->(b)
RETURN a, r, b
```

Relationships have their own
[property maps](../concepts/properties). `r.since = 2020` is stored on
the edge, not on the endpoints.

### Pattern-create

A single `CREATE` can produce both nodes and the relationship between
them:

```cypher
CREATE (a:User {name: 'Alice'})-[:FOLLOWS]->(b:User {name: 'Bob'})
RETURN a, b
```

### Multi-hop pattern-create

Chain freely:

```cypher
CREATE
  (ada:Person {name: 'Ada'}),
  (grace:Person {name: 'Grace'}),
  (alan:Person {name: 'Alan'}),
  (ada)-[:INFLUENCED]->(grace),
  (grace)-[:INFLUENCED]->(alan)
```

### Direction and type are mandatory

On `CREATE`:

- `(a)-[:T]-(b)` — **error** (no direction).
- `(a)-[]->(b)` — **error** (no type).
- `(a)-[:T]->(b)` — valid.
- `(a)<-[:T]-(b)` — valid (reversed).

See [Troubleshooting → Parse errors](../troubleshooting#parse-errors).

## Bulk create with UNWIND

The idiomatic bulk-load shape pairs `CREATE` with
[`UNWIND`](./unwind-merge#unwind) — one row per list element.

### Literal list

```cypher
UNWIND [
  {name: 'Ada',   born: 1815},
  {name: 'Grace', born: 1906},
  {name: 'Alan',  born: 1912}
] AS p
CREATE (:Person {name: p.name, born: p.born})
```

### Parameter list

Pass the list in from the host language:

```cypher
UNWIND $people AS p
CREATE (:Person {name: p.name, born: p.born})
```

Where `$people = [{name: 'Ada', born: 1815}, …]`. This is the
recommended way to load hundreds or thousands of rows in one query —
see each binding's parameters section
([Rust](../getting-started/rust#parameterised-query),
[Node](../getting-started/node#parameterised-query),
[Python](../getting-started/python#parameterised-query)).

### Bulk relationships

Pair an `UNWIND` with a [`MATCH`](./match) to wire up pre-existing nodes:

```cypher
UNWIND $edges AS e
MATCH (a:User {id: e.from}), (b:User {id: e.to})
CREATE (a)-[:FOLLOWS {since: e.since}]->(b)
```

## No uniqueness check

`CREATE` **never** deduplicates. Running the same `CREATE` twice
produces two distinct nodes with the same labels and properties.

```cypher
CREATE (:User {id: 1})
CREATE (:User {id: 1})
-- now there are two :User {id: 1} nodes
```

For create-if-not-exists semantics use [`MERGE`](./unwind-merge#merge):

```cypher
MERGE (u:User {id: 1}) RETURN u
```

Running that same query twice yields one node. See
[MERGE pattern caveats](./unwind-merge#pattern-caveats) for the rules on
what it matches.

## Returning what you created

`CREATE` can be followed by [`RETURN`](./return-with) to hand the new
entity back — essential when the host needs its internal ID.

```cypher
CREATE (u:User {email: $email})
RETURN id(u) AS id, u
```

`id()` is the [internal identity](../concepts/graph-model#identity).
Prefer your own stable property (`id`, `email`, …) for external
addressing.

## Common patterns

### Upsert with MERGE + SET

`CREATE` on its own can't express "insert or update". Combine
[`MERGE`](./unwind-merge#merge) with [`SET`](./set-delete#set--properties):

```cypher
MERGE (u:User {id: $id})
  ON CREATE SET u.created = timestamp()
  SET u.name = $name, u.updated = timestamp()
RETURN u
```

### Create + link

```cypher
MATCH (c:Category {slug: $cat})
CREATE (p:Product {
  name:  $name,
  price: $price
})-[:IN]->(c)
RETURN p
```

### Clone a node shape

```cypher
MATCH (src:Template {id: $src})
CREATE (dst:Template)
SET    dst = properties(src)
SET    dst.id = $new_id
RETURN dst
```

### Create from aggregated input

`CREATE` can consume rows produced by a preceding stage. This is how
you turn an aggregate into new nodes:

```cypher
MATCH (o:Order)
WITH o.region AS region, sum(o.amount) AS revenue
CREATE (:RegionStat {region: region, revenue: revenue})
```

One `:RegionStat` node per region.

### Mirror-write: create a node plus several relationships

```cypher
MATCH (u:User {id: $user_id}),
      (cat:Category {slug: $cat})
CREATE (p:Product {
  id:    $id,
  name:  $name,
  price: $price
})
CREATE (u)-[:OWNS]->(p)
CREATE (p)-[:IN]->(cat)
RETURN p
```

Each `CREATE` runs once per input row; since the `MATCH` produces one
row, this creates one product plus two edges in a single query.

### Create with conditional properties via CASE

```cypher
CREATE (u:User {
  id:       $id,
  email:    $email,
  tier:     CASE WHEN $paying THEN 'pro' ELSE 'free' END,
  created:  timestamp()
})
```

See [CASE expressions](./return-with#case-expressions).

## Edge cases

### `CREATE` over empty graph

Works unconditionally — there's no DDL step and no schema to validate
against. Labels and relationship types come into existence implicitly on
first use. See [Graph model → Schema-free](../concepts/graph-model#schema-free).

### `CREATE` with duplicate variable

Reusing a variable for different entities is an analysis error:

```cypher
CREATE (n:A), (n:B)    -- error: variable 'n' already bound
```

### `CREATE` with relationship to a non-existent node

Both endpoints must be in scope. A `CREATE (a)-[:R]->(b)` where `a` or
`b` isn't bound will fail analysis.

### Storage considerations

Every node is `O(1)` to create. Relationships are stored on both
endpoints, so creating one edge is `O(1)` too. There are **no property
indexes** (see [Limitations](../limitations)) — later `MATCH (n {p: v})`
lookups without a label are full scans.

## See also

- [**MATCH**](./match) — pattern matching.
- [**MERGE**](./unwind-merge#merge) — create-if-not-exists.
- [**UNWIND**](./unwind-merge#unwind) — bulk-load from a list.
- [**SET / REMOVE / DELETE**](./set-delete) — mutate after creation.
- [**Properties**](../concepts/properties) and
  [**Data Types**](../data-types/overview) — value shapes.
- [**Query Examples**](./examples) — end-to-end recipes.
