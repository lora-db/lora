---
title: Relationships Between Nodes
sidebar_label: Relationships
description: How LoraDB represents relationships — typed, directed edges between nodes with their own properties — and how to create, traverse, and constrain them in Cypher.
---

# Relationships Between Nodes

A **relationship** is a directed, typed edge between two nodes. Like
nodes, it can carry properties. Every relationship has:

- exactly one **type** (e.g. `KNOWS`, `ACTED_IN`);
- exactly one **direction** — start node `→` end node;
- any number of [**properties**](./properties).

## Quick reference

| You want… | Pattern |
|---|---|
| Create with direction | <CypherCode code="(a)-[:T]->(b)" /> |
| Reversed | <CypherCode code="(a)<-[:T]-(b)" /> |
| Match either direction | <CypherCode code="(a)-[:T]-(b)" /> (MATCH only) |
| Anonymous | <CypherCode code="(a)-[:T]->(b)" /> (no rel variable) |
| Bind the rel | <CypherCode code="(a)-[r:T]->(b)" /> |
| With properties | <CypherCode code="(a)-[:T {k: v}]->(b)" /> |
| Any type | <CypherCode code="(a)-[r]->(b)" /> (MATCH only) |
| Multiple types | `(a)-[:T1|T2]->(b)` (MATCH only — pipe is the "or" operator) |

## Create

Endpoints must exist first — either bound by an earlier `MATCH` or
created in the same clause.

### Match then create

```cypher
MATCH (a:Person {name: 'Ada'}), (b:Person {name: 'Grace'})
CREATE (a)-[:KNOWS {since: 1843}]->(b)
```

### Inline in one CREATE

```cypher
CREATE (a:Person {name: 'Ada'})-[:INFLUENCED]->(b:Person {name: 'Grace'})
```

### Chained patterns

A single `CREATE` can chain several edges through the same variables:

```cypher
CREATE
  (ada:Person {name: 'Ada'}),
  (grace:Person {name: 'Grace'}),
  (alan:Person {name: 'Alan'}),
  (ada)-[:INFLUENCED]->(grace),
  (grace)-[:INFLUENCED]->(alan)
```

### Idempotent create — MERGE

`CREATE` doesn't deduplicate. [`MERGE`](../queries/unwind-merge#merge)
does:

```cypher
MATCH (a:Person {name: 'Ada'}), (b:Person {name: 'Grace'})
MERGE (a)-[:KNOWS]->(b)
```

Running this twice yields exactly one `KNOWS` edge.

### Rules on CREATE

- Direction is **mandatory** — `(a)-[:T]-(b)` in a `CREATE` is an error.
- Type is **mandatory** — `(a)-[]->(b)` is an error.
- Both endpoints must be in scope.

See [Troubleshooting → Parse errors](../troubleshooting#parse-errors).

## Match

Relationships can be matched with or without a type, with or without a
direction.

```cypher
MATCH (a)-[r:KNOWS]->(b)        RETURN a, r, b   -- outgoing
MATCH (a)<-[r:KNOWS]-(b)        RETURN a, r, b   -- incoming
MATCH (a)-[r:KNOWS]-(b)         RETURN a, r, b   -- either direction
MATCH (a)-[r]->(b)              RETURN a, r, b   -- any type
MATCH (a)-[r:FOLLOWS|KNOWS]->(b) RETURN a, r, b  -- multiple types
MATCH (a)-[:FOLLOWS {since: 2020}]->(b) RETURN a, b
```

### Projection

```cypher
MATCH (a)-[r:FOLLOWS]->(b)
RETURN type(r), r.since, a.name, b.name
```

[`type(r)`](../functions/overview#entity-introspection) returns the
relationship's type as a string.

### Variable-length

```cypher
MATCH (a)-[:FOLLOWS*1..3]->(b) RETURN b
```

See [Paths → variable-length](../queries/paths#variable-length-relationships).

## Mutate or delete

```cypher
MATCH (a)-[r:KNOWS]->(b) SET r.since = 2025 RETURN r
MATCH (a)-[r:KNOWS]->(b) DELETE r
```

Deleting a node that has relationships requires
[`DETACH DELETE`](../queries/set-delete#detach-delete):

```cypher
MATCH (n:User {id: $id}) DETACH DELETE n
```

## Properties on relationships

Exactly the same shape as on nodes:

```cypher
MATCH (a)-[r:FOLLOWS]->(b)
SET r.since = 2025, r.visibility = 'public'

MATCH (a)-[r:FOLLOWS]->(b)
SET r += {muted: true}

MATCH (a)-[r:FOLLOWS]->(b)
REMOVE r.muted
```

Access and project them like any other
[property](./properties):

```cypher
MATCH (a)-[r:FOLLOWS]->(b)
RETURN a.name, b.name, r.since
```

## Direction conventions

LoraDB's direction is semantic — use it to reflect the real-world
direction of the relationship.

| Relationship | Direction |
|---|---|
| `FOLLOWS`, `KNOWS`, `LIKES` | `follower -> followee` |
| `WROTE`, `AUTHORED` | `author -> work` |
| `CONTAINS`, `OWNS` | `container -> item` |
| `IN`, `PART_OF` | `child -> parent` |

When in doubt, pick one and document it. Queries can always match
either direction with `(a)-[:T]-(b)` if you later need symmetry.

## Common patterns

### "Mutual" follows

```cypher
MATCH (a)-[:FOLLOWS]->(b)-[:FOLLOWS]->(a)
WHERE id(a) < id(b)
RETURN a.name, b.name
```

### Count by type

```cypher
MATCH (a)-[r]->(b)
RETURN type(r), count(*) ORDER BY count(*) DESC
```

### Per-node degree

```cypher
MATCH (n)-[r]->()
RETURN n.name, count(r) AS out_degree
ORDER BY out_degree DESC
```

```cypher
MATCH (n)<-[r]-()
RETURN n.name, count(r) AS in_degree
ORDER BY in_degree DESC
```

### Self-loops

Rare but sometimes the right modelling choice:

```cypher
MATCH (n)
WHERE (n)-[:RECURSES_INTO]->(n)
RETURN n
```

### Ensure this edge (once)

```cypher
MATCH (a:User {id: $u}), (r:Role {name: $role})
MERGE (a)-[:HAS_ROLE]->(r)
```

Repeatable without creating duplicates.

### Remove every edge of a type

```cypher
MATCH ()-[r:OBSOLETE_LINK]->()
DELETE r
```

### Oldest / newest edge per pair

Relationships carry their own properties and can be ranked like
nodes:

```cypher
MATCH (a:User)-[r:MESSAGED]->(b:User)
RETURN a.handle, b.handle,
       min(r.at) AS first,
       max(r.at) AS last,
       count(r)  AS total
ORDER BY total DESC
LIMIT 20
```

### "Any edge at all" between two nodes

```cypher
MATCH (a:User {id: $a}), (b:User {id: $b})
RETURN EXISTS { (a)-[]-(b) } AS connected
```

### Count in-degree vs out-degree in one pass

```cypher
MATCH (n:User)
OPTIONAL MATCH (n)-[out]->()
WITH n, count(out) AS out_deg
OPTIONAL MATCH (n)<-[in]-()
RETURN n.handle, out_deg, count(in) AS in_deg
ORDER BY in_deg + out_deg DESC
LIMIT 20
```

Note the two `OPTIONAL MATCH` stages — any node with zero edges in
either direction still appears.

### Convert one relationship type to another

Useful during a schema-change migration.

```cypher
MATCH (a)-[r:OLD_TYPE]->(b)
CREATE (a)-[r2:NEW_TYPE]->(b)
SET    r2 = properties(r)
DELETE r
```

## Edge cases

### Zero-or-more traversals

`[:R*0..]` includes the starting node itself as a zero-hop match — see
[Paths → zero-hop](../queries/paths#zero-hop-semantics).

### Matching either direction on CREATE

```cypher
CREATE (a)-[:T]-(b)   -- error
```

Direction is required on writes. Decide which way makes sense.

### Deleting a relationship that's been matched twice

A query like `MATCH (a)-[r]->(b)-[r]->(c)` won't compile — `r` must be
unique. Use two distinct variables:

```cypher
MATCH (a)-[r1:T]->(b)-[r2:T]->(c)
```

### Multiple parallel edges

Nothing prevents two `KNOWS` edges between the same pair:

```cypher
CREATE (a)-[:KNOWS]->(b)
CREATE (a)-[:KNOWS]->(b)
-- now there are two :KNOWS edges
```

Use `MERGE (a)-[:KNOWS]->(b)` for dedup. For modelling-time
uniqueness (one edge only), enforce in application code.

## Notes

- Relationship types are **case-sensitive**, conventionally `UPPER_SNAKE`.
- A relationship has **one** type — not a list.
- Relationships cannot be dangling — src and dst must both exist.
- Deleting a relationship does not delete its endpoints.

## See also

- [**Graph Model**](./graph-model) — where relationships fit.
- [**Nodes**](./nodes) — endpoints.
- [**Properties**](./properties) — key/value payload.
- [**CREATE**](../queries/create), [**MATCH**](../queries/match),
  [**MERGE**](../queries/unwind-merge#merge),
  [**SET / DELETE**](../queries/set-delete) — clause syntax.
- [**Paths**](../queries/paths) — variable-length and shortest-path traversals.
- [**Functions → Entity introspection**](../functions/overview#entity-introspection)
  — `id`, `type`, `labels`, `keys`, `properties`.
