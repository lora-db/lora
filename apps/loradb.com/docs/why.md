---
title: Why LoraDB
sidebar_label: Why LoraDB
description: Why an embedded, Rust-native graph database with a Cypher-like engine — built for systems that reason over connected, evolving context.
---

# Why LoraDB

Most systems end up modeling relationships. Users follow users, agents
reference entities, scenes contain objects, events depend on other
events. When the questions you care about are _"everything reachable
from here"_ or _"what connects these two"_, the shape of your data is
a graph — even when the store is a table.

LoraDB is a small, in-process graph database that treats relationships
as first-class and speaks a pragmatic subset of Cypher. It's built for
code paths where that model needs to live next to the code that uses
it.

## The problem with traditional approaches

### Relational stores fight relational questions

SQL expresses relationships with foreign keys and join tables. That
works, until the query is "find everyone within three hops", "what's
the shortest dependency chain", or "summarise the neighbourhood of
this node". Expressing those in SQL means self-joins stacked on
self-joins; running them means the planner guessing how to traverse a
graph it doesn't know is a graph.

### Document stores fight evolving relationships

Nesting works when ownership is strict and fan-out is small. Real
systems have bidirectional edges, many-to-many links, and entities
that migrate between documents. At that point every lookup becomes an
ad-hoc index or a second round-trip, and consistency moves into
application code.

### Graph platforms are often disproportionate

Neo4j, Memgraph, and friends solve real problems — clustering,
durability, multi-user isolation, huge graphs. They also come with a
service to deploy, a protocol to speak, and a TCO that only pays off
once your graph is big enough. For a service, agent, or pipeline that
wants a graph _data structure_ inside its own process, they're too
much.

## Why relationships matter

A graph model turns three things into first-class primitives:

- **Edges** — typed, directed, property-bearing. Not a join column.
- **Patterns** — `(a)-[:KNOWS]->(b)-[:WORKS_AT]->(c)` is the query,
  not an implementation detail.
- **Traversal** — "walk from here" is `O(degree)`, not a recursive
  CTE.

Cypher is a language optimised for _describing_ relationships rather
than _computing_ joins. Once the model matches the question, the
queries get short, and short queries are easier to read, review, and
optimise.

## Why modern systems need this

Agents, robots, and real-time pipelines all end up building the same
thing by accident: an in-memory structure of entities and relations
with typed keys and evolving shape. LoraDB is that structure, on
purpose.

### AI agents and LLM pipelines

An agent's context is a graph — tools, entities, observations,
decisions, and the links between them. Retrieval over that context is
a graph question:

```cypher
MATCH (t:Task {id: $task})-[:DEPENDS_ON*1..3]->(e:Entity)
OPTIONAL MATCH (e)-[:OBSERVED_IN]->(s:Session)
RETURN e.id, e.summary, collect(DISTINCT s.id) AS sessions
```

Vectors are great for similarity. Graphs are great for _structure_ —
what depends on what, what's been tried, what contradicts what. Most
agent systems end up needing both.

### Context memory and retrieval

Memory that's only a list of chunks loses the relationships between
them. A graph keeps them: which fact supports which claim, which
document cites which, which entity was introduced by which turn. When
the model asks _"why do we believe X"_, that's a traversal.

### Robotics and stateful systems

Scene graphs, task graphs, capability graphs — robotics is full of
them. LoraDB runs in the controller's process, so scene updates and
plan queries don't cross a network. Schemas can evolve as the robot
learns new object categories without a migration.

### Event-driven architectures

Stream processors resolve entities, infer relationships, and emit
enrichments. Doing that in memory next to the handler avoids a hot
round-trip per event. Cypher makes the rules readable:

```cypher
MATCH (u:User {id: $user})-[:PLACED]->(o:Order)-[:CONTAINS]->(p:Product)
WHERE o.placed_at >= datetime() - duration('P7D')
RETURN p.category, count(*) AS recent
```

### Real-time reasoning over relationships

Anywhere a decision has to look across multiple entities and their
links — fraud signals, access control, lineage, recommendations — a
graph query says what you mean in one expression.

## What LoraDB enables

### Model relationships naturally

Labels and types are first-class. The model is what you read in the
query:

```cypher
CREATE (ada:Person {name: 'Ada'})
CREATE (grace:Person {name: 'Grace'})
CREATE (ada)-[:INFLUENCED {year: 1843}]->(grace)
```

No join table, no enum column, no convention to remember.

### Compose queries for reasoning and analysis

<CypherCode code="WITH" /> pipes one stage's output into the next, so
non-trivial questions read top-to-bottom:

```cypher
MATCH (u:User)-[:PLACED]->(o:Order {status: 'paid'})
WITH u, count(o) AS orders
WHERE orders > 1
RETURN u.email, orders ORDER BY orders DESC
```

Filter, aggregate, filter again — one expression, no temp tables.

### Keep evolving context cheap

LoraDB is schema-free. Adding a new label, a new edge type, or a new
property means _writing_ it — there's no migration, no `ALTER`, no
restart. That fits systems that keep discovering new categories of
thing.

### Run in-process

Opening a database is a function call:

```rust
let mut db = Database::new();
db.execute("CREATE (:Person {name: 'Ada'})", None)?;
```

Same story in [Node](./getting-started/node),
[Python](./getting-started/python), and
[WASM](./getting-started/wasm). There's also an
[HTTP server](./getting-started/server) when you'd rather reach it
over the wire.

### Stay small enough to understand

A small Rust codebase — parser, analyzer, compiler, optimizer,
executor, storage — readable end-to-end. If the database matters to
your product, you should be able to read it. That's a deliberate
constraint, not a marketing line.

## Positioned against the alternatives

| | LoraDB | Managed graph DB | SQL + recursive CTEs | Document store |
|---|---|---|---|---|
| Deployment | A crate / binding | A service | Existing DB | Existing DB |
| Relationship model | First-class | First-class | Join tables | Nested or foreign-keyed |
| Query language | Cypher subset | Full Cypher / GQL | SQL | Proprietary |
| Schema | Free | Typed / free | Strict | Free |
| Latency to query | Function call | Network hop | Network hop | Network hop |
| Scale | Single process | Horizontal | Horizontal | Horizontal |
| Fit for embedded / agent loops | Direct | Indirect | Indirect | Indirect |

LoraDB is **not** a replacement for a full graph platform when you
need durability, multi-tenant isolation, or horizontal scale. It's
the option that was missing in the other direction — the one you
reach for when the graph belongs _inside_ your process.

## Concrete scenarios

- **Agent working memory.** An LLM agent stores tools, observations,
  and entity references as nodes, with typed edges between them. The
  retrieval step is a `MATCH` pattern, not a similarity score.
- **Robot scene graph.** Objects, rooms, and affordances as nodes;
  "is-on", "can-grasp", "observed-from" as edges. Plan queries run in
  the controller.
- **Streaming entity resolution.** Events flow in; LoraDB holds the
  current graph of resolved entities; enrichers consult it via
  Cypher and emit back.
- **Permission inference.** Users, groups, resources, and grants as a
  graph. "Can user U read resource R?" is a reachability query, not
  a recursive SQL.
- **Lineage and dependency analysis.** Pipelines, models, datasets —
  walk upstream or downstream in one query, not one join per hop.

## Where it's going

Near-term direction, not promises:

- **Persistence** — optional durable storage behind the same API.
- **Indexing** — hash and range indexes on property keys.
- **Query planner improvements** — better join ordering and cost
  estimates.
- **Richer Cypher surface** — procedures,
  <CypherCode code="UNION" />, list comprehensions.

See [**Limitations**](./limitations) for what isn't supported yet,
and the [**docs**](./) for what works today.

## See also

- [**What is LoraDB**](./) — introduction, audiences, and quick start.
- [**Installation**](./getting-started/installation) — pick your
  platform and get running.
- [**Ten-Minute Tour**](./getting-started/tutorial) — the case from
  "why" to a working query.
- [**Graph Model**](./concepts/graph-model) — the data model in
  four queries.
- [**Queries → Overview**](./queries/) — the Cypher surface LoraDB
  supports.
- [**Cookbook**](./cookbook) — scenario-driven recipes.
- [**Limitations**](./limitations) — what isn't supported yet.
