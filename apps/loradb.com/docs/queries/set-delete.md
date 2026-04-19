---
title: SET, REMOVE, DELETE — Mutating the Graph
sidebar_label: SET / REMOVE / DELETE
---

# SET, REMOVE, DELETE — Mutating the Graph

Mutating clauses always act on rows produced by a preceding
[`MATCH`](./match). They never run without a binding — a `SET` that
matched nothing is a silent no-op.

## Overview

| Goal | Clause |
|---|---|
| Change one property | <CypherCode code="SET n.prop = value" /> |
| Merge properties into a map | <CypherCode code="SET n += {k: v}" /> |
| Replace the whole property map | <CypherCode code="SET n = {k: v}" /> |
| Remove a property | <CypherCode code="REMOVE n.prop" /> or <CypherCode code="SET n.prop = null" /> |
| Add labels | <CypherCode code="SET n:Label" /> |
| Remove labels | <CypherCode code="REMOVE n:Label" /> |
| Delete a relationship | <CypherCode code="MATCH ()-[r]->() DELETE r" /> |
| Delete a standalone node | <CypherCode code="MATCH (n) DELETE n" /> |
| Delete a node **and** its edges | [<CypherCode code="DETACH DELETE" />](#detach-delete) |

## SET — properties

### Set a single property

```cypher
MATCH (n:User {name: 'Alice'})
SET n.age = 33
RETURN n
```

Set multiple keys in one clause by chaining with commas:

```cypher
MATCH (n:User {name: 'Alice'})
SET n.age = 33, n.updated = timestamp()
RETURN n
```

### Replace all properties (`=`)

`SET n = {...}` **replaces** the full property map. Every key not in
the new map is dropped — including keys you never touched.

```cypher
MATCH (n:User {name: 'Alice'})
SET n = {name: 'Alice', age: 33}
RETURN n
```

Almost always a mistake unless you really mean it. Use `+=` to merge. See
[Troubleshooting → SET wiped my properties](../troubleshooting#set-wiped-my-properties).

### Merge properties (`+=`)

`SET n += {...}` adds / overwrites keys without removing anything else.

```cypher
MATCH (n:User {name: 'Alice'})
SET n += {age: 33, city: 'Amsterdam'}
RETURN n
```

Combines naturally with parameter maps for patch-style updates:

```cypher
MATCH (u:User {id: $id})
SET u += $patch
RETURN u
```

Where `$patch = {name: 'New Name', active: true}`.

### Computed expressions

The right-hand side is any expression — reference other properties, call
functions, do arithmetic.

```cypher
MATCH (n:User) SET n.doubled = n.age * 2       RETURN n
MATCH (n:User) SET n.greeting = 'Hello ' + n.name RETURN n
MATCH (n:User) SET n.updated = timestamp()     RETURN n
```

### Clear a property

Setting a property to `null` removes it — the property simply ceases to
exist on that entity.

```cypher
MATCH (n:User {name: 'Alice'})
SET n.archived = null
RETURN n
```

`REMOVE n.prop` does the same.

### Copy properties from another entity

```cypher
MATCH (src:Template {id: $src}), (dst:Record {id: $dst})
SET dst += properties(src)
RETURN dst
```

[`properties()`](../functions/overview#entity-introspection) returns the
full map; `+=` folds it in.

## SET — labels

Adding a label on a node already carrying it is a no-op.

```cypher
MATCH (n:User {name: 'Alice'})
SET n:Admin
RETURN labels(n)

MATCH (n:User {name: 'Alice'})
SET n:Admin:Verified
RETURN labels(n)
```

## REMOVE

### Remove a label

```cypher
MATCH (n:User {name: 'Alice'})
REMOVE n:Admin
RETURN labels(n)
```

### Remove a property

```cypher
MATCH (n:User {name: 'Alice'})
REMOVE n.age
RETURN n
```

Equivalent to `SET n.age = null`. Use whichever reads more clearly at
the call site.

### Remove multiple labels / properties

Chain with commas:

```cypher
MATCH (n:User {name: 'Alice'})
REMOVE n:Admin, n:Verified, n.lastAudit
RETURN n
```

## DELETE

### Delete a node

```cypher
MATCH (n:User {name: 'Temp'})
DELETE n
```

A plain `DELETE` on a node requires the node to have **no
relationships**. Otherwise the executor returns
`DeleteNodeWithRelationships` — see
[Troubleshooting → DELETE fails](../troubleshooting#delete-fails-with-still-has-relationships).

### DETACH DELETE

Removes the node **and** every relationship attached to it in one step.

```cypher
MATCH (n:User {name: 'Alice'})
DETACH DELETE n
```

Use this for ordinary "delete the user" semantics. Plain `DELETE` is
rarely the right call on a node.

### Delete a relationship

```cypher
MATCH (a:User {name: 'Alice'})-[r:FOLLOWS]->(b:User)
DELETE r
```

Node endpoints survive — the edge alone is removed.

### Delete everything

```cypher
MATCH (n) DETACH DELETE n
```

Empties the graph. Note: there is no `TRUNCATE` clause. All bindings
expose a `clear()` helper that is faster and clearer — see
[Node → other methods](../getting-started/node#other-methods) /
[Python → other methods](../getting-started/python#other-methods).

## Common patterns

### Upsert (create-or-update)

```cypher
MERGE (u:User {id: $id})
  ON CREATE SET u.created = timestamp()
  SET u.name = $name, u.updated = timestamp()
RETURN u
```

`ON CREATE` only runs on insert; the trailing `SET` runs in both
branches. See [`MERGE`](./unwind-merge#merge).

### Patch with merge-semantics

```cypher
MATCH (u:User {id: $id})
SET u += $patch
RETURN u
```

Safe pattern for partial updates from a client payload.

### Touching a timestamp

```cypher
MATCH (n:User {id: $id})
SET n.last_seen = timestamp()
```

### Convert / migrate a property

```cypher
MATCH (u:User)
WHERE u.full_name IS NOT NULL AND u.name IS NULL
SET u.name = u.full_name
REMOVE u.full_name
```

Two-step rewrite in a single query — `SET` runs first, then `REMOVE`.

### Conditional label

```cypher
MATCH (u:User)
WHERE u.score >= 100
SET u:Pro
```

### Conditional value via CASE

[`CASE`](./return-with#case-expressions) is an expression, so it
composes into `SET`. Single-row-aware derivation:

```cypher
MATCH (u:User)
SET u.tier = CASE
               WHEN u.score >= 1000 THEN 'platinum'
               WHEN u.score >=  100 THEN 'gold'
               WHEN u.score >=   10 THEN 'silver'
               ELSE                       'bronze'
             END
```

Pairs cleanly with `MERGE`:

```cypher
MERGE (u:User {id: $id})
  ON CREATE SET u.tier = 'bronze', u.created = timestamp()
  SET u.last_seen = timestamp(),
      u.tier      = CASE
                      WHEN u.score >= 100 THEN 'gold'
                      ELSE                       coalesce(u.tier, 'bronze')
                    END
```

### Bulk patch via UNWIND

```cypher
UNWIND $patches AS patch
MATCH (u:User {id: patch.id})
SET u += patch.fields
```

Where `$patches = [{id: 1, fields: {name: '…'}}, …]`.

## Edge cases

### Mutate affects every matched row

A broad `MATCH` with a `SET` runs once per matched row. This is
intentional and powerful, but easy to misuse:

```cypher
-- Sets archived=true on EVERY user
MATCH (u:User) SET u.archived = true
```

Narrow the `MATCH` with [`WHERE`](./where) or inline properties to scope
the change.

### Mutate on `OPTIONAL MATCH`

`SET n.x = 1` after an `OPTIONAL MATCH` that missed will run on `null`
— a runtime error. Guard with `WHERE n IS NOT NULL`.

### DETACH DELETE during a pattern with aggregation

Aggregations, [`RETURN`](./return-with), and mutations interact in
complex ways. Prefer a simple `MATCH … DETACH DELETE n` step over
combining with aggregations in one query.

### Atomicity

Each query is one atomic step. There is no explicit transaction
boundary. If a query fails partway through execution, partial writes are
possible — keep mutations scoped and small where possible. See
[Queries → Execution model](./#execution-model).

### No uniqueness enforcement

`SET` can set a property to a value that already exists on another node.
LoraDB has no uniqueness constraints — see
[Limitations → Storage](../limitations#storage). Enforce uniqueness in
the host application or by matching before writing.

## See also

- [**MATCH**](./match) — source of bindings.
- [**MERGE**](./unwind-merge#merge) — create-or-match semantics.
- [**CREATE**](./create) — write-only counterpart.
- [**Properties**](../concepts/properties) — data-model background.
- [**Data Types**](../data-types/overview) — what values `SET` accepts.
- [**Troubleshooting**](../troubleshooting) — common mutation pitfalls.
