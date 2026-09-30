# GraphQL: Claims Inside Rule Strings

Design for item G-19 of the Festimap brief: tie a client-chosen `@key` to
its owner. Companion to [graphql-threat-model.md](graphql-threat-model.md).

## Problem

A rule can use a claim only as a whole string. `"$jwt.sub"` becomes the
caller's `sub`, but `"$jwt.sub-"` is read as a claim named `sub-` and the
model refuses it. So "a plan's key is `<my sub>-<slot>`" cannot be
written. The workaround, `key: { startsWith: "$jwt.sub" }`, leaks: user
`p1` passes it with the key `p10-slot`, which is user `p10`'s natural key.
A user can pre-create another user's keys and block their creates with
`CONSTRAINT_VIOLATION`.

## Options

1. **Interpolate claims inside rule strings**: `"${jwt.sub}-"`. The rule
   compares against a string built from the claim, so the key space of a
   user is `<sub>-…`, and the separator ends the claim.
2. **Scope keys per owner** (`@key(scope: "owner")`): uniqueness on
   `(owner, key)`. Keys address nodes everywhere in the API (`festival(key:)`,
   connect, cursors, change events), and they are globally unique by design.
   A scoped key changes every lookup into a pair and every constraint into
   a composite one. That is a new addressing model, not a rule feature.

Option 1 is small, local to the rule compiler and fully checked at
startup. It is what this design adopts. Option 2 is not pursued.

## Design

### Syntax

In the `node` part of a `filter` or `validate` rule, a string may embed
placeholders:

| Form                        | Meaning                                                                        |
| --------------------------- | ------------------------------------------------------------------------------ |
| `"$jwt.sub"`                | Unchanged: the whole string is the claim, with its own type (list, number, …). |
| `"$context.tenant"`         | Unchanged: the whole string is the context value.                              |
| `"${jwt.sub}-"`             | New: a string, with the claim's value in place of the placeholder.             |
| `"${context.t}:${jwt.sub}"` | New: several placeholders, from claims and context values.                     |

`${...}` was chosen over `$jwt.sub-` because the end of the name is
explicit: `$jwt.sub.x` could be a nested claim or `sub` followed by `.x`.
It reads like a JavaScript template literal, which is what it does.

### Values

- A placeholder takes a string, number, bigint or boolean claim, turned
  into its decimal or `true`/`false` text.
- A claim that is absent, `null`, a list or an object makes the rule
  **unknown**, exactly like a missing whole-string claim: false where it
  stands, and a `NOT` over it cannot make it true.
- The built string is bound as a parameter like any other rule value. It
  never reaches the statement text, so a claim cannot inject Cypher.
- A `${context.…}` placeholder records its read, so the compile cache
  keys on it like on a whole-string context value.

### Startup checks

- `${` followed by anything but `jwt.<path>}` or `context.<path>}` is a
  model error, as is an unterminated `${`.
- With a `@jwt` type, the claim's first segment must be one of its fields
  (the check whole-string claims already get).
- `"$jwt.sub-"` stays an error, and the message now points at
  `"${jwt.sub}-"`.
- Placeholders are only read in `node` parts. In a `jwt` part they are a
  model error: those parts compare claims with literals.

### What it guarantees, and what it does not

`key: { startsWith: "${jwt.sub}-" }` confines a user to keys that begin
with their `sub` and the separator. That is a key space per user only if
no `sub` contains the separator: user `a` passes the rule with the key
`a-b-slot`, which also begins with `a-b-`, the prefix of user `a-b`. Pick a
separator your subjects never contain (`:` for UUIDs or slugs), or match
the whole key with `eq` when it is the subject itself. The README and the
threat model state this.

## Tests

`test/consumer-regressions.test.ts`, G-19: the prefix rule refuses another
user's prefix (`p1` cannot create `p10:…`) and allows the caller's own, on
create and on update; a missing or non-scalar claim denies, under `NOT`
too; context placeholders; and the startup errors above.
