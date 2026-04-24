---
title: Paths and Graph Traversals
sidebar_label: Paths
description: Path traversals in LoraDB — variable-length patterns, bounded and unbounded quantifiers, zero-hop matches, path binding, shortestPath, and allShortestPaths.
---

# Paths and Graph Traversals

Paths describe traversals through the graph. Beyond fixed-hop
[`MATCH`](./match) patterns, LoraDB supports variable-length traversals,
path binding, and shortest-path search.

> For an intuition-first walkthrough, see the
> [**Ten-Minute Tour → Walk multi-step patterns**](../getting-started/tutorial#step-6--walk-multi-step-patterns).

## Overview

| Goal | Pattern |
|---|---|
| Fixed hop count | <CypherCode code="(a)-[:R]->(b)-[:R]->(c)" /> |
| 1 to 3 hops | <CypherCode code="(a)-[:R*1..3]->(b)" /> |
| Exactly 3 hops | <CypherCode code="(a)-[:R*3..3]->(b)" /> |
| Up to 3 hops | <CypherCode code="(a)-[:R*..3]->(b)" /> |
| 3 or more hops | <CypherCode code="(a)-[:R*3..]->(b)" /> |
| Any positive number of hops | <CypherCode code="(a)-[:R*]->(b)" /> |
| Zero-or-more hops (includes `a`) | <CypherCode code="(a)-[:R*0..]->(b)" /> |
| Single shortest path | [<CypherCode code="shortestPath(…)" />](#shortest-paths) |
| All shortest paths | [<CypherCode code="allShortestPaths(…)" />](#all-shortest-paths) |
| Pattern existence, no rows | [<CypherCode code="EXISTS { (…)-[…]->(…) }" />](./where#pattern-existence) |
| Attach 1-to-many inline | [Pattern comprehension](../functions/list#pattern-comprehension) |

## Variable-length relationships

Cypher's `*` operator marks a relationship as variable-length — the
traversal can cross any number of that relationship between the given
bounds.

```cypher
-- 1 to 2 hops
MATCH (a)-[:FOLLOWS*1..2]->(b) RETURN a, b

-- Exactly 3 hops
MATCH (a)-[:FOLLOWS*3..3]->(b) RETURN b

-- Up to 3 hops (same as 1..3)
MATCH (a)-[:FOLLOWS*..3]->(b) RETURN a, b

-- 3 or more, unbounded
MATCH (a)-[:FOLLOWS*3..]->(b) RETURN a, b

-- Any positive number of hops
MATCH (a)-[:FOLLOWS*]->(b) RETURN a, b

-- Zero-hop included — `a` itself matches as `b`
MATCH (a)-[:FOLLOWS*0..1]->(b) RETURN b
```

### Cycle avoidance

Within a single matched path, **no relationship is traversed twice**.
This makes `*..N` safe on cyclic graphs without explicit guard logic.

### Traverse any type

Drop the type to traverse any relationship kind:

```cypher
MATCH (a)-[*1..3]-(b) RETURN a, b
```

This is rarely what you want on non-trivial graphs — the result set
explodes fast.

## Binding the path

`MATCH p = (…)-[…]->(…)` binds the whole path to `p`. Use the built-in
path functions to inspect it.

```cypher
MATCH p = (a:User)-[:FOLLOWS*1..3]->(b:User)
RETURN length(p)        AS hops,
       nodes(p)         AS via,
       relationships(p) AS rels
```

### Path functions

| Function | Returns |
|---|---|
| `length(p)` | Number of relationships on the path (`Int`) |
| `nodes(p)` | List of nodes, start to end |
| `relationships(p)` | List of relationships, in traversal order |

`length(p)` for a zero-hop path is `0`; `nodes(p)` has one element; `relationships(p)` is empty.

### Project intermediate nodes

```cypher
MATCH p = (a:City {name: 'Amsterdam'})-[:ROUTE*2..3]->(b:City)
RETURN a.name, b.name,
       [n IN nodes(p) | n.name] AS via
```

## Shortest paths

### Single shortest path

Returns one path of minimum length per `(start, end)` pair.

```cypher
MATCH p = shortestPath(
  (a:Station {name: 'Amsterdam'})-[:ROUTE*]->(b:Station {name: 'Den Haag'})
)
RETURN p, length(p)
```

Ties are broken arbitrarily — if you need every minimum-length path,
use [`allShortestPaths`](#all-shortest-paths).

### All shortest paths

Returns every path tied for the minimum length.

```cypher
MATCH p = allShortestPaths(
  (a:Station {name: 'Amsterdam'})-[:ROUTE*]->(b:Station {name: 'Den Haag'})
)
RETURN p, length(p)
```

### Reachability check

```cypher
MATCH p = shortestPath((a:Node {id: $src})-[*]-(b:Node {id: $dst}))
RETURN length(p) AS hops
```

`hops` is `null` if no path connects the two (the `MATCH` finds nothing).

### Shortest path with type filter

```cypher
MATCH p = shortestPath(
  (a:User {id: $from})-[:FOLLOWS*]->(b:User {id: $to})
)
RETURN length(p)
```

Only `:FOLLOWS` edges count as hops.

## Patterns in expressions

### Pattern comprehension

Attach a one-to-many result inline — no extra `MATCH` needed. See also
[List Functions → pattern comprehension](../functions/list#pattern-comprehension).

```cypher
MATCH (p:Person)
RETURN p.name,
       [(p)-[:KNOWS]->(f) | f.name] AS friends
```

With a filter:

```cypher
MATCH (p:Person)
RETURN p.name,
       [(p)-[:WROTE]->(post:Post) WHERE post.published | post.title] AS posts
```

### Existence check

Use [`EXISTS { pattern }`](./where#pattern-existence) to filter rows by
whether a pattern matches — without introducing extra rows.

```cypher
MATCH (n:User)
WHERE EXISTS { (n)-[:FOLLOWS]->() }
RETURN n
```

## Common patterns

### Friends-of-friends

```cypher
MATCH (me:User {id: $id})-[:FOLLOWS*2..2]->(fof)
WHERE fof <> me
RETURN DISTINCT fof.name
```

`DISTINCT` removes duplicates when multiple 2-hop paths reach the same
person.

### Reachable set

```cypher
MATCH (start:Node {id: $id})-[*0..5]-(n)
RETURN DISTINCT n
```

Every node within 5 hops, in any direction.

### Shortest route length

```cypher
MATCH p = shortestPath(
  (a:City {name: $from})-[:ROAD*]->(b:City {name: $to})
)
RETURN length(p) AS hops
```

### All nearby neighbors of each node

```cypher
MATCH (n:Node)
RETURN n,
       [(n)-[*1..2]-(m) | m] AS within_two_hops
```

### Path with property check along the way

```cypher
MATCH p = (a:User {id: $from})-[:FOLLOWS*1..3]->(b:User {id: $to})
WHERE all(r IN relationships(p) WHERE r.active)
RETURN p
```

Filter the whole path with [list predicates](../functions/list#predicates-in-where)
like `all`, `any`, `none`.

### Path with minimum intermediate-property value

```cypher
MATCH p = (a:Station {code: $from})-[:ROUTE*1..6]->(b:Station {code: $to})
WHERE all(r IN relationships(p) WHERE r.status = 'open')
WITH p, reduce(cost = 0, r IN relationships(p) | cost + r.km) AS km
ORDER BY km ASC
LIMIT 1
RETURN p, km
```

A manual "cheapest path of length ≤ 6" — `shortestPath` counts hops
only, so for weighted traversals you [`reduce`](../functions/list#reduce)
the relationship list yourself, then pick the minimum.

### Excluded node on path

```cypher
MATCH p = (a:User {id: $from})-[:FOLLOWS*]->(b:User {id: $to})
WHERE none(n IN nodes(p) WHERE n.blocked)
RETURN p
```

Path must not pass through any `blocked` user.

### Unique nodes on path

#### Why duplicates happen

Variable-length patterns guarantee **no relationship is traversed
twice** on a single path (see [cycle avoidance](#cycle-avoidance)) —
but the same **node** can still appear more than once when two
different edges reach it. On a triangle `A → B → C → A`, the path
`A → B → C → A` visits `A` twice via two distinct edges.

This is path uniqueness (always enforced) vs node uniqueness (not
enforced). For "pure" acyclic walks you need to filter node
uniqueness yourself.

#### The duplicate in practice

```cypher
CREATE
  (a:Stop {code: 'A'}),
  (b:Stop {code: 'B'}),
  (c:Stop {code: 'C'}),
  (a)-[:NEXT]->(b),
  (b)-[:NEXT]->(c),
  (c)-[:NEXT]->(a)
```

```cypher
MATCH p = (:Stop {code: 'A'})-[:NEXT*1..4]->(end)
RETURN [n IN nodes(p) | n.code] AS path
```

| path |
|---|
| `['A', 'B']` |
| `['A', 'B', 'C']` |
| `['A', 'B', 'C', 'A']`  ← `A` repeats |
| `['A', 'B', 'C', 'A', 'B']`  ← `A` and `B` repeat |

The third and fourth rows are the "node-repeats" cases.

#### Workaround: filter on size equality

Collect node ids into a list, then require the list size to match the
size of its de-duplicated form. LoraDB has no `distinct(list)`
helper, so express dedup as "keep only elements whose first
occurrence is at their own index":

```cypher
MATCH p = (:Stop {code: 'A'})-[:NEXT*1..4]->(end)
WITH p,
     [n IN nodes(p) | id(n)]                                        AS ids
WITH p, ids,
     [i IN range(0, size(ids) - 1) WHERE NOT ids[i] IN ids[..i]
      | ids[i]]                                                     AS unique_ids
WHERE size(ids) = size(unique_ids)
RETURN [n IN nodes(p) | n.code] AS path
```

Same graph:

| path |
|---|
| `['A', 'B']` |
| `['A', 'B', 'C']` |

Cycles filtered.

#### Mental model

Treat `nodes(p)` as a list. Every list-level operation you'd reach for
in the [List Functions](../functions/list) page works on it. When a
query asks "are all elements unique?" there's no single function
— you express it by comparing `size(list)` with the size of the
filtered "first-occurrence-only" form.

#### Why this is awkward (and a future-facing note)

> This pattern is harder to read than it should be because there's no
> built-in `distinct(list)` or `collect(DISTINCT …)` helper **over a
> literal list**. `collect(DISTINCT …)` is an aggregate and only
> applies to rows. A future helper — for example `distinct_list(xs)`
> — would let the pattern collapse to
> `WHERE size(ids) = size(distinct_list(ids))`. Until then, the
> comprehension form above is idiomatic.

See [Limitations](../limitations) for the current list-function gaps.

#### Related

- [List comprehension](../functions/list#list-comprehension) — the
  syntax used for the "first-occurrence-only" filter.
- [`collect(DISTINCT …)`](../functions/aggregation#collect) — distinct
  values when *rows* are your input, not a literal list.
- [`UNWIND + collect(DISTINCT …)`](../functions/list#distinct-values-from-a-list)
  — the row-level workaround.
- [DISTINCT on `RETURN`](./return-with#distinct) — dedup whole output
  rows.
- [Cycle avoidance](#cycle-avoidance) — why relationship uniqueness is
  automatic.

### Longest path up to N

`shortestPath` has no "longest" counterpart. Bound the depth, collect,
and sort:

```cypher
MATCH p = (a:User {id: $id})-[:FOLLOWS*1..5]->(b:User)
WITH a, b, p
ORDER BY length(p) DESC
LIMIT 1
RETURN p, length(p) AS hops
```

Expensive on dense graphs — prefer a bounded variable-length match
with a small `*..N` cap.

## Edge cases

### Disconnected nodes

If no path connects `a` and `b`, the `MATCH` emits zero rows. A
following [`RETURN length(p)`](#path-functions) never runs. Wrap with
[`OPTIONAL MATCH`](./match#optional-match) if you still want a row:

```cypher
MATCH (a:User {id: $from}), (b:User {id: $to})
OPTIONAL MATCH p = shortestPath((a)-[:FOLLOWS*]->(b))
RETURN a, b, length(p) AS hops    -- hops = null if unreachable
```

### Self-loops

`(a)-[:R*1..]->(a)` matches only if there's a cycle back to `a`.
`(a)-[:R*0..]->(a)` always matches `a` via the zero-hop interpretation.

### Performance

Unbounded variable-length traversals can be expensive. Bound with a
maximum depth whenever the answer is "and not further":

```cypher
-- Good — bounded
MATCH (a)-[:KNOWS*1..6]-(b) …

-- Risky on large graphs — unbounded
MATCH (a)-[:KNOWS*]-(b) …
```

### Zero-hop semantics

`[:R*0..N]` includes the start node itself as a valid `b` via a
zero-hop "path". `nodes(p)` has one element, `relationships(p)` is
empty. Useful when the answer may be "myself plus my neighbors within
N".

## What's not supported

- **Weighted shortest paths** — `shortestPath` treats every relationship
  as cost 1. No cost argument, no Dijkstra-style weighting. Implement
  in the host language for now.
- **Quantified path patterns** (`((:X)-[:R]->(:Y)){1,3}`) — not in the
  grammar.
- **Procedures** like `apoc.path.*` — no CALL surface. See
  [Limitations](../limitations).
- **Inline `WHERE` inside `*` patterns** — parsed but not evaluated.
  Move the predicate out into a standalone `WHERE` using `nodes(p)` /
  `relationships(p)`.

## See also

- [**MATCH**](./match) — underlying pattern language.
- [**WHERE**](./where) — path predicates with `all` / `any` / `EXISTS`.
- [**RETURN / WITH**](./return-with) — project path contents.
- [**List Functions → Pattern comprehension**](../functions/list#pattern-comprehension).
- [**Functions → Entity introspection**](../functions/overview#entity-introspection) — `id`, `labels`, `type`.
- [**Graph Model → Relationship semantics**](../concepts/graph-model#relationship-semantics).
