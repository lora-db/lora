---
title: Paths and Graph Traversals
sidebar_label: Paths
description: Path traversals in LoraDB — variable-length patterns, bounded and unbounded quantifiers, zero-hop matches, path binding, shortestPath, and allShortestPaths.
---

# Paths and Graph Traversals

Paths describe traversals through the graph. On top of fixed-hop
[`MATCH`](./match) patterns, LoraDB supports variable-length
traversals, path binding, and shortest-path search.

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

<QueryCodeBlock code={String.raw`// 1 to 2 hops
MATCH (a)-[:FOLLOWS*1..2]->(b) RETURN a, b

;// Exactly 3 hops
MATCH (a)-[:FOLLOWS*3..3]->(b) RETURN b

;// Up to 3 hops (same as 1..3)
MATCH (a)-[:FOLLOWS*..3]->(b) RETURN a, b

;// 3 or more, unbounded
MATCH (a)-[:FOLLOWS*3..]->(b) RETURN a, b

;// Any positive number of hops
MATCH (a)-[:FOLLOWS*]->(b) RETURN a, b

;// Zero-hop included — \`a\` itself matches as \`b\`
MATCH (a)-[:FOLLOWS*0..1]->(b) RETURN b`} />

Each match produces one row per distinct traversal, so a single `a`
reached via two separate paths appears twice. Use
[`DISTINCT`](./return-with#distinct) on the `RETURN` when you only
care about which nodes are reachable, not how many ways.

### Cycle avoidance

Within a single matched path, **no relationship is traversed twice**.
This makes `*..N` safe on cyclic graphs without explicit guard logic.
Nodes can still repeat on a single path — see
[Unique nodes on path](#unique-nodes-on-path) for the distinction.

### Traverse any type

Drop the type to traverse any relationship kind:

<QueryCodeBlock code={String.raw`MATCH (a)-[*1..3]-(b) RETURN a, b`} />

This is rarely what you want on non-trivial graphs — the result set
explodes fast.

## Binding the path

`MATCH p = (…)-[…]->(…)` binds the whole path to `p`. Use the built-in
path functions to inspect it.

<QueryCodeBlock code={String.raw`MATCH p = (a:User)-[:FOLLOWS*1..3]->(b:User)
RETURN path.length(p)        AS hops,
       path.nodes(p)         AS via,
       path.edges(p) AS rels`} />

### Path functions

| Function | Returns |
|---|---|
| `path.length(p)` | Number of relationships on the path (`Int`) |
| `path.nodes(p)` | List of nodes, start to end |
| `path.edges(p)` | List of relationships, in traversal order |

`path.length(p)` for a zero-hop path is `0`; `path.nodes(p)` has one element; `path.edges(p)` is empty.

### Project intermediate nodes

<QueryCodeBlock code={String.raw`MATCH p = (a:City {name: 'Amsterdam'})-[:ROUTE*2..3]->(b:City)
RETURN a.name, b.name,
       [n IN path.nodes(p) | n.name] AS via`} />

## Shortest paths

### Single shortest path

Returns one path of minimum length per `(start, end)` pair.

<QueryCodeBlock code={String.raw`MATCH p = shortestPath(
  (a:Station {name: 'Amsterdam'})-[:ROUTE*]->(b:Station {name: 'Den Haag'})
)
RETURN p, path.length(p)`} />

Hop count here is the number of `:ROUTE` relationships traversed —
every relationship has cost 1, regardless of any weight property.
Ties are broken arbitrarily: if two paths have the same minimum
length you get one of them, not both — use
[`allShortestPaths`](#all-shortest-paths) when you need every
minimum-length path.

### All shortest paths

Returns every path tied for the minimum length.

<QueryCodeBlock code={String.raw`MATCH p = allShortestPaths(
  (a:Station {name: 'Amsterdam'})-[:ROUTE*]->(b:Station {name: 'Den Haag'})
)
RETURN p, path.length(p)`} />

### Reachability check

<QueryCodeBlock code={String.raw`MATCH p = shortestPath((a:Node {id: $src})-[*]-(b:Node {id: $dst}))
RETURN path.length(p) AS hops`} />

One row back when a path exists; **zero rows** when nothing connects
`a` and `b` — the outer `MATCH` emits nothing, so the `RETURN` never
runs. Wrap with [`OPTIONAL MATCH`](./match#optional-match) (see
[Disconnected nodes](#disconnected-nodes) below) if you want a row
back with `hops = null` for the unreachable case.

### Shortest path with type filter

<QueryCodeBlock code={String.raw`MATCH p = shortestPath(
  (a:User {id: $from})-[:FOLLOWS*]->(b:User {id: $to})
)
RETURN path.length(p)`} />

Only `:FOLLOWS` edges count as hops.

## Patterns in expressions

### Pattern comprehension

Attach a one-to-many result inline — no extra `MATCH` needed. See also
[List Functions → pattern comprehension](../functions/list#pattern-comprehension).

<QueryCodeBlock code={String.raw`MATCH (p:Person)
RETURN p.name,
       [(p)-[:KNOWS]->(f) | f.name] AS friends`} />

With a filter:

<QueryCodeBlock code={String.raw`MATCH (p:Person)
RETURN p.name,
       [(p)-[:WROTE]->(post:Post) WHERE post.published | post.title] AS posts`} />

### Existence check

Use [`EXISTS { pattern }`](./where#pattern-existence) to filter rows by
whether a pattern matches — without introducing extra rows.

<QueryCodeBlock code={String.raw`MATCH (n:User)
WHERE EXISTS { (n)-[:FOLLOWS]->() }
RETURN n`} />

## Common patterns

### Friends-of-friends

<QueryCodeBlock code={String.raw`MATCH (me:User {id: $id})-[:FOLLOWS*2..2]->(fof)
WHERE fof <> me
RETURN DISTINCT fof.name`} />

`*2..2` forces exactly two hops — direct follows don't count. A
given `fof` can be reached through multiple mutual friends, so
`DISTINCT` collapses those duplicates to one row per person. The
`fof <> me` guard drops cycles back to the starting user.

### Reachable set

<QueryCodeBlock code={String.raw`MATCH (start:Node {id: $id})-[*0..5]-(n)
RETURN DISTINCT n`} />

Every node within 5 hops, in any direction. `*0..5` includes the
start node itself (via the zero-hop interpretation), so `n` covers
`start` plus everything reachable within five relationships.
Unbounded range (`*0..`) works but will expand the entire connected
component — keep a cap.

### Shortest route length

<QueryCodeBlock code={String.raw`MATCH p = shortestPath(
  (a:City {name: $from})-[:ROAD*]->(b:City {name: $to})
)
RETURN path.length(p) AS hops`} />

### All nearby neighbors of each node

<QueryCodeBlock code={String.raw`MATCH (n:Node)
RETURN n,
       [(n)-[*1..2]-(m) | m] AS within_two_hops`} />

### Path with property check along the way

<QueryCodeBlock code={String.raw`MATCH p = (a:User {id: $from})-[:FOLLOWS*1..3]->(b:User {id: $to})
WHERE all(r IN path.edges(p) WHERE r.active)
RETURN p`} />

Filter the whole path with [list predicates](../functions/list#predicates-in-where)
like `all`, `any`, `none`.

### Path with minimum intermediate-property value

<QueryCodeBlock code={String.raw`MATCH p = (a:Station {code: $from})-[:ROUTE*1..6]->(b:Station {code: $to})
WHERE all(r IN path.edges(p) WHERE r.status = 'open')
WITH p, reduce(cost = 0, r IN path.edges(p) | cost + r.km) AS km
ORDER BY km ASC
LIMIT 1
RETURN p, km`} />

A manual "cheapest path of length ≤ 6" — `shortestPath` counts hops
only, so for weighted traversals you [`reduce`](../functions/list#reduce)
the relationship list yourself, then pick the minimum.

### Excluded node on path

<QueryCodeBlock code={String.raw`MATCH p = (a:User {id: $from})-[:FOLLOWS*]->(b:User {id: $to})
WHERE none(n IN path.nodes(p) WHERE n.blocked)
RETURN p`} />

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

<QueryCodeBlock code={String.raw`CREATE
  (a:Stop {code: 'A'}),
  (b:Stop {code: 'B'}),
  (c:Stop {code: 'C'}),
  (a)-[:NEXT]->(b),
  (b)-[:NEXT]->(c),
  (c)-[:NEXT]->(a)`} />

<QueryCodeBlock code={String.raw`MATCH p = (:Stop {code: 'A'})-[:NEXT*1..4]->(end)
RETURN [n IN path.nodes(p) | n.code] AS path`} />

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

<QueryCodeBlock code={String.raw`MATCH p = (:Stop {code: 'A'})-[:NEXT*1..4]->(end)
WITH p,
     [n IN path.nodes(p) | id(n)]                                        AS ids
WITH p, ids,
     [i IN list.range(0, value.size(ids) - 1) WHERE NOT ids[i] IN ids[..i]
      | ids[i]]                                                     AS unique_ids
WHERE value.size(ids) = value.size(unique_ids)
RETURN [n IN path.nodes(p) | n.code] AS path`} />

Same graph:

| path |
|---|
| `['A', 'B']` |
| `['A', 'B', 'C']` |

Cycles filtered.

#### Mental model

Treat `path.nodes(p)` as a list. Every list-level operation you'd reach for
in the [List Functions](../functions/list) page works on it. When a
query asks "are all elements unique?" there's no single function
— you express it by comparing `value.size(list)` with the size of the
filtered "first-occurrence-only" form.

#### Why this is awkward (and a future-facing note)

> This pattern is harder to read than it should be because there's no
> built-in `distinct(list)` or `collect(DISTINCT …)` helper **over a
> literal list**. `collect(DISTINCT …)` is an aggregate and only
> applies to rows. A future helper — for example `distinct_list(xs)`
> — would let the pattern collapse to
> `WHERE value.size(ids) = value.size(distinct_list(ids))`. Until then, the
> comprehension form above is idiomatic.

See [Limitations](../limitations) for the current list-function gaps.

#### Related

- [List comprehension](../functions/list#list-comprehension) — the
  syntax used for the "first-occurrence-only" filter.
- [`collect(DISTINCT …)`](../functions/aggregation#collect) — distinct
  values when *rows* are your input, not a literal list.
- [`list.unique` and set-style helpers](../functions/list#deduplicate-and-set-operations)
  — the row-level workaround.
- [DISTINCT on `RETURN`](./return-with#distinct) — dedup whole output
  rows.
- [Cycle avoidance](#cycle-avoidance) — why relationship uniqueness is
  automatic.

### Longest path up to N

`shortestPath` has no "longest" counterpart. Bound the depth, collect,
and sort:

<QueryCodeBlock code={String.raw`MATCH p = (a:User {id: $id})-[:FOLLOWS*1..5]->(b:User)
WITH a, b, p
ORDER BY path.length(p) DESC
LIMIT 1
RETURN p, path.length(p) AS hops`} />

Expensive on dense graphs — prefer a bounded variable-length match
with a small `*..N` cap.

## Edge cases

### Disconnected nodes

If no path connects `a` and `b`, the `MATCH` emits zero rows. A
following [`RETURN path.length(p)`](#path-functions) never runs. Wrap with
[`OPTIONAL MATCH`](./match#optional-match) if you still want a row:

<QueryCodeBlock code={String.raw`MATCH (a:User {id: $from}), (b:User {id: $to})
OPTIONAL MATCH p = shortestPath((a)-[:FOLLOWS*]->(b))
RETURN a, b, path.length(p) AS hops    // hops = null if unreachable`} />

### Self-loops

`(a)-[:R*1..]->(a)` matches only if there's a cycle back to `a`.
`(a)-[:R*0..]->(a)` always matches `a` via the zero-hop interpretation.

### Performance

Unbounded variable-length traversals can be expensive. Bound with a
maximum depth whenever the answer is "and not further":

<QueryCodeBlock code={String.raw`// Good — bounded
MATCH (a)-[:KNOWS*1..6]-(b) …

// Risky on large graphs — unbounded
MATCH (a)-[:KNOWS*]-(b) …`} />

### Zero-hop semantics

`[:R*0..N]` includes the start node itself as a valid `b` via a
zero-hop "path". `path.nodes(p)` has one element, `path.edges(p)` is
empty. Useful when the answer may be "myself plus my neighbors within
N".

## What's not supported

- **Weighted shortest paths.** `shortestPath` treats every relationship
  as cost 1 — no cost argument, no Dijkstra-style weighting. Compute
  weighted paths in host code (or via `reduce` over `path.edges(p)`).
- **Quantified path patterns** (`((:X)-[:R]->(:Y)){1,3}`) — not in
  the grammar.
- **Path utility procedures** — no `CALL` surface.
- **Inline `WHERE` inside `*` patterns** — parsed but not evaluated.
  Move the predicate into a standalone `WHERE` using `path.nodes(p)` /
  `path.edges(p)`.

See [Limitations](../limitations) for the full list.

## See also

- [**MATCH**](./match) — underlying pattern language.
- [**WHERE**](./where) — path predicates with `all` / `any` / `EXISTS`.
- [**RETURN / WITH**](./return-with) — project path contents.
- [**List Functions → Pattern comprehension**](../functions/list#pattern-comprehension).
- [**Functions → Entity introspection**](../functions/overview#entity-introspection) — `id`, `labels`, `type`.
- [**Graph Model → Relationship semantics**](../concepts/graph-model#relationship-semantics).
