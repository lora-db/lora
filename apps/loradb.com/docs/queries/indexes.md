---
title: Indexes
sidebar_label: Indexes
description: How to create, inspect, and drop LoraDB indexes for equality, range, text, lookup, and point predicates.
---

# Indexes

LoraDB is still schema-free: labels, relationship types, and property
keys appear when you write them. Indexes are optional catalog entries
that tell the in-memory store which secondary structures to maintain
for frequently used predicates.

## Create an index

```cypher
CREATE INDEX user_email FOR (u:User) ON (u.email);
CREATE INDEX user_age IF NOT EXISTS FOR (u:User) ON (u.age);
CREATE TEXT INDEX user_name FOR (u:User) ON (u.name);
CREATE POINT INDEX venue_location FOR (v:Venue) ON (v.location);
```

Relationship indexes use the relationship pattern form:

```cypher
CREATE INDEX rel_since FOR ()-[r:FOLLOWS]-() ON (r.since);
CREATE TEXT INDEX rel_note FOR ()-[r:TAGGED]-() ON (r.note);
CREATE POINT INDEX rel_location FOR ()-[r:DELIVERED]-() ON (r.location);
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
| POINT | `CREATE POINT INDEX ...` | `point.withinBBox(...)`, `point.distance(...) <= radius` |
| LOOKUP | `CREATE LOOKUP INDEX ...` | Catalog-visible label/type token indexes |

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
WHERE point.withinBBox(
  p.location,
  point({x: 0, y: 0}),
  point({x: 100, y: 100})
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

- No uniqueness constraints yet.
- No vector / ANN index yet; vector similarity remains exhaustive over
  the matched candidate set.
- No full-text ranking language yet; TEXT indexes accelerate string
  predicates but do not expose scoring.
- Composite RANGE indexes are cataloged, but current planner rewrites
  are single-property.

## See also

- [WHERE](./where) - predicates that can benefit from indexes.
- [Spatial functions](../functions/spatial) - point predicates.
- [HTTP `POST /explain`](../api/http#post-explain) - inspect the
  physical plan from HTTP, or use the equivalent binding methods.
- [Limitations](../limitations) - remaining schema and storage gaps.
