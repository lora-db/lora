---
title: Observing a LoraGraphQL API
sidebar_label: Observability
description: Statement hooks, OpenTelemetry spans, metrics, cost events and database error ids in @loradb/lora-graphql, and what each one reports.
---

# Observability

Every GraphQL root field becomes one or a few Cypher statements, so the
useful questions are about statements: which ran, how long they took, how
many rows they returned, and what they were estimated to cost. The
library reports these through hooks, spans and metrics. None of them is
on by default, and a failing hook never fails a request.

| Option | Fires | Use it for |
| --- | --- | --- |
| [`onStatement`](#onstatement) | Before each statement | Logging the Cypher, tests |
| [`onStatementEnd`](#onstatementend) | After each statement call | Slow-query logs, per-field timing |
| [`tracer`](#tracing) | Per root field and statement call | Distributed tracing |
| [`metrics`](#metrics) | After each statement call | Dashboards and alerts |
| [`onCost`](#cost) | Before each root field runs | Tuning `maxCost` and budgets |
| [`onError`](#database-errors) | On each database error | Correlating masked errors |
| [`timing`](#timing-in-responses) | In each `execute()` result | Showing clients where time went |

## onStatement

```ts
new LoraGraphQL({
  typeDefs,
  driver,
  onStatement: ({ field, statement }) => log.debug({ field, cypher: statement.text, params: statement.params }),
});
```

Called before every statement with the root field it belongs to and the
statement's text and parameters. Parameters hold request values: redact
them before logging in production.

## onStatementEnd

```ts
new LoraGraphQL({
  typeDefs,
  driver,
  onStatementEnd: (e) => {
    if (e.durationMs > 50) log.warn({ field: e.field, ms: e.durationMs, rows: e.rows, cost: e.cost });
  },
});
```

Called after every statement call with:

| Field | Holds |
| --- | --- |
| `field` | The root field |
| `statements` | The statements run in this call |
| `mode` | `read` or `write` |
| `durationMs` | Wall time of the call |
| `rows` | Rows returned, over every statement in the call |
| `error` | Set when the call failed |
| `cost` | The root field's estimated rows touched, for reads |
| `operationName` | The GraphQL operation name, when it has one |
| `persistedId` | The persisted operation id, when the request used one |

A read root field runs its statements in one call, so it reports one
event. A mutation runs its statements one at a time inside its
transaction, so it reports one event per statement.

## Tracing

Pass an OpenTelemetry tracer:

```ts
import { trace } from "@opentelemetry/api";

new LoraGraphQL({
  typeDefs,
  driver,
  tracer: trace.getTracer("lora-graphql"),
  traceStatements: false, // true puts the Cypher text in spans
});
```

The package has no dependency on `@opentelemetry/api`: any object with
`startActiveSpan(name, fn)` whose spans have `setAttribute`, `setStatus`,
`recordException` and `end` works.

Each root field gets a span, holding one span per statement call:

| Span | Attributes |
| --- | --- |
| `lora.graphql.field` | `graphql.field.name`, `graphql.operation.name` |
| `lora.cypher` | `db.system` (`loradb`), `db.operation.name` (`read` or `write`), `graphql.field.name`, `db.response.returned_rows`, and `db.statement` with `traceStatements: true` |

A failure records the exception on the span and sets its status to
error. Your server's own GraphQL instrumentation (for example an
Envelop or Apollo tracing plugin) supplies the request and operation
spans around these.

`db.statement` holds statement text only, never parameter values. The
text names labels, properties and relationship types from your model, so
leave `traceStatements` off if your traces leave your trust boundary.

## Metrics

`metrics` takes any object with two methods:

```ts
new LoraGraphQL({
  typeDefs,
  driver,
  metrics: {
    counter: (name, value, attributes) => meter.createCounter(name).add(value, attributes),
    histogram: (name, value, attributes) => meter.createHistogram(name).record(value, attributes),
  },
});
```

(Create each instrument once and cache it in real code.)

| Metric | Kind | Value |
| --- | --- | --- |
| `lora.graphql.statements` | counter | Statements run |
| `lora.graphql.errors` | counter | Failed statement calls |
| `lora.graphql.statement.duration` | histogram | Milliseconds per statement call |
| `lora.graphql.cost` | histogram | Estimated rows touched, for reads |

Every metric carries `field` and `mode` attributes, plus `operation` when
the operation is named. Named operations keep the attribute cardinality
bounded; anonymous operations all share the same attributes.

## Cost

Every read root field gets a cost estimate before it runs: the rows it
may touch, multiplying page sizes through nested lists and capped by
`@cardinality` or measured statistics. See
[statistics and cost](/docs/graphql/smart-layer#s6-statistics-and-cost).

```ts
new LoraGraphQL({
  typeDefs,
  driver,
  maxCost: 50_000,
  budget: (context) => ((context as { plan?: string }).plan === "free" ? 5_000 : undefined),
  onCost: ({ field, cost, total, limit }) => metrics.histogram("api.cost", cost, { field }),
});
```

`onCost` receives `field`, `cost` (this root field), `total` (the
operation so far, this field included), `limit` (from `budget` or
`maxCost`) and the `context`. It runs before the limit is checked, so it
also sees the fields that are then refused with `COST_EXCEEDED`.

`execute()` returns the operation's estimate as `extensions.cost`, so
clients can see what their queries cost and tune them.

To set a sensible `maxCost`, run with a high limit for a while, record
`onCost`, and set the limit above the largest cost your real clients
produce.

## Timing in responses

With `timing`, `execute()` returns the request's timings in
`extensions.timing`, next to `extensions.cost`:

```json
"extensions": {
  "cost": 51,
  "timing": {
    "totalMs": 4.21,
    "databaseMs": 3.05,
    "fields": {
      "all": { "totalMs": 2.9, "databaseMs": 2.4 },
      "one": { "totalMs": 0.8, "databaseMs": 0.65 }
    }
  }
}
```

- `totalMs` covers the whole `execute()` call: parse, validation and
  execution.
- `databaseMs` is the time LoraDB spent running statements, reads and
  mutations alike.
- `fields` gives both per root field, by response key (the alias when
  there is one).

```ts
const lora = new LoraGraphQL({
  typeDefs,
  driver,
  // every request, or decide per request from the GraphQL context:
  timing: (context) => context.jwt?.roles?.includes("admin") ?? false,
});
```

It is off by default: timings sent to clients can act as a timing side
channel (whether a filter rule hid rows, for example), so enable it for
trusted callers or in development. Servers that call graphql-js on
`getSchema()` directly get the same numbers from `onStatementEnd` and
`tracer`.

## Database errors

With `maskErrors` (the default when `NODE_ENV` is `production`), a client
sees only:

```json
{ "message": "database error (id 6f1c…)", "extensions": { "code": "DATABASE_ERROR", "id": "6f1c…" } }
```

`onError` receives the detail under the same `id`:

```ts
new LoraGraphQL({
  typeDefs,
  driver,
  onError: ({ id, field, message, error }) => log.error({ id, field, message }, "database error"),
});
```

`onError` fires for every database error, masked or not, so the same
handler works in development. When a user reports an error, search your
logs for its `id`. Errors the library writes itself (`FORBIDDEN`,
`NOT_FOUND`, `CONSTRAINT_VIOLATION` and the rest) are not database errors
and do not reach `onError`. See [errors](/docs/graphql/errors).

## Reading plans in production

The hooks above say that a statement was slow. To see why, compile and
plan the same operation:

```ts
const [{ reports }] = await lora.explain(document, variables, { context: { jwt } });
console.log(reports.map((r) => r.operators));
```

Pass the caller's context, since claim checks are folded into the
statement text. For a repeatable check, record the operation in your
`--operations` directory and let [`lora-graphql check --baseline`](/docs/graphql/cli#plan-baselines)
guard its plan.
