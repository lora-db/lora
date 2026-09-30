---
title: GraphQL Error Reference
sidebar_label: Errors
description: Every error code @loradb/lora-graphql returns in extensions.code, what raises it, the extra fields it carries, and how clients should handle it; plus ModelError at startup.
---

# Errors

`@loradb/lora-graphql` fails in two places:

- **At startup**, an invalid SDL throws one [`ModelError`](#modelerror)
  listing every problem.
- **At request time**, errors are standard GraphQL errors with a stable
  `extensions.code`.

Clients should branch on `extensions.code`, never on `message`: messages
are written for developers and may change.

## Request error codes

| Code | Raised when | Extra `extensions` | Client should |
| --- | --- | --- | --- |
| `BAD_USER_INPUT` | An argument is valid GraphQL but not a valid request | | Fix the request |
| `INVALID_CURSOR` | A cursor is malformed, forged, or from another sort | | Restart from the first page |
| `LIMIT_EXCEEDED` | A page, batch, document or subscriber queue is over its limit | | Ask for less, or resubscribe |
| `COST_EXCEEDED` | The operation's estimated rows exceed the limit | `cost`, `maxCost` | Ask for smaller pages or fewer nested lists |
| `UNAUTHENTICATED` | An operation or field needs an authenticated request | | Sign in |
| `FORBIDDEN` | The caller's claims fail a rule, or introspection is off | | Not retry |
| `NOT_FOUND` | A `connect`, `disconnect` or nested update names a node that does not exist or the caller cannot see | `type`, sometimes `keys` | Fix the key |
| `CONSTRAINT_VIOLATION` | A write would break a key, uniqueness, required value or cardinality rule | `type`, `field` | Fix the input |
| `DATABASE_ERROR` | The engine failed: a timeout, a cancelled request, or an unexpected error | `id` | Retry once, then report the `id` |
| `PERSISTED_QUERY_ONLY` | `execute()` got a document while `persistedOnly` is on | | Send a persisted id |

Every mutation runs in one transaction, so any error in it rolls the
whole mutation back: nothing is partially written.

### BAD_USER_INPUT

Input that passes GraphQL validation but that the library refuses. For
example:

- `limit` or `first` is negative, or `first` and `last` are both given;
- a `sort` item names more than one field, or the same field twice;
- a create leaves out a required field;
- an upsert sets a field that is settable only on update for a new key,
  or only on create for an existing key (plain create and update inputs
  leave such fields out, so GraphQL validation catches them there);
- an update both sets and adjusts the same field, or sets a required
  field to `null`;
- a single relationship gets both `connect` and `create`;
- a required relationship is left unset, or disconnected without a
  replacement;
- a vector has the wrong number of dimensions, or a similarity query
  gives both `vector` and `to` (or neither);
- a `@cypher`-computed field is used in a relationship filter or a
  nested sort, where it cannot run.

Updating a `@key` is not a `BAD_USER_INPUT`: the update input has no
`@key` field, so GraphQL validation rejects it.

### INVALID_CURSOR

A cursor is opaque and belongs to one sort. The library rejects a cursor
that does not decode, one produced under a different `sort` ("request the
first page again"), and, with `cursorSecret`, one whose signature does not
match. Changing `cursorSecret` invalidates every cursor in circulation.

### LIMIT_EXCEEDED

The library never clamps silently. It raises `LIMIT_EXCEEDED` when:

- `limit`, `first` or `last` is above `@limit(max:)` or `maxLimit`
  (default 100);
- a bulk update or delete, or a nested `delete`, matches more nodes than
  its `limit` (default `maxBatch`, 1000). Nothing is written;
- a mutation would create or delete more than `maxBatch` nodes;
- a document is over a guard: depth, aliases, root fields or tokens;
- a subscriber or `changes()` consumer falls `maxQueuedChanges` behind.
  The stream ends; resubscribe and reload.

### COST_EXCEEDED

The operation's estimated rows touched are over `maxCost` (default
50 000) or the request's `budget`. The check runs before each root field
executes and accumulates across the operation's root fields, so
repeating a field under aliases does not get around it. The error names
the field, the estimate and the limit, and `extensions` carries `cost`
and `maxCost`.

### UNAUTHENTICATED and FORBIDDEN

`UNAUTHENTICATED` means the request has no `jwt` in its context and the
operation or field has `@authentication`. `FORBIDDEN` means the request
has claims, but they fail an `@authentication(jwt:)` test or an
`@authorization` validate rule. Rows hidden by `@authorization` filter
rules are simply absent: they do not produce an error.

`FORBIDDEN` also covers:

- sorting, filtering or aggregating by a field that has row-level read
  rules, where it would reveal values the caller cannot read;
- introspection when it is off (`__typename` is always allowed).

### NOT_FOUND

Raised only by writes that name a node:

- `connect` to a key that does not exist or that the caller cannot see.
  `extensions.keys` lists the missing keys;
- `disconnect` or a nested `update` of a node that is not connected.

A root `updateFestival(key:)` or `deleteFestival(key:)` for a key that
does not exist is **not** an error: it returns `festival: null` or
`nodesDeleted: 0`. Check the result if the difference matters.

### CONSTRAINT_VIOLATION

A write would leave the graph in a state the model forbids. `extensions`
names the `type` and `field`:

- a `@key` or `@unique` value is taken ("must be unique; the value is
  taken"). A `@key` held by a node the caller cannot read is not reported
  this way: the create answers as it would for a free key, with
  `FORBIDDEN` where a free key would be created;
- the same key appears twice in one input;
- a required property would be missing, including a required
  `@populatedBy` field whose callback returned `null`;
- a single relationship would hold more than one node, from either side;
- a required relationship would become unset, including when its target
  is deleted;
- a delete is blocked by `@relationship(onDelete: RESTRICT)`.

### DATABASE_ERROR

The engine failed in a way the library does not map to one of the codes
above: a statement over `timeoutMs`, a request cancelled through its
`signal`, a mutation on a driver without transactions, or an unexpected
engine error.

With `maskErrors` (default when `NODE_ENV` is `production`) the message
is replaced by `database error (id ...)`, and `extensions.id` matches the
`id` your `onError` hook received with the full detail. Without masking,
the engine's message is kept and the `id` is still added. See
[observability](/docs/graphql/observability#database-errors).

## Errors without a code

Some errors come from `graphql-js` or the server rather than the library:

- **Validation errors** ("Cannot query field ...", "Field ... is not
  defined by type ...") for documents that do not match the schema. A
  filter or sort that the schema did not opt into shows up this way,
  because the generated input types do not have it. Servers often add
  their own code, such as `GRAPHQL_VALIDATION_FAILED`.
- **Unknown persisted operation** from `execute({ id })` when the id is
  not registered.

## ModelError

`new LoraGraphQL()`, `buildModel()` and every CLI command validate the
SDL and throw a single `ModelError` that lists every problem, each
located by type and field:

```text
Invalid LoraGraphQL type definitions (2 problems):
  - Festival.capacity: Int does not support CONTAINS (allowed: EQ, IN, LT, LTE, GT, GTE, IS_NULL)
  - Festival.top: the statement uses $nope, which is neither an argument nor $jwt
```

`error.problems` holds the same list as `{ type?, field?, message }`
objects, and `formatProblem(problem)` formats one as above. Typical
problems:

- unknown directives, unknown arguments, or arguments of the wrong type;
- a `@node` type without exactly one `@key`, or a `@key` that is not a
  non-null `String`, `ID`, `Int` or `BigInt`;
- a required field that no create can set (add `@default`, `@timestamp`,
  `@populatedBy(operations: [CREATE])` or `@key(generate: true)`, or drop
  `CREATE` from `@mutation`);
- a filter operator the field's type does not support;
- a `@relationship` to a type that is not a `@node`, interface or union;
- a `@cypher` statement that uses an undeclared `$parameter`, or writes
  outside `Mutation`;
- an `@authorization` rule that names an unknown field, operator or
  undeclared claim, or is empty;
- a `@populatedBy` callback or `@customResolver` resolver missing from
  the options.

Problems that do not stop the model, such as an unused `@cypher`
argument, are warnings in `lora.model.warnings` and in `lora-graphql
check` output.
