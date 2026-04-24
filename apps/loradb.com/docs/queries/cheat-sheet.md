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
WITH *, toLower(u.handle) AS key       -- pass-through plus computed
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
  ON CREATE SET u.created = timestamp()
  ON MATCH  SET u.last_seen = timestamp()

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
RETURN length(p), nodes(p), relationships(p)
```

More: [Paths](./paths).

## Strings

```cypher
toLower(s), toUpper(s)
trim(s), ltrim(s), rtrim(s)
substring(s, 0, 3), left(s, 2), right(s, 2)
replace(s, 'a', 'b'), reverse(s)
split(s, ','), size(s)
lpad(s, 10, '0'), rpad(s, 10, ' ')
```

More: [String functions](../functions/string).

## Math

```cypher
abs(x), ceil(x), floor(x), round(x)
sqrt(x), sign(x)
log(x), log10(x), exp(x)
sin(x), cos(x), tan(x), atan2(y, x)
radians(x), degrees(x)
pi(), e(), rand()
```

More: [Math functions](../functions/math).

## Lists

```cypher
size(xs), head(xs), tail(xs), last(xs)
reverse(xs), range(1, 10), range(1, 10, 2)
xs[0], xs[-1], xs[..3], xs[2..5]
[x IN xs WHERE x > 0]                  -- filter
[x IN xs | x * x]                      -- map
```

More: [List functions](../functions/list).

## Temporal

```cypher
date('2026-04-20')
time('12:00:00')
datetime('2026-04-20T12:00:00Z')
localdatetime('2026-04-20T12:00:00')
duration('P30D'), duration({days: 30})
date.truncate('month', d)
datetime.truncate('hour', dt)
duration.between(a, b)
dt.year, dt.month, dt.day, dt.hour
dt + duration('P1D')
```

More: [Temporal types](../data-types/temporal),
[Temporal functions](../functions/temporal).

## Spatial

```cypher
point({x: 1, y: 2})                                   -- Cartesian 2D
point({x: 1, y: 2, z: 3})                             -- Cartesian 3D
point({latitude: 52.37, longitude: 4.89})             -- WGS-84 2D
point({latitude: 52.37, longitude: 4.89, height: 5})  -- WGS-84 3D
distance(a, b)                                        -- same SRID only
p.x, p.y, p.latitude, p.longitude, p.z, p.height
```

More: [Spatial types](../data-types/spatial),
[Spatial functions](../functions/spatial).

## Type checks and conversions

```cypher
valueType(x)                           -- 'INTEGER', 'STRING', 'NODE', …
toInteger(s), toFloat(s), toString(n), toBoolean(s)
coalesce(a, b, c)
```

More: [Type functions](../functions/overview#type-conversion-and-checking).

## Entity introspection

```cypher
id(n), id(r)
labels(n), type(r)
keys(n), properties(n)
nodes(p), relationships(p), length(p)
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
