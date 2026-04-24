---
title: Nodes and Labels
sidebar_label: Nodes
description: How LoraDB represents nodes — vertices with zero or more labels, properties, and a stable internal id — and how to create, match, and label them with Cypher.
---

# Nodes and Labels

A **node** is a vertex in the graph. Every node has:

- Zero or more **labels** — tags that describe what kind of thing it
  is (`Person`, `Movie`).
- Zero or more **[properties](./properties)** — typed key/value pairs.
- A stable internal **id** — see
  [Graph Model → Identity](./graph-model#identity).

Each node is stored once. Multiple references to the same node via
different matches bind to the same identity.

## Create

```cypher
CREATE (:Person {name: 'Ada', born: 1815})     -- one label
CREATE (:Person:Admin {name: 'Bob'})           -- multiple labels
CREATE (:Temp)                                 -- no properties
CREATE ()                                      -- no labels, no properties
```

Even a fully-naked `()` is a valid node. Most real graphs give every
node at least one label — it's the primary way to scope queries.

### Bind a variable at creation

```cypher
CREATE (ada:Person {name: 'Ada'})
CREATE (ada)-[:WROTE]->(n:Note {text: 'Bernoulli numbers'})
RETURN ada, n
```

Variables (`ada`, `n`) stay in scope for the rest of the query.

## Match by label

Labels are the primary way to scope a query.

```cypher
MATCH (p:Person)         RETURN p                -- single label
MATCH (a:Person:Admin)   RETURN a                -- must have both
MATCH (n)                RETURN labels(n)        -- any node — all labels
```

### Match by label + property

```cypher
MATCH (u:User {email: $email})        RETURN u
MATCH (u:User {email: $email, active: true}) RETURN u
```

Inline maps are equality-only. For ranges, regex, `IN`, or null
checks, move the predicate into [`WHERE`](../queries/where):

```cypher
MATCH (u:User)
WHERE u.age BETWEEN 18 AND 65                     -- NOT supported
MATCH (u:User)
WHERE u.age >= 18 AND u.age <= 65                 -- idiomatic in LoraDB
RETURN u
```

## Labels

```cypher
MATCH (n:Person {name: 'Ada'}) SET    n:Pioneer   RETURN labels(n)
MATCH (n:Person {name: 'Ada'}) REMOVE n:Pioneer   RETURN labels(n)
```

### Multiple labels

A node can have any number of labels, including zero:

```cypher
MATCH (n:Person {name: 'Ada'}) SET n:Admin, n:Verified
```

### Inspect labels

```cypher
MATCH (n) RETURN labels(n), count(*)
ORDER BY count(*) DESC
```

One row per distinct label-set in the graph.

### Conventions

- **Case-sensitive** strings.
- Conventionally **PascalCase** (`Person`, not `person`).
- Use singular nouns (`User`, not `Users`).

See [Troubleshooting → Queries return empty results](../troubleshooting#queries-return-empty-results)
for the classic `:user` vs `:User` mistake.

## Properties on nodes

Any [supported data type](../data-types/overview):

```cypher
CREATE (c:City {
  name:       'Amsterdam',
  population: 918000,
  founded:    date('1275-10-27'),
  tags:       ['capital', 'port'],
  location:   point({latitude: 52.37, longitude: 4.89})
})
```

Read, patch, and remove with [`SET` / `REMOVE`](../queries/set-delete):

```cypher
MATCH (c:City {name: 'Amsterdam'}) RETURN c.population, c.tags

MATCH (c:City {name: 'Amsterdam'})
SET c.population = 920000, c.updated = timestamp()
RETURN c
```

See [Properties](./properties) for the full reference.

## Upsert

[`MERGE`](../queries/unwind-merge#merge) finds a node, or creates one.
Pair with [`ON MATCH` / `ON CREATE`](../queries/unwind-merge#on-match--on-create)
to run different side-effects per branch:

```cypher
MERGE (u:User {email: $email})
  ON CREATE SET u.created_at = timestamp()
  SET u.last_seen = timestamp()
RETURN u
```

## Delete

```cypher
-- Standalone node (no edges)
MATCH (n:Temp) DELETE n

-- Node + all edges
MATCH (n:User {id: $id}) DETACH DELETE n
```

See [`DETACH DELETE`](../queries/set-delete#detach-delete) for details.

## Common patterns

### Count by label

```cypher
MATCH (n) RETURN labels(n) AS labels, count(*) AS n
ORDER BY n DESC
```

### Ensure uniqueness at write time

LoraDB has no uniqueness constraints — enforce it yourself with
`MERGE`:

```cypher
MERGE (u:User {email: $email})
ON CREATE SET u.created_at = timestamp()
```

### Pattern-match on one label, filter on another

```cypher
MATCH (n:Person)
WHERE NOT n:Admin
RETURN n
```

### Sample a few of each

```cypher
MATCH (n)
WITH labels(n)[0] AS label, n
WITH label, collect(n)[..3] AS sample
RETURN label, sample
```

### Find nodes without a given relationship

Anti-pattern: "who doesn't…" — use
[`NOT EXISTS { … }`](../queries/where#pattern-existence):

```cypher
MATCH (u:User)
WHERE NOT EXISTS { (u)-[:WROTE]->(:Post) }
RETURN u.handle
```

### Bulk label migration

```cypher
MATCH (u:User)
WHERE u.role = 'admin' AND NOT u:Admin
SET u:Admin
```

### Move properties between nodes

```cypher
MATCH (src:Person {id: $src}), (dst:Person {id: $dst})
SET dst += properties(src)
REMOVE src:Person
SET src:Archived
SET src.archived_at = timestamp()
```

### Split one node into two

A modelling change where a property becomes its own entity — useful
when the value starts being reachable from multiple sides:

```cypher
MATCH (u:User) WHERE u.company IS NOT NULL
WITH u, u.company AS company
MERGE (c:Company {name: company})
CREATE (u)-[:WORKS_AT]->(c)
REMOVE u.company
```

## Edge cases

### Labelless nodes

Valid but rare. They don't show up in `MATCH (:Any_Label)` patterns
and are hard to find without the `id()` function.

### Many labels

Matching `(n:A:B:C)` requires **all** listed labels. If you want "any
of", `UNION` two matches or use `WHERE`:

```cypher
MATCH (n)
WHERE n:Person OR n:Robot
RETURN n
```

### Property-only match

```cypher
MATCH (n {external_id: $id}) RETURN n
```

This scans the entire node set — no label scoping. Always add a label
when you can.

### Identity vs equality

Two nodes with identical labels and properties are still distinct — they
have different internal ids. Use [`id()`](../functions/overview#entity-introspection)
to compare identity; use property equality for value-based matching.

## See also

- [**Graph Model**](./graph-model) — the model as a whole.
- [**Relationships**](./relationships) — edges between nodes.
- [**Properties**](./properties) — per-node key/value pairs.
- [**CREATE**](../queries/create), [**MATCH**](../queries/match),
  [**SET / REMOVE / DELETE**](../queries/set-delete) — clause syntax.
- [**MERGE**](../queries/unwind-merge#merge) — create-or-match.
- [**Troubleshooting**](../troubleshooting) — common match-failure causes.
