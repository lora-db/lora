---
title: Cypher Cheat Sheet
sidebar_label: Cheat Sheet
description: A one-page reference for the Cypher features LoraDB supports — read, write, path, aggregation, and expression forms — each with a minimal working example.
---

# Cypher Cheat Sheet

A one-page reference for the Cypher features LoraDB supports. For
scenario-driven recipes, see the [**Cookbook**](../cookbook); for
clause-by-clause detail, follow the links below.

## Read

```cypher
MATCH (n)                           -- every node
MATCH (n:Person)                    -- label filter
MATCH (n:Person {name: 'Ada'})      -- label + property
MATCH (a)-[:KNOWS]->(b)             -- outgoing relationship
MATCH (a)-[:KNOWS]-(b)              -- either direction (MATCH only)
MATCH (a)-[r:KNOWS|LIKES]->(b)      -- any of several types
MATCH p = (a)-[:R*1..3]->(b)        -- variable-length; bind path
OPTIONAL MATCH (u)-[:WROTE]->(p)    -- left-join shape
```

More: [MATCH](./match), [Paths](./paths).

## Filter

```cypher
WHERE p.born >= 1900 AND p.born < 1950
WHERE p.email IS NOT NULL
WHERE p.name STARTS WITH 'A'
WHERE p.name CONTAINS 'da'
WHERE p.name =~ '.*son$'
WHERE EXISTS { (p)-[:FOLLOWS]->() }
WHERE NOT EXISTS { (p)-[:BANNED]->() }
WHERE p.tier IN ['gold', 'platinum']
```

More: [WHERE](./where).

## Indexes

```cypher
CREATE INDEX user_email FOR (u:User) ON (u.email)
CREATE TEXT INDEX user_name FOR (u:User) ON (u.name)
CREATE POINT INDEX place_location FOR (p:Place) ON (p.location)
CREATE VECTOR INDEX doc_embedding FOR (d:Doc) ON (d.embedding)
  OPTIONS {indexConfig: {`vector.dimensions`: 1536, `vector.similarity_function`: 'cosine'}}
CREATE FULLTEXT INDEX article_search FOR (a:Article) ON EACH [a.title, a.body]
CREATE INDEX rel_since FOR ()-[r:FOLLOWS]-() ON (r.since)
CALL db.index.vector.queryNodes('doc_embedding', 5, $query) YIELD node, score
CALL db.index.fulltext.queryNodes('article_search', 'graph') YIELD node, score
SHOW INDEXES
DROP INDEX user_email IF EXISTS
```

More: [Indexes](./indexes).

## Constraints

```cypher
CREATE CONSTRAINT user_email FOR (u:User) REQUIRE u.email IS UNIQUE
CREATE CONSTRAINT author_name FOR (a:Author) REQUIRE a.name IS NOT NULL
CREATE CONSTRAINT actor_name FOR (a:Actor) REQUIRE (a.first, a.last) IS NODE KEY
CREATE CONSTRAINT owns_id FOR ()-[o:OWNS]-() REQUIRE o.ownershipId IS RELATIONSHIP KEY
CREATE CONSTRAINT doc_embedding FOR (d:Doc) REQUIRE d.embedding IS :: VECTOR<FLOAT32>(1536)
SHOW CONSTRAINTS
DROP CONSTRAINT user_email IF EXISTS
```

More: [Constraints](./constraints).

## Project

```cypher
RETURN p.name, p.born
RETURN p.name AS name, 2026 - p.born AS age
RETURN DISTINCT c.country
RETURN p { .name, .born }              -- map projection
RETURN p { .*, extra: p.handle }       -- map projection with override
```

More: [RETURN / WITH](./return-with),
[map projection](../data-types/lists-and-maps#map-projection).

## Pipe and group

```cypher
WITH u, count(p) AS posts              -- group + pipe forward
WITH u WHERE u.active                  -- HAVING-style filter
WITH *, string.lower(u.handle) AS key       -- pass-through plus computed
```

More: [WITH](./return-with#with).

## Sort and paginate

```cypher
ORDER BY n.name ASC
ORDER BY count(*) DESC
ORDER BY coalesce(p.rank, 9999) ASC
SKIP 20 LIMIT 10
```

More: [ORDER BY / SKIP / LIMIT](./ordering).

## Write

```cypher
CREATE (:Person {name: 'Ada', born: 1815})
CREATE (a)-[:FOLLOWS {since: 2020}]->(b)

MERGE (u:User {email: $email})
  ON CREATE SET u.created = temporal.timestamp()
  ON MATCH  SET u.last_seen = temporal.timestamp()

SET n.prop = value                     -- one key
SET n += {a: 1, b: 2}                  -- merge into map
SET n =  {id: 1, tier: 'gold'}         -- replace all properties
SET n:Admin                            -- add a label

REMOVE n.prop                          -- drop a property
REMOVE n:Admin                         -- drop a label

DELETE r                               -- a relationship
DETACH DELETE n                        -- node + its edges
```

More: [CREATE](./create), [MERGE / UNWIND](./unwind-merge),
[SET / REMOVE / DELETE](./set-delete).

## Iterate and combine

```cypher
UNWIND $rows AS row
CREATE (:User {id: row.id, name: row.name})

UNWIND [1, 2, 3] AS n RETURN n * n AS square

MATCH (u:User) RETURN u.name
UNION
MATCH (a:Admin) RETURN a.name
```

More: [UNWIND + MERGE](./unwind-merge),
[UNION](./return-with#union--union-all).

## Aggregate

```cypher
count(*)                               -- rows
count(x)                               -- rows where x is not null
count(DISTINCT x)
sum(x), avg(x), min(x), max(x)
collect(x), collect(DISTINCT x)
stdev(x), stdevp(x)
percentileCont(x, 0.95)
percentileDisc(x, 0.5)
```

More: [Aggregation (queries)](./aggregation),
[Aggregation (functions)](../functions/aggregation).

## Conditionals

```cypher
CASE WHEN x >= 50 THEN 'ok' ELSE 'low' END
CASE status WHEN 'paid' THEN 1 ELSE 0 END
count(CASE WHEN status = 'paid' THEN 1 END)   -- count-if
```

More: [CASE expressions](./return-with#case-expressions).

## Paths

```cypher
MATCH p = (a)-[:R*1..3]->(b)           -- 1 to 3 hops
MATCH p = shortestPath((a)-[:R*]->(b))
RETURN path.length(p), path.nodes(p), path.edges(p)
```

More: [Paths](./paths).

## Strings

```cypher
string.lower(s), string.upper(s)
string.trim(s), string.trim_left(s), string.trim_right(s)
string.slice(s, 0, 3), string.prefix(s, 2), string.suffix(s, 2)
string.replace(s, 'a', 'b'), value.reverse(s)
string.split(s, ','), value.size(s)
string.pad_left(s, 10, '0'), string.pad_right(s, 10, ' ')
```

More: [String functions](../functions/string).

## Math

```cypher
math.abs(x), math.ceil(x), math.floor(x), math.round(x)
math.sqrt(x), math.sign(x)
math.log(x), math.log10(x), math.exp(x)
math.sin(x), math.cos(x), math.tan(x), math.atan2(y, x)
math.radians(x), math.degrees(x)
math.pi(), math.e(), math.random()
```

More: [Math functions](../functions/math).

## Lists

```cypher
value.size(xs), list.first(xs), list.rest(xs), list.last(xs)
value.reverse(xs), list.range(1, 10), list.range(1, 10, 2)
xs[0], xs[-1], xs[..3], xs[2..5]
[x IN xs WHERE x > 0]                  -- filter
[x IN xs | x * x]                      -- map
```

More: [List functions](../functions/list).

## Temporal

```cypher
'2026-04-20'::DATE
'12:00:00'::TIME
'2026-04-20T12:00:00Z'::DATETIME
'2026-04-20T12:00:00'::LOCAL_DATETIME
'P30D'::DURATION, {days: 30}::DURATION
temporal.truncate('month', d)
temporal.truncate('hour', dt)
temporal.between(a, b)
dt.year, dt.month, dt.day, dt.hour
dt + 'P1D'::DURATION
```

More: [Temporal types](../data-types/temporal),
[Temporal functions](../functions/temporal).

## Spatial

```cypher
{x: 1, y: 2}::POINT                                   -- Cartesian 2D
{x: 1, y: 2, z: 3}::POINT                             -- Cartesian 3D
{latitude: 52.37, longitude: 4.89}::POINT             -- WGS-84 2D
{latitude: 52.37, longitude: 4.89, height: 5}::POINT  -- WGS-84 3D
geo.distance(a, b)                                        -- same SRID only
p.x, p.y, p.latitude, p.longitude, p.z, p.height
```

More: [Spatial types](../data-types/spatial),
[Spatial functions](../functions/spatial).

## Type checks and conversions

```cypher
type.of(x)                           -- 'INTEGER', 'STRING', 'NODE', …
toInteger(s), toFloat(s), toString(n), toBoolean(s)
coalesce(a, b, c)
```

More: [Type functions](../functions/overview#type-conversion-and-checking).

## Entity introspection

```cypher
id(n), id(r)
labels(n), type(r)
keys(n), properties(n)
path.nodes(p), path.edges(p), path.length(p)
```

More: [Entity / path functions](../functions/overview#entity-introspection).

## Parameters

```cypher
MATCH (u:User {id: $id}) RETURN u
UNWIND $rows AS row CREATE (:User {id: row.id})
```

Bind from the host — see [Parameters](./parameters).

## Conventions

| Category | Convention | Example |
|---|---|---|
| Labels | `PascalCase` | `:Person`, `:OrderItem` |
| Relationship types | `UPPER_SNAKE` | `:FOLLOWS`, `:WORKS_AT` |
| Property keys | `snake_case` or `camelCase` (pick one) | `created_at`, `createdAt` |
| Variables | Lowercase, short | `u`, `p`, `edge` |

## See also

- [**Queries → Overview**](./) — clauses and pipeline.
- [**Cookbook**](../cookbook) — scenario-based recipes.
- [**Query Examples**](./examples) — clause-by-clause recipes.
- [**Limitations**](../limitations) — what's intentionally not here.
- [**Troubleshooting**](../troubleshooting) — common errors.
