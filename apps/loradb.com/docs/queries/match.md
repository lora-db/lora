---
title: MATCH — Finding Patterns in the Graph
sidebar_label: MATCH
description: The MATCH clause in LoraDB — fixed and variable-length patterns, OPTIONAL MATCH, labels, directions, chained matches, and how it produces the row stream Cypher queries build on.
---

# MATCH — Finding Patterns in the Graph

`MATCH` describes a pattern to find in the graph. Every successful
match produces one row, with variables bound to concrete
[nodes](../concepts/nodes) or
[relationships](../concepts/relationships). A query can chain many
`MATCH` clauses in sequence — each reads the rows produced by the
previous one.

> New to pattern matching? Start with the
> [**Ten-Minute Tour → Find something**](../getting-started/tutorial#step-2--find-something),
> then come back for the full reference.

## Overview

| You want to… | Pattern |
|---|---|
| Match every node | <CypherCode code="MATCH (n)" /> |
| Match every node with a label | <CypherCode code="MATCH (n:Person)" /> |
| Match by label + properties | <CypherCode code="MATCH (n:Person {name: 'Ada'})" /> |
| Match a relationship | <CypherCode code="MATCH (a)-[:KNOWS]->(b)" /> |
| Match any type, any direction | <CypherCode code="MATCH (a)-[r]-(b)" /> |
| Left-join shape | [<CypherCode code="OPTIONAL MATCH" />](#optional-match) |
| Variable-length hops | [<CypherCode code="MATCH (a)-[:R*1..3]->(b)" />](./paths) |
| Bind the whole path | [<CypherCode code="MATCH p = (a)-[:R]->(b)" />](#path-binding) |

## Node patterns

Start with the simplest shape — a single node.

```cypher
MATCH (n) RETURN n
```

One row per node in the graph. Variables are local to the query — `n`
only exists between `MATCH` and `RETURN`.

### Filter by label

```cypher
MATCH (n:User)         RETURN n
MATCH (n:User:Admin)   RETURN n     -- must have BOTH labels
```

Multiple labels on the pattern narrow the match — the node must carry
every listed label. See [Nodes](../concepts/nodes) for the rules on
labels, case, and conventions.

### Inline property filter

Inline maps are a shorthand for equality-only filtering. They're
equivalent to a [`WHERE`](./where) clause on each property.

```cypher
MATCH (n:User {name: 'alice'})             RETURN n
MATCH (n:User {name: 'alice', age: 42})    RETURN n

-- Equivalent to:
MATCH (n:User)
WHERE n.name = 'alice' AND n.age = 42
RETURN n
```

Inline maps only express equality. For ranges, regex, or null checks,
move the predicate into [`WHERE`](./where).

### Anonymous node

If you don't need the node variable, drop it:

```cypher
MATCH (:User)-[:FOLLOWS]->(b) RETURN b
```

The anonymous form is handy in long patterns where only endpoints matter.

## Relationship patterns

A relationship pattern always has two endpoints, a direction (or its
absence), and optionally a type and properties.

```cypher
-- Outgoing (src → dst)
MATCH (a)-[r:FOLLOWS]->(b) RETURN a, r, b

-- Incoming (src ← dst)
MATCH (a)<-[r:FOLLOWS]-(b) RETURN a, r, b

-- Undirected — matches either direction, once
MATCH (a)-[r:KNOWS]-(b) RETURN a, r, b

-- Anonymous relationship variable (we don't need `r`)
MATCH (a)-[:FOLLOWS]->(b) RETURN a, b

-- Any type, any direction
MATCH (a)-[r]-(b) RETURN type(r), count(*)

-- Inline properties on the relationship
MATCH (a)-[r:FOLLOWS {since: 2020}]->(b) RETURN a, r, b
```

Direction on `MATCH` is optional. On [`CREATE`](./create) and
[`MERGE`](./unwind-merge#merge) it is mandatory.

### Multiple relationship types

Use `|` to match any of several types:

```cypher
MATCH (a)-[r:FOLLOWS|KNOWS]->(b)
RETURN type(r), count(*)
```

## Multi-hop patterns

Chain relationships to traverse further.

```cypher
-- Friends of friends
MATCH (a:User)-[:FOLLOWS]->(b)-[:FOLLOWS]->(c)
RETURN a.name AS who, c.name AS two_hops_away
```

Intermediate nodes can still be labelled and filtered:

```cypher
MATCH (p:Person)-[:WORKS_AT]->(c:Company)-[:IN]->(:City {name: 'Amsterdam'})
RETURN p.name, c.name
```

For unknown-length traversals (1 to N hops) see
[Paths → variable-length relationships](./paths#variable-length-relationships).

## Multiple patterns (cross-product)

Multiple comma-separated patterns produce a Cartesian product — one row
per combination.

```cypher
MATCH (a:User {id: 1}), (b:User {id: 2})
CREATE (a)-[:FOLLOWS]->(b)
```

Disconnected patterns are idiomatic when you want two endpoints for a
[`CREATE`](./create) or [`MERGE`](./unwind-merge#merge) that follows. In
a pure read query, they're usually a mistake:

```cypher
-- N * M rows — probably not what you want
MATCH (a:User), (b:User) RETURN a, b
```

For that same shape connected by a relationship, prefer:

```cypher
MATCH (a:User)-[:FOLLOWS]->(b:User) RETURN a, b
```

## Optional match

`OPTIONAL MATCH` is the graph equivalent of a SQL left join. When the
pattern matches, variables are bound. When it doesn't, they are `null`
— but the row from the previous clause still survives.

```cypher
MATCH (a:User)
OPTIONAL MATCH (a)-[:FOLLOWS]->(b)
RETURN a.name, b.name
```

Users with no outgoing `:FOLLOWS` edge still appear, with `b.name = null`.

### OPTIONAL MATCH with aggregation

Very common pattern: count related entities per node, including zero.

```cypher
MATCH (u:User)
OPTIONAL MATCH (u)-[:WROTE]->(p:Post)
RETURN u.name AS user, count(p) AS posts
ORDER BY posts DESC
```

`count(p)` — **not** `count(*)` — is crucial here: the optional miss
binds `p` to `null`, and `count(expr)` skips nulls. `count(*)` would
count the row and incorrectly yield `1` for users with no posts. See
[`count`](../functions/aggregation#count).

### Chained OPTIONAL MATCH

```cypher
MATCH (u:User {id: $id})
OPTIONAL MATCH (u)-[:OWNS]->(repo:Repo)
OPTIONAL MATCH (repo)-[:HAS_ISSUE]->(i:Issue {status: 'open'})
RETURN u.name, collect(DISTINCT repo.name) AS repos, count(i) AS open_issues
```

Each `OPTIONAL MATCH` is independent — a missing repo doesn't stop the
next optional from running.

## Path binding

Bind the whole traversal to a variable with `p = …`. See also
[Paths](./paths).

```cypher
MATCH p = (a)-[r:FOLLOWS]->(b)
RETURN p,
       length(p)          AS hops,
       nodes(p)           AS via,
       relationships(p)   AS rels
```

`length(p)` is the hop count; `nodes(p)` and `relationships(p)` return
lists (see [List Functions](../functions/list)).

## Progressive patterns

A brief tour, same shape, more useful at each step. Each example adds
one idea to the last.

### 1. Just the pattern

```cypher
MATCH (u:User)-[:FOLLOWS]->(other:User)
RETURN u, other
```

One row per `FOLLOWS` edge. Returns whole nodes.

### 2. Project properties

```cypher
MATCH (u:User)-[:FOLLOWS]->(other:User)
RETURN u.handle AS follower, other.handle AS leader
```

Cleaner for downstream consumers — only the fields that matter.

### 3. Filter

```cypher
MATCH (u:User)-[:FOLLOWS]->(other:User)
WHERE u.country = other.country
RETURN u.handle, other.handle
```

Same-country follows only — the predicate references both ends of the
relationship.

### 4. Order and paginate

```cypher
MATCH (u:User)-[:FOLLOWS]->(other:User)
WHERE u.country = other.country
RETURN u.handle, other.handle
ORDER BY u.handle
LIMIT 50
```

### 5. Aggregate

```cypher
MATCH (u:User)-[:FOLLOWS]->(other:User)
WHERE u.country = other.country
RETURN u.handle, count(other) AS same_country_follows
ORDER BY same_country_follows DESC
LIMIT 10
```

Each step is one more clause. Users who have never written Cypher
often try to start at step 5 — starting at step 1 and adding a clause
at a time is faster and catches mistakes earlier.

## Common patterns

### Lookup by unique property

```cypher
MATCH (u:User {email: $email})
RETURN u
LIMIT 1
```

LoraDB has no uniqueness constraints (see [Limitations](../limitations)),
so `LIMIT 1` is a belt-and-braces guard against duplicates.

### Filter chain

```cypher
MATCH (p:Product)-[:IN]->(c:Category {slug: $cat})
WHERE p.price <= $max AND p.in_stock
RETURN p
ORDER BY p.price ASC
LIMIT 20
```

### Two-sided match

```cypher
MATCH (src:User {id: $from}), (dst:User {id: $to})
MATCH (src)-[:FOLLOWS*1..3]->(dst)
RETURN src, dst
```

Useful with [shortest paths](./paths#shortest-paths).

### Related entities in one hop

```cypher
MATCH (p:Person {id: $id})-[:WORKS_AT]->(c:Company)
RETURN p.name, collect(c.name) AS companies
```

### Relationship existence check

Use [`EXISTS { pattern }`](./where#pattern-existence) to filter without
an extra row:

```cypher
MATCH (u:User)
WHERE EXISTS { (u)-[:FOLLOWS]->() }
RETURN u
```

## Edge cases

### Empty graph

`MATCH (:Unknown)` on an empty graph succeeds with zero rows. On a
populated graph without any node of that label, it fails at analysis:
`Unknown label :Unknown`. See [Troubleshooting](../troubleshooting#semantic-errors).

### Self-loops

A node connected to itself:

```cypher
MATCH (a)-[:FOLLOWS]->(a) RETURN a
```

### Duplicate rows

A node reached via two different paths produces two rows. Use
[`DISTINCT`](./return-with#distinct) on the `RETURN` if you only want
one:

```cypher
MATCH (a:User)-[:FOLLOWS]->(b)-[:FOLLOWS]->(c)
RETURN DISTINCT a, c
```

### Type mismatch in inline filter

Inline maps are strictly typed. `{id: 1}` does **not** match a node with
`{id: '1'}` — see [Troubleshooting](../troubleshooting#queries-return-empty-results).

### Symmetric-pair deduplication

An undirected `(a)-[:KNOWS]-(b)` match produces both `(alice, bob)`
and `(bob, alice)` rows. Filter with
[`id()`](../functions/overview#entity-introspection) to keep exactly
one representative per unordered pair:

```cypher
MATCH (a:Person)-[:KNOWS]-(b:Person)
WHERE id(a) < id(b)
RETURN a.name, b.name
```

### Same variable in two positions

A variable can appear multiple times in a pattern — every occurrence
must bind to the same entity. Useful for detecting cycles:

```cypher
MATCH (a)-[:FOLLOWS]->(b)-[:FOLLOWS]->(a)
RETURN a.name, b.name
```

Relationships within a single pattern must use **distinct** variable
names, even when the type is the same:

```cypher
-- Invalid: r reused across two relationships
MATCH (a)-[r]->(b)-[r]->(c) RETURN a, b, c

-- Valid
MATCH (a)-[r1]->(b)-[r2]->(c) RETURN a, b, c
```

See [Relationships → edge cases](../concepts/relationships#deleting-a-relationship-thats-been-matched-twice).

### Property pattern on both sides

Filters in an inline map apply to that one node only. Filtering both
endpoints uses the shorthand twice, or drops into `WHERE` for clarity:

```cypher
MATCH (a:User {country: 'NL'})-[:FOLLOWS]->(b:User {country: 'NL'})
RETURN a.handle, b.handle
```

```cypher
-- Equivalent, easier to read for larger filters
MATCH (a:User)-[:FOLLOWS]->(b:User)
WHERE a.country = 'NL' AND b.country = 'NL'
RETURN a.handle, b.handle
```

## See also

- [**CREATE**](./create) — writing nodes and relationships.
- [**WHERE**](./where) — predicate filtering.
- [**RETURN / WITH**](./return-with) — projecting and piping rows.
- [**Paths**](./paths) — variable-length and shortest paths.
- [**Aggregation**](./aggregation) — grouping after `MATCH`.
- [**Concepts → Nodes**](../concepts/nodes),
  [**Relationships**](../concepts/relationships),
  [**Properties**](../concepts/properties).
