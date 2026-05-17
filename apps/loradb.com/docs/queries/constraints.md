---
title: Constraints
sidebar_label: Constraints
description: How to create, inspect, and drop LoraDB schema constraints for uniqueness, existence, node keys, relationship keys, and property types.
---

# Constraints

LoraDB remains schema-free by default: labels, relationship types, and
property keys appear when you write them. Constraints are optional
catalog entries that add validation for selected labels or relationship
types.

Constraints are checked in two places:

- when the constraint is created, existing matching data must already
  satisfy it;
- after creation, writes that would violate the constraint fail.

## Create Constraints

Every constraint needs a name:

<QueryCodeBlock code={String.raw`CREATE CONSTRAINT user_email
FOR (u:User)
REQUIRE u.email IS UNIQUE;

CREATE CONSTRAINT author_name
FOR (a:Author)
REQUIRE a.name IS NOT NULL;

CREATE CONSTRAINT actor_name
FOR (a:Actor)
REQUIRE (a.first, a.last) IS NODE KEY;

CREATE CONSTRAINT owns_id
FOR ()-[o:OWNS]-()
REQUIRE o.ownershipId IS RELATIONSHIP KEY;

CREATE CONSTRAINT movie_title
FOR (m:Movie)
REQUIRE m.title IS :: STRING;`} />

Use `IF NOT EXISTS` to make creation idempotent:

<QueryCodeBlock code={String.raw`CREATE CONSTRAINT user_email IF NOT EXISTS
FOR (u:User)
REQUIRE u.email IS UNIQUE;`} />

Constraint names may also come from a string parameter:

<QueryCodeBlock code={String.raw`CREATE CONSTRAINT $name
FOR (u:User)
REQUIRE u.email IS UNIQUE;`} />

## Constraint Kinds

| Kind | Syntax | Applies to |
|---|---|---|
| Property uniqueness | `REQUIRE n.email IS UNIQUE` | Nodes or relationships; single or composite |
| Property existence | `REQUIRE n.email IS NOT NULL` | Nodes or relationships; single property |
| Node key | `REQUIRE (n.a, n.b) IS NODE KEY` | Nodes; existence plus uniqueness |
| Relationship key | `REQUIRE r.id IS RELATIONSHIP KEY` | Relationships; existence plus uniqueness |
| Property type | `REQUIRE n.title IS :: STRING` | Nodes or relationships; single property |

Composite uniqueness and key constraints must wrap the property list in
parentheses:

<QueryCodeBlock code={String.raw`CREATE CONSTRAINT order_line
FOR ()-[r:LINE_ITEM]-()
REQUIRE (r.orderId, r.lineNo) IS UNIQUE;`} />

Existence constraints are single-property only:

<QueryCodeBlock code={String.raw`CREATE CONSTRAINT published_at
FOR (p:Post)
REQUIRE p.publishedAt IS NOT NULL;`} />

## Property Types

Property type constraints accept scalar types, lists with non-null
elements, vectors, and closed unions:

<QueryCodeBlock code={String.raw`CREATE CONSTRAINT movie_released
FOR (m:Movie)
REQUIRE m.released IS :: DATE;

CREATE CONSTRAINT article_tags
FOR (a:Article)
REQUIRE a.tags IS :: LIST<STRING NOT NULL>;

CREATE CONSTRAINT doc_embedding
FOR (d:Doc)
REQUIRE d.embedding IS :: VECTOR<FLOAT32>(1536);

CREATE CONSTRAINT tagline
FOR (m:Movie)
REQUIRE m.tagline IS :: STRING | LIST<STRING NOT NULL>;`} />

Supported scalar type names are `BOOLEAN`, `STRING`, `INTEGER`,
`FLOAT`, `DATE`, `LOCAL TIME`, `ZONED TIME`, `LOCAL DATETIME`,
`ZONED DATETIME`, `DURATION`, and `POINT`.

`MAP`, `ANY`, and nullable list element types such as `LIST<STRING>`
are rejected for property type constraints. Vector dimensions must be
in `1..=4096`, and the coordinate type must match the stored vector
type.

## Inspect Constraints

<QueryCodeBlock code={String.raw`SHOW CONSTRAINTS;`} />

Rows contain:

| Column | Meaning |
|---|---|
| `name` | Constraint name |
| `type` | `NODE_PROPERTY_UNIQUENESS`, `RELATIONSHIP_PROPERTY_UNIQUENESS`, `NODE_PROPERTY_EXISTENCE`, `RELATIONSHIP_PROPERTY_EXISTENCE`, `NODE_KEY`, `RELATIONSHIP_KEY`, `NODE_PROPERTY_TYPE`, or `RELATIONSHIP_PROPERTY_TYPE` |
| `entityType` | `NODE` or `RELATIONSHIP` |
| `labelsOrTypes` | Label or relationship type scope |
| `properties` | Constrained property keys |
| `ownedIndex` | Backing index name for uniqueness and key constraints; `null` otherwise |
| `propertyType` | Type expression for property type constraints; `null` otherwise |

`SHOW CONSTRAINTS` accepts the same `YIELD`-anchored catalog pipeline
as `SHOW INDEXES`:

<QueryCodeBlock code={String.raw`SHOW CONSTRAINTS
YIELD name, type
WHERE type = 'NODE_PROPERTY_UNIQUENESS'
RETURN name
ORDER BY name;`} />

## Drop Constraints

<QueryCodeBlock code={String.raw`DROP CONSTRAINT user_email;
DROP CONSTRAINT maybe_missing IF EXISTS;
DROP CONSTRAINT $name;`} />

Uniqueness and key constraints own a backing `RANGE` index with the
same name. Drop the constraint, not the backing index; direct
`DROP INDEX` on a constraint-owned index is rejected.

## Write Enforcement

Constraints are enforced for matching writes:

<QueryCodeBlock code={String.raw`CREATE CONSTRAINT user_email
FOR (u:User)
REQUIRE u.email IS UNIQUE;

CREATE (:User {email: 'ada@example.com'});
CREATE (:User {email: 'ada@example.com'}); // rejected`} />

Enforcement covers node and relationship creation, property updates,
property replacement through `SET n = {...}`, property removal, and
adding a label that activates a node constraint.

## Durability

Constraint catalog entries are included in snapshots and replayed from
WAL records during recovery. Uniqueness and key constraints recreate
their backing indexes as part of the catalog state.

## Limitations

- Constraints are label/type scoped; there is no database-wide
  uniqueness constraint.
- Existence constraints are single-property only.
- Property type constraints do not support `MAP`, `ANY`, or nullable
  list element types.
- Constraint names are required.
- Dropping a backing index directly is rejected; use
  `DROP CONSTRAINT`.

## See Also

- [Indexes](./indexes) - backing indexes and catalog inspection.
- [CREATE](./create) - writes that may be checked by constraints.
- [SET / REMOVE / DELETE](./set-delete) - mutation clauses.
- [Schema-free](../concepts/schema-free) - optional schema controls.
