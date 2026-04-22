---
title: Cypher Query Examples for LoraDB
sidebar_label: Examples
---

# Cypher Query Examples for LoraDB

A copy-paste tour of LoraDB's Cypher-like syntax, organised by shape.
Each section is a standalone recipe — read top-to-bottom to pick up the
language, or jump straight to what you need.

> Working through this for the first time? Try the guided version at
> [**A Ten-Minute Tour**](../getting-started/tutorial) first.

## On this page

- [Creating data](#creating-data)
- [Pattern matching](#pattern-matching)
- [Filtering with WHERE](#filtering-with-where)
- [Optional patterns](#optional-patterns)
- [Aggregation](#aggregation)
- [Parameters](#parameters)
- [Updating and deleting](#updating-and-deleting)
- [CASE expressions](#case-expressions)
- [Escaping with backticks](#escaping-with-backticks)
- [Common patterns](#common-patterns)
- [Realistic shapes](#realistic-shapes) — fully worked domain examples

## Creating data

Before you can query a graph, you need one. [`CREATE`](./create) is the
simplest way to write [nodes](../concepts/nodes) and
[relationships](../concepts/relationships); relationships can only
connect existing nodes, so we bind the endpoints with their labels and
[properties](../concepts/properties) first.

```cypher
// Seed a tiny social graph
CREATE (ada:Person   {name: 'Ada',   born: 1815})
CREATE (grace:Person {name: 'Grace', born: 1906})
CREATE (alan:Person  {name: 'Alan',  born: 1912})
CREATE (ada)-[:INFLUENCED {decade: 1840}]->(alan)
CREATE (grace)-[:KNOWS]->(alan)
```

Three `Person` nodes and two relationships. The `INFLUENCED` edge
carries its own property (`decade`). `Person`, `INFLUENCED`, and `KNOWS`
are created implicitly on first use — LoraDB has no separate
`CREATE TABLE` step.

## Pattern matching

[`MATCH`](./match) finds every way to satisfy a pattern. One row per
match. The pattern `(p:Person)-[:KNOWS]->(other:Person)` reads as "a
Person `p` with an outgoing `KNOWS` edge to another Person `other`."

```cypher
MATCH (p:Person)-[:KNOWS]->(other:Person)
RETURN p.name AS from, other.name AS to
```

Returns one row (`Grace → Alan`) in our seed graph. `AS from` and
`AS to` rename the projected columns — useful when a consumer expects
specific field names.

### Multi-hop

```cypher
MATCH (a:Person)-[:INFLUENCED]->(b)-[:KNOWS]->(c)
RETURN a.name, b.name, c.name
```

### Either direction

```cypher
MATCH (a:Person)-[:KNOWS]-(b)
RETURN a.name, b.name
```

The undirected dash matches both `a -> b` and `a <- b` — see
[Match → Relationship patterns](./match#relationship-patterns).

## Filtering with WHERE

[`WHERE`](./where) runs after `MATCH` and can reference anything the
match bound. [String operators](../functions/string#string-operators-in-where)
like `STARTS WITH` and `CONTAINS` are case-sensitive — pass through
[`toLower`](../functions/string#tolower--toupper) / `toUpper` for
case-insensitive checks.

```cypher
MATCH (p:Person)
WHERE p.born < 1900 AND p.name STARTS WITH 'A'
RETURN p
ORDER BY p.born ASC
LIMIT 10
```

In our seed graph this finds Ada (born 1815), filters by name prefix,
and returns up to 10 matches sorted oldest-first.
[`ORDER BY` and `LIMIT`](./ordering) always run **after** projection.

### IN list membership

```cypher
MATCH (p:Person)
WHERE p.born IN [1815, 1906, 1912]
RETURN p.name, p.born
```

### NOT EXISTS (anti-join)

```cypher
MATCH (p:Person)
WHERE NOT EXISTS { (p)-[:INFLUENCED]->() }
RETURN p.name AS uninfluential_person
```

## Optional patterns

[`OPTIONAL MATCH`](./match#optional-match) is the graph equivalent of a
left-join — if the pattern doesn't match, bound variables are set to
`null` rather than dropping the row.

```cypher
MATCH (p:Person {name: 'Ada'})
OPTIONAL MATCH (p)-[:INFLUENCED]->(target)
RETURN p.name, target.name
```

Useful when you want "every person, plus what they influenced if
anything": people without `INFLUENCED` edges still appear, with
`target.name` as `null`.

## Aggregation

Any non-aggregated column becomes an
[implicit group key](./aggregation#grouping). Here, `p.name` groups rows
per person; [`count(friend)`](../functions/aggregation#count) is the
group size.

```cypher
MATCH (p:Person)-[r:KNOWS]->(friend)
RETURN p.name AS person, count(friend) AS friends
ORDER BY friends DESC
```

> **Why `count(friend)` not `count(*)`?** `count(*)` counts rows. If
> `p` never matched, there'd be no row to count. `count(friend)` counts
> non-null bindings — the distinction matters once you start mixing
> [`OPTIONAL MATCH`](./match#optional-match) with aggregation.

### Collect into a list

```cypher
MATCH (p:Person)-[:INFLUENCED]->(target:Person)
RETURN p.name AS influencer,
       collect(target.name) AS influenced
```

### Multiple aggregates

```cypher
MATCH (p:Person)
RETURN count(*)   AS people,
       min(p.born) AS earliest,
       max(p.born) AS latest,
       avg(p.born) AS mean_year
```

## Parameters

[Parameters](./#parameters) are the only way to safely mix untrusted
input into a query. Unbound parameters resolve to `null`, which usually
filters everything out — worth validating on the host side before you
call `execute`.

```cypher
MATCH (p:Person {name: $name})
WHERE p.born >= $minYear
RETURN p
```

A variable that happens to be the same name as a parameter doesn't
collide — `$name` always refers to the bound value, `p.name` always to
the property.

### List parameters

```cypher
MATCH (p:Person)
WHERE p.born IN $years
RETURN p
```

### Map parameters (patch update)

```cypher
MATCH (p:Person {name: $name})
SET p += $patch
RETURN p
```

## Updating and deleting

[`SET`](./set-delete#set--properties) updates properties. `SET n.a = null`
effectively removes the property. `SET n = {...}` replaces the full
property [map](../data-types/lists-and-maps#maps), which is almost never
what you want — use `SET n += {...}` to merge.

```cypher
MATCH (p:Person {name: 'Ada'})
SET p.born = 1815, p.field = 'Mathematics'
RETURN p
```

Deleting a node with relationships fails unless you use
[`DETACH DELETE`](./set-delete#detach-delete), which removes the edges
first:

```cypher
MATCH (p:Person {name: 'Alan'})
DETACH DELETE p
```

Once Alan is gone, so is the `Grace -> Alan` `KNOWS` edge.

## CASE expressions

[`CASE`](./return-with#case-expressions) is LoraDB's conditional
expression — the ternary / switch of Cypher. Two forms.

### Simple form — match on a value

```cypher
MATCH (o:Order)
RETURN o.id,
       CASE o.status
         WHEN 'paid'      THEN 'counted'
         WHEN 'cancelled' THEN 'refunded'
         WHEN 'pending'   THEN 'waiting'
         ELSE                  'unknown'
       END AS state
```

### Generic form — boolean per branch

```cypher
MATCH (p:Product)
RETURN p.name,
       CASE
         WHEN p.stock =  0 THEN 'out'
         WHEN p.stock < 10 THEN 'low'
         ELSE                   'ok'
       END AS availability
```

### Conditional count (CASE inside count)

`count(expr)` skips `null`, so a `CASE` with no `ELSE` is a clean way
to express "count the rows that match this condition":

```cypher
MATCH (r:Review)
RETURN r.product,
       count(CASE WHEN r.stars >= 4 THEN 1 END) AS positive,
       count(CASE WHEN r.stars <= 2 THEN 1 END) AS negative,
       count(*)                                 AS total
```

### Custom sort order

```cypher
MATCH (t:Task)
RETURN t.title
ORDER BY CASE t.priority
           WHEN 'p0' THEN 0
           WHEN 'p1' THEN 1
           WHEN 'p2' THEN 2
           ELSE           3
         END
```

Natural string order would give you `p0`, `p1`, `p2` by accident here
— but the moment you introduce `urgent` or `low`, `CASE` is the only
way to keep the order semantically meaningful.

### In SET (compute-then-assign)

```cypher
MATCH (u:User)
SET u.tier = CASE
               WHEN u.score >= 1000 THEN 'platinum'
               WHEN u.score >=  100 THEN 'gold'
               ELSE                       'bronze'
             END
```

See [RETURN → CASE](./return-with#case-expressions) for the full
reference.

## Escaping with backticks

Identifiers that clash with keywords or contain special characters can
be wrapped in backticks. Useful if you're importing data from a system
that doesn't share Cypher's identifier rules.

```cypher
MATCH (`first person`:Person)
RETURN `first person`.name
```

## Common patterns

### Count nodes by label

```cypher
MATCH (n:Person)
RETURN count(*) AS people
```

### Group by a property

```cypher
MATCH (p:Person)
RETURN p.born / 100 * 100 AS century, count(*) AS n
ORDER BY century
```

Divide-then-multiply truncates to the century. One row per century.

### Distinct values

```cypher
MATCH (p:Person)
RETURN DISTINCT p.born
ORDER BY p.born
```

### Filter before aggregating

```cypher
MATCH (p:Person)
WHERE p.born >= 1900
RETURN count(*) AS modern_people
```

### Filter after aggregating (HAVING-style)

Cypher has no `HAVING`. Pipe through [`WITH`](./return-with#with), then
filter:

```cypher
MATCH (p:Person)-[:KNOWS]->(friend)
WITH p.name AS person, count(friend) AS friends
WHERE friends >= 2
RETURN person, friends
```

### Top-N

```cypher
MATCH (p:Person)-[:KNOWS]->(friend)
RETURN p.name AS person, count(friend) AS friends
ORDER BY friends DESC
LIMIT 5
```

### Upsert (create-or-match)

[`MERGE`](./unwind-merge#merge) finds the pattern or creates it —
useful to avoid accidental duplicates.

```cypher
MERGE (u:User {id: $id})
  ON MATCH  SET u.last_seen = timestamp()
  ON CREATE SET u.created   = timestamp()
RETURN u
```

### Bulk load via UNWIND

```cypher
UNWIND $rows AS row
CREATE (:Event {id: row.id, at: datetime(row.at), kind: row.kind})
```

One row per element of the `$rows` parameter list — see
[`UNWIND`](./unwind-merge#unwind). The idiomatic way to import hundreds
or thousands of records in a single query.

### Shortest path between two nodes

```cypher
MATCH p = shortestPath(
  (a:Station {name: $from})-[:ROUTE*]->(b:Station {name: $to})
)
RETURN length(p) AS hops, [n IN nodes(p) | n.name] AS via
```

---

## Realistic shapes

A few fully-worked domain examples to stretch the patterns above into
something that looks like a real application query.

### Users and posts (social)

```cypher
// 10 most-read posts this week, each with author
MATCH (u:User)-[:WROTE]->(p:Post)
WHERE p.published_at >= datetime() - duration('P7D')
RETURN p.title   AS title,
       p.views   AS views,
       u.handle  AS author
ORDER BY views DESC
LIMIT 10
```

```cypher
// Users who haven't posted in 30 days
MATCH (u:User)
WHERE NOT EXISTS {
  (u)-[:WROTE]->(:Post)
  WHERE Post.published_at >= datetime() - duration('P30D')
}
RETURN u.handle
```

### Orders and items (e-commerce)

```cypher
// Revenue per category, only where > $1k
MATCH (o:Order)-[:CONTAINS]->(i:Item)-[:IN]->(c:Category)
WHERE o.status = 'paid'
WITH c.name AS category, sum(i.price * i.quantity) AS revenue
WHERE revenue > 1000
RETURN category, revenue
ORDER BY revenue DESC
```

```cypher
// Repeat buyers
MATCH (u:User)-[:PLACED]->(o:Order {status: 'paid'})
WITH u, count(o) AS orders
WHERE orders > 1
RETURN u.email, orders
ORDER BY orders DESC
```

### People and companies

```cypher
// Colleagues: same company, different people
MATCH (a:Person)-[:WORKS_AT]->(c:Company)<-[:WORKS_AT]-(b:Person)
WHERE id(a) < id(b)
RETURN c.name, a.name, b.name
```

### Events and time buckets

```cypher
// Events per month for the past year
MATCH (e:Event)
WHERE e.at >= datetime() - duration('P1Y')
RETURN date.truncate('month', e.at) AS month,
       count(*)                      AS events
ORDER BY month
```

### Locations and distance

```cypher
// Five cities closest to Amsterdam, with metres
MATCH (ams:City {name: 'Amsterdam'}), (other:City)
WHERE other.name <> 'Amsterdam'
RETURN other.name,
       distance(ams.location, other.location) AS metres
ORDER BY metres ASC
LIMIT 5
```

### Tag clouds

```cypher
// Top 20 tags across posts
MATCH (p:Post)-[:TAGGED]->(t:Tag)
RETURN t.name, count(p) AS uses
ORDER BY uses DESC
LIMIT 20
```

### Graph walk (recommendations)

```cypher
// "People you might know" — second-degree connections
MATCH (me:User {id: $id})-[:FOLLOWS]->(:User)-[:FOLLOWS]->(candidate:User)
WHERE candidate <> me
  AND NOT EXISTS { (me)-[:FOLLOWS]->(candidate) }
RETURN candidate.handle, count(*) AS shared_paths
ORDER BY shared_paths DESC
LIMIT 10
```

## See also

- [**Queries → Overview**](../queries) — clause-by-clause reference.
- [**Cheat sheet**](./cheat-sheet) — single-page quick reference.
- [**Tutorial**](../getting-started/tutorial) — same language, guided
  top-to-bottom.
- [**Cookbook**](../cookbook) — scenario-driven recipes by domain
  (social, e-commerce, events, geo).
- [**Parameters**](./parameters) — typed parameter binding (the `$id`
  used above).
- [**Functions**](../functions/overview) — every built-in.
- [**Concepts → Graph Model**](../concepts/graph-model) — the data
  model these queries run against.
