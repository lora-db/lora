import {
  execute as graphqlExecute,
  getVariableValues,
  GraphQLError,
  Kind,
  lexicographicSortSchema,
  parse,
  printSchema,
  specifiedRules,
  validate,
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
import { lintModel } from "./analyze/lint.js";
import { analyze, type Statistics } from "./analyze/statistics.js";
import { newContext, type CompileContext } from "./compile/context.js";
import {
  compileAbstractRoot,
  compileCypherRoot,
  compileRoot,
  compileSearch,
  type CompiledRead,
  type ReadSet,
  type RootKind,
} from "./compile/read.js";
import { fieldArgs, type SelectionContext } from "./compile/selection.js";
import type { LoraDriver, QueryResult, Statement } from "./driver.js";
import { affects, type WriteChange } from "./execute/changes.js";
import { executeCypherMutation } from "./execute/cypher-mutation.js";
import { LoraTransaction } from "./execute/transaction.js";
import { EngineFeed } from "./execute/feed.js";
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
} from "./compile/auth.js";
import { and, bin, printClauses, prop, v } from "./compile/cypher.js";
import { bind } from "./compile/context.js";
import { keyOf } from "./compile/read.js";
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
   * Reject a root field whose estimated rows touched exceed this, before
   * it runs. The estimate multiplies page sizes through nested lists,
   * capped by relationship degrees from `analyze()` or `@cardinality`.
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
   * Limits on the documents `execute()` and `persist()` accept: depth,
   * aliases, root fields, tokens, introspection. `false` turns them off.
   * For other servers use `validationRules()` or `envelopPlugin()`.
   */
  guards?: DocumentGuards | false;
  /**
   * Make `execute()` refuse `source` and run only persisted operations
   * by `id`. Default false.
   */
  persistedOnly?: boolean;
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
  /** The root field that failed. */
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
}

export interface CheckOptions {
  /**
   * Flag statements whose largest engine row estimate (from graph
   * statistics) exceeds this. Off by default: estimates ignore a LIMIT
   * that stops an index-ordered scan early.
   */
  rowBudget?: number;
  /** Operations to compile and plan-check (S2), with example variables. */
  operations?: Array<{
    name?: string;
    document: string | DocumentNode;
    variables?: Record<string, unknown>;
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
/** Compiled root fields kept per field node, across variable values. */
const COMPILED_PER_FIELD = 16;

interface CompiledEntry {
  variables: string;
  claims: string;
  statistics: number;
  contextReads: Array<[string, unknown]>;
  compiled: CompiledRead;
}

export class LoraGraphQL {
  readonly model: GraphModel;
  readonly #driver: LoraDriver;
  readonly #timeoutMs: number;
  readonly #maxCost: number;
  readonly #maxBatch: number;
  readonly #maxQueued: number;
  readonly #callbacks: Record<string, PopulatedByCallback>;
  readonly #resolvers: NonNullable<LoraGraphQLOptions["resolvers"]>;
  readonly #scalars: LoraGraphQLOptions["scalars"];
  /** Estimated cost spent per request context and operation. */
  readonly #spent = new WeakMap<object, Map<unknown, number>>();
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
  readonly #feed: EngineFeed | undefined;
  #feedReady: Promise<void> | undefined;
  readonly #documents = new Map<string, DocumentNode>();
  readonly #persisted = new Map<string, DocumentNode>();
  #degrees = new Map<string, number>();
  /** Bumped when statistics change: they shape cost estimates. */
  #statisticsVersion = 0;
  /**
   * Compiled reads by root field node (documents are cached, so a field
   * node repeats across requests), then by variables, claims and the
   * `$context` values the compile read.
   */
  readonly #compiled = new WeakMap<FieldNode, CompiledEntry[]>();
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

  constructor(options: LoraGraphQLOptions) {
    this.model = buildModel(options.typeDefs, options);
    this.#driver = options.driver;
    this.#timeoutMs = options.timeoutMs ?? 10_000;
    this.#maxCost = options.maxCost ?? 50_000;
    this.#maxBatch = options.maxBatch ?? 1000;
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
      this.#feed = new EngineFeed(options.driver, this.model, (change) => {
        change.timestamp ??= new Date().toISOString();
        for (const l of this.#feedListeners) {
          try {
            l(change);
          } catch {
            // A consumer's failure must not stop the feed.
          }
        }
      });
    }
    this.#observer = new Observer(options);
    this.#budget = options.budget;
    this.#onCost = options.onCost;
    this.#persistedOnly = options.persistedOnly ?? false;
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
    const span = <T>(info: GraphQLResolveInfo, fn: () => Promise<T>) =>
      this.#observer.field(
        {
          field: info.fieldName,
          operationName: info.operation.name?.value,
        },
        fn,
      );
    if (this.#schema) return this.#schema;
    const schema = buildSchema(this.model, {
      scalars: this.#scalars,
      customResolver: (node, field) => this.#resolvers[node.name]![field.name]!,
      subscribe: (node, args, context) => this.#subscribe(node, args, context),
      resolveChangedNode: (node, event, info, context) =>
        event.operation === "DELETE"
          ? Promise.resolve(null)
          : span(info, () =>
              this.#resolveByKey(
                node,
                event.key,
                info,
                context,
                (event as { [CHANGE]?: WriteChange })[CHANGE],
              ),
            ),
      resolveAbstract: (abstract, info, context) =>
        span(info, () => {
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
        span(info, () =>
          this.#resolveSearch(node, index, info, context, connection),
        ),
      resolveRoot: (kind, node, info, context) =>
        span(info, () => this.#resolveRoot(kind, node, info, context)),
      resolveNode: (id, info, context) =>
        span(info, () => this.#resolveNode(id, info, context)),
      resolveCypher: (field, info, context) =>
        span(info, () => this.#resolveCypher(field, info, context)),
      resolveMutation: (op, node, info, context) =>
        span(info, () => this.#resolveMutation(op, node, info, context)),
    });
    checkCustomRequires(this.model, schema);
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
    const present = (r: SchemaRequirement) => {
      if (r.kind === "fulltext" || r.kind === "vector") {
        const type = r.kind === "fulltext" ? "FULLTEXT" : "VECTOR";
        return indexes!.rows.some(
          (row) => row["name"] === r.name && row["type"] === type,
        );
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
    const created: SchemaRequirement[] = [];
    if (options.create) {
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
    };
  }

  // -------------------------------------------------------------------------
  // Analysis
  // -------------------------------------------------------------------------

  /**
   * Sample node counts and relationship degrees (S6). Cost estimates then
   * use each relationship's p99 degree instead of its page limit.
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
      Object.entries(stats.degrees).map(([k, d]) => [k, d.p99]),
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
  async check(options: CheckOptions = {}): Promise<CheckReport> {
    const report: CheckReport = {
      ok: true,
      warnings: this.model.warnings,
      cypher: await checkCypherFields(this.#driver, this.model),
      missing: (await this.assertSchema()).missing,
      unused: await this.#unusedIndexes(),
      lint: lintModel(this.model, { statistics: !!this.#statistics }),
      plans: [],
      errors: [],
    };
    for (const [i, op] of (options.operations ?? []).entries()) {
      const operation = op.name ?? `operation ${i + 1}`;
      try {
        const fields = await this.explain(op.document, op.variables ?? {}, {
          rowBudget: options.rowBudget,
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
      variables: coerced.coerced,
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
   * Execute an operation against the schema. Parsed and validated
   * documents are cached by source text; persisted ones by id.
   */
  async execute(args: ExecuteArgs): Promise<ExecutionResult> {
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
        if (this.#documents.size >= DOCUMENT_CACHE_SIZE) {
          this.#documents.delete(this.#documents.keys().next().value!);
        }
        this.#documents.set(args.source, document);
      }
    } else {
      return {
        errors: [new GraphQLError("execute() needs a source or an id")],
      };
    }
    const contextValue = args.context ?? {};
    const result = await graphqlExecute({
      schema: this.getSchema(),
      document,
      variableValues: args.variables,
      operationName: args.operationName,
      contextValue,
    });
    // The operation's cost estimate, so clients can tune their queries.
    const spent =
      contextValue !== null && typeof contextValue === "object"
        ? this.#spent.get(contextValue)
        : undefined;
    if (spent && spent.size > 0) {
      const cost = [...spent.values()].reduce((a, b) => a + b, 0);
      return {
        ...result,
        extensions: { ...result.extensions, cost: Math.ceil(cost) },
      };
    }
    return result;
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
   * resolver. Only writes made through this instance are seen.
   */
  changes(
    options: { signal?: AbortSignal; maxQueued?: number } = {},
  ): AsyncIterableIterator<WriteChange> {
    const limit = options.maxQueued ?? this.#maxQueued;
    const queue: WriteChange[] = [];
    let wake: (() => void) | undefined;
    let done = false;
    let overflow = false;
    const listener = (change: WriteChange) => {
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
    let stop: () => void;
    let ready: Promise<void> | undefined;
    if (this.#feed) {
      this.#feedListeners.add(listener);
      stop = () => this.#feedListeners.delete(listener);
      this.#feedReady ??= this.#feed.start();
      ready = this.#feedReady;
    } else {
      stop = this.onWrite(listener);
    }
    const finish = () => {
      done = true;
      stop();
      wake?.();
    };
    options.signal?.addEventListener("abort", finish, { once: true });
    const iterator: AsyncIterableIterator<WriteChange> = {
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

  #context(base: SelectionContext, context: unknown): CompileContext {
    return newContext(base, this.model, {
      jwt: this.#jwt(context),
      degrees: this.#degrees,
      requestContext: context,
    });
  }

  /**
   * Charge `cost` to the operation: the limit holds per operation, so
   * aliasing a root field many times does not multiply it.
   */
  #charge(
    field: string,
    cost: number,
    context: unknown,
    info?: GraphQLResolveInfo,
  ) {
    let total = cost;
    if (info && context !== null && typeof context === "object") {
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
  ): Promise<QueryResult[]> {
    const key = change ? stableKey(statements) : undefined;
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
  ): Promise<unknown> {
    this.#charge(field, compiled.cost, context, info);
    for (const statement of compiled.statements) {
      this.#onStatement?.({ field, statement });
    }
    const signal = (context as LoraGraphQLContext | undefined)?.signal;
    const owned = (context as LoraGraphQLContext | undefined)?.transaction;
    let results;
    try {
      results = await this.#observer.statements(
        this.#meta(field, compiled.mode, info, compiled.cost),
        compiled.statements,
        () =>
          owned
            ? runInOrder(owned, compiled.statements)
            : this.#shared(change, compiled.statements, () =>
                this.#driver.run(compiled.statements, {
                  mode: compiled.mode,
                  timeoutMs: this.#timeoutMs,
                  signal,
                  // Queries and object @cypher fields are checked read-only
                  // when the model is built; writes never reach this path.
                  verified: compiled.mode === "read",
                }),
              ),
      );
    } catch (err) {
      throw this.#databaseError(field, err);
    }
    return assertReadable(compiled.shape(results));
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
   * with the same field node, variables, claims and `$context` values.
   * Statement text depends on all of them (claims are folded in).
   */
  #cachedCompile(
    info: GraphQLResolveInfo,
    context: unknown,
    compile: (ctx: CompileContext) => CompiledRead,
  ): CompiledRead {
    const field = info.fieldNodes[0]!;
    const variables = stableKey(info.variableValues);
    const claims = stableKey(this.#jwt(context) ?? null);
    const ctx = this.#context(infoContext(info), context);
    if (variables === undefined || claims === undefined) return compile(ctx);
    const entries = this.#compiled.get(field) ?? [];
    const hit = entries.find(
      (e) =>
        e.variables === variables &&
        e.claims === claims &&
        e.statistics === this.#statisticsVersion &&
        e.contextReads.every(
          ([path, value]) =>
            stableKey(lookupPath(context, path)) === stableKey(value),
        ),
    );
    if (hit) return hit.compiled;
    const compiled = compile(ctx);
    entries.unshift({
      variables,
      claims,
      statistics: this.#statisticsVersion,
      contextReads: ctx.contextReads,
      compiled,
    });
    entries.length = Math.min(entries.length, COMPILED_PER_FIELD);
    this.#compiled.set(field, entries);
    return compiled;
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
  ): AsyncGenerator<ChangeEvent> {
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
    return this.#events(node, args, context, base);
  }

  async *#events(
    node: NodeType,
    args: Record<string, unknown>,
    context: unknown,
    base: SelectionContext,
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
    const signal = (context as LoraGraphQLContext | undefined)?.signal;
    for await (const change of this.changes(signal ? { signal } : {})) {
      const events = changeEvents(change, node).filter(
        (e) =>
          wanted.has(e.operation) &&
          (key == null || keyOf(e.key) === keyOf(key)),
      );
      if (events.length === 0) continue;
      // A deleted node cannot be checked: its deletion reaches only
      // subscribers that follow its key, and only unfiltered ones.
      const deletions = events.filter(
        (e) => e.operation === "DELETE" && (!check || (key != null && !where)),
      );
      const live = events.filter((e) => e.operation !== "DELETE");
      const visible = check
        ? await this.#visible(
            node,
            live.map((e) => e.key),
            where,
            base,
            context,
            change,
          )
        : undefined;
      for (const event of [...live, ...deletions]) {
        if (
          event.operation !== "DELETE" &&
          visible &&
          !visible.has(keyOf(event.key))
        ) {
          continue;
        }
        yield event;
      }
    }
  }

  /** Which of `keys` the subscriber may read (and `where` matches), in one query. */
  async #visible(
    node: NodeType,
    keys: unknown[],
    where: Record<string, unknown> | null | undefined,
    base: SelectionContext,
    context: unknown,
    change?: object,
  ): Promise<Set<string>> {
    if (keys.length === 0) return new Set();
    const ctx = this.#context(base, context);
    const text = printClauses([
      { kind: "unwind", expr: bind(ctx, keys), alias: "k" },
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
    const statements = [{ text, params: ctx.params }];
    const [result] = await this.#shared(change, statements, () =>
      this.#driver.run(statements, {
        mode: "read",
        timeoutMs: this.#timeoutMs,
        verified: true,
      }),
    );
    return new Set(result!.rows.map((r) => keyOf(r["key"])));
  }

  #resolveByKey(
    node: NodeType,
    key: unknown,
    info: GraphQLResolveInfo,
    context: unknown,
    change?: object,
  ): Promise<unknown> {
    const ctx = this.#context(infoContext(info), context);
    const compiled = compileRoot(
      ctx,
      "single",
      node,
      { [node.key.name]: key },
      info.fieldNodes,
    );
    return this.#run(info.fieldName, compiled, context, info, change);
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
    const args = this.#args(ctx, info.parentType, info.fieldNodes);
    if (field.owner !== "Mutation") {
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
      requestContext: context,
      callbacks: this.#callbacks,
      transaction: (context as LoraGraphQLContext | undefined)?.transaction
        ?.driverTransaction,
      onStatement: (statement) =>
        this.#onStatement?.({ field: info.fieldName, statement }),
      observe: this.#observer.active
        ? (statement, run) =>
            this.#observer.statements(
              this.#meta(info.fieldName, "write", info),
              [statement],
              run,
            )
        : undefined,
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

/**
 * A deterministic key for JSON-like values (object keys sorted, bigints
 * tagged); undefined when the value cannot be keyed (functions, cycles,
 * class instances), which turns caching off for that request.
 */
function stableKey(value: unknown): string | undefined {
  const seen = new Set<unknown>();
  let ok = true;
  const walk = (v: unknown): unknown => {
    if (typeof v === "bigint") return { $bigint: v.toString() };
    if (typeof v === "function" || typeof v === "symbol") {
      ok = false;
      return null;
    }
    if (v === null || typeof v !== "object") return v;
    if (seen.has(v)) {
      ok = false;
      return null;
    }
    seen.add(v);
    if (Array.isArray(v)) return v.map(walk);
    const proto = Object.getPrototypeOf(v);
    if (proto !== Object.prototype && proto !== null) {
      ok = false;
      return null;
    }
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(v).sort()) {
      out[k] = walk((v as Record<string, unknown>)[k]);
    }
    return out;
  };
  const json = JSON.stringify(walk(value));
  return ok ? (json ?? "undefined") : undefined;
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
