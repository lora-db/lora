---
title: LoraGraphQL API Reference
sidebar_label: API reference
description: Every constructor option, method and export of @loradb/lora-graphql, with defaults, return types and the driver interface for custom databases.
---

# API reference

This page lists the package's public surface. The guides explain when to
use each piece; this page says exactly what it takes and returns.

```ts
import { LoraGraphQL, loraDriver } from "@loradb/lora-graphql";
import {
  createTestLoraGraphQL,
  expectAccess,
  expectSeeks,
} from "@loradb/lora-graphql/testing";
```

The package is ESM only and needs Node 20 or later. `graphql` 16 or 17 is
a peer dependency. `@loradb/lora-node` is an optional peer: you need it
for mutations, `check()`, `explain()`, the change feed, the CLI's `check`
and `analyze` commands, and the testing helpers.

## new LoraGraphQL(options)

Builds the model from the SDL and validates it. An invalid model throws a
[`ModelError`](/docs/graphql/errors#modelerror) listing every problem. So
does a `@populatedBy` callback or `@customResolver` resolver that the
options do not supply. `defaultLimit` or `maxLimit` below 1 is a
`ModelError` too, and a `defaultLimit` above `maxLimit` is lowered to it.

### Required

| Option | Type | Meaning |
| --- | --- | --- |
| `typeDefs` | `string \| DocumentNode` | The annotated SDL: the graph model and the API in one document |
| `driver` | `LoraDriver` | Usually `loraDriver(db)`. See [drivers](#drivers) |

### Limits

| Option | Default | Meaning |
| --- | --- | --- |
| `defaultLimit` | 25 | Page size when a list or connection gets no `limit` or `first` |
| `maxLimit` | 100 | Largest page size. More is `LIMIT_EXCEEDED`, not a clamp. `@limit(max:)` may only lower it |
| `maxCost` | 50 000 | Estimated rows one operation may touch, checked before it runs. `Infinity` disables |
| `budget` | | `(context) => number \| undefined`: the cost limit for one request. `undefined` falls back to `maxCost` |
| `timeoutMs` | 10 000 | Per statement, in milliseconds, the wait for the writer lock included. `0` disables |
| `operationTimeoutMs` | 2 × `timeoutMs` | For all root fields of one query together. Past it the operation's statements are aborted and its unfinished fields fail with `TIMEOUT`. `0` disables. Subscriptions are not bounded by it; mutations keep `timeoutMs` per statement |
| `maxConcurrentStatements` | 2 | Statements one operation runs at once. Aliased root fields queue for a slot, so one request cannot take every worker thread. Lookups by `@key` do not need a slot |
| `maxBatch` | 1000 | Nodes one mutation may create, update or delete, nested ones included, and ten times as many relationships. Also the default and the ceiling of the `limit` of bulk updates and deletes |
| `maxFilterDepth` | 2 | Relationship levels one `where` may nest. Quantifiers, connection filters, `<field>Exists` and filters through a single relationship count one each. Deeper is `BAD_USER_INPUT` |
| `maxListFilter` | 1000 | Items in an `in` operand. More is `BAD_USER_INPUT` |
| `maxStringFilter` | 10 000 | Characters in a string filter operand. More is `BAD_USER_INPUT` |
| `maxListArgument` | 1000 | Items in a list argument of a `@cypher` field without `@size(max:)`. More is `BAD_USER_INPUT` |
| `compileCacheBytes` | 64 MiB | Approximate bytes the compile cache may hold, and separately the parsed document cache. Oldest entries are evicted first. A field whose variables exceed 16 KiB (long `in` lists, vectors) is compiled but not cached |

Subscriptions have their own limits:

| Option | Default | Meaning |
| --- | --- | --- |
| `maxQueuedChanges` | 1000 | Changes a `changes()` consumer or subscriber may fall behind before it is ended with `LIMIT_EXCEEDED` |
| `maxSubscriptions` | 100 | Live subscriptions per scope. One more is `LIMIT_EXCEEDED` |
| `subscriptionScope` | the context object | `(context) => object \| undefined`: what `maxSubscriptions` counts per. Return the connection or the user when your server builds a new context per subscription |
| `maxSubscriptionFilterDepth` | 1 | Relationship levels in a subscription's `where`, which runs on every change. Deeper is `LIMIT_EXCEEDED` |
| `subscriptionTimeoutMs` | 2000, or `timeoutMs` when lower | Per statement a subscription runs to check a change |

### Security

| Option | Default | Meaning |
| --- | --- | --- |
| `jwt` | `(context) => context.jwt` | Where the request's verified claims are |
| `cursorSecret` | | Sign cursors with HMAC-SHA-256 and reject any the server did not issue. Changing it invalidates every cursor |
| `maskErrors` | `NODE_ENV === "production"` | Clients get `DATABASE_ERROR` and an `id` instead of the engine's message |
| `guards` | see [guards](#documentguards) | Document limits for `execute()`, `subscribe()` and `persist()`. `false` turns them off |
| `persistedOnly` | `false` | `execute()` and `subscribe()` refuse `source` and run persisted operations only |
| `timing` | `false` | `true`, or a function of the context: `execute()` adds `extensions.timing` (total, database and per-root-field milliseconds). See [Observability](./observability#timing-in-responses) |
| `mutationTransaction` | `"field"` | `"field"`: each root field of a mutation commits on its own, so a later failure leaves earlier fields committed. `"operation"`: `execute()` runs every root field in one transaction, rolled back (with `data: null`) when any fails. Envelop and Yoga servers get the same from `lora.envelopPlugin()`. A `transaction` in the context takes precedence |

### Extensibility

| Option | Meaning |
| --- | --- |
| `callbacks` | `Record<string, PopulatedByCallback>`: named callbacks for `@populatedBy(callback:)` |
| `resolvers` | `Record<Type, Record<field, GraphQLFieldResolver>>`: resolvers of `@customResolver` fields |
| `scalars` | `Record<string, GraphQLScalarType>`: implementations of `@storedAs` custom scalars. Without one, values pass through |
| `changeFeed` | `true` feeds subscriptions and `changes()` from the engine's committed change feed. Needs a driver with `changes()` (`@loradb/lora-node`); throws at construction otherwise |

A `PopulatedByCallback` receives one object and returns the value to
store (or a promise of it). For a non-null field, returning `null` or
`undefined` fails the mutation with `CONSTRAINT_VIOLATION`:

```ts
type PopulatedByCallback = (args: {
  operation: "CREATE" | "UPDATE";
  type: string; // node type
  field: string; // field being populated
  key: unknown; // the node's @key value
  input: Record<string, unknown>; // the node's input in this mutation
  context: unknown; // the GraphQL context
}) => unknown;
```

A `@customResolver` resolver's source object holds the node's selected
fields plus the fields named in `requires`.

### Observability

| Option | Meaning |
| --- | --- |
| `onStatement` | `({ field, statement }) => void`, before every statement runs |
| `onStatementEnd` | After every statement call, with duration, rows, error and cost |
| `onError` | `({ id, field, message, error }) => void`, for every database error, masked or not. `field` is the root field, `changeFeed` for a failure of the change feed, or `commit` for a failed commit of an operation-level transaction |
| `onCost` | `({ field, cost, total, limit, context }) => void`, for every root field's estimate, before it runs |
| `tracer` | An OpenTelemetry-shaped tracer |
| `traceStatements` | Put the Cypher text in spans as `db.statement`. Default `false` |
| `metrics` | Any object with `counter()` and `histogram()` |

The [observability](/docs/graphql/observability) page describes each
event, span and metric.

## Schema

### getSchema()

```ts
getSchema(): GraphQLSchema
```

The executable schema for any `graphql-js` server. Built on the first call
and cached; that call can throw a `ModelError` if the generated schema is
invalid. See [serving](/docs/graphql/serving).

### printPublicSchema()

```ts
printPublicSchema(): string
```

The SDL clients see: generated types only, no model directives, sorted.
The same output as `lora-graphql print`.

### model

```ts
readonly model: GraphModel
```

The parsed model. Useful fields: `nodes` (a `Map` of `@node` types by
name), `abstracts` (interfaces and unions), `enums`, `queries` and
`mutations` (`@cypher` root fields), `scalars` (custom scalars and their
storage type), `jwt` (declared claims) and `warnings` (problems that do
not stop the model, such as an unused `@cypher` argument).

### validationRules() and envelopPlugin()

```ts
validationRules(): ValidationRule[]
envelopPlugin(): EnvelopPlugin
```

The configured document guards, for servers other than `execute()`. See
[serving](/docs/graphql/serving#document-guards).

The instance's `envelopPlugin()` does two more things, so a Yoga or
Envelop server on `getSchema()` behaves like `execute()`: with
`mutationTransaction: "operation"` it runs a mutation's root fields in one
transaction, and it [collapses repeated errors](/docs/graphql/errors#repeated-errors).
It also checks once that the server executes with the same copy of
`graphql` the library uses, and logs an error when it does not: two copies
in `node_modules` make every schema check fail in confusing ways.

## Database schema

### requirements()

```ts
requirements(): SchemaRequirement[]
```

Every constraint and index the API needs, with a `reason` for each. A
requirement is one of:

| `kind` | Fields |
| --- | --- |
| `constraint` | `constraint` (`NODE_KEY`, `UNIQUE` or `NOT_NULL`), `name`, `label`, `property` |
| `index` | `index` (`RANGE`, `TEXT` or `POINT`), `name`, `label`, `property` |
| `fulltext` | `name`, `label`, `properties`, `analyzer` (`STANDARD` or `SIMPLE`) |
| `vector` | `name`, `label`, `property`, `dimensions`, `similarity` (`COSINE` or `EUCLIDEAN`) |

### assertSchema(options?)

```ts
assertSchema(options?: { create?: boolean }): Promise<{
  required: SchemaRequirement[];
  missing: SchemaRequirement[];
  created: SchemaRequirement[];
  mismatched: SchemaRequirement[];
  recreated: SchemaRequirement[];
}>
```

Compares `requirements()` with `SHOW INDEXES` and `SHOW CONSTRAINTS`.
Without options it reports `missing`, and `mismatched`: full-text and
vector indexes present under their name but defined differently from the
model (labels, fields, kind, analyzer), which search would keep using.
With `create: true` it runs one `CREATE ... IF NOT EXISTS` statement per
missing requirement (LoraDB does not run schema commands in transactions)
and reports them as `created`, and drops and re-creates each mismatched
index, reported as `recreated`. Idempotent. It drops nothing else.
`check()` fails on a missing or mismatched requirement.

## Analysis

### check(options?)

```ts
check(options?: {
  operations?: Array<{
    name?: string;
    document: string | DocumentNode;
    variables?: Record<string, unknown>;
    context?: unknown; // for example { jwt }: plan the statements this caller gets
  }>;
  rowBudget?: number;
}): Promise<CheckReport>
```

The CI gate behind `lora-graphql check`. It checks the database the
driver points at, as it is: it does not create indexes. The report:

| Field | Holds | Fails `ok` |
| --- | --- | --- |
| `ok` | `true` when nothing below fails | |
| `warnings` | Model warnings | No |
| `lint` | Valid but costly or risky choices, authorization lints included | No |
| `security` | `@mutation` types with generated writes no rule guards (declare intended ones with `@authorization(public: [...])`) | Yes |
| `cypher` | `@cypher` statements the engine rejects, or that write from a query | Yes |
| `missing` | Constraints and indexes the database lacks, and full-text or vector indexes defined differently from the model | Yes |
| `unused` | Indexes the database has that the API does not need | No |
| `plans` | Plan reports per operation and root field | Yes, if any has findings |
| `errors` | Operations that failed to compile | Yes |

### accessMatrix()

```ts
accessMatrix(): AccessEntry[]
// { type, field?, operation, principal, verdict, by }
```

Who may do what, read off the model: for every type and guarded field,
each operation as each kind of caller (`anonymous`, `authenticated`, and
one principal per claim value the rules test, such as `roles:admin`). When
rules name the caller's node (`isViewer`), `viewer` is the caller on nodes
that are theirs: every rule part that only names them passes, so it shows
the most a related caller gets, where `authenticated` shows what any
signed-in caller gets. A part under `NOT` stays as written and is decided
per row, so `viewer` never reads below `authenticated`. A relationship field
has a CONNECT row when a rule or
`@authentication(operations: [CREATE_RELATIONSHIP])` guards it, and a
DISCONNECT row for a rule or `DELETE_RELATIONSHIP`. The
`verdict` is `allowed`, `filtered`, `validated`, `masked`, `denied` or
`unauthenticated`, and `by` names the rules that decide it (`filter[0]`,
`validate[1]`, `@authentication`, `bypass`, `public`). The order is stable,
so a snapshot in CI turns access changes into reviewable diffs. The CLI
prints it with [`lora-graphql access`](./cli#access).

### operationAccess(document, operationName?, variables?)

```ts
operationAccess(
  document: string | DocumentNode, // or the id of a persisted operation
  operationName?: string,
  variables?: Record<string, unknown>,
): OperationAccess

interface OperationAccess {
  operation: "query" | "mutation" | "subscription";
  name: string | undefined;
  principals: string[]; // in the order accessMatrix() lists them
  fields: RootFieldAccess[]; // document order, fragments followed
  verdicts: Record<string, AccessVerdict>; // per principal, the most restrictive
}

interface RootFieldAccess {
  field: string;
  alias?: string;
  type: string; // node type, interface, union, Node, or Query / Mutation for @cypher
  operations: string[]; // READ, CREATE, UPDATE, ..., EXECUTE
  access: Record<string, { verdict: AccessVerdict; by: string[] }>;
  unresolved?: string[]; // variables given no value
}
```

Who may run one operation, from the same rules as `accessMatrix()`. Each
root field gets a verdict per kind of caller, and `verdicts` holds the
most restrictive one per caller: `denied` or `unauthenticated` there means
the operation cannot succeed for that caller. A mutation's inputs are read
for the relationship writes they make (a nested connect, disconnect,
create, update or delete), which count towards the field's verdict. Pass
`variables` for inputs the document takes as variables; one without a
value is listed in `unresolved`. It throws when the document holds several
operations and `operationName` does not pick one.

A typical use is checking a client bundle at build time, for example that
nothing shipped to anonymous users needs a signed-in caller:

```ts
for (const [id, source] of Object.entries(publicOperations)) {
  const { verdicts } = lora.operationAccess(source);
  if (verdicts.anonymous === "unauthenticated" || verdicts.anonymous === "denied") {
    throw new Error(`${id} cannot run anonymously`);
  }
}
```

### explain(document, variables?, options?)

```ts
explain(
  document: string | DocumentNode,
  variables?: Record<string, unknown>,
  options?: { operationName?: string; context?: unknown; rowBudget?: number },
): Promise<Array<{ field: string; reports: PlanReport[] }>>
```

Compiles a query and plans every statement with the engine's `explain()`,
without running it. Each `PlanReport` has the `statement`, the `plan`,
its `operators` (root to leaves), the engine's `estimatedRows`,
`findings`, and `notes`: label scans inside `CALL {}` bodies, reported as
lint and not as failures. A finding's `rule` is one of:

| Rule | Means |
| --- | --- |
| `full-scan` | A label scan where the compiler expected an index seek |
| `scan-expand` | An expansion that starts from a full scan instead of a seek |
| `mutating-read` | A read statement with a mutating plan |
| `result-columns` | The plan's columns differ from what the projection expects |
| `row-budget` | The engine estimates more rows than `rowBudget` |

Pass `context` (for example `{ jwt }`) to explain the statements a given
caller gets: claim checks are folded into the statement text. Queries
only; a mutation throws. Needs a driver with `explain()`.

### compile(document, variables?, options?)

```ts
compile(
  document: string | DocumentNode,
  variables?: Record<string, unknown>,
  options?: { operationName?: string; context?: unknown },
): Array<{ field: string; compiled: CompiledRead }>
```

Compiles a query without planning or running it. Each `CompiledRead` has
the `statements` (text and parameters), the `columns` the first
statement returns, the read-set (`reads.labels`, `reads.relationships`),
the `expectations` (which statement should seek which label, and how),
the `cost` estimate and the `mode`. Useful for tests and for reading the
Cypher an operation produces. Queries only.

### analyze(options?)

```ts
analyze(options?: { sample?: number }): Promise<Statistics>
```

Counts the nodes of every `@node` type and measures the degree of every
list relationship over the first `sample` nodes of the type (default
1000; not a random sample). `max` is the exception: it is measured over
every node, since it is what cost estimates rely on. The result is applied
with `useStatistics()` and returned:

```ts
interface Statistics {
  nodes: Record<string, number>; // count per @node type
  degrees: Record<string, { sampled: number; mean: number; median: number; p99: number; max: number }>; // by "Type.field"
}
```

### useStatistics(stats) and statistics

```ts
useStatistics(stats: Statistics): void
readonly statistics: Statistics | undefined
```

Use statistics gathered earlier, for example by `lora-graphql analyze`
in a job. Cost estimates then use each relationship's maximum degree
instead of its page size, and relationship filters that name a related key
can start from that node. The maximum, not a percentile, because callers
choose which parent they nest under and can pick the busiest one. Changing
statistics invalidates the compile cache.

## Execution

### execute(args)

```ts
execute(args: {
  source?: string;
  id?: string;
  variables?: Record<string, unknown>;
  operationName?: string;
  context?: unknown;
  readSet?: boolean;
}): Promise<LoraExecutionResult> // ExecutionResult & { readonly readSet?: ReadSet }
```

Runs a query or mutation. Pass `id` for a persisted operation, or
`source` for an ad hoc document (parsed, guarded, validated and cached by
source text, up to 500 documents). The result's `extensions.cost` holds
the operation's estimated rows. Errors are returned in `errors`, never
thrown. Given a subscription, it returns a `WRONG_OPERATION_TYPE` error.

With `readSet: true` the result carries a non-enumerable `readSet`: the
labels and relationship types the operation read. It does not show up in
`JSON.stringify(result)`. Keep it next to a cached response and drop the
response when [`affects(readSet, change)`](#affectsreads-change) says a
write touched it. A mutation, or a query with a `@cypher` field, has
`readSet.opaque: true`: the library cannot tell what it read, so
`affects()` answers `true` for every change. A `@customResolver` that reads
the database on its own is not in the read-set.

```ts
const result = await lora.execute({ source, variables, context, readSet: true });
cache.set(cacheKey, { body: JSON.stringify(result), reads: result.readSet });

lora.onWrite((change) => {
  for (const [key, entry] of cache) {
    if (lora.affects(entry.reads, change)) cache.delete(key);
  }
});
```

### subscribe(args)

```ts
subscribe(args: {
  source?: string;
  id?: string;
  variables?: Record<string, unknown>;
  operationName?: string;
  context?: unknown;
}): Promise<AsyncIterableIterator<ExecutionResult> | ExecutionResult>
```

Runs a subscription with the same document handling as `execute()`:
persisted ids, guards, `persistedOnly`, the document cache. It resolves to
an async iterator of results, or to a single `ExecutionResult` holding
`errors` when the subscription could not start. Given a query or mutation
it returns `WRONG_OPERATION_TYPE`. See
[serving](/docs/graphql/serving#subscriptions-over-websockets).

### persist(operations)

```ts
persist(operations: Record<string, string>): void
```

Registers persisted operations, id to source. Every document is parsed,
guarded and validated now; any invalid one throws, listing every
problem.

### buildManifest(operations), generateTypes(manifest) and loadManifest(manifest)

```ts
buildManifest(operations: Record<string, string>): OperationManifest
generateTypes(manifest: OperationManifest): string
loadManifest(manifest: OperationManifest): void
```

The build-time path behind `lora-graphql compile`. `buildManifest`
validates operations into a manifest that records a hash of the public
schema. `generateTypes` returns TypeScript declarations with
`<Operation>Variables` and `<Operation>Result` for each operation, named
after the operation (or its id when it has no name).
`loadManifest` registers the operations without parsing or validating
again, and throws if the manifest was built for another schema.

### begin()

```ts
begin(): Promise<LoraTransaction>
```

Opens a write transaction the caller owns. Put it in the context as
`transaction` and every operation of the request runs in it. Nothing is
visible to others, and no change event fires, until you commit.

```ts
class LoraTransaction {
  readonly isOpen: boolean;
  execute(query: string, params?: Record<string, unknown>): Promise<{ columns: string[]; rows: Record<string, unknown>[] }>;
  commit(): Promise<void>; // then change events fire
  rollback(): Promise<void>; // pending change events are dropped
}
```

A failed mutation in the transaction rolls the whole transaction back.
Writes made with `tx.execute()` are not part of any write-set. The
transaction holds LoraDB's writer lock until it ends: keep it short.
Needs a driver with `begin()`.

## Change tracking

### onWrite(listener)

```ts
onWrite(listener: (change: WriteChange) => void): () => void
```

Called after every committed mutation made through this instance, with
its exact write-set. Returns an unsubscribe function. A throwing listener
does not fail the write. With `changeFeed: true` it still reports only
this instance's mutations.

### changes(options?)

```ts
changes(options?: { signal?: AbortSignal; maxQueued?: number }): AsyncIterableIterator<WriteChange>
```

Committed writes as an async iterator: this instance's mutations, or
every committed write with `changeFeed: true`. A consumer that falls
`maxQueued` (default `maxQueuedChanges`) behind is ended with
`LIMIT_EXCEEDED`. Aborting the signal, or breaking out of the loop, ends
it.

### WriteChange

| Field | Type | Holds |
| --- | --- | --- |
| `operation` | string | `CREATE`, `UPDATE`, `DELETE`, `UPSERT`, `CYPHER`, or `EXTERNAL` (from the change feed) |
| `field` | string | The `Mutation` field that made the change |
| `timestamp` | string | When it was committed, ISO-8601 |
| `created`, `updated`, `deleted` | `{ type, key }[]` | Nodes |
| `connected`, `disconnected` | `{ type, field, from, to }[]` | Relationships, with the declaring `Owner.field` and both ends |
| `entities` | `{ type, key }[]` | Every node whose observable state changed, relationship ends included |
| `types`, `relationshipTypes` | `string[]` | Node types and relationship types touched |
| `broad` | boolean | The write-set is unknown (a `@cypher` mutation): treat everything as changed |
| `before` | array | Stored properties before the write, for types with `@subscription(previousState: true)` |

`onWrite` and `changes()` carry the full write-set with no read rules
applied. Keep them on the server.

### affects(reads, change)

```ts
affects(reads: { labels: string[]; relationships: string[] }, change: WriteChange): boolean
```

Whether a read with this read-set (from `compile()`) may observe the
change. Label level: any write to a `Festival` affects every read of
`:Festival`. Always `true` for a broad change.

### close()

```ts
close(): void
```

Stops the engine change feed. Call it on shutdown when `changeFeed` is
on; otherwise it does nothing.

## Drivers

`loraDriver(db)` adapts a `Database` from `@loradb/lora-node` or
`@loradb/lora-wasm`. A read of one statement runs with `execute()`; with
lora-node that runs on a libuv worker, not the JavaScript thread, so a
slow read does not hold up other requests. A lookup by `@key` that
selects only stored fields (`bounded`) streams its single row
synchronously, which is faster than the hop to a worker. A read of several
statements runs in one read-only transaction, and a write in a read-write
one. It exposes `begin()`, `explain()` and `changes()` when the database
has them.

| Driver method | Needed for | lora-node | lora-wasm |
| --- | --- | --- | --- |
| `run(statements, options)` | Everything | Yes | Yes |
| `begin(options)` | Mutations, `lora.begin()` | Yes | No |
| `explain(statement)` | `explain()`, `check()`, `expectSeeks()` | Yes | No |
| `changes(options)` | `changeFeed: true` | Yes | No |

To put the library in front of something else (a remote LoraDB, a
connection pool, a recording proxy in tests), implement `LoraDriver`:

```ts
interface LoraDriver {
  /** Run statements atomically, in one transaction, in order. */
  run(statements: Statement[], options: RunOptions): Promise<QueryResult[]>;
  begin?(options: RunOptions): Promise<DriverTransaction>;
  explain?(statement: Statement): Promise<QueryPlan>;
  // Batches of committed changes, as the lora-node change feed yields them.
  changes?(options: { fromLsn?: number; signal?: AbortSignal }): AsyncIterable<DriverChangeBatch> & { ready: Promise<void> };
}

interface Statement { text: string; params: Record<string, unknown> }
interface QueryResult { columns: string[]; rows: Array<Record<string, unknown>> }
interface RunOptions {
  mode: "read" | "write"; // "read" must reject writes
  timeoutMs?: number;
  signal?: AbortSignal;
  verified?: boolean; // a read the library vouches for; may skip the read-only transaction
  bounded?: boolean; // a verified read of at most one row by @key: cheap enough to run synchronously
}
interface DriverTransaction {
  execute(statement: Statement): Promise<QueryResult>;
  commit(): Promise<void>;
  rollback(): Promise<void>;
  readonly isOpen: boolean;
}
```

Rows are plain objects keyed by column, with LoraDB values as the Node
binding returns them. Mutations rely on `begin()` giving an interactive
transaction whose statements see earlier writes.

## Standalone exports

| Export | Does |
| --- | --- |
| `loraDriver(db)` | Adapts a LoraDB `Database` |
| `directiveTypeDefs` | The directive definitions as SDL, for editors and codegen |
| `buildModel(typeDefs, options?)` | Builds and validates the model without a driver; throws `ModelError` |
| `inferRequirements(model)` | Requirements of a model, as `requirements()` |
| `requirementDdl(requirement)` | The `CREATE ... IF NOT EXISTS` statement for one requirement |
| `checkPlans(driver, compiled, options?)` | Plan-checks one `CompiledRead` |
| `scanExpands(statement, plan)` | Findings for expansions that start from a full scan |
| `diffSchemas(before, after, options?)` | Database statements and API changes between two SDLs. `options` are the model options (`defaultLimit`, `maxLimit`) both are built with. With `graphql` 17 the API comparison is not available and comes back as one breaking `UNSUPPORTED` change. See [the smart layer](/docs/graphql/smart-layer#s7-schema-diff) |
| `schemaHash(schema)` | The SHA-256 hash of a public schema that manifests record |
| `toGlobalId(type, key)`, `fromGlobalId(id)` | Encode and decode the opaque ids of `@relayId` types |
| `validationRules(guards?)`, `parseOptions(guards?)`, `envelopPlugin(guards?, onRealmMismatch?)` | Document guards without an instance. `onRealmMismatch(message)` is called once if the server runs a different copy of `graphql`; the default logs with `console.error` |
| `DEFAULT_GUARDS` | `{ maxDepth: 12, maxIntrospectionDepth: 20, maxAliases: 30, maxRootFields: 20, maxTokens: 5000 }` |
| `ModelError`, `formatProblem(problem)` | The model error and its line formatter |
| `LoraTransaction` | The transaction class `begin()` returns |
| `LORA_GRAPHQL_ERROR_CODES` | The request error codes, as a readonly array |
| `isLoraGraphQLError(error)` | Whether a `GraphQLError` carries one of those codes. See [errors](/docs/graphql/errors#telling-library-errors-apart) |

Every type named on this page is exported too, among them
`LoraGraphQLOptions`, `LoraGraphQLContext`, `ExecuteArgs`,
`LoraExecutionResult`, `CheckOptions`, `CheckReport`, `AssertSchemaOptions`,
`SchemaAssertion`, `SchemaRequirement`, `PlanReport`, `PlanFinding`,
`CompiledRead`, `ReadSet`, `Statistics`, `AccessEntry`, `AccessVerdict`,
`OperationAccess`, `RootFieldAccess`, `WriteChange`, `EntityRef`,
`RelationshipRef`, `MutationInfo`, `PopulatedByCallback`,
`StatementEvent`, `StatementEndEvent`, `CostEvent`, `DatabaseErrorEvent`,
`ExecutionTiming`, `OperationManifest`, `SchemaDiff`, `ApiChange`,
`LoraGraphQLErrorCode`, `ModelProblem`, the model types (`GraphModel`,
`NodeType` and the field types), and the driver types above.

### DocumentGuards

| Field | Default | Limit |
| --- | --- | --- |
| `maxDepth` | 12 | Field nesting, fragments followed; `__schema` and `__type` count as one level |
| `maxIntrospectionDepth` | 20 | Nesting under `__schema` / `__type`: the standard introspection query needs 15 |
| `maxAliases` | 30 | Aliased fields in one document |
| `maxRootFields` | 20 | Root fields in one operation |
| `maxTokens` | 5000 | Lexer tokens in one document, checked while parsing |
| `introspection` | `NODE_ENV !== "production"` | Allow `__schema` and `__type`. `__typename` is always allowed |

## Testing exports

`@loradb/lora-graphql/testing` exports `createTestLoraGraphQL`,
`expectSeeks` and `expectAccess`, with the types `TestDatabase`,
`TestLoraGraphQLOptions` and `AccessExpectations`. See
[testing](/docs/graphql/testing).

## Versions

`@loradb/lora-graphql` X.Y.Z is tested against `@loradb/lora-node` X.Y.Z
and declares it as a `^X.Y.Z` peer: upgrade the two together. The test
suite runs on both `graphql` 16 and 17.
