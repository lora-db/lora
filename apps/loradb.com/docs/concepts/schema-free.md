---
title: Schema-Free Writes and Soft Validation
sidebar_label: Schema-Free
description: How LoraDB handles schema — labels, relationship types, and property keys spring into existence on write, while reads stay strict — and where validation still applies.
---

# Schema-Free Writes and Soft Validation

LoraDB has no `CREATE TABLE` step. Labels, relationship types, and
property keys are created the first time you use them in a write —
and the engine keeps two different sets of rules for writes and
reads.

In one sentence:

> **Writes are permissive, reads are strict.** `CREATE` / `MERGE` /
> `SET` accept any label, type, or property key you throw at them.
> `MATCH` refuses labels and types it has never seen.

## What "schema-free" actually means

The graph stores:

- A set of **labels** seen on any node since process start.
- A set of **relationship types** seen on any edge.
- The **property keys** seen on any node or edge.

No declaration, no `ALTER TABLE`, no migration. The first write that
mentions a new name brings it into existence; subsequent writes reuse
it.

```cypher
CREATE (c:Country {name: 'NL', iso: 'NLD'})
```

On an empty graph, this creates the label `Country` and the property
keys `name` and `iso`. The next `MATCH (:Country)` will succeed.

### The opposite is not true

A `MATCH` for a label that was **never** created fails at analysis:

```cypher
MATCH (u:NeverWritten) RETURN u
-- Unknown label :NeverWritten
```

This is deliberate — typo-catching. The alternative (silently return
zero rows) hides the bug until your integration tests reach
production. See [Troubleshooting → Semantic errors](../troubleshooting#semantic-errors).

## Permissive writes

`CREATE`, [`MERGE`](../queries/unwind-merge#merge), and
[`SET`](../queries/set-delete) accept any name without complaint.

```cypher
CREATE (:Spaceship {name: 'Rocinante', crew: 4})
-- "Spaceship" was never declared. Fine — it now exists.

MATCH (s:Spaceship)
SET s.engine = 'Epstein drive'
-- Adds a new property key; totally legal.
```

This is good for quick iteration and bad for safety. There is no
constraint preventing you from creating a second `:Spaceship` with
completely different properties, or from typo-ing `Spaceshi` and
polluting the label set.

### Things the engine won't catch

- Two `:Person` nodes with different property sets
  (`{name, born}` vs `{username, dob}`).
- A property named `email` on one node and `e_mail` on another.
- A `:FOLLOWS` edge with an `active` property on one and not on
  another.
- A property value that's an `Integer` in one place and `String` in
  another.

If any of these matter, enforce them at the application layer, or in
a `MATCH`-before-`CREATE` idiom, or with [`MERGE`](#merge-for-idempotent-writes).

## Strict reads

[`MATCH`](../queries/match) validates label and relationship-type
names against live graph state. The "live" part matters:

- On an **empty** graph, every label and type is unknown — but
  `MATCH (:Foo)` on an empty graph succeeds with zero rows. There's
  nothing to validate against.
- On a **populated** graph, the label has to have been seen before.

Property keys in `MATCH` are **not** validated this way — a missing
property simply yields `null` on access. See
[Properties → missing vs null](./properties#missing-vs-null).

### Reading back what you wrote

The two rules meet cleanly in this pattern:

```cypher
CREATE (:Spaceship {name: 'Rocinante'});
MATCH (s:Spaceship) RETURN s;  -- works — :Spaceship now exists
```

And break in this one:

```cypher
-- Empty graph
MATCH (s:NeverWritten) RETURN s;  -- analysis error on a populated graph
```

## `MERGE` for idempotent writes

`MERGE` is the closest thing LoraDB has to a uniqueness constraint —
it matches on the given pattern, creating only if missing:

```cypher
MERGE (u:User {email: $email})
  ON CREATE SET u.created = timestamp()
  ON MATCH  SET u.last_seen = timestamp()
```

It's an important building block for schema-free writes:

- **Safe upsert** — a repeated run won't create duplicates.
- **No indexes required** — `MERGE` does a full-label scan on the key
  map, which is fine for moderate scales. See
  [Limitations → Storage](../limitations#storage).

See [MERGE](../queries/unwind-merge#merge) for the full reference.

## Runtime type checks

Because a property's type is only enforced when written — not when
declared — you occasionally need to verify it at query time:

```cypher
MATCH (r:Record)
WHERE valueType(r.id) = 'INTEGER'
RETURN r
```

See [Functions → type conversion and checking](../functions/overview#type-conversion-and-checking)
for `valueType`, `toInteger`, `toString`, and friends.

## Trade-offs at a glance

| Property | Traditional schema | LoraDB |
|---|---|---|
| Declare up front | Required (`CREATE TABLE`) | Not required |
| Add a new property | Migration | Just `SET it` |
| Enforce "every node has X" | Constraint | Application code |
| Enforce "X is unique" | `UNIQUE` | `MERGE` on the key |
| Catch typos in writes | Schema | Code review / tests |
| Catch typos in reads | Schema | Analyzer rejects unknown labels/types |
| Index lookups | Fast | No property indexes — scope to a label |

## When to add a "soft schema" at the app layer

Schema-free is a tool, not a lifestyle. If your data model stabilises,
pin it down in host code:

- A small module that returns the valid labels / types and fails fast
  on typos.
- A `create_user` function that's the *only* writer of `:User` nodes
  and always sets the same property keys.
- A `MERGE` on the business key rather than letting callers fan out
  to different shapes.

You lose the "schema" catch-your-typo net. Good architecture puts the
net back, where it's cheap.

## See also

- [Graph data model](./graph-model) — nodes, relationships, properties.
- [MERGE](../queries/unwind-merge#merge) — idempotent writes.
- [Properties](./properties) — missing vs null, value typing.
- [Troubleshooting → Semantic errors](../troubleshooting#semantic-errors)
  — typo-catching on reads.
- [Limitations → Storage](../limitations#storage) — no indexes, no
  constraints.
