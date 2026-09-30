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
import { createTestLoraGraphQL, expectSeeks } from "@loradb/lora-graphql/testing";
```

The package is ESM only and needs Node 20 or later. `graphql` 16 or 17 is
a peer dependency. `@loradb/lora-node` is an optional peer: you need it
for mutations, `check()`, `explain()`, the change feed, the CLI's `check`
and `analyze` commands, and the testing helpers.

## new LoraGraphQL(options)

Builds the model from the SDL and validates it. An invalid model throws a
[`ModelError`](/docs/graphql/errors#modelerror) listing every problem. So
does a `@populatedBy` callback or `@customResolver` resolver that the
options do not supply.

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
| `timeoutMs` | 10 000 | Per statement, in milliseconds. `0` disables |
| `maxBatch` | 1000 | Nodes one mutation may create or delete, nested ones included. Also the default `limit` of bulk updates and deletes |
| `maxQueuedChanges` | 1000 | Changes a `changes()` consumer or subscriber may fall behind before it is ended with `LIMIT_EXCEEDED` |

### Security

| Option | Default | Meaning |
| --- | --- | --- |
| `jwt` | `(context) => context.jwt` | Where the request's verified claims are |
| `cursorSecret` | | Sign cursors with HMAC-SHA-256 and reject any the server did not issue. Changing it invalidates every cursor |
| `maskErrors` | `NODE_ENV === "production"` | Clients get `DATABASE_ERROR` and an `id` instead of the engine's message |
| `guards` | see [guards](#documentguards) | Document limits for `execute()` and `persist()`. `false` turns them off |
| `persistedOnly` | `false` | `execute()` refuses `source` and runs persisted operations only |

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
| `onError` | `({ id, field, message, error }) => void`, for every database error, masked or not |
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

The executable schema for any `graphql-js` server. Built once and cached.
See [serving](/docs/graphql/serving).

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
}>
```

Compares `requirements()` with `SHOW INDEXES` and `SHOW CONSTRAINTS`.
Without options it only reports `missing`. With `create: true` it runs
one `CREATE ... IF NOT EXISTS` statement per missing requirement (LoraDB
does not run schema commands in transactions) and reports them as
`created`. Idempotent. It never drops anything.

## Analysis

### check(options?)

```ts
check(options?: {
  operations?: Array<{ name?: string; document: string | DocumentNode; variables?: Record<string, unknown> }>;
  rowBudget?: number;
}): Promise<CheckReport>
```

The CI gate behind `lora-graphql check`. It checks the database the
driver points at, as it is: it does not create indexes. The report:

| Field | Holds | Fails `ok` |
| --- | --- | --- |
| `ok` | `true` when nothing below fails | |
| `warnings` | Model warnings | No |
| `lint` | Valid but costly or risky choices | No |
| `cypher` | `@cypher` statements the engine rejects, or that write from a query | Yes |
| `missing` | Constraints and indexes the database lacks | Yes |
| `unused` | Indexes the database has that the API does not need | No |
| `plans` | Plan reports per operation and root field | Yes, if any has findings |
| `errors` | Operations that failed to compile | Yes |

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
its `operators` (root to leaves), the engine's `estimatedRows`, and
`findings`. A finding's `rule` is one of:

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
1000; not a random sample). The result is applied with `useStatistics()`
and returned:

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
in a job. Cost estimates then use each relationship's p99 degree instead
of its page size, and relationship filters that name a related key can
start from that node. Changing statistics invalidates the compile cache.

## Execution

### execute(args)

```ts
execute(args: {
  source?: string;
  id?: string;
  variables?: Record<string, unknown>;
  operationName?: string;
  context?: unknown;
}): Promise<ExecutionResult>
```

Runs a query or mutation. Pass `id` for a persisted operation, or
`source` for an ad hoc document (parsed, guarded, validated and cached by
source text, up to 500 documents). The result's `extensions.cost` holds
the operation's estimated rows. Errors are returned in `errors`, never
thrown. Given a subscription, it returns a `WRONG_OPERATION_TYPE` error:
run subscriptions with `subscribe()`, which takes the same arguments.

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
| `diffSchemas(before, after)` | Database statements and API changes between two SDLs. See [the smart layer](/docs/graphql/smart-layer#s7-schema-diff) |
| `schemaHash(schema)` | The SHA-256 hash of a public schema that manifests record |
| `toGlobalId(type, key)`, `fromGlobalId(id)` | Encode and decode the opaque ids of `@relayId` types |
| `validationRules(guards?)`, `parseOptions(guards?)`, `envelopPlugin(guards?)` | Document guards without an instance |
| `DEFAULT_GUARDS` | `{ maxDepth: 12, maxAliases: 30, maxRootFields: 20, maxTokens: 5000 }` |
| `ModelError`, `formatProblem(problem)` | The model error and its line formatter |
| `LoraTransaction` | The transaction class `begin()` returns |

### DocumentGuards

| Field | Default | Limit |
| --- | --- | --- |
| `maxDepth` | 12 | Field nesting, fragments followed |
| `maxAliases` | 30 | Aliased fields in one document |
| `maxRootFields` | 20 | Root fields in one operation |
| `maxTokens` | 5000 | Lexer tokens in one document, checked while parsing |
| `introspection` | `NODE_ENV !== "production"` | Allow `__schema` and `__type`. `__typename` is always allowed |

## Testing exports

`@loradb/lora-graphql/testing` exports `createTestLoraGraphQL` and
`expectSeeks`. See [testing](/docs/graphql/testing).
