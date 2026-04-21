---
title: The LoraDB Graph Data Model
sidebar_label: Graph Data Model
---

# The LoraDB Graph Data Model

LoraDB uses the **labeled property graph** model.

Three things live in the graph:

| | Purpose | Example |
|---|---|---|
| [**Node**](./nodes) | An entity | a `:Person`, a `:City`, a `:Movie` |
| [**Relationship**](./relationships) | A typed, directed link between two nodes | `(a)-[:KNOWS]->(b)` |
| [**Property**](./properties) | A typed key/value attached to a node or relationship | `{name: 'Ada', born: 1815}` |

Both nodes and relationships can carry any number of properties.
Relationships always have exactly one **type** and exactly one
**direction**; they always connect two (possibly equal) nodes.

## See it in four queries

The model is easier to feel than to describe. Walk through these four
queries in order — they build a tiny graph and read it back four
different ways.

### 1. Make two nodes

```cypher
CREATE (:Person {name: 'Ada',   born: 1815})
CREATE (:Person {name: 'Grace', born: 1906})
```

Two nodes with label `Person` and two properties each.

### 2. Connect them

```cypher
MATCH (ada:Person {name: 'Ada'}), (grace:Person {name: 'Grace'})
CREATE (ada)-[:INFLUENCED {year: 1843}]->(grace)
```

One directed relationship with type `INFLUENCED` and its own `year`
property.

### 3. Read nodes

```cypher
MATCH (p:Person) RETURN p.name, p.born
```

Two rows — same shape as the properties we wrote. Label and direction
are invisible in this projection.

### 4. Read through the relationship

```cypher
MATCH (a)-[r:INFLUENCED]->(b)
RETURN a.name AS influencer, r.year AS year, b.name AS influenced
```

One row — Ada → Grace, with the relationship's own property alongside.
Notice we can project properties from the relationship itself, not just
from the nodes at its ends. That's the shape of a property graph:
**every piece — nodes, their labels, relationships, their types, and
the properties on either side — is addressable in a query.**

## Vocabulary

| Term | Meaning |
|---|---|
| **Label** | A tag on a node. Zero or more per node. <CypherCode code=":Person" />, <CypherCode code=":Admin" />. Case-sensitive, conventionally `PascalCase`. |
| **Type** | The kind of a relationship. Exactly one per edge. <CypherCode code=":FOLLOWS" />, <CypherCode code=":WORKS_AT" />. Case-sensitive, conventionally `UPPER_SNAKE_CASE`. |
| **Property key** | The name of a key in a property map. Case-sensitive string. |
| **Direction** | Source → destination on a relationship. Mandatory at creation, optional in [<CypherCode code="MATCH" />](../queries/match). |
| **Degree** | The number of relationships touching a node. |
| **Path** | An alternating sequence of nodes and relationships, produced by a matched traversal. |

## Schema-free

LoraDB has no `CREATE TABLE` step. Labels, relationship types, and
property keys are created implicitly the first time you use them in a
write:

```cypher
CREATE (c:Country {name: 'NL', iso: 'NLD'})
```

The first time this runs, the label `Country` and properties `name`,
`iso` come into existence. Writes are permissive; reads validate
labels and relationship types against the live graph. The full rules
— and the trade-offs that come with "no schema" — live on their own
page: [**Schema-free writes and soft validation**](./schema-free).

Handle the lack of constraints in application code, or by matching
before writing, or with [`MERGE`](../queries/unwind-merge#merge) for
idempotent writes.

## Relationship semantics

- Direction is mandatory at creation (`(a)-[:T]->(b)`) but optional in
  `MATCH`: `(a)-[:T]-(b)` matches both directions.
- A relationship has **one** type, not a list.
- Types are case-sensitive strings, conventionally `UPPER_SNAKE`.
- Relationships cannot be dangling — `src` and `dst` must exist at
  creation. Deleting a node with edges requires
  [`DETACH DELETE`](../queries/set-delete#detach-delete).
- Self-loops are allowed: `(a)-[:R]->(a)`.

## Property values

Properties are typed. See [Data Types](../data-types/overview) for the
full list — in short:
[scalars](../data-types/scalars) (null, booleans, integers, floats,
strings), [lists and maps](../data-types/lists-and-maps),
[temporals](../data-types/temporal) (`Date`, `Time`, `DateTime`,
`Duration`, …), and [spatial points](../data-types/spatial) (2D and 3D,
Cartesian and WGS-84).

```cypher
CREATE (:Trip {
  from:     'AMS',
  to:       'LHR',
  when:     datetime('2026-04-20T08:00:00Z'),
  duration: duration('PT75M'),
  route:    ['AMS', 'LHR'],
  origin:   point({latitude: 52.31, longitude: 4.76})
})
```

## Identity

Every node and relationship gets an auto-generated `u64` ID. IDs are:

- **Stable** within a process — they do not change after creation.
- **Opaque** — don't treat the number as meaningful; use properties for
  external identity.
- **Not reused** — deleting an entity does not free its ID.

Use the built-in [`id()` function](../functions/overview#entity-introspection)
to read the internal ID if you really need it, but prefer matching on
your own property keys.

```cypher
MATCH (n:User {email: $email}) RETURN id(n) AS internal_id
```

### One useful trick

To avoid symmetric-pair duplicates in an undirected match, filter by
`id(a) < id(b)`:

```cypher
MATCH (a:Person)-[:KNOWS]-(b:Person)
WHERE id(a) < id(b)
RETURN a.name, b.name
```

Otherwise you'd get both `(alice, bob)` and `(bob, alice)` rows.

## Storage model (at a glance)

- **In-memory only.** All data lives in the process; nothing persists
  across restarts. See [Limitations → Storage](../limitations#storage).
- **Single mutex.** Queries serialise. No per-row locking, no isolation
  levels.
- **Adjacency on both ends.** Each relationship is reachable from both
  endpoints without a separate index.

## Diagram (planned)

**Type:** graph

**Purpose:** Show the three primitives — node, relationship, property
— in one picture so a reader who read
[See it in four queries](#see-it-in-four-queries) can map every term
to a visual element.

**Elements:**
- Two labelled nodes (`:Person {name: 'Ada'}`, `:Person {name: 'Grace'}`)
- One directed relationship between them (`INFLUENCED {year: 1843}`)
- Inline property annotations on each node and the edge

**Labels:**
- Node label (top, e.g. "Person")
- Property map (beneath the label)
- Relationship type on the arrow (e.g. "INFLUENCED")
- Relationship properties hanging off the arrow body

**Description:**
Two rounded rectangles connected by a solid arrow. Each rectangle
has a label pill (`:Person`) and a key/value list. The arrow carries
its own pill (`:INFLUENCED`) plus a small map (`{year: 1843}`). The
diagram should make clear that properties live on **both** nodes and
the edge, and that direction is part of the relationship — the
arrow head is never ambiguous.

## Comparison to other models

| Model | How LoraDB differs |
|---|---|
| Relational (SQL) | No schema, no joins — relationships are first-class edges. |
| Document (JSON) | Relationships are explicit, queryable, and indexable. |
| RDF / triplestore | Relationships carry properties; labels are per-node. |
| Hypergraph | Not supported — every edge connects exactly two nodes. |

## Modelling checklist

A short, pragmatic checklist when deciding how to model a new thing.

### "Is it a node or a property?"

| Use a node | Use a property |
|---|---|
| You'll traverse to it from elsewhere | It's a leaf value |
| It has its own identity / lifecycle | It's strictly owned by one entity |
| It's enumerated over by other queries | It's only read alongside its owner |

**Example** — `email`: address of a user. On a `User` node, a `string`
property. If two users share emails across accounts, promote to an
`:Email` node with `(u)-[:HAS_EMAIL]->(e)`.

### "Is it a relationship or a node?"

If the "edge" itself has a lot of data, including another relationship
pointing at it, it's probably a node. Cypher can't point edges at
other edges:

```cypher
-- Edge carrying a little data — fine
CREATE (a)-[:RATED {stars: 4, at: datetime()}]->(b)

-- Edge with further lifecycle / attachments — promote to node
CREATE (a)-[:WROTE]->(r:Review {stars: 4, body: '…', at: datetime()})
CREATE (r)-[:ABOUT]->(b)
CREATE (r)-[:IN_LANG]->(:Language {code: 'en'})
```

### "Directional, undirected, or two edges?"

| Data is… | Model it as |
|---|---|
| Asymmetric (`FOLLOWS`, `REPORTS_TO`) | One directed edge |
| Symmetric (`FRIEND`, `MARRIED`) | One directed edge + undirected `MATCH` |
| Both sides carry data (mutual but with direction-specific fields) | Two directed edges |

Symmetric relationships storing *one* edge with undirected `MATCH` is
cheaper and avoids mutability bugs — you never have to keep both
mirror edges consistent.

### "Small enumeration — property or labelled node?"

For something like order status with a known set of values:

```cypher
-- String property — simpler
CREATE (:Order {id: 1, status: 'paid'})

-- Label — makes WHERE slightly more efficient and pattern-readable
CREATE (:Order:Paid {id: 1})
```

Labels as status flags work well when the status rarely changes and
is often the primary filter. Property status scales better when
values churn or when you carry status metadata
(`status_changed_at`, `status_reason`).

## What is _not_ modeled

- Hyperedges (a relationship connects exactly two nodes).
- Typed schemas with required properties — LoraDB will happily create
  two `:Person` nodes with different property sets.
- Uniqueness constraints — nothing prevents two nodes with identical
  labels and properties. Enforce uniqueness in your application code,
  or by matching before creating, or with
  [`MERGE`](../queries/unwind-merge#merge).
- Weighted relationships as a native primitive —
  [shortest paths](../queries/paths#shortest-paths) count hops
  regardless of any `weight` property.

## See also

- [**Nodes**](./nodes) — labels, identity, match and mutate.
- [**Relationships**](./relationships) — types, direction, properties.
- [**Properties**](./properties) — per-entity key/value data.
- [**Schema-free**](./schema-free) — what implicit schema means for
  writes and reads.
- [**Result formats**](./result-formats) — how queries come back.
- [**Data Types**](../data-types/overview) — what property values can be.
- [**Queries → Overview**](../queries/) — clause-by-clause reference.
- [**Tutorial**](../getting-started/tutorial) — guided walkthrough.
