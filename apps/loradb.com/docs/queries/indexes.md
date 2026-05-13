---
title: Indexes
sidebar_label: Indexes
description: How to create, inspect, query, and drop LoraDB indexes for range, text, point, lookup, vector, and full-text workloads.
---

# Indexes

LoraDB is still schema-free by default: labels, relationship types,
and property keys appear when you write them. Indexes are optional
catalog entries that tell the in-memory store which secondary
structures to maintain for frequently used predicates, vector search,
and full-text search.

## Create an index

```cypher
CREATE INDEX user_email FOR (u:User) ON (u.email);
CREATE INDEX user_age IF NOT EXISTS FOR (u:User) ON (u.age);
CREATE TEXT INDEX user_name FOR (u:User) ON (u.name);
CREATE POINT INDEX venue_location FOR (v:Venue) ON (v.location);
CREATE VECTOR INDEX doc_embedding FOR (d:Doc) ON (d.embedding)
OPTIONS {indexConfig: {`vector.dimensions`: 1536, `vector.similarity_function`: 'cosine'}};
CREATE FULLTEXT INDEX article_search FOR (a:Article) ON EACH [a.title, a.body];
```

Relationship indexes use the relationship pattern form:

```cypher
CREATE INDEX rel_since FOR ()-[r:FOLLOWS]-() ON (r.since);
CREATE TEXT INDEX rel_note FOR ()-[r:TAGGED]-() ON (r.note);
CREATE POINT INDEX rel_location FOR ()-[r:DELIVERED]-() ON (r.location);
CREATE VECTOR INDEX rel_embedding FOR ()-[r:CONTAINS]-() ON (r.embedding)
OPTIONS {indexConfig: {`vector.dimensions`: 384, `vector.similarity_function`: 'euclidean'}};
CREATE FULLTEXT INDEX rel_summary FOR ()-[r:WROTE]-() ON EACH [r.summary];
```

If you omit the name, LoraDB creates a deterministic `index_...` name:

```cypher
CREATE INDEX FOR (p:Product) ON (p.sku);
```

Index names may also come from a string parameter:

```cypher
CREATE INDEX $name FOR (u:User) ON (u.email);
```

## Index kinds

| Kind | Syntax | Useful predicates |
|---|---|---|
| RANGE | `CREATE INDEX ...` or `CREATE RANGE INDEX ...` | `=`, `<`, `<=`, `>`, `>=`, bounded ranges |
| TEXT | `CREATE TEXT INDEX ...` | `STARTS WITH`, `CONTAINS`, `ENDS WITH` |
| POINT | `CREATE POINT INDEX ...` | `geo.within_bbox(...)`, `geo.distance(...) <= radius` |
| LOOKUP | `CREATE LOOKUP INDEX ...` | Catalog-visible label/type token indexes |
| VECTOR | `CREATE VECTOR INDEX ... OPTIONS {indexConfig: {...}}` | `db.index.vector.queryNodes`, `db.index.vector.queryRelationships` |
| FULLTEXT | `CREATE FULLTEXT INDEX ... ON EACH [...]` | `db.index.fulltext.queryNodes`, `db.index.fulltext.queryRelationships` |

Lookup indexes are catalog entries over labels or relationship types:

```cypher
CREATE LOOKUP INDEX node_labels FOR (n) ON EACH labels(n);
CREATE LOOKUP INDEX rel_types FOR ()-[r]-() ON EACH type(r);
```

Composite RANGE indexes are accepted and shown in the catalog:

```cypher
CREATE INDEX person_age_country FOR (p:Person) ON (p.age, p.country);
```

Current optimizer rewrites use single-property scopes. Keep composite
indexes for catalog policy and future planner work rather than expecting
multi-column seek behavior today.

## Vector indexes

Vector indexes are single-property node or relationship indexes. They
require an `indexConfig` map with:

- `vector.dimensions` - integer dimension in `1..=4096`;
- `vector.similarity_function` - `'cosine'` or `'euclidean'`.

```cypher
CREATE VECTOR INDEX movie_embedding
FOR (m:Movie)
ON (m.embedding)
OPTIONS {indexConfig: {
  `vector.dimensions`: 3,
  `vector.similarity_function`: 'cosine'
}};

CREATE (:Movie {title: 'A', embedding: [1.0, 0.0, 0.0]::VECTOR<FLOAT32>(3)});
CREATE (:Movie {title: 'B', embedding: [0.9, 0.1, 0.0]::VECTOR<FLOAT32>(3)});

CALL db.index.vector.queryNodes('movie_embedding', 2, [1.0, 0.0, 0.0])
YIELD node, score;
```

The relationship procedure has the same shape but yields
`relationship`:

```cypher
CALL db.index.vector.queryRelationships('rel_embedding', 10, $query)
YIELD relationship, score;
```

`k` must be positive. The query argument can be a `VECTOR`, a
`[...]::VECTOR<COORD>(DIM)` cast, a numeric list, or a parameter containing a vector.
Numeric lists are coerced to `FLOAT32` vectors. The query dimension
must match the index dimension.

:::note Current execution
The vector procedure uses the cataloged vector index definition for
scope, dimensions, and similarity, but nearest-neighbour execution is
currently a flat scan over label/type-matching entities. Results are
sorted by descending score. A dedicated ANN structure is still future
work.
:::

## Full-text indexes

Full-text indexes use `ON EACH [...]` and can cover multiple properties.
Node full-text indexes may cover multiple labels; relationship
full-text indexes may cover multiple relationship types:

```cypher
CREATE FULLTEXT INDEX article_search
FOR (a:Article|Note)
ON EACH [a.title, a.body]
OPTIONS {`fulltext.analyzer`: 'standard'};

CALL db.index.fulltext.queryNodes('article_search', 'graph search')
YIELD node, score;
```

Relationship full-text search yields `relationship`:

```cypher
CREATE FULLTEXT INDEX wrote_search
FOR ()-[r:WROTE]-()
ON EACH [r.summary];

CALL db.index.fulltext.queryRelationships('wrote_search', 'graph')
YIELD relationship, score;
```

Procedure calls return the yielded columns directly. The current
analyzer tokenizes by lowercasing and splitting on
non-alphanumeric characters. Multiple query terms use AND semantics:
all terms must be present. Scores are based on summed term frequency
and results are sorted by descending score.

`fulltext.analyzer` accepts `'standard'` and `'simple'`; unsupported
names are rejected. `fulltext.eventually_consistent` accepts a boolean
option, but index maintenance is currently synchronous.

## Inspect indexes

```cypher
SHOW INDEXES;
```

Rows contain:

| Column | Meaning |
|---|---|
| `name` | Index name |
| `type` | `RANGE`, `TEXT`, `POINT`, or `LOOKUP` |
| `entityType` | `NODE` or `RELATIONSHIP` |
| `labelsOrTypes` | Label or relationship type scope, empty for lookup indexes |
| `properties` | Indexed property keys |
| `state` | Currently `ONLINE` for created indexes |
| `populationPercent` | `100.0` for online indexes |

`type` can be `RANGE`, `TEXT`, `POINT`, `LOOKUP`, `VECTOR`, or
`FULLTEXT`.

Use a type filter when you only want one kind:

```cypher
SHOW RANGE INDEXES;
SHOW TEXT INDEXES;
SHOW POINT INDEXES;
SHOW LOOKUP INDEXES;
SHOW VECTOR INDEXES;
SHOW FULLTEXT INDEXES;
SHOW ALL INDEXES;
```

The singular spelling also works:

```cypher
SHOW RANGE INDEX;
```

Catalog output can be shaped with a `YIELD`-anchored pipeline:

```cypher
SHOW INDEXES
YIELD name, type, entityType
WHERE type = 'VECTOR'
RETURN name
ORDER BY name
LIMIT 10;
```

## Drop an index

```cypher
DROP INDEX user_email;
DROP INDEX maybe_missing IF EXISTS;
```

Dropping a missing index without `IF EXISTS` returns a stable
GQLSTATUS-shaped error (`42N51`). Creating an index with a duplicate
name returns `22N71`; creating an equivalent index under a different
name returns `22N70`. `IF NOT EXISTS` turns either conflict into a
no-op.

Indexes owned by constraints cannot be dropped directly. Use
[`DROP CONSTRAINT`](./constraints#drop-constraints) for those.

## What the optimizer uses

Declared indexes can replace scan-and-filter plans with specialized
operators:

```cypher
CREATE INDEX person_age FOR (p:Person) ON (p.age);
CREATE TEXT INDEX person_name FOR (p:Person) ON (p.name);
CREATE POINT INDEX place_location FOR (p:Place) ON (p.location);
```

Inspect the plan with your binding's `explain` method or HTTP
`POST /explain`. These queries should show the specialized scan names
in the returned plan tree:

```cypher
MATCH (p:Person) WHERE p.age >= 30 AND p.age < 50 RETURN p
-- NodeByPropertyRangeScan

MATCH (p:Person) WHERE p.name STARTS WITH 'Al' RETURN p
-- NodeByTextScan

MATCH (p:Place)
WHERE geo.within_bbox(
  p.location,
  {x: 0, y: 0}::POINT,
  {x: 100, y: 100}::POINT
)
RETURN p
-- NodeByPointScan
```

The same rewrite family exists for relationship scans when the pattern
can be satisfied from the relationship index:

```cypher
CREATE INDEX knows_since FOR ()-[r:KNOWS]-() ON (r.since);

MATCH ()-[r:KNOWS]->()
WHERE r.since > 2020
RETURN r;
```

The original predicate still runs after the index candidate set is
produced. That keeps semantics correct for compound predicates and for
conservative TEXT/POINT candidate indexes.

## Durability

Index catalog changes are part of the normal write path. WAL-backed
databases replay `CREATE INDEX` and `DROP INDEX` events during recovery,
and snapshots include the index catalog trailer in the current body
format. Older snapshots without a catalog still load with an empty
index list.

## Limitations

- Vector procedures use flat scan execution today; no ANN structure yet.
- Full-text query strings use term intersection and term-frequency
  scoring, not a Lucene-style query language.
- Composite RANGE indexes are cataloged, but current planner rewrites
  are single-property.
- FULLTEXT indexes require `ON EACH [...]`; non-full-text indexes use
  `ON (...)`.

## See also

- [WHERE](./where) - predicates that can benefit from indexes.
- [Constraints](./constraints) - uniqueness, existence, keys, and type checks.
- [Spatial functions](../functions/spatial) - point predicates.
- [Vector values](../data-types/vectors) - storing and querying embeddings.
- [HTTP `POST /explain`](../api/http#post-explain) - inspect the
  physical plan from HTTP, or use the equivalent binding methods.
- [Limitations](../limitations) - remaining schema and storage gaps.
