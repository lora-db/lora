import {
  execute as graphqlExecute,
  subscribe as graphqlSubscribe,
  getOperationAST,
  getVariableValues,
  GraphQLError,
  Kind,
  lexicographicSortSchema,
  parse,
  printSchema,
  specifiedRules,
  validate,
  validateSchema,
  type DocumentNode,
  type ExecutionResult,
  type FieldNode,
  type FragmentDefinitionNode,
  type GraphQLObjectType,
  type GraphQLResolveInfo,
  type GraphQLSchema,
  type OperationDefinitionNode,
  type GraphQLFieldResolver,
  type GraphQLScalarType,
  type ValidationRule,
} from "graphql";
import {
  checkCypherFields,
  type CypherFinding,
} from "./analyze/cypher-check.js";
import {
  inferRequirements,
  requirementDdl,
  type SchemaRequirement,
} from "./analyze/indexes.js";
import { checkPlans, type PlanReport } from "./analyze/plans.js";
import { lintModel, unguardedMutations } from "./analyze/lint.js";
import {
  accessLints,
  accessMatrix,
  type AccessEntry,
} from "./analyze/access.js";
import { analyze, type Statistics } from "./analyze/statistics.js";
import { newContext, type CompileContext } from "./compile/context.js";
import {
  COMPILE_CACHE_BYTES,
  COMPILED_TOTAL,
  CompileCache,
  stableKey,
} from "./compile/cache.js";
import {
  compileAbstractRoot,
  compileCypherRoot,
  compileRoot,
  compileSearch,
  MAX_LIST_ARGUMENT,
  type CompiledRead,
  type ReadSet,
  type RootKind,
} from "./compile/read.js";
import {
  coercedVariables,
  fieldArgs,
  variableValuesOf,
  type SelectionContext,
} from "./compile/selection.js";
import type { LoraDriver, QueryResult, Statement } from "./driver.js";
import { affects, type WriteChange } from "./execute/changes.js";
import { executeCypherMutation } from "./execute/cypher-mutation.js";
import { LoraTransaction } from "./execute/transaction.js";
import { EngineFeed } from "./execute/feed.js";
import {
  anySignal,
  MAX_CONCURRENT_STATEMENTS,
  OPERATION_TIMEOUT_FACTOR,
  OperationBudget,
} from "./execute/budget.js";
import type { MutationKind } from "./schema/mutations.js";
import {
  executeMutation,
  mapWriteError,
  type MutationEnv,
  type PopulatedByCallback,
} from "./execute/mutate.js";
import { ModelError, requestError } from "./errors.js";
import {
  buildManifest,
  generateTypes,
  schemaHash,
  type OperationManifest,
} from "./codegen.js";
import {
  envelopPlugin,
  nodeEnv,
  parseOptions,
  validationRules,
  type DocumentGuards,
} from "./guards.js";
import { buildModel, type ModelOptions } from "./model/build.js";
import {
  Observer,
  type ObservabilityOptions,
  type StatementMeta,
} from "./observe.js";
import type {
  CypherField,
  GraphModel,
  ModelWarning,
  NodeType,
  SearchIndex,
} from "./model/types.js";
import { buildSchema, type ChangeEvent } from "./schema/build.js";
import {
  authFilter,
  authValidate,
  checkAuthentication,
  checkFieldAuthentication,
  rootFieldGuard,
} from "./compile/auth.js";
import {
  and,
  bin,
  printClauses,
  printExpr,
  prop,
  v,
} from "./compile/cypher.js";
import { bind } from "./compile/context.js";
import { keyOf } from "./compile/read.js";
import { MAX_FILTER_DEPTH } from "./compile/cost.js";
import { compileNodeWhere } from "./compile/filter.js";
import { lookupPath } from "./compile/auth.js";
import { fromGlobalId } from "./schema/global-id.js";
import { assertReadable } from "./schema/guard.js";
import { names } from "./schema/names.js";

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
   * Most nodes one mutation may create or delete, nested ones included.
   * Default 1000: larger imports belong in a Cypher load, not a GraphQL
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
   * `transaction` in the context takes precedence. Servers that call
   * graphql-js on `getSchema()` directly get per-operation atomicity by
   * putting a `lora.begin()` transaction in the context.
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

/** `extensions.timing` of an `execute()` result (see the `timing` option). */
export interface ExecutionTiming {
  /** The whole `execute()` call: parse, validation, execution. */
  totalMs: number;
  /** Time spent running statements in LoraDB. */
  databaseMs: number;
  /** Per root field, by response key (alias or name). */
  fields: Record<string, { totalMs: number; databaseMs: number }>;
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

export interface SchemaAssertion {
  required: SchemaRequirement[];
  missing: SchemaRequirement[];
  created: SchemaRequirement[];
  /**
   * Full-text and vector indexes present under their name but defined
   * differently (labels, properties, kind): search would keep using the
   * old definition. Without `create`; with it they are re-created.
   */
  mismatched: SchemaRequirement[];
  /** Indexes `create` dropped and created again with the model's definition. */
  recreated: SchemaRequirement[];
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

export interface CheckReport {
  /** True when nothing below is a failure. */
  ok: boolean;
  /** Model warnings, e.g. an unused @cypher argument. */
  warnings: readonly ModelWarning[];
  /** @cypher statements the engine rejects or that write from a query. */
  cypher: CypherFinding[];
  /** Constraints and indexes the database lacks. */
  missing: SchemaRequirement[];
  /** Indexes the database has that no part of the API needs. */
  unused: Array<{
    name: string;
    type: string;
    labels: string[];
    properties: string[];
  }>;
  /** Schema lint: valid but costly or risky choices (not failures). */
  lint: readonly ModelWarning[];
  /**
   * `@mutation` types with writes no rule guards (failures): declare them
   * with `@authorization(public: [...])` when that is intended.
   */
  security: readonly ModelWarning[];
  /** Plan findings per operation and root field. */
  plans: Array<{ operation: string; field: string; reports: PlanReport[] }>;
  /** Operations that failed to compile. */
  errors: Array<{ operation: string; message: string }>;
}

export interface ExecuteArgs {
  /** The document, or `id` of a persisted operation. */
  source?: string;
  id?: string;
  variables?: Record<string, unknown>;
  operationName?: string;
  context?: unknown;
}

const DOCUMENT_CACHE_SIZE = 500;
/**
 * A parsed document's approximate size: measured at about 100 bytes of
 * AST, locations and tokens per source character.
 */
const documentBytes = (source: string) => 100 * source.length;
/** A subscriber's share of one change: the events of its node type. */
interface SubscriberDelivery {
  change: WriteChange;
  events: ChangeEvent[];
}
type SubscriberSink = (delivery: SubscriberDelivery) => void;

interface SubscriberGroup {
  node: NodeType;
  all: Set<SubscriberSink>;
  byKey: Map<string, Set<SubscriberSink>>;
}

/**
 * A statement compiled once for a subscription, reused while its claims
 * and the `$context` values it read are unchanged. `param` names the
 * parameter each run fills in; `share` keys the reads subscribers share.
 */
interface SubscriptionCompile<C> {
  claims: string;
  statistics: number;
  variables: string | undefined;
  contextReads: Array<[string, unknown]>;
  compiled: C;
  param: string;
  share: string | undefined;
}

export class LoraGraphQL {
  readonly model: GraphModel;
  readonly #driver: LoraDriver;
  readonly #timeoutMs: number;
  readonly #operationTimeoutMs: number;
  readonly #maxConcurrentStatements: number;
  /** Deadline and statement slots per request context and operation. */
  readonly #operations = new WeakMap<
    object,
    Map<OperationDefinitionNode, OperationBudget>
  >();
  readonly #maxCost: number;
  readonly #maxBatch: number;
  readonly #maxListArgument: number;
  readonly #maxFilterDepth: number;
  readonly #maxListFilter: number | undefined;
  readonly #maxStringFilter: number | undefined;
  readonly #maxQueued: number;
  readonly #callbacks: Record<string, PopulatedByCallback>;
  readonly #resolvers: NonNullable<LoraGraphQLOptions["resolvers"]>;
  readonly #scalars: LoraGraphQLOptions["scalars"];
  /** Estimated cost spent per request context and operation. */
  readonly #spent = new WeakMap<object, Map<unknown, number>>();
  /**
   * Cost spent per subscription event (its root value). A subscription
   * reuses one context and operation for every event, so each event is
   * charged on its own, like one operation.
   */
  readonly #eventSpent = new WeakMap<object, number>();
  readonly #jwt: (context: unknown) => Record<string, unknown> | undefined;
  readonly #onStatement: LoraGraphQLOptions["onStatement"];
  readonly #maskErrors: boolean;
  readonly #observer: Observer;
  readonly #budget: LoraGraphQLOptions["budget"];
  readonly #onCost: LoraGraphQLOptions["onCost"];
  /** Persisted operation ids by operation node, for statement events. */
  readonly #persistedIds = new WeakMap<OperationDefinitionNode, string>();
  readonly #guards: DocumentGuards | false;
  readonly #persistedOnly: boolean;
  readonly #onError: LoraGraphQLOptions["onError"];
  readonly #listeners = new Set<(change: WriteChange) => void>();
  /** Consumers of `changes()` when the engine feed is on. */
  readonly #feedListeners = new Set<(change: WriteChange) => void>();
  /**
   * Subscribers by node type, then unfiltered or by key: a change reaches
   * only the subscribers of the types it touches, with its events built
   * once per type.
   */
  readonly #subscribers = new Map<string, SubscriberGroup>();
  #undispatch: (() => unknown) | undefined;
  /** Changed-node reads compiled per subscription context and field node. */
  readonly #byKeyCompiles = new WeakMap<
    object,
    WeakMap<FieldNode, SubscriptionCompile<CompiledRead>>
  >();
  readonly #feed: EngineFeed | undefined;
  #feedReady: Promise<void> | undefined;
  readonly #documents = new Map<string, DocumentNode>();
  readonly #persisted = new Map<string, DocumentNode>();
  #degrees = new Map<string, number>();
  /** Bumped when statistics change: they shape cost estimates. */
  #statisticsVersion = 0;
  /**
   * Compiled reads by root field node (documents are cached, so a field
   * node repeats across requests), then by what each compile read.
   */
  readonly #compiled: CompileCache;
  readonly #documentBytesMax: number;
  #documentBytes = 0;
  /**
   * Reads made for subscribers of one change, by statement text and
   * parameters: subscribers whose checks or node reads compile to the same
   * statement share one database call.
   */
  readonly #sharedReads = new WeakMap<
    object,
    Map<string, Promise<QueryResult[]>>
  >();
  #statistics: Statistics | undefined;
  #schema: GraphQLSchema | undefined;
  readonly #mutationTransaction: "field" | "operation";
  readonly #timing: LoraGraphQLOptions["timing"];
  /** Per-request timing collectors, by GraphQL context (see `timing`). */
  readonly #timings = new WeakMap<
    object,
    Map<string, { totalMs: number; databaseMs: number }>
  >();

  constructor(options: LoraGraphQLOptions) {
    this.model = buildModel(options.typeDefs, options);
    this.#driver = options.driver;
    this.#timeoutMs = options.timeoutMs ?? 10_000;
    const cacheBytes = options.compileCacheBytes ?? COMPILE_CACHE_BYTES;
    this.#compiled = new CompileCache(COMPILED_TOTAL, cacheBytes);
    this.#documentBytesMax = cacheBytes;
    this.#operationTimeoutMs =
      options.operationTimeoutMs ?? this.#timeoutMs * OPERATION_TIMEOUT_FACTOR;
    this.#maxConcurrentStatements =
      options.maxConcurrentStatements ?? MAX_CONCURRENT_STATEMENTS;
    this.#maxCost = options.maxCost ?? 50_000;
    this.#maxBatch = options.maxBatch ?? 1000;
    this.#maxListArgument = options.maxListArgument ?? MAX_LIST_ARGUMENT;
    this.#maxFilterDepth = options.maxFilterDepth ?? MAX_FILTER_DEPTH;
    this.#maxListFilter = options.maxListFilter;
    this.#maxStringFilter = options.maxStringFilter;
    this.#maxQueued = options.maxQueuedChanges ?? 1000;
    this.#callbacks = options.callbacks ?? {};
    const missing = [...this.model.nodes.values()].flatMap((n) =>
      [...n.fields.values()].flatMap((f) =>
        f.kind === "scalar" &&
        f.populatedBy &&
        !(f.populatedBy.callback in this.#callbacks)
          ? [`${n.name}.${f.name}: no callback named ${f.populatedBy.callback}`]
          : [],
      ),
    );
    this.#resolvers = options.resolvers ?? {};
    this.#scalars = options.scalars;
    for (const n of this.model.nodes.values()) {
      for (const f of n.fields.values()) {
        if (f.kind === "custom" && !this.#resolvers[n.name]?.[f.name]) {
          missing.push(
            `${n.name}.${f.name}: @customResolver needs resolvers.${n.name}.${f.name}`,
          );
        }
      }
    }
    if (missing.length > 0) {
      throw new ModelError(missing.map((message) => ({ message })));
    }
    this.#jwt =
      options.jwt ??
      ((context) => (context as LoraGraphQLContext | undefined)?.jwt);
    this.#onStatement = options.onStatement;
    this.#maskErrors = options.maskErrors ?? nodeEnv() === "production";
    this.#onError = options.onError;
    this.#guards = options.guards ?? {};
    if (options.changeFeed) {
      if (!options.driver.changes) {
        throw new Error(
          "changeFeed needs a driver with changes() (@loradb/lora-node)",
        );
      }
      this.#feed = new EngineFeed(
        options.driver,
        this.model,
        (change) => {
          change.timestamp ??= new Date().toISOString();
          for (const l of this.#feedListeners) {
            try {
              l(change);
            } catch {
              // A consumer's failure must not stop the feed.
            }
          }
        },
        (err) =>
          this.#onError?.({
            id: globalThis.crypto.randomUUID(),
            field: "changeFeed",
            message: err instanceof Error ? err.message : String(err),
            error: err,
          }),
      );
    }
    this.#observer = new Observer(options);
    this.#budget = options.budget;
    this.#onCost = options.onCost;
    this.#persistedOnly = options.persistedOnly ?? false;
    this.#mutationTransaction = options.mutationTransaction ?? "field";
    this.#timing = options.timing;
  }

  /** The configured document guards as validation rules. */
  validationRules(): ValidationRule[] {
    return this.#guards === false ? [] : validationRules(this.#guards);
  }

  /** The configured document guards as an Envelop / Yoga plugin. */
  envelopPlugin(): ReturnType<typeof envelopPlugin> {
    return envelopPlugin(
      this.#guards === false
        ? {
            maxDepth: Infinity,
            maxIntrospectionDepth: Infinity,
            maxAliases: Infinity,
            maxRootFields: Infinity,
            maxTokens: Infinity,
            introspection: true,
          }
        : this.#guards,
    );
  }

  #parse(source: string): DocumentNode {
    return parse(
      source,
      this.#guards === false ? undefined : parseOptions(this.#guards),
    );
  }

  #validate(document: DocumentNode): readonly GraphQLError[] {
    return validate(this.getSchema(), document, [
      ...specifiedRules,
      ...this.validationRules(),
    ]);
  }

  /** The executable schema, for any graphql-js server. */
  getSchema(): GraphQLSchema {
    const span = <T>(
      info: GraphQLResolveInfo,
      context: unknown,
      fn: () => Promise<T>,
    ) => {
      const traced = () =>
        this.#observer.field(
          {
            field: info.fieldName,
            operationName: info.operation.name?.value,
          },
          fn,
        );
      const timing = this.#timingOf(context);
      if (!timing || info.path.prev !== undefined) return traced();
      const start = performance.now();
      return traced().finally(() => {
        timingEntry(timing, String(info.path.key)).totalMs +=
          performance.now() - start;
      });
    };
    if (this.#schema) return this.#schema;
    const schema = buildSchema(this.model, {
      scalars: this.#scalars,
      customResolver: (node, field) => this.#resolvers[node.name]![field.name]!,
      subscribe: (node, args, context) => this.#subscribe(node, args, context),
      resolveChangedNode: (node, event, info, context) =>
        event.operation === "DELETE"
          ? Promise.resolve(null)
          : span(info, context, () =>
              this.#resolveByKey(
                node,
                event.key,
                info,
                context,
                (event as { [CHANGE]?: WriteChange })[CHANGE],
              ),
            ),
      resolveAbstract: (abstract, info, context) =>
        span(info, context, () => {
          const compiled = this.#cachedCompile(info, context, (ctx) =>
            compileAbstractRoot(
              ctx,
              abstract,
              this.#args(ctx, info.parentType, info.fieldNodes),
              info.fieldNodes,
            ),
          );
          return this.#run(info.fieldName, compiled, context, info);
        }),
      resolveSearch: (node, index, info, context, connection) =>
        span(info, context, () =>
          this.#resolveSearch(node, index, info, context, connection),
        ),
      resolveRoot: (kind, node, info, context) =>
        span(info, context, () => this.#resolveRoot(kind, node, info, context)),
      resolveNode: (id, info, context) =>
        span(info, context, () => this.#resolveNode(id, info, context)),
      resolveCypher: (field, info, context) =>
        span(info, context, () => this.#resolveCypher(field, info, context)),
      resolveMutation: (op, node, info, context) =>
        span(info, context, () =>
          this.#resolveMutation(op, node, info, context),
        ),
    });
    checkCustomRequires(this.model, schema);
    // A generated schema graphql-js rejects would fail every request:
    // fail here instead, where the server wires the schema up.
    const invalid = validateSchema(schema);
    if (invalid.length > 0) {
      throw new ModelError(
        invalid.map((e) => ({ message: `generated schema: ${e.message}` })),
      );
    }
    this.#schema = schema;
    return schema;
  }

  /** The client-facing SDL: generated types only, no model directives. */
  printPublicSchema(): string {
    return printSchema(lexicographicSortSchema(this.getSchema()));
  }

  /** Constraints and indexes the API needs (S1), with the reason for each. */
  requirements(): SchemaRequirement[] {
    return inferRequirements(this.model);
  }

  /**
   * Verify the database has every constraint and index the API needs;
   * with `create`, add what is missing. Idempotent. Runs one DDL
   * statement at a time: LoraDB rejects schema commands in transactions.
   */
  async assertSchema(
    options: AssertSchemaOptions = {},
  ): Promise<SchemaAssertion> {
    const required = this.requirements();
    const [indexes, constraints] = await this.#driver.run(
      [
        { text: "SHOW INDEXES", params: {} },
        { text: "SHOW CONSTRAINTS", params: {} },
      ],
      { mode: "read", timeoutMs: this.#timeoutMs },
    );
    const byName = (r: SchemaRequirement) =>
      indexes!.rows.find((row) => row["name"] === r.name);
    // A named index whose definition differs from the model's: matching by
    // name alone would keep searching the old field list.
    const differs = (r: SchemaRequirement) => {
      if (r.kind !== "fulltext" && r.kind !== "vector") return false;
      const row = byName(r);
      if (!row) return false;
      const type = r.kind === "fulltext" ? "FULLTEXT" : "VECTOR";
      const wantedProps = r.kind === "fulltext" ? r.properties : [r.property];
      const same = (a: unknown, b: readonly string[]) =>
        Array.isArray(a) &&
        a.length === b.length &&
        [...a].map(String).sort().join("\0") === [...b].sort().join("\0");
      const analyzer = (
        row["options"] as Record<string, unknown> | undefined
      )?.["fulltext.analyzer"];
      return (
        row["type"] !== type ||
        !same(row["labelsOrTypes"], [r.label]) ||
        !same(row["properties"], wantedProps) ||
        (r.kind === "fulltext" &&
          typeof analyzer === "string" &&
          analyzer.toUpperCase() !== r.analyzer)
      );
    };
    const present = (r: SchemaRequirement) => {
      if (r.kind === "fulltext" || r.kind === "vector") {
        return byName(r) !== undefined;
      }
      if (r.kind === "constraint") {
        const wanted = {
          NODE_KEY: ["NODE_KEY"],
          UNIQUE: ["NODE_KEY", "NODE_PROPERTY_UNIQUENESS"],
          NOT_NULL: ["NODE_KEY", "NODE_PROPERTY_EXISTENCE"],
        }[r.constraint];
        return constraints!.rows.some(
          (row) =>
            wanted.includes(String(row["type"])) &&
            sameTarget(row, r.label, r.property),
        );
      }
      return indexes!.rows.some(
        (row) =>
          row["type"] === r.index && sameTarget(row, r.label, r.property),
      );
    };
    const missing = required.filter((r) => !present(r));
    const mismatched = required.filter(differs);
    const created: SchemaRequirement[] = [];
    const recreated: SchemaRequirement[] = [];
    if (options.create) {
      for (const r of mismatched) {
        await this.#driver.run(
          [
            { text: `DROP INDEX ${quoteName(r.name)}`, params: {} },
            { text: requirementDdl(r), params: {} },
          ],
          { mode: "write", timeoutMs: this.#timeoutMs },
        );
        recreated.push(r);
      }
      for (const r of missing) {
        await this.#driver.run([{ text: requirementDdl(r), params: {} }], {
          mode: "write",
          timeoutMs: this.#timeoutMs,
        });
        created.push(r);
      }
    }
    return {
      required,
      missing: options.create ? [] : missing,
      created,
      mismatched: options.create ? [] : mismatched,
      recreated,
    };
  }

  // -------------------------------------------------------------------------
  // Analysis
  // -------------------------------------------------------------------------

  /**
   * Count nodes and measure relationship degrees (S6). Cost estimates then
   * use each relationship's maximum degree, capped by its page limit,
   * instead of the page limit alone; filters use node counts and mean
   * degrees (see `src/compile/cost.ts`).
   */
  async analyze(options: { sample?: number } = {}): Promise<Statistics> {
    const stats = await analyze(this.#driver, this.model, {
      ...options,
      timeoutMs: this.#timeoutMs,
    });
    this.useStatistics(stats);
    return stats;
  }

  /** Use statistics gathered earlier, e.g. by the CLI. */
  useStatistics(stats: Statistics): void {
    this.#statistics = stats;
    this.#degrees = new Map(
      // The maximum: the caller chooses the parents (see statistics.ts).
      Object.entries(stats.degrees).map(([k, d]) => [k, d.max]),
    );
    this.#statisticsVersion++;
  }

  get statistics(): Statistics | undefined {
    return this.#statistics;
  }

  /**
   * The CI gate: @cypher statements plan, the database has what the API
   * needs, and each operation's statements seek where they should.
   */
  /**
   * Who may do what: for every type and guarded field, each operation as
   * each kind of caller (anonymous, authenticated, and each role the rules
   * test), with the verdict and the rules that decide it. Read off the
   * model; stable, so it can be snapshotted and reviewed as a diff.
   */
  accessMatrix(): AccessEntry[] {
    return accessMatrix(this.model, this.getSchema());
  }

  async check(options: CheckOptions = {}): Promise<CheckReport> {
    const report: CheckReport = {
      ok: true,
      warnings: this.model.warnings,
      cypher: await checkCypherFields(this.#driver, this.model),
      // An index defined differently from the model is as good as missing.
      missing: await this.assertSchema().then((a) => [
        ...a.missing,
        ...a.mismatched,
      ]),
      unused: await this.#unusedIndexes(),
      lint: [
        ...lintModel(this.model, { statistics: !!this.#statistics }),
        ...accessLints(this.model, this.getSchema()),
      ],
      security: unguardedMutations(this.model),
      plans: [],
      errors: [],
    };
    for (const [i, op] of (options.operations ?? []).entries()) {
      const operation = op.name ?? `operation ${i + 1}`;
      try {
        const fields = await this.explain(op.document, op.variables ?? {}, {
          rowBudget: options.rowBudget,
          context: op.context,
        });
        for (const f of fields) report.plans.push({ operation, ...f });
      } catch (err) {
        report.errors.push({
          operation,
          message: err instanceof Error ? err.message : String(err),
        });
      }
    }
    report.ok =
      report.security.length === 0 &&
      report.cypher.length === 0 &&
      report.missing.length === 0 &&
      report.errors.length === 0 &&
      report.plans.every((p) =>
        p.reports.every((r) => r.findings.length === 0),
      );
    return report;
  }

  /** Indexes in the database that no requirement of the API matches. */
  async #unusedIndexes(): Promise<CheckReport["unused"]> {
    const [indexes, constraints] = await this.#driver.run(
      [
        { text: "SHOW INDEXES", params: {} },
        { text: "SHOW CONSTRAINTS", params: {} },
      ],
      { mode: "read", timeoutMs: this.#timeoutMs },
    );
    // A constraint's backing index serves the constraint: LoraDB names it
    // in the constraint's `ownedIndex`, not on the index row.
    const owned = new Set(
      constraints!.rows.map((row) => row["ownedIndex"]).filter(Boolean),
    );
    const required = this.requirements();
    return indexes!.rows
      .filter(
        (row) =>
          row["type"] !== "LOOKUP" &&
          !row["owningConstraint"] &&
          !owned.has(row["name"]),
      )
      .filter(
        (row) =>
          !required.some((r) =>
            r.kind === "fulltext" || r.kind === "vector"
              ? r.name === row["name"]
              : r.kind === "index" &&
                r.index === row["type"] &&
                sameTarget(row, r.label, r.property),
          ),
      )
      .map((row) => ({
        name: String(row["name"]),
        type: String(row["type"]),
        labels: (row["labelsOrTypes"] as string[] | null) ?? [],
        properties: (row["properties"] as string[] | null) ?? [],
      }));
  }

  /**
   * Compile a query without running it: one entry per root field, with its
   * statements, parameters, read-set, cost and expected access path.
   */
  compile(
    document: string | DocumentNode,
    variables: Record<string, unknown> = {},
    options: { operationName?: string; context?: unknown } = {},
  ): Array<{ field: string; compiled: CompiledRead }> {
    const schema = this.getSchema();
    const doc = typeof document === "string" ? parse(document) : document;
    const invalid = validate(schema, doc);
    if (invalid.length > 0) throw invalid[0]!;
    const operation = findOperation(doc, options.operationName);
    if (operation.operation !== "query") {
      throw new GraphQLError("compile() and explain() take queries");
    }
    const coerced = getVariableValues(
      schema,
      operation.variableDefinitions ?? [],
      variables,
    );
    if (coerced.errors) throw coerced.errors[0]!;
    const fragments: Record<string, FragmentDefinitionNode> = {};
    for (const def of doc.definitions) {
      if (def.kind === Kind.FRAGMENT_DEFINITION)
        fragments[def.name.value] = def;
    }
    const base: SelectionContext = {
      schema,
      fragments,
      variables: variableValuesOf(coerced),
    };
    const queryType = schema.getQueryType()!;
    const out: Array<{ field: string; compiled: CompiledRead }> = [];
    for (const sel of operation.selectionSet.selections) {
      if (sel.kind !== Kind.FIELD || sel.name.value.startsWith("__")) continue;
      const def = queryType.getFields()[sel.name.value];
      if (!def) continue;
      const ctx = this.#context(base, options.context);
      const args = fieldArgs(ctx, def, sel);
      const cypher = this.model.queries.find((q) => q.name === sel.name.value);
      const root = this.#rootOf(sel.name.value);
      const search = this.#searchOf(sel.name.value);
      let compiled: CompiledRead | undefined;
      const abstract = [...this.model.abstracts.values()].find(
        (x) => x.read && x.plural === sel.name.value,
      );
      if (cypher) compiled = compileCypherRoot(ctx, cypher, args, [sel]);
      else if (abstract) {
        compiled = compileAbstractRoot(ctx, abstract, args, [sel]);
      } else if (search) {
        compiled = compileSearch(
          ctx,
          search.node,
          search.index,
          args,
          [sel],
          search.connection,
        );
      } else if (root) {
        compiled = compileRoot(ctx, root.kind, root.node, args, [sel]);
      }
      if (compiled) {
        out.push({ field: sel.alias?.value ?? sel.name.value, compiled });
      }
    }
    return out;
  }

  /** Compile a query and check every statement's plan (S2). */
  async explain(
    document: string | DocumentNode,
    variables: Record<string, unknown> = {},
    options: {
      operationName?: string;
      context?: unknown;
      rowBudget?: number | undefined;
    } = {},
  ): Promise<Array<{ field: string; reports: PlanReport[] }>> {
    const out: Array<{ field: string; reports: PlanReport[] }> = [];
    for (const { field, compiled } of this.compile(
      document,
      variables,
      options,
    )) {
      out.push({
        field,
        reports: await checkPlans(this.#driver, compiled, {
          ...(options.rowBudget !== undefined
            ? { rowBudget: options.rowBudget }
            : {}),
        }),
      });
    }
    return out;
  }

  // -------------------------------------------------------------------------
  // Execution helpers
  // -------------------------------------------------------------------------

  /**
   * Register persisted operations by id. Every document is parsed and
   * validated now, so a broken one fails at startup; at request time an
   * id is looked up and executed without parsing or validating.
   */
  persist(operations: Record<string, string>): void {
    const problems: string[] = [];
    for (const [id, source] of Object.entries(operations)) {
      try {
        const doc = this.#parse(source);
        const errors = this.#validate(doc);
        if (errors.length > 0) {
          problems.push(`${id}: ${errors.map((e) => e.message).join("; ")}`);
        } else {
          this.#persisted.set(id, doc);
          for (const def of doc.definitions) {
            if (def.kind === Kind.OPERATION_DEFINITION) {
              this.#persistedIds.set(def, id);
            }
          }
        }
      } catch (err) {
        problems.push(
          `${id}: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    }
    if (problems.length > 0) {
      throw new Error(
        `invalid persisted operations:\n  ${problems.join("\n  ")}`,
      );
    }
  }

  /**
   * Validate persisted operations (id → source) into a manifest for
   * `loadManifest()`, built once at build time (`lora-graphql compile`).
   */
  buildManifest(operations: Record<string, string>): OperationManifest {
    return buildManifest(this.getSchema(), operations, this.validationRules());
  }

  /** TypeScript types for a manifest's variables and results. */
  generateTypes(manifest: OperationManifest): string {
    return generateTypes(this.getSchema(), manifest, this.model.scalars);
  }

  /**
   * Register a manifest's operations as persisted operations, without
   * parsing or validating them again. Refused when the manifest was built
   * for another schema.
   */
  loadManifest(manifest: OperationManifest): void {
    const hash = schemaHash(this.getSchema());
    if (manifest.schemaHash !== hash) {
      throw new Error(
        "the manifest was built for a different schema; run lora-graphql compile again",
      );
    }
    for (const [id, entry] of Object.entries(manifest.operations)) {
      this.#persisted.set(id, entry.document);
      for (const def of entry.document.definitions) {
        if (def.kind === Kind.OPERATION_DEFINITION) {
          this.#persistedIds.set(def, id);
        }
      }
    }
  }

  /**
   * Execute a query or mutation against the schema. Parsed and validated
   * documents are cached by source text; persisted ones by id. A
   * subscription runs with {@link LoraGraphQL.subscribe}; given one, this
   * returns a `WRONG_OPERATION_TYPE` error.
   */
  async execute(args: ExecuteArgs): Promise<ExecutionResult> {
    const started = performance.now();
    const document = this.#document(args);
    if (!isDocument(document)) return document;
    if (operationType(document, args.operationName) === "subscription") {
      return {
        errors: [
          requestError(
            "WRONG_OPERATION_TYPE",
            "execute() runs queries and mutations; run a subscription with subscribe()",
          ),
        ],
      };
    }
    let contextValue = args.context ?? {};
    // One transaction for every root field of the mutation, when asked.
    const operation = getOperationAST(document, args.operationName);
    const atomic =
      this.#mutationTransaction === "operation" &&
      operation?.operation === "mutation" &&
      operation.selectionSet.selections.length +
        (operation.selectionSet.selections.some((s) => s.kind !== Kind.FIELD)
          ? 1
          : 0) >
        1 &&
      !(contextValue as LoraGraphQLContext).transaction &&
      this.#driver.begin !== undefined;
    let tx: LoraTransaction | undefined;
    if (atomic) {
      tx = await this.begin();
      contextValue = Object.assign(
        Object.create(Object.getPrototypeOf(contextValue) as object) as object,
        contextValue,
        { transaction: tx },
      );
    }
    const timed =
      typeof this.#timing === "function"
        ? this.#timing(contextValue)
        : this.#timing === true;
    if (timed && contextValue !== null && typeof contextValue === "object") {
      this.#timings.set(contextValue, new Map());
    }
    let result: ExecutionResult;
    try {
      result = (await graphqlExecute({
        schema: this.getSchema(),
        document,
        variableValues: args.variables,
        operationName: args.operationName,
        contextValue,
      })) as ExecutionResult;
    } catch (err) {
      await tx?.rollback();
      throw err;
    }
    // graphql-js reports bad variables without a code; clients branch on it.
    if (result.errors?.some(isVariableError)) {
      result = {
        ...result,
        errors: result.errors.map((e) =>
          isVariableError(e)
            ? new GraphQLError(e.message, {
                ...(e.nodes ? { nodes: e.nodes } : {}),
                extensions: { ...e.extensions, code: "BAD_USER_INPUT" },
              })
            : e,
        ),
      };
    }
    if (tx) {
      if (result.errors?.length) {
        // Nothing of the operation was written: say so with the data.
        await tx.rollback();
        result = { ...result, data: null };
      } else {
        try {
          await tx.commit();
        } catch (err) {
          await tx.rollback().catch(() => undefined);
          const error = this.#databaseError("commit", err);
          return {
            data: null,
            errors: [
              error instanceof GraphQLError
                ? error
                : new GraphQLError(
                    error instanceof Error ? error.message : String(error),
                  ),
            ],
          };
        }
      }
    }
    // The operation's cost estimate, so clients can tune their queries.
    const spent =
      contextValue !== null && typeof contextValue === "object"
        ? this.#spent.get(contextValue)
        : undefined;
    if (spent && spent.size > 0) {
      const cost = [...spent.values()].reduce((a, b) => a + b, 0);
      result = {
        ...result,
        extensions: { ...result.extensions, cost: Math.ceil(cost) },
      };
    }
    const timing = this.#timingOf(contextValue);
    if (timing) {
      const fields: ExecutionTiming["fields"] = {};
      let databaseMs = 0;
      for (const [key, t] of timing) {
        fields[key] = { totalMs: ms(t.totalMs), databaseMs: ms(t.databaseMs) };
        databaseMs += t.databaseMs;
      }
      const executionTiming: ExecutionTiming = {
        totalMs: ms(performance.now() - started),
        databaseMs: ms(databaseMs),
        fields,
      };
      result = {
        ...result,
        extensions: { ...result.extensions, timing: executionTiming },
      };
    }
    return result;
  }

  /**
   * Run a subscription: an async iterable of results, one per event, or a
   * single result with the errors when it cannot start (an unknown id, a
   * validation error, a denied subscribe). Takes the same arguments as
   * {@link LoraGraphQL.execute}, a persisted id included, with the same
   * document cache and guards; a query or mutation gets a
   * `WRONG_OPERATION_TYPE` error. End it with `return()` on the iterator
   * or with an `AbortSignal` passed as `context.signal`: either ends it at
   * once, without waiting for another event, and drops its listener. The
   * cost limit applies to each event on its own.
   */
  async subscribe(
    args: ExecuteArgs,
  ): Promise<AsyncIterableIterator<ExecutionResult> | ExecutionResult> {
    const document = this.#document(args);
    if (!isDocument(document)) return document;
    const type = operationType(document, args.operationName);
    if (type && type !== "subscription") {
      return {
        errors: [
          requestError(
            "WRONG_OPERATION_TYPE",
            `subscribe() runs subscriptions; run a ${type} with execute()`,
          ),
        ],
      };
    }
    return graphqlSubscribe({
      schema: this.getSchema(),
      document,
      variableValues: args.variables,
      operationName: args.operationName,
      contextValue: args.context ?? {},
    }) as Promise<AsyncIterableIterator<ExecutionResult> | ExecutionResult>;
  }

  /**
   * The document `args` names: a persisted one by id, or `source` parsed
   * and validated through the cache. An error result when there is none.
   */
  #document(args: ExecuteArgs): DocumentNode | ExecutionResult {
    let document: DocumentNode | undefined;
    if (args.id !== undefined) {
      document = this.#persisted.get(args.id);
      if (!document) {
        return {
          errors: [new GraphQLError(`unknown persisted operation ${args.id}`)],
        };
      }
    } else if (args.source !== undefined) {
      if (this.#persistedOnly) {
        return {
          errors: [
            new GraphQLError("only persisted operations are accepted", {
              extensions: { code: "PERSISTED_QUERY_ONLY" },
            }),
          ],
        };
      }
      document = this.#documents.get(args.source);
      if (!document) {
        try {
          document = this.#parse(args.source);
        } catch (err) {
          return { errors: [err as GraphQLError] };
        }
        const errors = this.#validate(document);
        if (errors.length > 0) return { errors };
        const bytes = documentBytes(args.source);
        if (bytes <= this.#documentBytesMax) {
          while (
            this.#documents.size > 0 &&
            (this.#documents.size >= DOCUMENT_CACHE_SIZE ||
              this.#documentBytes + bytes > this.#documentBytesMax)
          ) {
            const oldest = this.#documents.keys().next().value!;
            this.#documents.delete(oldest);
            this.#documentBytes -= documentBytes(oldest);
          }
          this.#documents.set(args.source, document);
          this.#documentBytes += bytes;
        }
      }
    } else {
      return {
        errors: [new GraphQLError("a source or an id is needed")],
      };
    }
    return document;
  }

  // -------------------------------------------------------------------------
  // Change tracking (S5)
  // -------------------------------------------------------------------------

  /** Called after every committed mutation with its exact write-set. */
  onWrite(listener: (change: WriteChange) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  /**
   * Committed writes as an async iterator, e.g. for a subscription
   * resolver. Only writes made through this instance are seen. `return()`
   * or aborting `signal` (already aborted included) ends it at once.
   */
  changes(
    options: { signal?: AbortSignal; maxQueued?: number } = {},
  ): AsyncIterableIterator<WriteChange> {
    return this.#queue<WriteChange>((listener) => {
      if (this.#feed) {
        this.#feedListeners.add(listener);
        return () => this.#feedListeners.delete(listener);
      }
      return this.onWrite(listener);
    }, options);
  }

  /**
   * An async iterator over the items a `register`ed listener receives,
   * queued up to `maxQueued`; `register` returns the unregister call.
   */
  #queue<T>(
    register: (listener: (item: T) => void) => () => void,
    options: { signal?: AbortSignal; maxQueued?: number },
  ): AsyncIterableIterator<T> {
    const limit = options.maxQueued ?? this.#maxQueued;
    const queue: T[] = [];
    let wake: (() => void) | undefined;
    let done = false;
    let overflow = false;
    const listener = (change: T) => {
      // A consumer that falls this far behind is ended with an error,
      // instead of holding every write in memory.
      if (queue.length >= limit) {
        overflow = true;
        stop();
      } else {
        queue.push(change);
      }
      wake?.();
    };
    const unregister = register(listener);
    const stop = () => void unregister();
    let ready: Promise<void> | undefined;
    if (this.#feed) {
      this.#feedReady ??= this.#feed.start();
      ready = this.#feedReady;
    }
    const finish = () => {
      done = true;
      stop();
      options.signal?.removeEventListener("abort", finish);
      wake?.();
    };
    if (options.signal?.aborted) finish();
    else options.signal?.addEventListener("abort", finish, { once: true });
    const iterator: AsyncIterableIterator<T> = {
      [Symbol.asyncIterator]: () => iterator,
      next: async () => {
        if (ready) await ready;
        while (queue.length === 0 && !done && !overflow) {
          await new Promise<void>((resolve) => (wake = resolve));
          wake = undefined;
        }
        if (queue.length > 0) return { value: queue.shift()!, done: false };
        if (overflow && !done) {
          done = true;
          throw requestError(
            "LIMIT_EXCEEDED",
            `the subscriber fell ${limit} changes behind; resubscribe and reload`,
          );
        }
        return { value: undefined, done: true };
      },
      return: async () => {
        finish();
        return { value: undefined, done: true };
      },
    };
    return iterator;
  }

  /** Whether a read with this read-set may observe the change. */
  affects(reads: ReadSet, change: WriteChange): boolean {
    return affects(
      reads,
      change,
      (type) => this.model.nodes.get(type)?.labels ?? [],
    );
  }

  // -------------------------------------------------------------------------
  // Resolvers
  // -------------------------------------------------------------------------

  #rootOf(fieldName: string): { kind: RootKind; node: NodeType } | undefined {
    for (const node of this.model.nodes.values()) {
      if (!node.read) continue;
      if (fieldName === names.listRoot(node)) return { kind: "list", node };
      if (fieldName === names.connectionRoot(node))
        return { kind: "connection", node };
      if (fieldName === names.singleRoot(node)) return { kind: "single", node };
      if (node.aggregate && fieldName === names.aggregateRoot(node)) {
        return { kind: "aggregate", node };
      }
      if (node.aggregate && fieldName === names.groupedRoot(node)) {
        return { kind: "grouped", node };
      }
    }
    return undefined;
  }

  #searchOf(
    fieldName: string,
  ): { node: NodeType; index: SearchIndex; connection: boolean } | undefined {
    for (const node of this.model.nodes.values()) {
      if (!node.read) continue;
      for (const index of node.search) {
        if (index.queryName === fieldName) {
          return { node, index, connection: false };
        }
        if (`${index.queryName}Connection` === fieldName) {
          return { node, index, connection: true };
        }
      }
    }
    return undefined;
  }

  #context(
    base: SelectionContext,
    context: unknown,
    jwt = this.#jwt(context),
  ): CompileContext {
    return newContext(base, this.model, {
      jwt,
      degrees: this.#degrees,
      statistics: this.#statistics,
      requestContext: context,
      maxListArgument: this.#maxListArgument,
      maxFilterDepth: this.#maxFilterDepth,
      maxListFilter: this.#maxListFilter,
      maxStringFilter: this.#maxStringFilter,
    });
  }

  /**
   * Charge `cost` to the operation: the limit holds per operation, so
   * aliasing a root field many times does not multiply it. A
   * subscription is charged per event.
   */
  #charge(
    field: string,
    cost: number,
    context: unknown,
    info?: GraphQLResolveInfo,
  ) {
    let total = cost;
    const event: unknown =
      info?.operation.operation === "subscription" ? info.rootValue : undefined;
    if (event !== null && typeof event === "object") {
      total = (this.#eventSpent.get(event) ?? 0) + cost;
      this.#eventSpent.set(event, total);
    } else if (info && context !== null && typeof context === "object") {
      const byOperation =
        this.#spent.get(context) ?? new Map<unknown, number>();
      total = (byOperation.get(info.operation) ?? 0) + cost;
      byOperation.set(info.operation, total);
      this.#spent.set(context, byOperation);
    }
    const limit = this.#budget?.(context) ?? this.#maxCost;
    try {
      this.#onCost?.({ field, cost, total, limit, context });
    } catch {
      // Observers must not fail the request.
    }
    if (total > limit) {
      throw requestError(
        "COST_EXCEEDED",
        `${field} would bring the operation to about ${Math.ceil(total)} rows touched; the limit is ${limit}. Ask for smaller pages or fewer nested lists.`,
        undefined,
        { cost: Math.ceil(total), maxCost: limit },
      );
    }
  }

  #args(
    ctx: CompileContext,
    parent: GraphQLObjectType,
    fieldNodes: readonly FieldNode[],
  ): Record<string, unknown> {
    const def = parent.getFields()[fieldNodes[0]!.name.value]!;
    return fieldArgs(ctx, def, fieldNodes[0]!);
  }

  /** `run` once per change for each distinct statement set. */
  #shared(
    change: object | undefined,
    statements: Statement[],
    run: () => Promise<QueryResult[]>,
    /** Precomputed key of the statements, from a reused compile. */
    shareKey?: string,
  ): Promise<QueryResult[]> {
    const key = change ? (shareKey ?? stableKey(statements)) : undefined;
    if (!change || key === undefined) return run();
    let reads = this.#sharedReads.get(change);
    if (!reads) {
      reads = new Map();
      this.#sharedReads.set(change, reads);
    }
    let pending = reads.get(key);
    if (!pending) {
      pending = run();
      reads.set(key, pending);
    }
    return pending;
  }

  async #run(
    field: string,
    compiled: CompiledRead,
    context: unknown,
    info?: GraphQLResolveInfo,
    change?: object,
    shareKey?: string,
  ): Promise<unknown> {
    this.#charge(field, compiled.cost, context, info);
    for (const statement of compiled.statements) {
      this.#onStatement?.({ field, statement });
    }
    const signal = (context as LoraGraphQLContext | undefined)?.signal;
    const owned = (context as LoraGraphQLContext | undefined)?.transaction;
    const budget = this.#operationBudget(context, info);
    let results;
    const timing = this.#timingOf(context);
    const dbStart = timing ? performance.now() : 0;
    try {
      const run = (opSignal?: AbortSignal) =>
        this.#driver.run(compiled.statements, {
          mode: compiled.mode,
          timeoutMs:
            budget && this.#timeoutMs > 0
              ? Math.max(1, Math.min(this.#timeoutMs, budget.remaining()))
              : this.#timeoutMs,
          signal: opSignal ? anySignal(signal, opSignal) : signal,
          // Queries and object @cypher fields are checked read-only
          // when the model is built; writes never reach this path.
          verified: compiled.mode === "read",
          ...(compiled.bounded && { bounded: true }),
        });
      results = await this.#observer.statements(
        this.#meta(field, compiled.mode, info, compiled.cost),
        compiled.statements,
        () =>
          owned
            ? runInOrder(owned, compiled.statements)
            : this.#shared(
                change,
                compiled.statements,
                // A bounded lookup streams synchronously: no slot needed.
                () => (budget && !compiled.bounded ? budget.run(run) : run()),
                shareKey,
              ),
      );
    } catch (err) {
      throw this.#databaseError(field, err);
    } finally {
      if (budget) this.#leaveOperation(context as object, info!, budget);
      if (timing) {
        timingEntry(timing, rootKey(info) ?? field).databaseMs +=
          performance.now() - dbStart;
      }
    }
    return assertReadable(compiled.shape(results));
  }

  /**
   * The deadline and statement slots of the query `info` belongs to,
   * started by its first root field. Subscriptions run indefinitely and
   * are bounded per statement only.
   */
  #operationBudget(
    context: unknown,
    info: GraphQLResolveInfo | undefined,
  ): OperationBudget | undefined {
    if (!info || info.operation.operation === "subscription") return undefined;
    if (context === null || typeof context !== "object") return undefined;
    let byOperation = this.#operations.get(context);
    if (!byOperation) {
      byOperation = new Map();
      this.#operations.set(context, byOperation);
    }
    let budget = byOperation.get(info.operation);
    if (!budget) {
      budget = new OperationBudget(
        this.#operationTimeoutMs,
        this.#maxConcurrentStatements,
      );
      byOperation.set(info.operation, budget);
    }
    budget.active++;
    return budget;
  }

  /**
   * A root field is done. When none is left in flight after the current
   * turn (graphql-js starts a query's root fields together, and the next
   * root field of a mutation in a microtask), the operation is over: a
   * context reused for another request starts a fresh budget.
   */
  #leaveOperation(
    context: object,
    info: GraphQLResolveInfo,
    budget: OperationBudget,
  ): void {
    budget.active--;
    if (budget.active > 0) return;
    setTimeout(() => {
      if (budget.active > 0) return;
      budget.dispose();
      const byOperation = this.#operations.get(context);
      if (byOperation?.get(info.operation) === budget) {
        byOperation.delete(info.operation);
      }
    }, 0);
  }

  #meta(
    field: string,
    mode: "read" | "write",
    info: GraphQLResolveInfo | undefined,
    cost?: number,
  ): StatementMeta {
    return {
      field,
      mode,
      cost,
      operationName: info?.operation.name?.value,
      persistedId: info && this.#persistedIds.get(info.operation),
    };
  }

  /**
   * Compile a read root field, or reuse the compile of an earlier request
   * that agrees on what this compile reads: the field node, the variables
   * it references, the claims and `$context` values it looked up (see
   * `CompileCache`).
   */
  #cachedCompile(
    info: GraphQLResolveInfo,
    context: unknown,
    compile: (ctx: CompileContext) => CompiledRead,
  ): CompiledRead {
    const base = infoContext(info);
    return this.#compiled.get(
      {
        fieldNodes: info.fieldNodes,
        fragments: info.fragments,
        variables: coercedVariables(info.variableValues),
        jwt: this.#jwt(context),
        context,
        statistics: this.#statisticsVersion,
      },
      (jwt) => {
        const ctx = this.#context(base, context, jwt);
        return { ctx, compiled: compile(ctx) };
      },
    );
  }

  #databaseError(field: string, err: unknown): unknown {
    let error: unknown;
    if (err instanceof GraphQLError) error = err;
    else {
      const mapped = mapWriteError(this.model, err);
      if (mapped !== err) error = mapped;
      else if ((err as { code?: string }).code === "LORA_INVALID_VECTOR") {
        error = requestError(
          "BAD_USER_INPUT",
          err instanceof Error ? err.message : String(err),
          err,
        );
      } else {
        error = requestError(
          "DATABASE_ERROR",
          err instanceof Error ? err.message : String(err),
          err,
        );
      }
    }
    if (!(error instanceof GraphQLError)) return error;
    if (error.extensions["code"] === "DATABASE_ERROR") {
      const id = globalThis.crypto.randomUUID();
      try {
        this.#onError?.({
          id,
          field,
          message: error.message,
          error: error.originalError ?? err,
        });
      } catch {
        // A failing error hook must not replace the request's error.
      }
      return this.#maskErrors
        ? new GraphQLError(`database error (id ${id})`, {
            extensions: { code: "DATABASE_ERROR", id },
          })
        : new GraphQLError(error.message, {
            extensions: { ...error.extensions, id },
            originalError: error.originalError ?? undefined,
          });
    }
    // Errors the package wrote itself are safe to show; masked, they
    // lose the engine error they were mapped from.
    return this.#maskErrors && error.originalError
      ? new GraphQLError(error.message, { extensions: error.extensions })
      : error;
  }

  #resolveRoot(
    kind: RootKind,
    node: NodeType,
    info: GraphQLResolveInfo,
    context: unknown,
  ): Promise<unknown> {
    const compiled = this.#cachedCompile(info, context, (ctx) =>
      compileRoot(
        ctx,
        kind,
        node,
        this.#args(ctx, info.parentType, info.fieldNodes),
        info.fieldNodes,
      ),
    );
    return this.#run(info.fieldName, compiled, context, info);
  }

  /**
   * Events for one node type, from the exact write-sets of mutations made
   * through this instance. Nodes the subscriber may not read are skipped;
   * deletions of rule-protected types are only sent to subscribers that
   * follow that key (they cannot be checked after the fact).
   */
  #subscribe(
    node: NodeType,
    args: Record<string, unknown>,
    context: unknown,
  ): AsyncIterableIterator<ChangeEvent> {
    const base: SelectionContext = {
      schema: this.getSchema(),
      fragments: {},
      variables: {},
    };
    // Checked now, so an unauthenticated subscribe fails as a result
    // rather than as a broken stream.
    const ctx = this.#context(base, context);
    checkAuthentication(ctx, node, "SUBSCRIBE");
    checkAuthentication(ctx, node, "READ");
    // Rules the claims alone decide against fail here too.
    for (const op of ["SUBSCRIBE", "READ"] as const) {
      authFilter(ctx, node, "n", op);
      authValidate(ctx, node, "n", op, "BEFORE");
    }
    // The stream's own signal: ended by `context.signal` or by return().
    // An async generator runs return() only once its pending next()
    // yields, and a quiet or filtered stream may never yield again, so
    // return() aborts the changes() underneath, which wakes that next().
    const stop = new AbortController();
    const outer = (context as LoraGraphQLContext | undefined)?.signal;
    const end = () => {
      outer?.removeEventListener("abort", end);
      stop.abort();
    };
    if (outer?.aborted) end();
    else outer?.addEventListener("abort", end, { once: true });
    const events = this.#events(node, args, context, base, stop.signal);
    const iterator: AsyncIterableIterator<ChangeEvent> = {
      [Symbol.asyncIterator]: () => iterator,
      next: async () => {
        const result = await events.next();
        if (result.done) end();
        return result;
      },
      return: (value?: unknown) => {
        end();
        return events.return(value);
      },
      throw: (error?: unknown) => {
        end();
        return events.throw(error);
      },
    };
    return iterator;
  }

  async *#events(
    node: NodeType,
    args: Record<string, unknown>,
    context: unknown,
    base: SelectionContext,
    signal: AbortSignal,
  ): AsyncGenerator<ChangeEvent> {
    const key = args[node.key.name];
    const where = args["where"] as Record<string, unknown> | null | undefined;
    const offered = new Set<string>(node.subscriptions);
    if (node.subscriptionOptions.relationships) {
      offered.add("CONNECT");
      offered.add("DISCONNECT");
    }
    const wanted = new Set(
      ((args["operations"] as string[] | null) ?? [...offered]).filter((op) =>
        offered.has(op),
      ),
    );
    const ruled = (r: { operations: ReadonlySet<string> }) =>
      r.operations.has("READ") || r.operations.has("SUBSCRIBE");
    const guarded =
      !!node.authorization?.filter.some(ruled) ||
      !!node.authorization?.validate.some(ruled);
    // Whether events must be checked in the database: read rules, or a
    // `where` (on the node as it is after the write).
    const check = guarded || (where != null && Object.keys(where).length > 0);
    const visible = check
      ? this.#visibleCheck(node, where, base, context)
      : undefined;
    // Only changes to this type (and this key, when one is followed)
    // arrive, with their events already built.
    const deliveries = this.#queue<SubscriberDelivery>(
      (sink) =>
        this.#addSubscriber(node, key == null ? undefined : keyOf(key), sink),
      { signal },
    );
    for await (const { change, events: all } of deliveries) {
      const events = all.filter((e) => wanted.has(e.operation));
      if (events.length === 0) continue;
      // A deleted node cannot be checked: its deletion reaches only
      // subscribers that follow its key, and only unfiltered ones.
      const deletions = events.filter(
        (e) => e.operation === "DELETE" && (!check || (key != null && !where)),
      );
      const live = events.filter((e) => e.operation !== "DELETE");
      const seen = visible
        ? await visible(
            live.map((e) => e.key),
            change,
          )
        : undefined;
      for (const event of [...live, ...deletions]) {
        if (
          event.operation !== "DELETE" &&
          seen &&
          !seen.has(keyOf(event.key))
        ) {
          continue;
        }
        // Events are built once per change and type: each subscriber gets
        // its own copy, since per-event cost is charged by root value.
        const own = { ...event };
        Object.defineProperty(own, CHANGE, { value: change });
        yield own;
      }
    }
  }

  /** Index a subscriber by type and key; returns its removal. */
  #addSubscriber(
    node: NodeType,
    key: string | undefined,
    sink: SubscriberSink,
  ): () => void {
    // One listener serves every subscriber, registered while any exist.
    if (!this.#undispatch) {
      const dispatch = (change: WriteChange) => this.#dispatch(change);
      if (this.#feed) {
        this.#feedListeners.add(dispatch);
        this.#undispatch = () => this.#feedListeners.delete(dispatch);
      } else {
        this.#undispatch = this.onWrite(dispatch);
      }
    }
    let group = this.#subscribers.get(node.name);
    if (!group) {
      group = { node, all: new Set(), byKey: new Map() };
      this.#subscribers.set(node.name, group);
    }
    const g = group;
    let sinks = g.all;
    if (key !== undefined) {
      sinks = g.byKey.get(key) ?? new Set();
      g.byKey.set(key, sinks);
    }
    sinks.add(sink);
    return () => {
      sinks.delete(sink);
      if (key !== undefined && sinks.size === 0) g.byKey.delete(key);
      if (
        g.all.size === 0 &&
        g.byKey.size === 0 &&
        this.#subscribers.get(node.name) === g
      ) {
        this.#subscribers.delete(node.name);
        if (this.#subscribers.size === 0) {
          this.#undispatch?.();
          this.#undispatch = undefined;
        }
      }
    };
  }

  /**
   * Hand one change to the subscribers of the types it touches: each
   * type's events are built once, and keyed subscribers get only the
   * events of their key.
   */
  #dispatch(change: WriteChange): void {
    if (this.#subscribers.size === 0) return;
    const types = new Set<string>();
    for (const refs of [change.deleted, change.created, change.entities]) {
      for (const e of refs) types.add(e.type);
    }
    for (const refs of [change.connected, change.disconnected]) {
      for (const r of refs) {
        types.add(r.from.type);
        types.add(r.to.type);
      }
    }
    const deliver = (sinks: Set<SubscriberSink>, events: ChangeEvent[]) => {
      for (const sink of sinks) {
        try {
          sink({ change, events });
        } catch {
          // One subscriber's failure must not stop the others.
        }
      }
    };
    for (const type of types) {
      const group = this.#subscribers.get(type);
      if (!group) continue;
      const events = changeEvents(change, group.node);
      if (events.length === 0) continue;
      if (group.all.size > 0) deliver(group.all, events);
      if (group.byKey.size === 0) continue;
      const byKey = new Map<string, ChangeEvent[]>();
      for (const e of events) {
        const id = keyOf(e.key);
        if (!group.byKey.has(id)) continue;
        const list = byKey.get(id);
        if (list) list.push(e);
        else byKey.set(id, [e]);
      }
      for (const [id, list] of byKey) {
        const sinks = group.byKey.get(id);
        if (sinks) deliver(sinks, list);
      }
    }
  }

  /**
   * Whether a subscription may see nodes: which of `keys` it may read
   * (and its `where` matches), in one query per change. The statement is
   * compiled once per subscription, again only when its claims or the
   * `$context` values it read change.
   */
  #visibleCheck(
    node: NodeType,
    where: Record<string, unknown> | null | undefined,
    base: SelectionContext,
    context: unknown,
  ): (keys: unknown[], change: object) => Promise<Set<string>> {
    let cached: SubscriptionCompile<Statement> | undefined;
    return async (keys, change) => {
      if (keys.length === 0) return new Set();
      const claims = stableKey(this.#jwt(context) ?? null);
      let entry = cached;
      if (
        !entry ||
        claims === undefined ||
        entry.claims !== claims ||
        !sameContextReads(entry.contextReads, context)
      ) {
        const ctx = this.#context(base, context);
        const slot: unknown[] = [];
        const text = printClauses([
          { kind: "unwind", expr: bind(ctx, slot), alias: "k" },
          {
            kind: "match",
            pattern: {
              start: { variable: "n", labels: [node.labels[0]!] },
              hops: [],
            },
            where: and(
              bin("=", prop(v("n"), node.key.property), v("k")),
              compileNodeWhere(ctx, node, "n", where),
              authFilter(ctx, node, "n", "READ"),
              authValidate(ctx, node, "n", "READ", "BEFORE"),
              authFilter(ctx, node, "n", "SUBSCRIBE"),
              authValidate(ctx, node, "n", "SUBSCRIBE", "BEFORE"),
            ),
          },
          {
            kind: "return",
            items: [{ expr: prop(v("n"), node.key.property), alias: "key" }],
          },
        ]);
        // The key list is bound directly, so the slot is always found.
        entry = slotCompile(
          { text, params: ctx.params },
          slot,
          claims,
          0,
          undefined,
          ctx.contextReads,
        )!;
        cached = claims === undefined ? undefined : entry;
      }
      const statements = [
        {
          text: entry.compiled.text,
          params: { ...entry.compiled.params, [entry.param]: keys },
        },
      ];
      const share =
        entry.share === undefined
          ? undefined
          : `${entry.share}\0${keys.map(keyOf).join("\0")}`;
      const [result] = await this.#shared(
        change,
        statements,
        () =>
          this.#driver.run(statements, {
            mode: "read",
            timeoutMs: this.#timeoutMs,
            verified: true,
          }),
        share,
      );
      return new Set(result!.rows.map((r) => keyOf(r["key"])));
    };
  }

  /**
   * A changed node for a subscriber. The read is compiled once per
   * subscription (context), field node, claims and variables, with the
   * key as its only varying parameter.
   */
  #resolveByKey(
    node: NodeType,
    key: unknown,
    info: GraphQLResolveInfo,
    context: unknown,
    change?: object,
  ): Promise<unknown> {
    const field = info.fieldNodes[0]!;
    const claims = stableKey(this.#jwt(context) ?? null);
    const variables = stableKey(coercedVariables(info.variableValues));
    const byField =
      context !== null && typeof context === "object"
        ? (this.#byKeyCompiles.get(context) ??
          this.#byKeyCompiles.set(context, new WeakMap()).get(context)!)
        : undefined;
    let entry = byField?.get(field);
    if (
      !entry ||
      claims === undefined ||
      variables === undefined ||
      entry.claims !== claims ||
      entry.variables !== variables ||
      entry.statistics !== this.#statisticsVersion ||
      !sameContextReads(entry.contextReads, context)
    ) {
      const ctx = this.#context(infoContext(info), context);
      const slot = {};
      const compiled = compileRoot(
        ctx,
        "single",
        node,
        { [node.key.name]: slot },
        info.fieldNodes,
      );
      entry =
        compiled.statements.length === 1
          ? slotCompile(
              compiled,
              slot,
              claims,
              this.#statisticsVersion,
              variables,
              ctx.contextReads,
              compiled.statements[0]!,
            )
          : undefined;
      if (!entry) {
        // Not reusable: compile again with the key itself.
        const direct = compileRoot(
          this.#context(infoContext(info), context),
          "single",
          node,
          { [node.key.name]: key },
          info.fieldNodes,
        );
        return this.#run(info.fieldName, direct, context, info, change);
      }
      if (claims !== undefined && variables !== undefined) {
        byField?.set(field, entry);
      }
    }
    const statement = entry.compiled.statements[0]!;
    const compiled: CompiledRead = {
      ...entry.compiled,
      statements: [
        {
          text: statement.text,
          params: { ...statement.params, [entry.param]: key },
        },
      ],
    };
    const share =
      entry.share === undefined ? undefined : `${entry.share}\0${keyOf(key)}`;
    return this.#run(info.fieldName, compiled, context, info, change, share);
  }

  #resolveSearch(
    node: NodeType,
    index: SearchIndex,
    info: GraphQLResolveInfo,
    context: unknown,
    connection = false,
  ): Promise<unknown> {
    const compiled = this.#cachedCompile(info, context, (ctx) =>
      compileSearch(
        ctx,
        node,
        index,
        this.#args(ctx, info.parentType, info.fieldNodes),
        info.fieldNodes,
        connection,
      ),
    );
    return this.#run(info.fieldName, compiled, context, info);
  }

  async #resolveNode(
    id: string,
    info: GraphQLResolveInfo,
    context: unknown,
  ): Promise<unknown> {
    const decoded = fromGlobalId(id);
    const node = decoded && this.model.nodes.get(decoded.type);
    if (!decoded || !node || !node.key.relayId || !node.read) return null;
    let key: unknown = decoded.key;
    if (node.key.type === "BigInt" && typeof key === "string") {
      const n = BigInt(key);
      key = Number.isSafeInteger(Number(n)) ? Number(n) : n;
    }
    const ctx = this.#context(infoContext(info), context);
    const compiled = compileRoot(
      ctx,
      "single",
      node,
      { [node.key.name]: key },
      info.fieldNodes,
    );
    const value = (await this.#run(
      info.fieldName,
      compiled,
      context,
    )) as Record<string, unknown> | null;
    return value && { ...value, __typename: node.name };
  }

  async #resolveCypher(
    field: CypherField,
    info: GraphQLResolveInfo,
    context: unknown,
  ): Promise<unknown> {
    const ctx = this.#context(infoContext(info), context);
    // A root query reads; a root mutation's operations are its own, so
    // any listed operation guards it.
    const op =
      field.owner === "Mutation"
        ? [...(field.authentication ?? [])][0]
        : ("READ" as const);
    if (op) checkFieldAuthentication(ctx, field.owner, field, op);
    // @authorization: the claims decide here; a `viewer` test is left for
    // the database, before the statement runs.
    const rule = rootFieldGuard(ctx, field);
    const guard: Statement | undefined = rule && {
      text: `RETURN coalesce(${printExpr(rule)}, false) AS allowed`,
      params: ctx.params,
    };
    const args = this.#args(ctx, info.parentType, info.fieldNodes);
    if (field.owner !== "Mutation") {
      if (guard) await this.#checkGuard(field, guard, context);
      const compiled = this.#cachedCompile(info, context, (c) =>
        compileCypherRoot(
          c,
          field,
          this.#args(c, info.parentType, info.fieldNodes),
          info.fieldNodes,
        ),
      );
      return this.#run(info.fieldName, compiled, context, info);
    }
    let value: unknown;
    try {
      value = await executeCypherMutation(
        this.#mutationEnv(info, context, ctx.jwt),
        field,
        args,
        info.fieldNodes,
        guard,
      );
    } catch (err) {
      throw this.#databaseError(info.fieldName, err);
    }
    // A hand-written write has no known write-set: report it broadly.
    this.#emit({
      operation: "CYPHER",
      field: field.name,
      created: [],
      updated: [],
      deleted: [],
      connected: [],
      disconnected: [],
      entities: [],
      types: [],
      relationshipTypes: [],
      broad: true,
    });
    return value;
  }

  /** Run a root @cypher field's `viewer` guard; FORBIDDEN when it fails. */
  async #checkGuard(
    field: CypherField,
    guard: Statement,
    context: unknown,
  ): Promise<void> {
    this.#onStatement?.({ field: field.name, statement: guard });
    const owned = (context as LoraGraphQLContext | undefined)?.transaction;
    let result: QueryResult | undefined;
    try {
      [result] = owned
        ? await runInOrder(owned, [guard])
        : await this.#driver.run([guard], {
            mode: "read",
            timeoutMs: this.#timeoutMs,
            signal: (context as LoraGraphQLContext | undefined)?.signal,
          });
    } catch (err) {
      throw this.#databaseError(field.name, err);
    }
    if (result?.rows[0]?.["allowed"] !== true) {
      throw requestError(
        "FORBIDDEN",
        `not allowed to run ${field.owner}.${field.name}`,
      );
    }
  }

  async #resolveMutation(
    op: MutationKind,
    node: NodeType,
    info: GraphQLResolveInfo,
    context: unknown,
  ): Promise<unknown> {
    const ctx = this.#context(infoContext(info), context);
    const args = this.#args(ctx, info.parentType, info.fieldNodes);
    try {
      const { payload, change } = await executeMutation(
        this.#mutationEnv(info, context, ctx.jwt),
        op,
        node,
        args,
        info.fieldNodes,
        info.fieldName,
      );
      const owned = (context as LoraGraphQLContext | undefined)?.transaction;
      if (owned) owned.record(change);
      else this.#emit(change);
      return payload;
    } catch (err) {
      throw this.#databaseError(info.fieldName, err);
    }
  }

  /**
   * How a mutation's statements are observed: traced and reported when
   * observability is on, timed when the request collects timings.
   */
  #mutationObserve(
    info: GraphQLResolveInfo,
    context: unknown,
  ): MutationEnv["observe"] {
    const timing = this.#timingOf(context);
    if (!this.#observer.active && !timing) return undefined;
    return async (statement, run) => {
      const start = performance.now();
      try {
        return await (this.#observer.active
          ? this.#observer.statements(
              this.#meta(info.fieldName, "write", info),
              [statement],
              run,
            )
          : run());
      } finally {
        if (timing) {
          timingEntry(timing, rootKey(info) ?? info.fieldName).databaseMs +=
            performance.now() - start;
        }
      }
    };
  }

  /** The request's timing collector, when `timing` asked for one. */
  #timingOf(
    context: unknown,
  ): Map<string, { totalMs: number; databaseMs: number }> | undefined {
    return context !== null && typeof context === "object"
      ? this.#timings.get(context)
      : undefined;
  }

  #mutationEnv(
    info: GraphQLResolveInfo,
    context: unknown,
    jwt: Record<string, unknown> | undefined,
  ): MutationEnv {
    return {
      model: this.model,
      driver: this.#driver,
      selection: infoContext(info),
      jwt,
      timeoutMs: this.#timeoutMs,
      signal: (context as LoraGraphQLContext | undefined)?.signal,
      degrees: this.#degrees,
      maxBatch: this.#maxBatch,
      maxListArgument: this.#maxListArgument,
      maxFilterDepth: this.#maxFilterDepth,
      maxListFilter: this.#maxListFilter,
      maxStringFilter: this.#maxStringFilter,
      statistics: this.#statistics,
      requestContext: context,
      callbacks: this.#callbacks,
      transaction: (context as LoraGraphQLContext | undefined)?.transaction
        ?.driverTransaction,
      onStatement: (statement) =>
        this.#onStatement?.({ field: info.fieldName, statement }),
      observe: this.#mutationObserve(info, context),
    };
  }

  /**
   * Open a transaction the caller owns. Put it in the GraphQL context as
   * `transaction`: every operation of those requests runs in it, next to
   * the application's own `tx.execute(cypher)`. Nothing is visible to
   * others, and no change event fires, until `tx.commit()`.
   */
  async begin(): Promise<LoraTransaction> {
    if (!this.#driver.begin) {
      throw new Error("transactions need @loradb/lora-node");
    }
    const tx = await this.#driver.begin({
      mode: "write",
      timeoutMs: this.#timeoutMs,
    });
    return new LoraTransaction(tx, (change) => this.#emit(change));
  }

  /** Stop the engine change feed (with `changeFeed: true`). */
  close(): void {
    this.#feed?.stop();
    this.#feedReady = undefined;
  }

  #emit(change: WriteChange): void {
    change.timestamp ??= new Date().toISOString();
    for (const listener of this.#listeners) {
      try {
        listener(change);
      } catch {
        // A listener's failure must not fail a committed write.
      }
    }
  }
}

function isDocument(
  value: DocumentNode | ExecutionResult,
): value is DocumentNode {
  return (value as DocumentNode).kind === Kind.DOCUMENT;
}

/**
 * The type of the operation `operationName` selects; undefined when it
 * selects none, which graphql then reports.
 */
/** A variable graphql-js could not coerce: a request error with no code. */
function isVariableError(e: GraphQLError): boolean {
  return (
    e.extensions?.["code"] === undefined &&
    e.path === undefined &&
    e.message.startsWith('Variable "$')
  );
}

/** A request's timing entry for a root field, created on first use. */
function timingEntry(
  timing: Map<string, { totalMs: number; databaseMs: number }>,
  key: string,
): { totalMs: number; databaseMs: number } {
  let entry = timing.get(key);
  if (!entry) {
    entry = { totalMs: 0, databaseMs: 0 };
    timing.set(key, entry);
  }
  return entry;
}

/** The response key of the root field `info` belongs to. */
function rootKey(info: GraphQLResolveInfo | undefined): string | undefined {
  let path = info?.path;
  if (!path) return undefined;
  while (path.prev) path = path.prev;
  return String(path.key);
}

/** Milliseconds, to a hundredth. */
const ms = (value: number) => Math.round(value * 100) / 100;

/** An index name as Cypher takes it: backquoted. */
function quoteName(name: string): string {
  return "`" + name.replaceAll("`", "``") + "`";
}

function operationType(
  document: DocumentNode,
  operationName: string | null | undefined,
): "query" | "mutation" | "subscription" | undefined {
  return getOperationAST(document, operationName)?.operation as
    | "query"
    | "mutation"
    | "subscription"
    | undefined;
}

/** The write an event came from, for reads shared across its subscribers. */
const CHANGE = Symbol("change");

/** A write's events for one node type, one per node, most specific first. */
function changeEvents(change: WriteChange, node: NodeType): ChangeEvent[] {
  const out = new Map<string, ChangeEvent>();
  const previous = new Map<string, Record<string, unknown>>();
  for (const b of change.before ?? []) {
    if (b.type === node.name) previous.set(keyOf(b.key), b.properties);
  }
  const make = (
    operation: ChangeEvent["operation"],
    key: unknown,
    extra: Partial<ChangeEvent> = {},
  ): ChangeEvent => {
    const event: ChangeEvent = {
      operation,
      key,
      timestamp: change.timestamp,
      ...extra,
    };
    if (operation === "UPDATE" || operation === "DELETE") {
      event.previous = previous.get(keyOf(key));
    }
    Object.defineProperty(event, CHANGE, { value: change });
    return event;
  };
  const add = (operation: ChangeEvent["operation"], key: unknown) => {
    const id = keyOf(key);
    if (!out.has(id)) out.set(id, make(operation, key));
  };
  for (const e of change.deleted)
    if (e.type === node.name) add("DELETE", e.key);
  for (const e of change.created)
    if (e.type === node.name) add("CREATE", e.key);
  // Updated nodes, and nodes that gained or lost a relationship.
  for (const e of change.entities)
    if (e.type === node.name) add("UPDATE", e.key);
  const events = [...out.values()];
  if (node.subscriptionOptions.relationships) {
    for (const [operation, refs] of [
      ["CONNECT", change.connected],
      ["DISCONNECT", change.disconnected],
    ] as const) {
      for (const r of refs) {
        for (const [self, other] of [
          [r.from, r.to],
          [r.to, r.from],
        ] as const) {
          if (self.type !== node.name) continue;
          events.push(
            make(operation, self.key, {
              relationship: {
                field: r.field,
                type: r.type,
                relatedType: other.type,
                relatedKey: other.key,
              },
            }),
          );
        }
      }
    }
  }
  return events;
}

async function runInOrder(
  tx: LoraTransaction,
  statements: Statement[],
): Promise<QueryResult[]> {
  const out: QueryResult[] = [];
  for (const s of statements) out.push(await tx.driverTransaction.execute(s));
  return out;
}

function infoContext(info: GraphQLResolveInfo): SelectionContext {
  return {
    schema: info.schema,
    fragments: info.fragments,
    variables: info.variableValues,
  };
}

function findOperation(
  doc: DocumentNode,
  operationName: string | undefined,
): OperationDefinitionNode {
  const ops = doc.definitions.filter(
    (d): d is OperationDefinitionNode => d.kind === Kind.OPERATION_DEFINITION,
  );
  const op = operationName
    ? ops.find((o) => o.name?.value === operationName)
    : ops.length === 1
      ? ops[0]
      : undefined;
  if (!op) {
    throw new GraphQLError(
      operationName
        ? `unknown operation ${operationName}`
        : "the document must contain exactly one operation, or name one",
    );
  }
  return op;
}

/**
 * Every `@customResolver(requires:)` is a valid selection on its type and
 * does not require custom fields (they are computed after the read).
 */
function checkCustomRequires(model: GraphModel, schema: GraphQLSchema): void {
  const problems: Array<{ type: string; field: string; message: string }> = [];
  for (const node of model.nodes.values()) {
    for (const f of node.fields.values()) {
      if (f.kind !== "custom" || !f.requires) continue;
      let doc: DocumentNode;
      try {
        doc = parse(`fragment R on ${node.name} { ${f.requires} }`);
      } catch (err) {
        problems.push({
          type: node.name,
          field: f.name,
          message: `requires: ${(err as Error).message}`,
        });
        continue;
      }
      const errors = validate(
        schema,
        doc,
        specifiedRules.filter((r) => r.name !== "NoUnusedFragmentsRule"),
      );
      for (const e of errors) {
        problems.push({
          type: node.name,
          field: f.name,
          message: `requires: ${e.message}`,
        });
      }
      for (const name of requiredCustomFields(node, f.requires)) {
        problems.push({
          type: node.name,
          field: f.name,
          message: `requires: ${name} is a @customResolver field`,
        });
      }
    }
  }
  if (problems.length > 0) throw new ModelError(problems);
}

function requiredCustomFields(node: NodeType, requires: string): string[] {
  const doc = parse(`{ ${requires} }`);
  const op = doc.definitions[0] as OperationDefinitionNode;
  return op.selectionSet.selections.flatMap((s) =>
    s.kind === Kind.FIELD && node.fields.get(s.name.value)?.kind === "custom"
      ? [s.name.value]
      : [],
  );
}

/** A cached compile still holds for these `$context` values. */
function sameContextReads(
  reads: Array<[string, unknown]>,
  context: unknown,
): boolean {
  return reads.every(
    ([path, value]) =>
      stableKey(lookupPath(context, path)) === stableKey(value),
  );
}

/**
 * A compile made with `slot` as one parameter's value, reusable with that
 * parameter filled in per run; undefined unless exactly one parameter
 * holds the slot.
 */
function slotCompile<C>(
  compiled: C,
  slot: unknown,
  claims: string | undefined,
  statistics: number,
  variables: string | undefined,
  contextReads: Array<[string, unknown]>,
  statement: Statement = compiled as Statement,
): SubscriptionCompile<C> | undefined {
  const slots = Object.keys(statement.params).filter(
    (name) => statement.params[name] === slot,
  );
  if (slots.length !== 1) return undefined;
  const param = slots[0]!;
  const rest = { ...statement.params };
  delete rest[param];
  const fixed = stableKey({ text: statement.text, params: rest });
  return {
    claims: claims ?? "",
    statistics,
    variables,
    contextReads,
    compiled,
    param,
    share: fixed === undefined ? undefined : `s:${param}\0${fixed}`,
  };
}

function sameTarget(
  row: Record<string, unknown>,
  label: string,
  property: string,
): boolean {
  const labels = row["labelsOrTypes"] as string[] | null;
  const props = row["properties"] as string[] | null;
  return (
    row["entityType"] === "NODE" &&
    labels?.length === 1 &&
    labels[0] === label &&
    props?.length === 1 &&
    props[0] === property
  );
}
