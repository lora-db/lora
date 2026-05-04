# GQL Syntax Examples

Created: 2026-05-03

This file is generated from a local analysis of [`gql.yml`](./gql.yml), which
contains 814 BNF definitions from the ISO GQL grammar. The examples below are
syntax-oriented: they are meant to show what the grammar shape permits, not to
claim that every form is already implemented by LoraDB.

Use this file as a test-design and implementation-planning companion:

- **Standard GQL examples** demonstrate grammar families from `gql.yml`.
- **LoraDB parser seed examples** near the end focus on the currently intended
  small executable subset.
- Prefer adding parser tests from the seed examples first, then expand section
  by section as the GQL parser grows.

## Major Grammar Families Found

The YAML grammar contains rules for:

- Programs, sessions, transaction commands, and session parameters.
- Procedure bodies, binding variables, statement blocks, and `NEXT`.
- Catalog changes: schemas, graphs, graph types, and graph type copies.
- Focused and ambient query statements, `USE`, `AT`, `SELECT`, and `RETURN`.
- `MATCH`, `OPTIONAL MATCH`, match modes, graph patterns, path searches, and
  path modes.
- Data modification: `INSERT`, `SET`, `REMOVE`, and `DELETE`.
- Result shaping: `DISTINCT`, `ALL`, `GROUP BY`, `ORDER BY`, `OFFSET`/`SKIP`,
  `LIMIT`, `NO BINDINGS`, and `FINISH`.
- Expressions, predicates, functions, value types, list/record/path values, and
  procedure calls.

## Session And Transaction Examples

```gql
SESSION SET SCHEMA app;
```

```gql
SESSION SET PROPERTY GRAPH CURRENT_GRAPH;
```

```gql
SESSION SET GRAPH social;
```

```gql
SESSION SET TIME ZONE 'Europe/Amsterdam';
```

```gql
SESSION SET VALUE $tenantId = 'dream';
```

```gql
SESSION SET VALUE IF NOT EXISTS $limit = 100;
```

```gql
SESSION SET PROPERTY GRAPH IF NOT EXISTS $workingGraph = CURRENT_GRAPH;
```

```gql
SESSION RESET SCHEMA;
```

```gql
SESSION RESET PROPERTY GRAPH;
```

```gql
SESSION RESET TIME ZONE;
```

```gql
SESSION RESET ALL PARAMETERS;
```

```gql
SESSION RESET ALL CHARACTERISTICS;
```

```gql
SESSION RESET PARAMETER $tenantId;
```

```gql
START TRANSACTION READ ONLY;
```

```gql
START TRANSACTION READ WRITE;
```

```gql
START TRANSACTION READ WRITE, READ ONLY;
```

```gql
COMMIT;
```

```gql
ROLLBACK;
```

```gql
SESSION CLOSE;
```

## Procedure And Statement Block Examples

```gql
{
  MATCH (p:Person)
  RETURN p.name AS name
}
```

```gql
AT app {
  MATCH (p:Person)
  RETURN p
}
```

```gql
VALUE minAge = 18
MATCH (p:Person)
WHERE p.age >= minAge
RETURN p.name AS name
```

```gql
PROPERTY GRAPH g = CURRENT_GRAPH
MATCH (n)
RETURN n
```

```gql
BINDING TABLE recent =
{
  MATCH (p:Post)
  RETURN p.id AS id
}
MATCH (p:Post)
RETURN p
```

```gql
MATCH (p:Person)
RETURN p.name AS name
NEXT
MATCH (c:Company)
RETURN c.name AS name
```

```gql
MATCH (p:Person)
RETURN p.name AS name
NEXT YIELD name
MATCH (c:City)
RETURN name, c.name AS city
```

```gql
LET fullName = p.firstName || ' ' || p.lastName
RETURN fullName AS name
```

```gql
LET VALUE minScore INTEGER = 90
MATCH (s:Student)
WHERE s.score >= minScore
RETURN s.name AS name
```

```gql
FOR tag IN ['gql', 'graph', 'parser']
RETURN tag
```

```gql
FOR tag IN ['gql', 'graph', 'parser'] WITH ORDINALITY ord
RETURN tag, ord
```

```gql
FOR tag IN ['gql', 'graph', 'parser'] WITH OFFSET idx
RETURN tag, idx
```

## Catalog And Graph Definition Examples

```gql
CREATE SCHEMA IF NOT EXISTS app;
```

```gql
DROP SCHEMA IF EXISTS old_app;
```

```gql
CREATE PROPERTY GRAPH social ANY;
```

```gql
CREATE GRAPH IF NOT EXISTS social ANY;
```

```gql
CREATE OR REPLACE GRAPH social ANY;
```

```gql
CREATE PROPERTY GRAPH social LIKE CURRENT_GRAPH;
```

```gql
CREATE PROPERTY GRAPH social AS COPY OF archive.social;
```

```gql
DROP PROPERTY GRAPH IF EXISTS social;
```

```gql
CREATE PROPERTY GRAPH TYPE social_type {
  (Person {name STRING, age INTEGER}),
  (Company {name STRING}),
  (Person)-[WORKS_AT {since DATE}]->(Company)
};
```

```gql
CREATE OR REPLACE GRAPH TYPE social_type AS {
  (Account {id STRING NOT NULL}),
  (Post {id STRING, body STRING}),
  (Account)-[WROTE]->(Post)
};
```

```gql
CREATE GRAPH TYPE route_type COPY OF transport_type;
```

```gql
CREATE PROPERTY GRAPH TYPE route_type LIKE CURRENT_GRAPH;
```

```gql
DROP GRAPH TYPE IF EXISTS social_type;
```

## Focused Graph And Current Graph Examples

```gql
USE social
MATCH (p:Person)
RETURN p.name AS name
```

```gql
USE CURRENT_GRAPH
MATCH (n)
RETURN n
```

```gql
USE CURRENT_PROPERTY_GRAPH
MATCH (n)
RETURN n
```

```gql
USE HOME_GRAPH
MATCH (n)
RETURN n
```

```gql
USE $workingGraph
MATCH (n)
RETURN n
```

## Basic Match Examples

```gql
MATCH (p:Person)
RETURN p
```

```gql
MATCH (p IS Person)
RETURN p
```

```gql
MATCH (p:Person {name: 'Ada'})
RETURN p
```

```gql
MATCH (p:Person)-[:KNOWS]->(friend:Person)
RETURN p.name AS person, friend.name AS friend
```

```gql
MATCH (p:Person)<-[:FOLLOWS]-(follower:Person)
RETURN p, follower
```

```gql
MATCH (a:Person)-[:KNOWS]-(b:Person)
RETURN a, b
```

```gql
MATCH (p:Person)-[r:WORKS_AT]->(c:Company)
RETURN p.name AS person, c.name AS company, r.since AS since
```

```gql
MATCH (p:Person)-[r]->(x)
WHERE r.since >= DATE '2020-01-01'
RETURN p, x
```

```gql
MATCH (p:Person), (c:Company)
WHERE p.employerId = c.id
RETURN p.name AS person, c.name AS company
```

```gql
OPTIONAL MATCH (p:Person)-[:LIVES_IN]->(city:City)
RETURN p.name AS person, city.name AS city
```

```gql
OPTIONAL {
  MATCH (p:Person)-[:HAS_EMAIL]->(email:Email)
}
RETURN p, email
```

## Match Modes, Pattern Filters, And Yield Examples

```gql
MATCH DIFFERENT EDGES (a)-[e:KNOWS]->(b)
RETURN a, e, b
```

```gql
MATCH REPEATABLE ELEMENTS (a)-[e:KNOWS]->(b)
RETURN a, e, b
```

```gql
MATCH (p:Person)-[:KNOWS]->(friend:Person)
WHERE p.age >= 18
RETURN friend.name AS friend
```

```gql
MATCH (p:Person)-[:KNOWS]->(friend:Person)
YIELD p.name AS person, friend.name AS friend
RETURN person, friend
```

```gql
MATCH KEEP SHORTEST 3 PATHS (a:Station)-[:ROUTE]->+(b:Station)
RETURN a.name AS from, b.name AS to
```

```gql
MATCH (p:Person WHERE p.age >= 18)-[:KNOWS]->(friend:Person)
RETURN p, friend
```

```gql
MATCH (a:Person)-[r:KNOWS WHERE r.strength > 0.7]->(b:Person)
RETURN a, b
```

## Path Pattern And Search Examples

```gql
MATCH p = (a:Station)-[:ROUTE]->(b:Station)
RETURN p
```

```gql
MATCH p = (a:Station)-[:ROUTE]->{1,3}(b:Station)
RETURN p
```

```gql
MATCH p = (a:Station)-[:ROUTE]->*(b:Station)
RETURN p
```

```gql
MATCH p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH p = (a:Station)-[:ROUTE]->?(b:Station)
RETURN p
```

```gql
MATCH ALL PATHS p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH ANY PATH p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH ANY 5 PATHS p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH SHORTEST 3 PATHS p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH ALL SHORTEST PATHS p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH ANY SHORTEST PATH p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH SHORTEST 2 GROUPS p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH WALK PATH p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH TRAIL PATH p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH SIMPLE PATH p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH ACYCLIC PATH p = (a:Station)-[:ROUTE]->+(b:Station)
RETURN p
```

```gql
MATCH p = ((a:Station)-[:ROUTE]->(b:Station) | (a)-[:TRANSFER]->(b))
RETURN p
```

```gql
MATCH p = (a:Station)-[:ROUTE]->(b:Station) | (a)-[:TRANSFER]->(b)
RETURN p
```

```gql
MATCH p = (a:Station)-[:ROUTE]->(b:Station) || (b)-[:ROUTE]->(c:Station)
RETURN p
```

## Return, Grouping, Ordering, And Paging Examples

```gql
MATCH (p:Person)
RETURN *
```

```gql
MATCH (p:Person)
RETURN p.name AS name, p.age AS age
```

```gql
MATCH (p:Person)
RETURN DISTINCT p.country AS country
```

```gql
MATCH (p:Person)
RETURN ALL p.country AS country
```

```gql
MATCH (p:Person)
RETURN NO BINDINGS
```

```gql
MATCH (p:Person)
RETURN p.country AS country, COUNT(*) AS people
GROUP BY p.country
```

```gql
MATCH (p:Person)
RETURN p.country AS country, AVG(p.age) AS avgAge
GROUP BY p.country
```

```gql
MATCH (p:Person)
RETURN p.name AS name
ORDER BY name ASC
```

```gql
MATCH (p:Person)
RETURN p.name AS name
ORDER BY name DESC NULLS LAST
```

```gql
MATCH (p:Person)
RETURN p.name AS name, p.age AS age
ORDER BY age DESC NULLS LAST, name ASC NULLS FIRST
```

```gql
MATCH (p:Person)
RETURN p.name AS name
OFFSET 10
LIMIT 25
```

```gql
MATCH (p:Person)
RETURN p.name AS name
SKIP 10
LIMIT 25
```

```gql
MATCH (p:Person)
RETURN p.name AS name
LIMIT 10
```

```gql
MATCH (p:Person)
FINISH
```

## SELECT Examples

```gql
SELECT *
```

```gql
SELECT DISTINCT name, age
WHERE age >= 18
ORDER BY name
OFFSET 20
LIMIT 10
```

```gql
SELECT country, COUNT(*) AS people
GROUP BY country
HAVING people > 100
ORDER BY people DESC
```

```gql
SELECT name AS displayName, score AS rankScore
ORDER BY rankScore DESC NULLS LAST
```

## Data Modification Examples

```gql
INSERT (:Person {name: 'Ada', age: 37})
FINISH
```

```gql
INSERT (p:Person {name: 'Ada'})-[:WORKS_AT {since: DATE '2020-01-01'}]->(:Company {name: 'Dream'})
RETURN p
```

```gql
INSERT
  (:Person {name: 'Grace'}),
  (:Person {name: 'Katherine'})
FINISH
```

```gql
MATCH (p:Person {name: 'Ada'})
SET p.age = 38
RETURN p
```

```gql
MATCH (p:Person {name: 'Ada'})
SET p = {name: 'Ada Lovelace', age: 38}
RETURN p
```

```gql
MATCH (p:Person {name: 'Ada'})
SET p:Mathematician
RETURN p
```

```gql
MATCH (p:Person {name: 'Ada'})
SET p IS Mathematician
RETURN p
```

```gql
MATCH (p:Person {name: 'Ada'})
REMOVE p.age
RETURN p
```

```gql
MATCH (p:Person {name: 'Ada'})
REMOVE p:Mathematician
RETURN p
```

```gql
MATCH (p:Person {name: 'Ada'})
REMOVE p IS Mathematician
RETURN p
```

```gql
MATCH (p:Person {name: 'Ada'})
DELETE p
FINISH
```

```gql
MATCH (p:Person {name: 'Ada'})
DETACH DELETE p
FINISH
```

```gql
MATCH (p:Person {name: 'Ada'})
NODETACH DELETE p
FINISH
```

```gql
MATCH (a:Person)-[r:KNOWS]->(b:Person)
DELETE r
RETURN a, b
```

## Call And Yield Examples

```gql
CALL app.refreshScores()
YIELD updated
RETURN updated
```

```gql
OPTIONAL CALL app.findProfile($userId)
YIELD profile
RETURN profile
```

```gql
CALL {
  MATCH (p:Person)
  RETURN p.name AS name
}
RETURN name
```

```gql
CALL () {
  MATCH (p:Person)
  RETURN p.name AS name
}
RETURN name
```

```gql
CALL app.searchPeople('Ada', 10)
YIELD person, score
RETURN person.name AS name, score
ORDER BY score DESC
```

## Predicate Examples

```gql
MATCH (p:Person)
WHERE p.age >= 18
RETURN p
```

```gql
MATCH (p:Person)
WHERE p.email IS NOT NULL
RETURN p
```

```gql
MATCH (p:Person)
WHERE p.email IS NULL
RETURN p
```

```gql
MATCH (p:Person)
WHERE EXISTS { (p)-[:WORKS_AT]->(:Company) }
RETURN p
```

```gql
MATCH (p:Person)
WHERE EXISTS {
  MATCH (p)-[:KNOWS]->(:Person {name: 'Grace'})
}
RETURN p
```

```gql
MATCH (a:Person)-[r:KNOWS]->(b:Person)
WHERE ALL_DIFFERENT(a, b)
RETURN a, b
```

```gql
MATCH (a:Person)-[r:KNOWS]->(b:Person)
WHERE SAME(a, b) IS FALSE
RETURN a, b
```

```gql
MATCH (p:Person)
WHERE PROPERTY_EXISTS(p, name)
RETURN p
```

```gql
MATCH (p:Person)-[r]->(x)
WHERE r IS DIRECTED
RETURN p, x
```

```gql
MATCH (p)
WHERE p IS LABELED Person
RETURN p
```

```gql
MATCH (p)
WHERE p IS Person
RETURN p
```

```gql
MATCH (p:Person)-[r:WORKS_AT]->(c:Company)
WHERE c IS DESTINATION OF r
RETURN p, c
```

```gql
MATCH (p:Person)
WHERE p.name STARTS WITH 'A'
RETURN p
```

```gql
MATCH (p:Person)
WHERE p.name ENDS WITH 'a'
RETURN p
```

```gql
MATCH (p:Person)
WHERE p.name CONTAINS 'da'
RETURN p
```

```gql
MATCH (p:Person)
WHERE p.country IN ['NL', 'US', 'UK']
RETURN p
```

```gql
MATCH (p:Person)
WHERE p.active IS TRUE OR p.verified IS TRUE
RETURN p
```

```gql
MATCH (p:Person)
WHERE p.active IS UNKNOWN
RETURN p
```

## Expression And Function Examples

```gql
RETURN 1 + 2 * 3 AS value
```

```gql
RETURN POWER(2, 10) AS value
```

```gql
RETURN ABS(-42) AS value
```

```gql
RETURN FLOOR(3.7) AS value
```

```gql
RETURN CEILING(3.2) AS value
```

```gql
RETURN LOG10(1000) AS value
```

```gql
RETURN EXP(1) AS value
```

```gql
RETURN SIN(0) AS value, COS(0) AS other
```

```gql
RETURN CHARACTER_LENGTH('graph') AS chars
```

```gql
RETURN LOWER('GQL') AS lower, UPPER('gql') AS upper
```

```gql
RETURN TRIM(BOTH ' ' FROM '  graph  ') AS value
```

```gql
RETURN SUBSTRING('property graph' FROM 1 FOR 8) AS value
```

```gql
RETURN NORMALIZE('cafe') AS value
```

```gql
RETURN DATE '2026-05-03' AS d
```

```gql
RETURN TIME '14:30:00' AS t
```

```gql
RETURN DATETIME '2026-05-03T14:30:00' AS dt
```

```gql
RETURN LOCAL_DATETIME '2026-05-03T14:30:00' AS dt
```

```gql
RETURN DURATION 'P1Y2M3DT4H5M6S' AS duration
```

```gql
RETURN CURRENT_DATE AS today, CURRENT_TIMESTAMP AS now
```

```gql
RETURN COALESCE(NULL, 'fallback') AS value
```

```gql
RETURN NULLIF(status, 'unknown') AS status
```

```gql
RETURN CASE status
  WHEN 'new' THEN 1
  WHEN 'active' THEN 2
  ELSE 0
END AS statusRank
```

```gql
RETURN CASE
  WHEN score >= 90 THEN 'excellent'
  WHEN score >= 70 THEN 'good'
  ELSE 'review'
END AS band
```

```gql
MATCH (p:Person)
RETURN COUNT(*) AS total
```

```gql
MATCH (p:Person)
RETURN COUNT(DISTINCT p.country) AS countries
```

```gql
MATCH (p:Person)
RETURN MIN(p.age) AS minAge, MAX(p.age) AS maxAge
```

```gql
MATCH (p:Person)
RETURN SUM(p.score) AS totalScore, AVG(p.score) AS avgScore
```

```gql
MATCH p = (a)-[:KNOWS]->+(b)
RETURN ELEMENTS(p) AS elements
```

```gql
MATCH p = (a)-[:KNOWS]->+(b)
RETURN ELEMENT_ID(a) AS id
```

## List, Record, And Path Value Examples

```gql
RETURN [1, 2, 3] AS numbers
```

```gql
RETURN LIST<STRING>['Ada', 'Grace'] AS names
```

```gql
RETURN [] AS emptyList
```

```gql
RETURN RECORD {name: 'Ada', age: 37} AS person
```

```gql
RETURN {name: 'Ada', age: 37} AS person
```

```gql
MATCH p = (a:Person)-[:KNOWS]->(b:Person)
RETURN PATH [a, b] AS pathValue
```

```gql
MATCH p = (a:Person)-[:KNOWS]->(b:Person)
RETURN p || p AS doubledPath
```

## Value Type Examples

```gql
VALUE name STRING = 'Ada'
RETURN name
```

```gql
VALUE age INTEGER NOT NULL = 37
RETURN age
```

```gql
VALUE tags LIST<STRING> = ['graph', 'gql']
RETURN tags
```

```gql
VALUE person RECORD {name STRING, age INTEGER} = {name: 'Ada', age: 37}
RETURN person
```

```gql
VALUE maybeName STRING | NULL = NULL
RETURN maybeName
```

```gql
VALUE n NODE = VARIABLE someNode
RETURN n
```

```gql
VALUE e EDGE = VARIABLE someEdge
RETURN e
```

```gql
VALUE g GRAPH = CURRENT_GRAPH
RETURN g
```

## Graph Type Snippet Examples

```gql
CREATE GRAPH TYPE people_graph AS {
  (Person {id STRING NOT NULL, name STRING, age INTEGER}),
  (City {name STRING}),
  (Person)-[LIVES_IN]->(City)
};
```

```gql
CREATE GRAPH TYPE employment_graph AS {
  (Person {id STRING NOT NULL}),
  (Company {id STRING NOT NULL}),
  (Person)-[WORKS_AT {since DATE, title STRING}]->(Company)
};
```

```gql
CREATE GRAPH TYPE dependency_graph AS {
  (Package {name STRING, version STRING}),
  (Package)-[DEPENDS_ON]->(Package)
};
```

```gql
CREATE GRAPH TYPE open_graph COPY OF existing_type;
```

```gql
CREATE GRAPH TYPE graph_like_live LIKE CURRENT_GRAPH;
```

## LoraDB Parser Seed Examples

These examples stay close to the GQL subset currently being introduced in the
LoraDB parser facade. They are good candidates for executable parser and
database tests before moving on to broader ISO grammar forms.

```gql
MATCH (n) RETURN n
```

```gql
MATCH (n:User) RETURN n
```

```gql
MATCH (n:User {name: 'Alice'}) RETURN n
```

```gql
OPTIONAL MATCH (n:User)-[:FOLLOWS]->(m:User) RETURN n, m
```

```gql
MATCH (n) WHERE n.age >= 18 RETURN n
```

```gql
MATCH (n) RETURN n.name AS name ORDER BY name OFFSET 5 LIMIT 10
```

```gql
MATCH (n) RETURN n.name AS name SKIP 5 LIMIT 10
```

```gql
RETURN 1 AS one
```

```gql
RETURN TRUE AS ok, FALSE AS no, UNKNOWN AS maybe
```

```gql
RETURN CASE WHEN 1 = 1 THEN 'yes' ELSE 'no' END AS answer
```

```gql
INSERT (:User {name: 'Alice'}) FINISH
```

```gql
INSERT (n:User {name: 'Alice'}) RETURN n
```

```gql
INSERT (:User {name: 'Alice'})-[:FOLLOWS]->(:User {name: 'Bob'}) FINISH
```

```gql
MATCH (n:User {name: 'Alice'}) SET n.age = 30 RETURN n
```

```gql
MATCH (n:User {name: 'Alice'}) SET n += {active: true} RETURN n
```

```gql
MATCH (n:User {name: 'Alice'}) SET n:Admin RETURN n
```

```gql
MATCH (n:User {name: 'Alice'}) REMOVE n.age RETURN n
```

```gql
MATCH (n:User {name: 'Alice'}) REMOVE n:Admin RETURN n
```

```gql
MATCH (n:User {name: 'Alice'}) DELETE n
```

```gql
MATCH (n:User {name: 'Alice'}) DETACH DELETE n
```

```gql
MATCH (n) RETURN n UNION MATCH (m) RETURN m
```

```gql
MATCH (n) RETURN n UNION ALL MATCH (m) RETURN m
```
