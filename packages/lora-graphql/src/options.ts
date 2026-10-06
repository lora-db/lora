// What callers pass to `LoraGraphQL`: the constructor options, the events
// its hooks receive, the per-request GraphQL context, and the arguments of
// `execute()`, `subscribe()`, `assertSchema()` and `check()`.

import type {
  DocumentNode,
  GraphQLFieldResolver,
  GraphQLScalarType,
} from "graphql";
import type { LoraDriver, Statement } from "./driver.js";
import type { PopulatedByCallback } from "./execute/mutate/env.js";
import type { LoraTransaction } from "./execute/transaction.js";
import type { DocumentGuards } from "./guards.js";
import type { ModelOptions } from "./model/build/limits.js";
import type { ObservabilityOptions } from "./observe.js";

export interface LoraGraphQLOptions extends ModelOptions, ObservabilityOptions {
  /** Annotated SDL: the graph model and the API in one document. */
  typeDefs: string | DocumentNode;
  driver: LoraDriver;
  /** Timeout for every statement, in milliseconds. Default 10 000; 0 disables. */
  timeoutMs?: number;
  /**
   * Time budget for all root fields of one query, in milliseconds: past
   * it, the operation's statements are aborted and its unfinished fields
   * fail with TIMEOUT. Default twice `timeoutMs` (20 000); 0 disables.
   * Subscriptions are not bounded by it; mutations keep `timeoutMs` per
   * statement.
   */
  operationTimeoutMs?: number;
  /**
   * Statements one operation runs at once: aliased root fields queue for
   * a slot instead of taking every libuv worker, so one request cannot
   * starve the others. Default 2.
   */
  maxConcurrentStatements?: number;
  /**
   * Approximate bytes the compile cache may hold, and again the parsed
   * document cache: entries are evicted oldest first past it, besides the
   * entry caps. Requests with more than 16 KiB of variables a field uses
   * (long `in:` lists, embedding vectors) are compiled but not cached.
   * Default 64 MiB.
   */
  compileCacheBytes?: number;
  /**
   * Reject a root field whose estimated rows touched exceed this, before
   * it runs. The estimate multiplies page sizes through nested lists,
   * capped by relationship degrees from `analyze()` or `@cardinality`,
   * and charges each filter the rows it examines: a label scan when no
   * index answers it, the related nodes of every relationship it follows.
   * Default 50 000; `Infinity` disables.
   */
  maxCost?: number;
  /**
   * The cost limit for one request, from its context (for example by
   * user or plan). Undefined falls back to `maxCost`.
   */
  budget?: (context: unknown) => number | undefined;
  /** Called with every root field's cost estimate, before it runs. */
  onCost?: (event: CostEvent) => void;
  /**
   * The request's verified claims, from the GraphQL context. Default:
   * `context.jwt`. The library never verifies tokens: do that in the
   * server and put the claims in the context.
   */
  jwt?: (context: unknown) => Record<string, unknown> | undefined;
  /**
   * Most nodes one mutation may create, update or delete, nested ones
   * included (and ten times as many relationships written). Default 1000: larger imports belong in a Cypher load, not a GraphQL
   * request. Also the default `limit` of bulk updates and deletes.
   */
  maxBatch?: number;
  /**
   * Most items a list argument of a `@cypher` field takes, unless its
   * `@size(max:)` says otherwise. More is BAD_USER_INPUT before the
   * statement runs. Default 1000.
   */
  maxListArgument?: number;
  /**
   * Relationship levels one `where` may nest: quantifiers (`some`,
   * `none`, `all`, `single`, `count`, `aggregate`), connection filters,
   * `<field>Exists` and filters through a single relationship each count
   * one. Deeper is BAD_USER_INPUT before anything runs: each level
   * multiplies the work by the relationship's degree. Default 2.
   */
  maxFilterDepth?: number;
  /**
   * Items an `in` filter operand may hold. More is BAD_USER_INPUT before
   * anything runs. Default 1000.
   */
  maxListFilter?: number;
  /**
   * Characters a string filter operand (`eq`, `contains`, an `in` item, …)
   * may hold. More is BAD_USER_INPUT before anything runs. Default 10 000.
   */
  maxStringFilter?: number;
  /**
   * Changes a `changes()` consumer or subscriber may fall behind before it
   * is ended with an error. Default 1000.
   */
  maxQueuedChanges?: number;
  /**
   * Live subscriptions one scope (see `subscriptionScope`) may hold; one
   * more is LIMIT_EXCEEDED. Default 100.
   */
  maxSubscriptions?: number;
  /**
   * What `maxSubscriptions` counts per: by default the context object, so
   * a server that reuses one context per connection (graphql-ws) limits
   * each connection. Return the connection (or user) otherwise.
   */
  subscriptionScope?: (context: unknown) => object | undefined;
  /**
   * How deep relationship filters may nest in a subscription's `where`,
   * which runs on every change. Deeper is LIMIT_EXCEEDED. Default 1.
   */
  maxSubscriptionFilterDepth?: number;
  /**
   * Per statement a subscription runs to check a change (visibility,
   * `where`, related nodes). Default 2000, or `timeoutMs` when lower.
   */
  subscriptionTimeoutMs?: number;
  /** Named callbacks for `@populatedBy(callback:)`. */
  callbacks?: Record<string, PopulatedByCallback>;
  /**
   * Implementations of custom scalars declared with `@storedAs`, e.g. one
   * that validates e-mail addresses. Without one a scalar passes through.
   */
  scalars?: Record<string, GraphQLScalarType>;
  /**
   * Feed subscriptions and `changes()` from the engine's committed change
   * feed (lora-node `db.changes()`): every write to the database, from
   * any path in the process that owns it, in commit order. `onWrite`
   * still reports this instance's mutations, and `previousState` needs
   * them. Default false.
   */
  changeFeed?: boolean;
  /**
   * Resolvers of `@customResolver` fields, by type and field name. The
   * source holds the node's selected fields plus the field's `requires`.
   */
  resolvers?: Record<
    string,
    Record<string, GraphQLFieldResolver<Record<string, unknown>, unknown>>
  >;
  /** Called with every statement before it runs; for logging and tests. */
  onStatement?: (event: StatementEvent) => void;
  /**
   * Hide database error details from clients: they get
   * `extensions.code` `DATABASE_ERROR` and an `id`, and `onError` gets the
   * engine's message. Default: on when `NODE_ENV` is `production`.
   */
  maskErrors?: boolean;
  /** Called with every database error, masked or not. */
  onError?: (event: DatabaseErrorEvent) => void;
  /**
   * Limits on the documents `execute()`, `subscribe()` and `persist()`
   * accept: depth,
   * aliases, root fields, tokens, introspection. `false` turns them off.
   * For other servers use `validationRules()` or `envelopPlugin()`.
   */
  guards?: DocumentGuards | false;
  /**
   * Make `execute()` and `subscribe()` refuse `source` and run only
   * persisted operations by `id`. Default false.
   */
  persistedOnly?: boolean;
  /**
   * How `execute()` commits a mutation with several root fields.
   * `"field"` (the default): each root field in its own transaction, so a
   * later failure leaves the earlier ones committed. `"operation"`: every
   * root field in one transaction, committed only when the operation
   * reports no error, rolled back (with `data: null`) otherwise. A
   * `transaction` in the context takes precedence. Envelop / Yoga servers
   * on `getSchema()` get the same with `lora.envelopPlugin()`; other
   * servers calling graphql-js directly put a `lora.begin()` transaction
   * in the context (without one, a one-time warning says each root field
   * committed on its own).
   */
  mutationTransaction?: "field" | "operation";
  /**
   * Add `extensions.timing` to `execute()` results: the request's total
   * time, the time spent in LoraDB, and both per root field, in
   * milliseconds. `true` for every request, or a function deciding per
   * request from the GraphQL context (for example only for admins or in
   * development). Default off: timings sent to clients can act as a
   * timing side channel.
   */
  timing?: boolean | ((context: unknown) => boolean);
}

export interface CostEvent {
  field: string;
  /** Estimated rows this root field touches. */
  cost: number;
  /** Estimated rows of the operation so far, this field included. */
  total: number;
  /** The limit that applies: `budget(context)` or `maxCost`. */
  limit: number;
  context: unknown;
}

export interface DatabaseErrorEvent {
  /** Correlation id, also in the client error's `extensions.id`. */
  id: string;
  /**
   * The root field that failed, or `changeFeed` for a failure of the
   * change feed (it reopens from where it stopped after a backoff).
   */
  field: string;
  /** The engine's message, which may name labels, properties and Cypher. */
  message: string;
  /** The error as thrown by the driver. */
  error: unknown;
}

export interface StatementEvent {
  field: string;
  statement: Statement;
}

/** Per-request options read from the GraphQL context, when present. */
export interface LoraGraphQLContext {
  /** Verified claims; enables @authentication and @authorization. */
  jwt?: Record<string, unknown>;
  /** Cancels the request's statements when aborted. */
  signal?: AbortSignal;
  /**
   * Run in this transaction (from `lora.begin()`) instead of one per
   * field: the caller commits, together with its own Cypher.
   */
  transaction?: LoraTransaction;
}

export interface AssertSchemaOptions {
  /** Create what is missing instead of only reporting it. */
  create?: boolean;
}

export interface CheckOptions {
  /**
   * Flag statements whose largest engine row estimate (from graph
   * statistics) exceeds this. Off by default: estimates ignore a LIMIT
   * that stops an index-ordered scan early.
   */
  rowBudget?: number;
  /**
   * Operations to compile and plan-check (S2), with example variables and
   * the GraphQL context to compile them under (e.g. `{ jwt }`, so rules
   * compile as they do for a signed-in caller). Without one, an operation
   * compiles as an anonymous request would.
   */
  operations?: Array<{
    name?: string;
    document: string | DocumentNode;
    variables?: Record<string, unknown>;
    context?: unknown;
  }>;
}

export interface ExecuteArgs {
  /** The document, or `id` of a persisted operation. */
  source?: string;
  id?: string;
  variables?: Record<string, unknown>;
  operationName?: string;
  context?: unknown;
  /**
   * Attach the operation's read-set to the result as a non-enumerable
   * `readSet` property (never serialized to the client), for a response
   * cache to invalidate with `lora.affects(result.readSet, change)`.
   */
  readSet?: boolean;
}
