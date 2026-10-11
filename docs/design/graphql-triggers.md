# lora-graphql: triggers and workers

Date: 2026-10-11. Status: built the same day, phases 1 to 4 and the docs
page of phase 5; what is open is under [What was built](#what-was-built).
Written against `main` at `5972d65` (v0.24.2).

The question: how does an application react to a node or relationship
being written (send a notification, keep a counter, call a webhook), and
how is that built into lora-graphql so that other features can sit on top
of it. This document records what exists, what was decided and why, and
the phases to build it.

Claims are marked as in `known-risks.md`: **Observed** was run or read in
the code, **Inferred** was deduced.

## Summary of the design

1. **A trigger runs inside the transaction of the mutation that fired
   it**, after the mutation's writes and checks and before the commit. It
   can write more, or throw to veto. What it writes commits or rolls back
   with the mutation.
2. **Triggers are declared in the SDL** with `@trigger`, as a Cypher
   statement that is plan-checked at startup like a `@cypher` field. A
   `triggers` option takes JavaScript functions for what Cypher cannot
   say.
3. **Triggers fire by write-set, not by root field.** A `Comment` created
   by a nested create inside `createPosts` fires the `Comment` trigger.
4. **A trigger's own writes join the mutation's write-set**, so
   subscriptions and `onWrite` see them without `changeFeed`.
5. **Triggers do not cascade.** A trigger's writes fire no other trigger.
6. **Events are nodes.** A notification is a `Notification` node written
   by a trigger. It is as durable as the data, with or without a WAL.
7. **A worker delivers side effects after the commit** (email, push,
   webhooks): it drains nodes of an `@outbox` type, at least once, with
   retries, and survives a restart by scanning for pending nodes.
8. **Clients are reached through the existing `@subscription`**, with the
   read rules already compiled in.
9. **This is API-level.** Raw Cypher and `@cypher` mutations fire no
   trigger, as they bypass validation and `@uniqueTogether`. Engine
   triggers are a separate project.

Three terms, kept apart in code and docs:

| Term | When | What |
| --- | --- | --- |
| Trigger | In the transaction, before commit | `@trigger`, `triggers` option |
| Event | After commit | `WriteChange`, `ChangeEvent` (exist today) |
| Worker | After commit, retried | `lora.worker()` over an `@outbox` type |

## What exists today

- **`onWrite` and `changes()`** (Observed, `subscriptions/change-hub.ts`).
  Called after every committed mutation of this instance with its exact
  write-set. Listeners run synchronously on the request path, their
  errors are swallowed, nothing is retried.
- **`changeFeed: true`** (Observed, `execute/feed.ts`,
  `crates/lora-database/src/changes/mod.rs`). Every committed write from
  any path, in commit order, with an LSN. The position is held in memory
  only (`feed.ts:26`), LSNs are stable across restarts only with a WAL,
  and 1024 batches are retained.
- **Generated subscriptions** (Observed, `schema/subscriptions.ts`), with
  read rules applied per subscriber.
- **A hook inside the transaction already exists** (Observed):
  `beforeDelete` in `execute/mutate/env.ts:78`, called from
  `mutate/delete.ts:169` once every doomed node is known and before any
  is removed.
- **The write-set is complete before the commit** (Observed):
  `runner.ts:269` and `:908` and `delete.ts:220` fill `change` as the
  writes run; `mutate.ts:294` commits.
- **Custom `Subscription` types are refused** (Observed,
  `model/build.ts:220`). There is no `publish()`.
- **A composed trigger statement runs in the engine** (Observed, probe on
  2026-10-11 against the built `@loradb/lora-node`): `UNWIND $keys AS key
  MATCH (this:Comment {id: key})` followed by a hand-written `MATCH ...
  CREATE ... RETURN n AS created` ran inside `db.begin()`, returned the
  created nodes with `labels` and `properties`, accepted `$jwt.sub`, and
  `explain()` reported `shape: "mutating"` and `resultColumns:
  ["created"]`. A broken statement failed `explain()` with a parse error.
- **Writes inside `CALL { }` are refused by the engine** (Observed,
  `execute/cypher-mutation.ts:1-2`), so a trigger statement is appended
  to its prefix, not wrapped.

## Why not handlers after the commit

The first design considered was `lora.on(type, operation, handler)` on top
of the change hub, with a persisted LSN cursor for durability. It was
dropped:

- The derived write is a second transaction. A crash between the two
  loses it, which is the only reason the cursor was needed.
- The cursor needs WAL-stable LSNs and fits inside a 1024-batch window.
  The database is in-memory first.
- A handler cannot veto the write.
- Nothing about it can be checked at startup.

## Decisions

### D1. The directive

```graphql
directive @trigger(
  on: [MutationOperation!]!
  statement: String!
  "Fire an UPDATE trigger only when one of these fields was set."
  fields: [String!]
) repeatable on OBJECT
```

Allowed on `@node` types only. Refused on interfaces, unions,
`@relationshipProperties` types and types without `@mutation`, each with a
message in `model/positions.ts`. `fields` is refused without `UPDATE` in
`on`.

### D2. What a statement sees

The library prepends the match and runs the statement as written:

```cypher
UNWIND $keys AS key
MATCH (this:Comment {id: key})
<statement>
```

- `this` is each written node of the type. One statement per trigger per
  mutation, whatever the number of nodes.
- Parameters: `$jwt` and `$viewer`, bound as for `@cypher` fields
  (`compile/read/cypher-field.ts:107-121`), and `$operation`.
- Values are always parameters. Nothing from the request is interpolated
  into the text.
- The statement runs under `timeoutMs` through `runStatement`, so
  `onStatement`, tracing and metrics see it.

### D3. When triggers run

| Operation | Runs | `this` is |
| --- | --- | --- |
| `CREATE` | After all writes and checks, before commit (`mutate.ts:294`) | The node as created |
| `UPDATE` | Same point | The node after the update |
| `DELETE` | At the `beforeDelete` point (`delete.ts:169`) | The node, still present |

- **Fired by write-set.** `CREATE` fires for `change.created` of the
  type, `UPDATE` for `change.updated`, `DELETE` for the doomed nodes,
  cascaded ones included. An upsert fires whichever happened.
- **Order.** Triggers of one type run in SDL order; types in the order
  the model lists them. Statement triggers run before JavaScript ones.
- **After the checks.** `checkUniqueTogether` and the hidden-key and
  unique checks (`mutate.ts:282-293`) run first, so a trigger never acts
  on a write that is about to be refused.
- **Shared transactions.** Under `lora.begin()` or
  `mutationTransaction: "operation"` triggers run at the end of each root
  field, inside the shared transaction.

### D4. A trigger's writes join the write-set

A statement reports what it wrote through two reserved result columns:

- `created`: a node or a list of nodes it created.
- `updated`: a node or a list of nodes it changed.

Each returned node is mapped to its `@node` type by label and to its key
by the key property (the mapping `EngineFeed.#entity` already does,
`feed.ts:170`) and added to `change.created` or `change.updated` before
`fillChange`. `fillChange` moves ahead of the commit for this.

- A statement that writes (`explain()` says `mutating`) and returns
  neither column makes the change `broad: true`, as a `@cypher` mutation
  does today (`lora-graphql.ts:1296`), and is a startup warning.
- A returned node of no `@node` type is ignored.
- Relationships a trigger creates are not reported in `connected`.
  Return both ends as `updated` when subscribers should hear of it.

### D5. No cascades

A trigger's writes fire no trigger, statement or JavaScript. This keeps
the cost of a mutation readable from the schema and makes loops
impossible. A chain is written as one statement, or the second step is a
worker.

### D6. Trigger writes are system writes

They are the schema author's own Cypher. They skip `@authorization`,
input validation, `@default`, `@timestamp` and `@populatedBy`, exactly as
a `@cypher` mutation does. Two checks still apply:

- `@uniqueTogether` types the statement may write are checked after it,
  with the scan `checkUniqueTogetherOf` already does for `@cypher`
  mutations (`cypher-mutation.ts:66`).
- `@outbox` state is stamped on nodes returned as `created` (D9).

### D7. JavaScript triggers

```ts
new LoraGraphQL({
  typeDefs,
  driver,
  triggers: {
    Comment: {
      create: async ({ keys, change, transaction, context }) => { ... },
      update: async (...) => { ... },
      delete: async (...) => { ... },
    },
  },
});
```

- `transaction` is a `LoraTransaction` over the mutation's transaction.
  `transaction.execute(cypher, params)` runs raw Cypher in it.
  `lora.execute({ ..., context: { ...context, transaction } })` runs a
  generated mutation in it, and its write-set is merged into the parent
  change. Such a nested mutation fires no trigger (D5).
- **Veto.** Throwing rolls the transaction back. A `GraphQLError` or a
  `requestError` reaches the client as thrown. Anything else is masked
  like a database error (`runtime/database-error.ts`) and reported to
  `onError` with `field: "trigger:Comment.create"`.
- **Bounded.** A JavaScript trigger holds the writer lock. It is ended
  with `TIMEOUT` after `triggerTimeoutMs` (default 1000) and the mutation
  rolls back. The docs say plainly: no network calls in a trigger, that
  is what workers are for.
- An unknown type or operation in `triggers` is a startup error.

### D8. What fires nothing

- `@cypher` mutations: the write-set is unknown.
- Raw Cypher through the driver, and `transaction.execute()`.
- Writes of other triggers and of workers.

This is the first paragraph of the docs page. Covering every write path
needs triggers in the engine.

### D9. Outbox types and workers

```graphql
type Notification @node @outbox @subscription(operations: [CREATE]) { ... }
```

`@outbox` gives the type library-owned stored properties that are never
part of the API: a state (`PENDING`, `CLAIMED`, `DONE`, `FAILED`), an
attempt count, the next attempt time, a lease expiry and the last error.
The state property gets an index through `analyze/indexes.ts`, so
`assertSchema` and `lora-graphql check` know of it.

- Generated creates set `PENDING` in the write plan. Trigger-created
  nodes returned as `created` are stamped in one extra statement, only
  when a trigger returned nodes of an `@outbox` type.
- Nodes created by raw Cypher are not stamped and not delivered.

```ts
const worker = lora.worker("Notification", {
  handle: async (notification, { attempt, signal }) => {
    await push.send(notification.recipientId, notification.kind);
  },
  concurrency: 4,       // default
  maxAttempts: 8,       // default, then FAILED and onError
  sweepMs: 30_000,      // default
  leaseMs: 60_000,      // default
});
```

- **Wake.** The worker registers with `ChangeHub.listen()` and takes the
  keys of created nodes of its type from each change: no polling in the
  normal case.
- **Sweep.** At start and every `sweepMs` it reads pending nodes whose
  next attempt is due and claimed nodes whose lease expired. This is
  what recovers from a crash or a restart.
- **Claim.** One write sets `CLAIMED` and the lease. The writer lock
  makes it atomic, so two instances on one engine do not both take a
  node.
- **Handle.** The node is read by key as a system read (all stored
  scalar fields, no read rules).
- **Finish.** Success sets `DONE` (or deletes the node). Failure
  increments the attempt, sets the next attempt with exponential backoff,
  and after `maxAttempts` sets `FAILED` and calls `onError` with `field:
  "worker:Notification"`.
- **No noise.** State writes change no API-visible field, so they emit
  no `WriteChange`.
- **At least once.** A handler may run twice for one node (a crash after
  the side effect, before `DONE`). Handlers must be idempotent; the
  node's key is the idempotency key.
- **Lifecycle.** `worker.stop()`, `worker.idle()` (resolves when nothing
  is pending, for tests), and `lora.close()` stops every worker.

### D10. Later, not in this plan

- `CONNECT` and `DISCONNECT` triggers on relationship fields.
- Previous values in an `UPDATE` trigger (today only with
  `previousState`).
- `lora.publish()` and custom `Subscription` fields for events that
  should not be stored (typing indicators, presence). The only part that
  needs new authorization design.
- Triggers in the engine, for raw Cypher.

## Worked example: comment notifications

```graphql
type User @node {
  id: ID! @key
  posts: [Post!]! @relationship(type: "WROTE", direction: OUT)
}

type Post @node @mutation {
  id: ID! @key
  title: String!
  comments: [Comment!]! @relationship(type: "ON", direction: IN)
}

type Comment
  @node
  @mutation
  @trigger(
    on: [CREATE]
    statement: """
    MATCH (this)-[:ON]->(:Post)<-[:WROTE]-(author:User)
    WHERE author.id <> $jwt.sub
    CREATE (n:Notification {
      id: randomUUID(), kind: "COMMENT", recipientId: author.id,
      commentId: this.id, createdAt: datetime()
    })
    RETURN n AS created
    """
  ) {
  id: ID! @key
  text: String!
  post: Post! @relationship(type: "ON", direction: OUT)
}

type Notification
  @node
  @outbox
  @subscription(operations: [CREATE])
  @authorization(
    filter: [{ where: { node: { recipientId: { eq: "$jwt.sub" } } } }]
  ) {
  id: ID! @key
  kind: String!
  recipientId: ID!
  commentId: ID!
  createdAt: DateTime!
}
```

What happens on `createComments`:

1. The comment is written and checked.
2. The trigger creates one `Notification` per post author, in the same
   transaction, and the node is stamped `PENDING`.
3. The commit makes both visible at once. The `WriteChange` lists the
   comment and the notification under `created`.
4. The author's open `notificationChanged` subscription receives it,
   filtered by the `@authorization` rule.
5. The worker wakes, claims the node, sends the push and sets `DONE`.

The docs page carries this schema with the full request and the real
response of each step, run by `test/docs.test.ts`.

## Cost

- One statement per trigger per mutation, under the writer lock. This
  adds to the open P1 line in `docs/TODO.md` (several JS round trips
  under the writer lock); triggers should ride `tx.executeMany` when
  that lands.
- A mutation on a type without triggers pays one map lookup.
- A worker costs two writes per node (claim, finish) and one keyed read.
- `bench/load` gets a scenario with and without a trigger to put a
  number on it (Phase 1).

## Phases

Each phase ships on its own, with tests, and leaves `main` releasable.

### Phase 1. Statement triggers for CREATE and UPDATE

- `model/directives.ts`: `@trigger`. `model/positions.ts`: where it is
  allowed. `model/types.ts`: `NodeType.triggers`.
- New `model/build/trigger.ts`: parse, check `fields` against the type,
  scan parameters with `cypher-lexer.ts`, refuse unknown parameters.
- `analyze/cypher-check.ts`: `explain()` each composed statement as it
  will run; record `shape` and `resultColumns`; warn on a writing
  statement with neither reserved column.
- New `execute/mutate/triggers.ts`: compose, bind, run through
  `runStatement`, map returned nodes into the change.
- `execute/mutate.ts`: call it before the commit; move `fillChange`
  ahead of the commit.
- `runner.ts`: record which fields each update set, for `fields:`.
- `cli.ts`: `check` reports trigger findings; `directives` lists it.
- Tests: new `test/triggers.test.ts` (fires on create, on nested create,
  on upsert both ways, on update with and without `fields`; rollback
  takes the trigger's writes with it; a failing statement rolls back the
  mutation; write-set contains returned nodes; a subscription on the
  trigger-created type fires without `changeFeed`; no cascade; shared
  transaction), `test/directive-positions.test.ts`, `test/cli.test.ts`,
  `test/mutation-transaction.test.ts`.
- `bench/load`: a trigger scenario.

### Phase 2. DELETE triggers

- `execute/mutate/delete.ts`: run delete triggers at the `beforeDelete`
  point, for every doomed type.
- Tests: `this` is readable; cascaded deletes fire; a trigger that
  touches a doomed node does not break the delete.

### Phase 3. JavaScript triggers and veto

- `options.ts`: `triggers`, `triggerTimeoutMs`.
- `execute/transaction.ts`: a `LoraTransaction` over a mutation's own
  transaction whose recorded changes merge into the parent change, and a
  marker that stops nested mutations from firing triggers.
- Error mapping and `onError` field names.
- `codegen.ts`: argument types for trigger functions.
- Tests: veto with a coded error and with a plain error (masked);
  timeout; nested `lora.execute` merges its write-set and fires nothing.

### Phase 4. Outbox types and workers

- `@outbox` in the model; state properties and their index in
  `analyze/indexes.ts` and `analyze/assert-schema.ts`.
- `execute/mutate/plan.ts`: stamp `PENDING` on generated creates.
  `execute/mutate/triggers.ts`: stamp trigger-created nodes.
- New `src/events/worker.ts`: wake, sweep, claim, handle, finish.
- `lora-graphql.ts`: `worker()`, and `close()` stops workers.
- `testing.ts`: a helper to drain workers in tests.
- Tests: new `test/workers.test.ts` (delivers once on the happy path;
  retries with backoff; `FAILED` after `maxAttempts`; a node left
  `PENDING` or with an expired lease is picked up by a new instance on
  the same database; two instances do not both handle one node; state
  writes emit no change).

### Phase 5. Docs and release

- New `apps/loradb.com/docs/graphql/triggers.md`: the worked example
  above with complete schema, requests and real responses, wired into
  `test/docs.test.ts` and `sidebars.js`.
- Update `directives.md`, `api-reference.md`, `limitations.md` (what
  fires nothing), `smart-layer.md` (S5), `serving.md` (workers and
  shutdown), `cypher-fields.md` and `authorization.md` (system writes).
- `docs/design/graphql-threat-model.md`: trigger statements as trusted
  schema input, system writes, the writer-lock bound on JavaScript
  triggers.
- `examples/tutorial`: add the notification flow.
- The docs site deploys on every push to `main`, so the docs commit
  goes out with the release, not before.

## What was built

Phases 1 to 4 as designed, with these differences:

- **The payload is read after the triggers**, in the same round trip as
  the trigger statements, so it shows what they wrote. The design had
  not settled this.
- **Triggers ride the mutation's batch.** A create with two statement
  triggers and its payload read is two calls into the transaction
  (`test/triggers.test.ts`, "one statement per trigger").
- **`fields:` is exact per node**: the runner records the fields each
  update named, per type and key (`Runner.updatedFields`).
- **The unreported-write warning is a lint**, not a startup warning:
  `check()` reports it under `lint`, static from the statement's text.
- **A failing trigger function is not reported as
  `trigger:Type.operation`**: it goes through the mutation's own error
  path, so `onError` names the root field.
- **No `onDone: "delete"`.** A worker deleting a node with raw Cypher
  skipped `onDelete: RESTRICT`, cascades and `DELETE` triggers (found in
  review), so the option was removed: delete through a generated
  mutation.
- **Triggers are not batched with the mutation's checks.** They are
  issued after the checks pass, which costs a round trip on types that
  have triggers and closes an error-precedence leak (see the threat
  model).
- **An attempt is counted when a node is claimed**, not when its handler
  fails, and every later write names the lease it took: a handler that
  hangs uses up its attempts, and a late result cannot undo a newer
  claim.
- **With `changeFeed: true` a worker's state writes are filtered out of
  the feed** (`execute/feed.ts`, by `setKeys`), so subscribers do not
  see an `UPDATE` per delivery.
- **Listeners still run synchronously** on the request path (the open
  P3 line): the worker only enqueues there.

Open: `CONNECT` / `DISCONNECT` triggers, previous values, `publish()`,
the tutorial project, `bench/load` scenarios, a threat-model entry. Each
is on the list in `docs/TODO.md`.

## Open questions

1. **`fields:` on UPDATE** needs the runner to record the fields each
   update set, including `adjust`. Inferred to be small; not read in
   detail.
2. **Lease and sweep defaults** are guesses until Phase 4 is measured.
3. **A trigger that needs a veto in Cypher.** Not planned: a statement
   cannot throw a coded error. A JavaScript trigger covers it.
4. **Worker threads sharing one engine.** The claim is atomic under the
   writer lock (Inferred); the open P2 line on multi-worker serving
   should include a worker test.
