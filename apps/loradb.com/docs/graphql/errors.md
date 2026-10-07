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

## What an error looks like

A failed `connect`, as the client receives it:

```json
{
  "errors": [
    {
      "message": "Student.courses: no Course with code \"NOPE\"",
      "locations": [{ "line": 1, "column": 12 }],
      "path": ["updateStudent"],
      "extensions": {
        "code": "NOT_FOUND",
        "type": "Course",
        "keys": ["NOPE"]
      }
    }
  ],
  "data": null
}
```

`path` names the root field that failed, `extensions.code` is the stable
code, and some codes add fields of their own: here the type and the keys
that were not found. An operation refused for its cost carries the
numbers:

```json
{
  "errors": [
    {
      "message": "students would bring the operation to about 10100 rows touched; the limit is 2000. Ask for smaller pages or fewer nested lists.",
      "path": ["students"],
      "extensions": { "code": "COST_EXCEEDED", "cost": 10100, "maxCost": 2000 }
    }
  ],
  "data": null,
  "extensions": { "cost": 10100 }
}
```

A field-level failure leaves the rest of the response in place: `data`
holds what could be read, with `null` where the field failed. The
[authentication](/docs/graphql/authentication#one-field) page shows one.

### Handling errors in a client

```ts
type Coded = { message: string; extensions?: { code?: string } };

function handle(errors: readonly Coded[]) {
  for (const error of errors) {
    switch (error.extensions?.code) {
      case "UNAUTHENTICATED":
        return redirectToSignIn();
      case "FORBIDDEN":
        return show("You do not have access to this.");
      case "INVALID_CURSOR":
        return reloadFromFirstPage();
      case "BAD_USER_INPUT":
      case "CONSTRAINT_VIOLATION":
      case "NOT_FOUND":
        return showFormError(error.message);
      case "LIMIT_EXCEEDED":
      case "COST_EXCEEDED":
        return show("That request is too large. Narrow it and try again.");
      case "TIMEOUT":
      case "DATABASE_ERROR":
        return retryOnceThenReport(error);
      default:
        // No code: a validation or syntax error, which is a bug in the
        // client's document, not something the user can fix.
        return report(error);
    }
  }
}
```

More real responses, each with the request that caused it, are in
[queries and mutations by example](/docs/graphql/examples#errors-you-will-meet)
and the [authorization recipes](/docs/graphql/authorization-recipes).

## Request error codes

| Code | Raised when | Extra `extensions` | Client should |
| --- | --- | --- | --- |
| `BAD_USER_INPUT` | An argument is valid GraphQL but not a valid request | | Fix the request |
| `INVALID_CURSOR` | A cursor is malformed, forged, or from another sort | | Restart from the first page |
| `LIMIT_EXCEEDED` | A page, batch, document, subscription count or subscriber queue is over its limit | | Ask for less, or resubscribe |
| `COST_EXCEEDED` | The operation's estimated rows exceed the limit | `cost`, `maxCost` | Ask for smaller pages or fewer nested lists |
| `TIMEOUT` | A query's root fields together ran past `operationTimeoutMs` | `operationTimeoutMs` | Ask for less, or retry later |
| `UNAUTHENTICATED` | An operation or field needs an authenticated request | | Sign in |
| `FORBIDDEN` | The caller's claims fail a rule, or introspection is off | | Not retry |
| `NOT_FOUND` | A `connect` names a node that does not exist or the caller cannot see, or a nested update one that is not connected | `type`, sometimes `keys` | Fix the key |
| `CONSTRAINT_VIOLATION` | A write would break a key, uniqueness, required value or cardinality rule | `type`, `field` | Fix the input |
| `DATABASE_ERROR` | The engine failed: a timeout, a cancelled request, or an unexpected error | `id` | Retry once, then report the `id` |
| `PERSISTED_QUERY_ONLY` | `execute()` or `subscribe()` got a document while `persistedOnly` is on | | Send a persisted id |
| `WRONG_OPERATION_TYPE` | `execute()` got a subscription, or `subscribe()` a query or mutation | | Fix the server: route the operation to the other method |

Every mutation root field runs in one transaction, so any error in it
rolls that field back: nothing is partially written. A mutation with
several root fields commits each on its own unless you set
[`mutationTransaction: "operation"`](/docs/graphql/api-reference#security),
which makes the whole operation one transaction.

### Telling library errors apart

```ts
import { isLoraGraphQLError, LORA_GRAPHQL_ERROR_CODES } from "@loradb/lora-graphql";

for (const error of result.errors ?? []) {
  if (isLoraGraphQLError(error)) {
    // error.extensions.code is one of LORA_GRAPHQL_ERROR_CODES
  }
}
```

`LORA_GRAPHQL_ERROR_CODES` is the list in the table above, and
`LoraGraphQLErrorCode` is its union type.

### Repeated errors

A field that every row of a list refuses (an `@authentication` field read
anonymously, say) would produce one error per row: 10,000 copies for a
page of 100 by 100. `execute()` and `lora.envelopPlugin()` return such an
error once, at the first path, with `extensions.count` (how many rows) and
`extensions.pathPattern` (the path with list indices as `"*"`). Servers
that call `graphql-js` directly on `getSchema()` get one error per row.

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
  nested sort, where it cannot run;
- a `where` nests relationship filters deeper than `maxFilterDepth`
  (default 2), an `in` list is longer than `maxListFilter` (1000), or a
  string operand is longer than `maxStringFilter` (10,000 characters);
- a list argument of a `@cypher` field is longer than its `@size(max:)` or
  `maxListArgument` (1000), or a number is outside its `@range`;
- a vector the engine rejects as invalid.

`execute()` also gives this code to variables that fail coercion (a string
where the document declares `Int`). A server calling `graphql-js` directly
reports those without a code.

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
- a mutation would create, update or delete more than `maxBatch` nodes,
  or write more than ten times that many relationships;
- an upsert has more than `maxBatch` inputs, or a bulk `limit` argument is
  above `maxBatch`;
- a cascading delete would reach more than `maxBatch` nodes;
- a document is over a guard: depth, introspection depth, aliases or root
  fields. The token guard is the exception: it stops the parser, so it
  comes back as a syntax error [without a code](#errors-without-a-code);
- a caller opens more than `maxSubscriptions` subscriptions (default 100
  per scope), or a subscription's `where` nests relationship filters
  deeper than `maxSubscriptionFilterDepth` (default 1);
- a subscriber or `changes()` consumer falls `maxQueuedChanges` behind.
  The stream ends; resubscribe and reload.

### COST_EXCEEDED

The operation's estimated rows touched are over `maxCost` (default
50 000) or the request's `budget`. The check runs before each root field
executes and accumulates across the operation's root fields, so
repeating a field under aliases does not get around it. A subscription is
charged per event: each event's fields are checked against the limit on
their own, so a long-lived subscription does not run out of budget. The error names
the field, the estimate and the limit, and `extensions` carries `cost`
and `maxCost`.

### TIMEOUT

Two timeouts apply to a query. `timeoutMs` (default 10 seconds) bounds
each statement and surfaces as `DATABASE_ERROR`. `operationTimeoutMs`
(default twice that) bounds all root fields of the query together: past
it the statements still running are aborted and every unfinished root
field fails with `TIMEOUT`, while fields that already finished keep their
data. `extensions.operationTimeoutMs` holds the limit that applied.
Without it, an operation with twenty aliased root fields could run for
twenty times `timeoutMs`.

Mutations and subscriptions are not bounded by the operation deadline.

### UNAUTHENTICATED and FORBIDDEN

`UNAUTHENTICATED` means the operation or field has `@authentication` and
the request does not satisfy it: it has no `jwt` in its context, or its
claims fail the directive's `jwt` test. `FORBIDDEN` means an
`@authorization` validate rule refused the request. Rows hidden by `@authorization` filter
rules are simply absent: they do not produce an error.

`FORBIDDEN` also covers:

- sorting, filtering or aggregating by a field that has row-level read
  rules, where it would reveal values the caller cannot read;
- introspection when it is off (`__typename` is always allowed);
- replacing a single relationship whose current target the caller cannot
  read.

### NOT_FOUND

Raised only by writes that name a node:

- `connect` to a key that does not exist or that the caller cannot see.
  `extensions.keys` lists the missing keys;
- a nested `update` of a node that is not connected.

A `disconnect` of a key that is not connected is not an error: it is
ignored, and `relationshipsDeleted` shows what was removed.

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
above:

- a statement ran over `timeoutMs`;
- a write waited longer than `timeoutMs` for the writer lock ("timed out
  after N ms waiting for the writer lock"). LoraDB has one writer at a
  time, so this means writes are queuing behind a slow one;
- the request was cancelled through its `signal`;
- a mutation ran on a driver without transactions;
- the commit of an operation-level transaction failed;
- an unexpected engine error.

With `maskErrors` (default when `NODE_ENV` is `production`) the message
is replaced by `database error (id ...)`, and `extensions.id` matches the
`id` your `onError` hook received with the full detail. Without masking,
the engine's message is kept and the `id` is still added. See
[observability](/docs/graphql/observability#database-errors).

### WRONG_OPERATION_TYPE

`execute()` runs queries and mutations; `subscribe()` runs subscriptions.
Each returns this error, without running anything, for an operation of
the other kind. It points at the server's routing, not the client's
request.

## Errors without a code

Some errors come from `graphql-js` or the server rather than the library:

- **Validation errors** ("Cannot query field ...", "Field ... is not
  defined by type ...") for documents that do not match the schema. A
  filter or sort that the schema did not opt into shows up this way,
  because the generated input types do not have it. Servers often add
  their own code, such as `GRAPHQL_VALIDATION_FAILED`.
- **Syntax errors** for documents that do not parse, including a document
  over the `maxTokens` guard ("Document contains more that 5000 tokens.
  Parsing aborted.").
- **Unknown persisted operation** from `execute({ id })` when the id is
  not registered, and "a source or an id is needed" when it gets neither.

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
