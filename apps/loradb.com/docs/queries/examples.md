---
title: Cypher Query Examples for LoraDB
sidebar_label: Examples
description: Copy-paste Cypher examples that run against one seeded LoraDB playground graph.
---

# Cypher Query Examples for LoraDB

These examples are designed for the playground. Start with a fresh database,
run the seed query once, then run any example on this page. The snippets avoid
host-side parameters so they can execute directly in the browser.

> Working through this for the first time? Try the guided version at
> [**A Ten-Minute Tour**](../getting-started/tutorial) first.

## On this page

- [Seed graph](#seed-graph)
- [Pattern matching](#pattern-matching)
- [Filtering with WHERE](#filtering-with-where)
- [Optional patterns](#optional-patterns)
- [Aggregation](#aggregation)
- [Parameter-shaped filters](#parameter-shaped-filters)
- [Updating and deleting](#updating-and-deleting)
- [CASE expressions](#case-expressions)
- [Common patterns](#common-patterns)
- [Domain shapes](#domain-shapes)

## Seed graph

Run this once in an empty playground database. It creates people, companies,
projects, tasks, stations, movies, and relationships between them.

<QueryCodeBlock code={String.raw`CREATE (alice:Person:User {id: 1, name: 'Alice', handle: 'alice', born: 1989, city: 'London', score: 1200, tier: 'bronze'})
CREATE (bob:Person:User {id: 2, name: 'Bob', handle: 'bob', born: 1994, city: 'Berlin', score: 240, tier: 'bronze'})
CREATE (carol:Person:User {id: 3, name: 'Carol', handle: 'carol', born: 1982, city: 'London', score: 80, tier: 'bronze'})
CREATE (dave:Person:User {id: 4, name: 'Dave', handle: 'dave', born: 1996, city: 'Paris', score: 30, tier: 'bronze'})
CREATE (eve:Person:User:Influencer {id: 5, name: 'Eve', handle: 'eve', born: 1991, city: 'Berlin', score: 500, tier: 'bronze'})
CREATE (frank:Person {id: 6, name: 'Frank', handle: 'frank', born: 1978, city: 'London', score: 40})
CREATE (alice)-[:KNOWS {since: 2015, strength: 5}]->(bob)
CREATE (alice)-[:KNOWS {since: 2018, strength: 8}]->(carol)
CREATE (bob)-[:KNOWS {since: 2019, strength: 4}]->(carol)
CREATE (bob)-[:KNOWS {since: 2020, strength: 3}]->(dave)
CREATE (carol)-[:KNOWS {since: 2017, strength: 6}]->(eve)
CREATE (eve)-[:KNOWS {since: 2016, strength: 7}]->(frank)
CREATE (alice)-[:FOLLOWS]->(carol)
CREATE (alice)-[:FOLLOWS]->(eve)
CREATE (bob)-[:FOLLOWS]->(alice)
CREATE (carol)-[:FOLLOWS]->(frank)
CREATE (dave)-[:FOLLOWS]->(alice)
CREATE (frank)-[:FOLLOWS]->(bob)
CREATE (music:Interest {name: 'Music'})
CREATE (travel:Interest {name: 'Travel'})
CREATE (sports:Interest {name: 'Sports'})
CREATE (alice)-[:INTERESTED_IN {level: 'high'}]->(music)
CREATE (alice)-[:INTERESTED_IN {level: 'medium'}]->(travel)
CREATE (bob)-[:INTERESTED_IN {level: 'high'}]->(sports)
CREATE (dave)-[:INTERESTED_IN {level: 'high'}]->(music)
CREATE (eve)-[:INTERESTED_IN {level: 'high'}]->(travel)
CREATE (acme:Company {name: 'Acme', founded: 2010})
CREATE (contoso:Company {name: 'Contoso', founded: 2018})
CREATE (alice)-[:WORKS_AT {since: 2018}]->(acme)
CREATE (bob)-[:WORKS_AT {since: 2020}]->(acme)
CREATE (carol)-[:WORKS_AT {since: 2015}]->(acme)
CREATE (dave)-[:WORKS_AT {since: 2021}]->(contoso)
CREATE (eve)-[:WORKS_AT {since: 2022}]->(contoso)
CREATE (alpha:Project {name: 'Alpha', budget: 100000})
CREATE (beta:Project {name: 'Beta', budget: 50000})
CREATE (alice)-[:ASSIGNED_TO {role: 'lead'}]->(alpha)
CREATE (bob)-[:ASSIGNED_TO {role: 'dev'}]->(alpha)
CREATE (carol)-[:ASSIGNED_TO {role: 'lead'}]->(beta)
CREATE (eve)-[:ASSIGNED_TO {role: 'dev'}]->(beta)
CREATE (:Task {title: 'Fix login', status: 'done', priority: 'p1'})
CREATE (:Task {title: 'Ship billing', status: 'pending', priority: 'p0'})
CREATE (:Task {title: 'Archive logs', status: 'cancelled', priority: 'p2'})
CREATE (ams:Station {name: 'Amsterdam', zone: 1})
CREATE (utrecht:Station {name: 'Utrecht', zone: 1})
CREATE (rotterdam:Station {name: 'Rotterdam', zone: 2})
CREATE (denhaag:Station {name: 'Den Haag', zone: 2})
CREATE (eindhoven:Station {name: 'Eindhoven', zone: 3})
CREATE (ams)-[:ROUTE {distance: 40, duration: 25}]->(utrecht)
CREATE (utrecht)-[:ROUTE {distance: 40, duration: 25}]->(ams)
CREATE (ams)-[:ROUTE {distance: 60, duration: 40}]->(rotterdam)
CREATE (rotterdam)-[:ROUTE {distance: 60, duration: 40}]->(ams)
CREATE (utrecht)-[:ROUTE {distance: 55, duration: 35}]->(rotterdam)
CREATE (rotterdam)-[:ROUTE {distance: 55, duration: 35}]->(utrecht)
CREATE (rotterdam)-[:ROUTE {distance: 25, duration: 15}]->(denhaag)
CREATE (denhaag)-[:ROUTE {distance: 25, duration: 15}]->(rotterdam)
CREATE (utrecht)-[:ROUTE {distance: 100, duration: 60}]->(eindhoven)
CREATE (eindhoven)-[:ROUTE {distance: 100, duration: 60}]->(utrecht)
CREATE (matrix:Movie {title: 'Matrix', year: 1999, genre: 'sci-fi'})
CREATE (inception:Movie {title: 'Inception', year: 2010, genre: 'sci-fi'})
CREATE (amelie:Movie {title: 'Amelie', year: 2001, genre: 'drama'})
CREATE (jaws:Movie {title: 'Jaws', year: 1975, genre: 'thriller'})
CREATE (alice)-[:RATED {score: 5}]->(matrix)
CREATE (alice)-[:RATED {score: 4}]->(inception)
CREATE (alice)-[:RATED {score: 3}]->(amelie)
CREATE (bob)-[:RATED {score: 5}]->(matrix)
CREATE (bob)-[:RATED {score: 2}]->(jaws)
CREATE (carol)-[:RATED {score: 4}]->(amelie)
CREATE (carol)-[:RATED {score: 5}]->(inception)
CREATE (:Scratch {name: 'temporary'})`} />

## Pattern matching

[`MATCH`](./match) finds every way to satisfy a pattern. This returns one row
for each directed `KNOWS` relationship.

<QueryCodeBlock code={String.raw`MATCH (p:Person)-[:KNOWS]->(other:Person)
RETURN p.name AS person, other.name AS knows
ORDER BY person, knows`} />

### Multi-hop

<QueryCodeBlock code={String.raw`MATCH (a:Person)-[:KNOWS]->(b:Person)-[:KNOWS]->(c:Person)
RETURN a.name AS start, b.name AS middle, c.name AS finish
ORDER BY start, finish`} />

### Either direction

<QueryCodeBlock code={String.raw`MATCH (a:Person)-[:KNOWS]-(b:Person)
RETURN a.name AS person, b.name AS connected
ORDER BY person, connected`} />

## Filtering with WHERE

[`WHERE`](./where) runs after `MATCH` and can reference anything the match
bound.

<QueryCodeBlock code={String.raw`MATCH (p:Person)
WHERE p.born < 1990 AND p.name STARTS WITH 'A'
RETURN p.name AS name, p.born AS born
ORDER BY born ASC`} />

### IN list membership

<QueryCodeBlock code={String.raw`MATCH (p:Person)
WHERE p.city IN ['London', 'Berlin']
RETURN p.name AS name, p.city AS city
ORDER BY city, name`} />

### NOT EXISTS anti-join

<QueryCodeBlock code={String.raw`MATCH (p:Person)
WHERE NOT EXISTS { (p)-[:FOLLOWS]->() }
RETURN p.name AS person_without_outgoing_follows`} />

## Optional patterns

[`OPTIONAL MATCH`](./match#optional-match) keeps the row and fills missing
bindings with `null`.

<QueryCodeBlock code={String.raw`MATCH (p:Person {name: 'Eve'})
OPTIONAL MATCH (p)-[:FOLLOWS]->(target:Person)
RETURN p.name AS person, target.name AS follows`} />

## Aggregation

Any non-aggregated column becomes an
[implicit group key](./aggregation#grouping).

<QueryCodeBlock code={String.raw`MATCH (p:Person)-[:KNOWS]->(friend:Person)
RETURN p.name AS person, count(friend) AS friends
ORDER BY friends DESC, person`} />

### Collect into a list

<QueryCodeBlock code={String.raw`MATCH (p:Person)-[:INTERESTED_IN]->(interest:Interest)
RETURN p.name AS person, collect(interest.name) AS interests
ORDER BY person`} />

### Multiple aggregates

<QueryCodeBlock code={String.raw`MATCH (p:Person)
RETURN count(*) AS people,
       min(p.born) AS earliest,
       max(p.born) AS latest,
       avg(p.born) AS mean_year`} />

## Parameter-shaped filters

The playground does not expose host-side parameter binding yet, so this page
uses literal values. In application code, replace the literals with `$name`,
`$city`, or other parameters through your binding.

<QueryCodeBlock code={String.raw`MATCH (p:Person {name: 'Alice'})
WHERE p.city = 'London'
RETURN p.name AS name, p.city AS city, p.score AS score`} />

See [Parameters](./parameters) for host API examples that bind `$name`,
`$city`, and structured values safely.

## Updating and deleting

[`SET`](./set-delete#set--properties) updates properties. `SET n += {...}`
merges a patch into the existing property map.

<QueryCodeBlock code={String.raw`MATCH (p:Person {name: 'Alice'})
SET p += {city: 'Oxford', score: 1300}
RETURN p.name AS name, p.city AS city, p.score AS score`} />

Deleting a node with relationships requires
[`DETACH DELETE`](./set-delete#detach-delete). This example deletes only the
disposable `Scratch` node from the seed data.

<QueryCodeBlock code={String.raw`MATCH (s:Scratch {name: 'temporary'})
DETACH DELETE s`} />

## CASE expressions

[`CASE`](./return-with#case-expressions) is LoraDB's conditional expression.

### Simple form

<QueryCodeBlock code={String.raw`MATCH (t:Task)
RETURN t.title AS task,
       CASE t.status
         WHEN 'done'      THEN 'counted'
         WHEN 'cancelled' THEN 'closed'
         WHEN 'pending'   THEN 'waiting'
         ELSE                  'unknown'
       END AS state
ORDER BY task`} />

### Generic form

<QueryCodeBlock code={String.raw`MATCH (u:User)
RETURN u.name AS user,
       CASE
         WHEN u.score >= 1000 THEN 'platinum'
         WHEN u.score >=  100 THEN 'gold'
         ELSE                       'bronze'
       END AS tier
ORDER BY tier, user`} />

### Conditional count (CASE inside count)

`count(expr)` skips `null`, so a `CASE` without `ELSE` can count only the
rows that match a condition.

<QueryCodeBlock code={String.raw`MATCH (u:User)-[r:RATED]->(m:Movie)
RETURN m.title AS movie,
       count(CASE WHEN r.score >= 4 THEN 1 END) AS positive,
       count(CASE WHEN r.score <= 2 THEN 1 END) AS negative,
       count(*) AS total
ORDER BY movie`} />

### Custom sort order

<QueryCodeBlock code={String.raw`MATCH (t:Task)
RETURN t.title AS task, t.priority AS priority
ORDER BY CASE t.priority
           WHEN 'p0' THEN 0
           WHEN 'p1' THEN 1
           WHEN 'p2' THEN 2
           ELSE           3
         END`} />

### In SET

<QueryCodeBlock code={String.raw`MATCH (u:User)
SET u.tier = CASE
               WHEN u.score >= 1000 THEN 'platinum'
               WHEN u.score >=  100 THEN 'gold'
               ELSE                       'bronze'
             END
RETURN u.name AS user, u.tier AS tier
ORDER BY user`} />

## Common patterns

### Count nodes by label

<QueryCodeBlock code={String.raw`MATCH (n:Person)
RETURN count(*) AS people`} />

### Group by a property

<QueryCodeBlock code={String.raw`MATCH (p:Person)
RETURN p.city AS city, count(*) AS people
ORDER BY people DESC, city`} />

### Distinct values

<QueryCodeBlock code={String.raw`MATCH (p:Person)
RETURN DISTINCT p.city AS city
ORDER BY city`} />

### Filter before aggregating

<QueryCodeBlock code={String.raw`MATCH (p:Person)
WHERE p.born >= 1990
RETURN count(*) AS born_since_1990`} />

### Filter after aggregating

Cypher has no `HAVING`. Pipe through [`WITH`](./return-with#with), then
filter.

<QueryCodeBlock code={String.raw`MATCH (p:Person)-[:KNOWS]->(friend:Person)
WITH p.name AS person, count(friend) AS friends
WHERE friends >= 2
RETURN person, friends
ORDER BY friends DESC`} />

### Top-N

<QueryCodeBlock code={String.raw`MATCH (p:Person)-[:KNOWS]->(friend:Person)
RETURN p.name AS person, count(friend) AS friends
ORDER BY friends DESC
LIMIT 3`} />

### Upsert with MERGE

[`MERGE`](./unwind-merge#merge) finds a pattern or creates it.

<QueryCodeBlock code={String.raw`MERGE (u:User {id: 99})
  ON MATCH SET u.name = 'Zoe'
  ON CREATE SET u.name = 'Zoe', u.handle = 'zoe', u.score = 0
RETURN u.id AS id, u.name AS name, u.handle AS handle`} />

### Bulk load with UNWIND

<QueryCodeBlock code={String.raw`UNWIND ['click', 'signup', 'click'] AS kind
CREATE (:Event {kind: kind})
RETURN kind AS imported`} />

### Shortest path between two stations

<QueryCodeBlock code={String.raw`MATCH p = shortestPath(
  (from:Station {name: 'Amsterdam'})-[:ROUTE*]->(to:Station {name: 'Den Haag'})
)
RETURN path.length(p) AS hops, [n IN path.nodes(p) | n.name] AS route`} />

## Domain shapes

These examples use the same seed graph but read like application queries.

### People you might know

<QueryCodeBlock code={String.raw`MATCH (me:User {name: 'Alice'})-[:FOLLOWS]->(:User)-[:FOLLOWS]->(candidate:User)
WHERE candidate <> me
  AND NOT EXISTS { (me)-[:FOLLOWS]->(candidate) }
RETURN candidate.name AS candidate, count(*) AS shared_paths
ORDER BY shared_paths DESC, candidate`} />

### Colleagues at the same company

<QueryCodeBlock code={String.raw`MATCH (a:Person)-[:WORKS_AT]->(company:Company)<-[:WORKS_AT]-(b:Person)
WHERE id(a) < id(b)
RETURN company.name AS company, a.name AS person_a, b.name AS person_b
ORDER BY company, person_a, person_b`} />

### Project staffing

<QueryCodeBlock code={String.raw`MATCH (p:Person)-[assignment:ASSIGNED_TO]->(project:Project)
RETURN project.name AS project,
       collect(p.name) AS people,
       count(assignment) AS team_size
ORDER BY project`} />

### Movie recommendations

<QueryCodeBlock code={String.raw`MATCH (viewer:User {name: 'Bob'})-[:RATED]->(:Movie)<-[:RATED]-(peer:User)-[rating:RATED]->(movie:Movie)
WHERE NOT EXISTS { (viewer)-[:RATED]->(movie) }
RETURN movie.title AS movie, avg(rating.score) AS score
ORDER BY score DESC, movie`} />

### Route options

<QueryCodeBlock code={String.raw`MATCH (from:Station {name: 'Amsterdam'})-[route:ROUTE]->(to:Station)
RETURN to.name AS station,
       route.distance AS distance_km,
       route.duration AS duration_min
ORDER BY duration_min ASC`} />

## See also

- [**Queries -> Overview**](../queries) - clause-by-clause reference.
- [**Cheat sheet**](./cheat-sheet) - single-page quick reference.
- [**Tutorial**](../getting-started/tutorial) - same language, guided
  top-to-bottom.
- [**Cookbook**](../cookbook) - scenario-driven recipes by domain.
- [**Parameters**](./parameters) - typed parameter binding.
- [**Functions**](../functions/overview) - every built-in.
- [**Concepts -> Graph Model**](../concepts/graph-model) - the data model.
