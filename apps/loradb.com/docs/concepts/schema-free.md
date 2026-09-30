---
title: Schema-Free Writes and Soft Validation
sidebar_label: Schema-Free
description: How LoraDB handles schema — labels, relationship types, and property keys spring into existence on write, reads never depend on which names exist — and where validation still applies.
---

# Schema-Free Writes and Soft Validation

**Writes are permissive, and so are reads.** `CREATE` / `MERGE` /
`SET` accept any label, relationship type, or property key without a
`CREATE TABLE` step — names come into existence the first time they're
written. `MATCH` accepts any name too: a label or relationship type no
entity carries simply matches nothing, and a missing property key
yields `null`.

That is standard Cypher behaviour, and it keeps answers stable: the same
query gives the same *kind* of answer on an empty graph, a populated
one, and one whose last `:Comment` was just deleted or rolled back.
Whether a query is valid never depends on what data happens to exist.

## What "schema-free" actually means

The graph tracks three things as the process runs:

- The set of **labels** seen on any node since process start.
- The set of **relationship types** seen on any relationship.
- The **property keys** seen on any node or relationship.

No declaration, no `ALTER TABLE`, no migration. The first write that
mentions a new name brings it into existence; subsequent writes reuse
it.

<QueryCodeBlock code={String.raw`CREATE (c:Country {name: 'NL', iso: 'NLD'})`} />

On an empty graph, this creates the label `Country` and the property
keys `name` and `iso`. The next `MATCH (:Country)` will find it.

### Unknown names are not errors

A `MATCH` for a label that was **never** created is still a valid
query — it returns zero rows:

<QueryCodeBlock code={String.raw`MATCH (u:NeverWritten) RETURN u
// 0 rows`} />

The flip side: a typo such as `:Persn` also returns zero rows instead of
failing. Catch those in tests or with a small app-layer list of valid
labels (see [below](#when-to-add-a-soft-schema-at-the-app-layer)).

## Permissive writes

`CREATE`, [`MERGE`](../queries/unwind-merge#merge), and
[`SET`](../queries/set-delete) accept any name without complaint.

<QueryCodeBlock code={String.raw`CREATE (:Spaceship {name: 'Rocinante', crew: 4})
;// "Spaceship" was never declared. Fine — it now exists.

MATCH (s:Spaceship)
SET s.engine = 'Epstein drive'
// Adds a new property key; totally legal.`} />

This is good for quick iteration and bad for safety. Unless you add a
[constraint](../queries/constraints), nothing prevents you from creating
a second `:Spaceship` with completely different properties, and nothing
ever stops you from typo-ing `Spaceshi` and polluting the label set.

### Things the engine won't catch without constraints

- Two `:Person` nodes with different property sets
  (`{name, born}` vs `{username, dob}`).
- A property named `email` on one node and `e_mail` on another.
- A `:FOLLOWS` edge with an `active` property on one and not on
  another.
- A property value that's an `Integer` in one place and `String` in
  another.

If any of these matter, declare [constraints](../queries/constraints)
(existence, uniqueness, key, or property type), enforce them at the
application layer, or use [`MERGE`](#merge-for-idempotent-writes).
Differently-named keys such as `email` vs `e_mail` are only ever caught
at the application layer.

## Lenient reads

[`MATCH`](../queries/match) does not check label, relationship-type, or
property-key names against the stored data. The same rules hold on an
empty graph, a populated one, and after the last entity of a kind was
deleted, had its label removed, or was rolled back:

- A label or relationship type no entity carries matches nothing.
- A property key an entity doesn't have yields `null` on access. See
  [Properties → missing vs null](./properties#missing-vs-null).
- `OPTIONAL MATCH` over an unknown name keeps the outer row with
  `null`s; `UNION` branches over unknown names contribute no rows.

### Reading back what you wrote

<CypherSnippet code={String.raw`CREATE (:Spaceship {name: 'Rocinante'});
MATCH (s:Spaceship) RETURN s;      // 1 row
MATCH (s:NeverWritten) RETURN s;   // 0 rows — not an error`} />

## `MERGE` for idempotent writes

`MERGE` is the write-side idempotency tool: it matches on the given
pattern, creating only if missing. Add a uniqueness constraint when you
also need the database to reject duplicate keys:

<QueryCodeBlock code={String.raw`MERGE (u:User {email: $email})
  ON CREATE SET u.created = temporal.timestamp()
  ON MATCH  SET u.last_seen = temporal.timestamp()`} />

It's an important building block for schema-free writes:

- **Safe upsert** — a repeated run won't create duplicates.
- **Constraint-friendly** — a matching uniqueness constraint rejects
  competing duplicate writes. See [Constraints](../queries/constraints).
- **No index required** — without a supporting index or constraint,
  `MERGE` scans the label/type scope for the key map, which is fine for
  moderate scales. See [Limitations → Storage](../limitations#storage).

See [MERGE](../queries/unwind-merge#merge) for the full reference.

## Runtime type checks

Because a property's type is only enforced when written — not when
declared — you occasionally need to verify it at query time:

<QueryCodeBlock code={String.raw`MATCH (r:Record)
WHERE type.of(r.id) = 'INTEGER'
RETURN r`} />

See [Functions → type conversion and checking](../functions/overview#type-conversion-and-checking)
for `type.of`, `toInteger`, `toString`, and friends.

## Trade-offs at a glance

| Property | Traditional schema | LoraDB |
|---|---|---|
| Declare up front | Required (`CREATE TABLE`) | Not required |
| Add a new property | Migration | Just `SET it` |
| Enforce "every node has X" | Constraint | [Existence constraint](../queries/constraints) (`IS NOT NULL`) |
| Enforce "X is unique" | `UNIQUE` | [Uniqueness constraint](../queries/constraints) + `MERGE` on the key |
| Catch typos in writes | Schema | Code review / tests |
| Catch typos in reads | Schema | Tests / app-layer name list — unknown names return zero rows |
| Index lookups | Explicit schema-managed indexes | Optional RANGE/TEXT/POINT/LOOKUP/VECTOR/FULLTEXT indexes for performance and search |

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
  — what the analyzer still rejects.
- [Limitations → Storage](../limitations#storage) — scoped index and
  constraint coverage.
