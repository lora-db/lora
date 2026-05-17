---
title: Properties on Nodes and Relationships
sidebar_label: Properties
description: Typed key/value properties on nodes and relationships in LoraDB — reading, setting, removing, and the type rules that apply to each supported data type.
---

# Properties on Nodes and Relationships

**Properties** are typed key/value pairs attached to
[nodes](./nodes) and [relationships](./relationships). Keys are
case-sensitive strings; values are any of the
[supported data types](../data-types/overview).

## At a glance

| Operation | Clause |
|---|---|
| Set a single key | <CypherCode code="SET n.prop = value" /> |
| Merge keys | <CypherCode code="SET n += {k: v}" /> |
| Replace the whole map | <CypherCode code="SET n = {…}" /> |
| Remove a key | <CypherCode code="REMOVE n.prop" /> / <CypherCode code="SET n.prop = null" /> |
| Bulk patch from param | <CypherCode code="SET n += $patch" /> |
| Read a key | <CypherCode code="n.prop" /> |
| Read the whole map | <CypherCode code="properties(n)" /> |
| List the keys | <CypherCode code="keys(n)" /> |
| Project a subset | <CypherCode code="n {.name, .born}" /> |

## Write properties

### On create

<QueryCodeBlock code={String.raw`CREATE (:City {
  name:       'Amsterdam',
  population: 918000,
  location:   {latitude: 52.37, longitude: 4.89}::POINT,
  founded:    '1275-10-27'::DATE,
  tags:       ['capital', 'port']
})`} />

### On an existing entity

<QueryCodeBlock code={String.raw`MATCH (c:City {name: 'Amsterdam'})
SET c.population = 920000`} />

### Patch with a parameter map

<QueryCodeBlock code={String.raw`MATCH (c:City {name: 'Amsterdam'})
SET c += $patch
RETURN c`} />

See [`SET`](../queries/set-delete#set--properties) for the full rules
on `=`, `+=`, and `null` assignment.

## Read properties

### Dot access

<QueryCodeBlock code={String.raw`MATCH (c:City {name: 'Amsterdam'})
RETURN c.name, c.population, c.location.latitude`} />

### Bracket access (computed key)

<QueryCodeBlock code={String.raw`MATCH (c:City {name: 'Amsterdam'})
WITH c, 'population' AS k
RETURN c[k]`} />

### Full map

<QueryCodeBlock code={String.raw`MATCH (c:City) RETURN properties(c)`} />

### Key list

<QueryCodeBlock code={String.raw`MATCH (c:City) RETURN keys(c)   // e.g. ['name', 'population', …]`} />

### Map projection

Shape an entity into a map with only the keys you want — see also
[Lists & Maps → Map projection](../data-types/lists-and-maps#map-projection):

<QueryCodeBlock code={String.raw`MATCH (c:City)
RETURN c {.name, .population};
RETURN c {.*};
RETURN c {.name, density: c.population / c.area}`} />

## Update

| Operation | Clause |
|---|---|
| Set a single property | `SET n.prop = value` |
| Merge keys into the map | `SET n += {k1: v1, k2: v2}` |
| Replace the whole map | `SET n = {…}` |
| Remove a property | `REMOVE n.prop` or `SET n.prop = null` |

<QueryCodeBlock code={String.raw`MATCH (c:City {name: 'Amsterdam'})
SET c += {updated_at: temporal.now(), active: true}
RETURN c`} />

### Replace vs merge

`SET n = {…}` **replaces** the entire property map — every key not in
the new map is dropped. `SET n += {…}` only adds/overwrites the listed
keys. Pick `+=` for partial updates (almost always what you want). See
[Troubleshooting → SET wiped my properties](../troubleshooting#set-wiped-my-properties).

## Properties on relationships

Identical shape — relationships have their own property map.

<QueryCodeBlock code={String.raw`MATCH (a)-[r:KNOWS]->(b)
SET r.since = 2020, r.visibility = 'public'
RETURN r.since`} />

## Value types

Properties accept every [LoraDB data type](../data-types/overview):

| Category | Pages |
|---|---|
| Scalars (`Null`, `Boolean`, `Integer`, `Float`, `String`) | [Scalars](../data-types/scalars) |
| Collections (`List`, `Map`) | [Lists & Maps](../data-types/lists-and-maps) |
| Temporals (`Date`, `Time`, `DateTime`, `Duration`, …) | [Temporal](../data-types/temporal) |
| Spatial (`Point`) | [Spatial](../data-types/spatial) |
| `Vector` (typed fixed-dimension coordinates) | [Vectors](../data-types/vectors) |

Graph types (`Node`, `Relationship`, `Path`) are **not** storable as
properties — they only appear in query results.

A `VECTOR` can be a property value and can appear as a value inside a
`Map` property, but a **list that contains a `VECTOR` is rejected at
write time** — store many embeddings as separate nodes, not as a list.
See [Vectors → Storage](../data-types/vectors#storage) for the exact
rule.

## Common patterns

### Default via coalesce

<QueryCodeBlock code={String.raw`MATCH (u:User)
RETURN u.name, coalesce(u.nickname, u.name) AS display`} />

`coalesce` returns the first non-null argument, so users with a
nickname get it under `display` and everyone else falls back to
`name`. No extra row work — this is a per-row projection.

### Touch a timestamp on write

<QueryCodeBlock code={String.raw`MATCH (u:User {id: $id})
SET u.last_seen = temporal.timestamp()`} />

### Migrate a property

<QueryCodeBlock code={String.raw`MATCH (u:User)
WHERE u.full_name IS NOT NULL AND u.name IS NULL
SET u.name = u.full_name
REMOVE u.full_name`} />

Moves values from `full_name` to `name` on every matched row.
Because the predicate filters out rows that already have a `name`,
this is safe to re-run — users who are already migrated are
skipped.

### Conditional add

<QueryCodeBlock code={String.raw`MERGE (u:User {id: $id})
  ON CREATE SET u.created = temporal.timestamp()
  SET u.updated = temporal.timestamp()`} />

### Copy properties from one entity to another

<QueryCodeBlock code={String.raw`MATCH (src:Template {id: $src}), (dst:Record {id: $dst})
SET dst += properties(src)`} />

[`properties(src)`](../functions/overview#entity-introspection)
returns every key on `src` as a map; `+=` merges that map into
`dst`, overwriting matching keys and leaving anything unique to
`dst` untouched. Useful for applying a template over an existing
record without nuking custom fields.

### Bulk patch via UNWIND

<QueryCodeBlock code={String.raw`UNWIND $patches AS patch
MATCH (u:User {id: patch.id})
SET u += patch.fields`} />

### Derived property with CASE

<QueryCodeBlock code={String.raw`MATCH (u:User)
SET u.tier = CASE
               WHEN u.score >= 1000 THEN 'platinum'
               WHEN u.score >=  100 THEN 'gold'
               ELSE                       'bronze'
             END`} />

See [`CASE`](../queries/return-with#case-expressions).

### Compact property dump for debugging

<QueryCodeBlock code={String.raw`MATCH (n)
WHERE id(n) = $raw_id
RETURN labels(n)          AS labels,
       keys(n)            AS keys,
       properties(n)      AS props`} />

### Keys as a set

`keys(n)` is a list. Use list predicates to ask set-style questions:

<QueryCodeBlock code={String.raw`MATCH (u:User)
WHERE all(k IN ['email', 'name', 'created_at'] WHERE k IN keys(u))
RETURN u`} />

One row per user who carries all three required keys — the list
predicate [`all`](../functions/list#predicates-in-where) holds only
when every element of the input list passes the inner `WHERE`. Swap
`all` for `none` to find users *missing* a required key.

### Nullable property check

<QueryCodeBlock code={String.raw`MATCH (n)
WHERE n.optional IS NULL     RETURN n;   // missing or explicitly null
MATCH (n)
WHERE n.optional IS NOT NULL RETURN n   // present and non-null`} />

## Edge cases

### Missing vs null

A missing key and a key set to `null` are indistinguishable — both
return `null` on access. `SET n.prop = null` is the idiomatic way to
remove a key (see [SET](../queries/set-delete#clear-a-property)).

### Keys as strings

Keys are always case-sensitive strings. `user.Name` ≠ `user.name`.

### Type drift

Without a property type constraint, the same key can hold different
types on different entities:

<QueryCodeBlock code={String.raw`CREATE (:Item {stock: 5})
CREATE (:Item {stock: '5'})    // legal but will surprise you`} />

Use [`type.of`](../functions/overview#type-conversion-and-checking)
to detect at query time, normalise on write, or add a
[property type constraint](../queries/constraints#property-types) for
labels or relationship types that need a fixed shape.

### Indexes and nested values

Property filters without a label are broad scans. Always scope to a
label when you can, and add an explicit index for hot top-level
properties:

<QueryCodeBlock code={String.raw`CREATE INDEX user_email FOR (u:User) ON (u.email)
MATCH (u:User {email: $email}) RETURN u

// Broad scan
MATCH ({email: $email}) RETURN`} />

See [Queries → Indexes](../queries/indexes).

### Deep property paths

Accessing nested map keys works:

<QueryCodeBlock code={String.raw`MATCH (c:City)
RETURN c.location.latitude, c.tags[0]`} />

But explicit indexes target top-level entity properties, not inner map
paths. For frequent inner-key filters, promote to a dedicated property
at write time.

## Notes

- Property keys are case-sensitive strings.
- A single entity can hold any number of properties.
- Required properties and property types are optional constraints; when
  no constraint is present, different nodes with the same label can have
  different property sets.
- For the full type catalogue see [Data Types](../data-types/overview).

## See also

- [**Graph Model**](./graph-model) — where properties fit.
- [**Nodes**](./nodes) / [**Relationships**](./relationships) — carriers.
- [**Data Types**](../data-types/overview) — value types.
- [**SET / REMOVE / DELETE**](../queries/set-delete) — mutation clauses.
- [**WHERE**](../queries/where) — property-based filters.
- [**Constraints**](../queries/constraints) — required properties,
  uniqueness, keys, and property types.
- [**Functions → Entity introspection**](../functions/overview#entity-introspection)
  — `keys`, `properties`, `id`.
