// The `LoraGraphQL` class: one annotated SDL document as a GraphQL API on
// LoraDB. It builds the model and the schema, compiles and runs root
// fields, and owns the pieces that do the rest: documents
// (`runtime/documents.ts`), cost and deadlines (`runtime/limits.ts`),
// committed writes and subscriptions (`subscriptions/`).

import {
  execute as graphqlExecute,
  type ExecutionArgs,
  subscribe as graphqlSubscribe,
  getOperationAST,
  getVariableValues,
  GraphQLError,
  Kind,
  lexicographicSortSchema,
  parse,
  printSchema,
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
  type ValidationRule,
} from "graphql";
import {
  assertSchema,
  unusedIndexes,
  type SchemaEnv,
} from "./analyze/assert-schema.js";
import { checkCypherFields } from "./analyze/cypher-check.js";
import {
  inferRequirements,
  type SchemaRequirement,
} from "./analyze/indexes.js";
import { checkPlans, type PlanReport } from "./analyze/plans.js";
import { lintModel, unguardedMutations } from "./analyze/lint.js";
import { accessLints } from "./analyze/access-lint.js";
import {
  accessMatrix,
  operationAccess,
  type OperationAccess,
} from "./analyze/access.js";
import { type AccessEntry } from "./analyze/verdicts.js";
import { analyze, type Statistics } from "./analyze/statistics.js";
import { newContext, type CompileContext } from "./compile/context.js";
import {
  COMPILE_CACHE_BYTES,
  COMPILED_TOTAL,
  CompileCache,
} from "./compile/cache.js";
import { compileAbstractRoot } from "./compile/read/abstract.js";
import {
  compileCypherRoot,
  MAX_LIST_ARGUMENT,
} from "./compile/read/cypher-field.js";
import { compileRoot } from "./compile/read/root.js";
import { compileSearch } from "./compile/read/search.js";
import {
  type CompiledRead,
  type ReadSet,
  type RootKind,
} from "./compile/read/types.js";
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
import {
  anySignal,
  MAX_CONCURRENT_STATEMENTS,
  OPERATION_TIMEOUT_FACTOR,
} from "./execute/budget.js";
import type { MutationKind } from "./schema/mutations.js";
import { executeMutation } from "./execute/mutate.js";
import {
  type MutationEnv,
  type PopulatedByCallback,
} from "./execute/mutate/env.js";
import { collapseErrors, ModelError, requestError } from "./errors.js";
import {
  buildManifest,
  generateTypes,
  schemaHash,
  type OperationManifest,
} from "./codegen.js";
import {
  envelopPlugin,
  nodeEnv,
  validationRules,
  type DocumentGuards,
} from "./guards.js";
import { buildModel } from "./model/build.js";
import { Observer, type StatementMeta } from "./observe.js";
import type {
  CypherField,
  GraphModel,
  NodeType,
  SearchIndex,
} from "./model/types.js";
import type {
  AssertSchemaOptions,
  CheckOptions,
  ExecuteArgs,
  LoraGraphQLContext,
  LoraGraphQLOptions,
} from "./options.js";
import type {
  CheckReport,
  ExecutionTiming,
  LoraExecutionResult,
  SchemaAssertion,
} from "./results.js";
import { checkCustomRequires } from "./runtime/custom-requires.js";
import { databaseError } from "./runtime/database-error.js";
import { Documents } from "./runtime/documents.js";
import { OperationLimits } from "./runtime/limits.js";
import {
  findOperation,
  infoContext,
  isDocument,
  isVariableError,
  multiRoot,
  operationType,
  rootOf,
  runInOrder,
  searchOf,
} from "./runtime/request.js";
import { ms, rootKey, timingEntry } from "./runtime/timing.js";
import { buildSchema } from "./schema/build.js";
import {
  checkFieldAuthentication,
  claimsMaskedValue,
  rootFieldGuard,
} from "./compile/auth.js";
import { printExpr } from "./compile/cypher.js";
import { MAX_FILTER_DEPTH } from "./compile/cost.js";
import { fromGlobalId } from "./schema/global-id.js";
import { assertReadable } from "./schema/guard.js";
import { ChangeHub } from "./subscriptions/change-hub.js";
import { CHANGE } from "./subscriptions/events.js";
import { Subscriptions } from "./subscriptions/subscriptions.js";

/** Read-sets being collected, by GraphQL context (see `ExecuteArgs.readSet`). */
interface ReadCollector {
  labels: Set<string>;
  relationships: Set<string>;
  opaque: boolean;
  /** Concurrent `execute()` calls sharing the context. */
  active: number;
}

export class LoraGraphQL {
  readonly model: GraphModel;
  readonly #driver: LoraDriver;
  readonly #timeoutMs: number;
  /** Cost charged and deadlines held, per operation. */
  readonly #limits: OperationLimits;
  readonly #maxBatch: number;
  readonly #maxListArgument: number;
  readonly #maxFilterDepth: number;
  readonly #maxListFilter: number | undefined;
  readonly #maxStringFilter: number | undefined;
  readonly #callbacks: Record<string, PopulatedByCallback>;
  readonly #resolvers: NonNullable<LoraGraphQLOptions["resolvers"]>;
  readonly #scalars: LoraGraphQLOptions["scalars"];
  readonly #jwt: (context: unknown) => Record<string, unknown> | undefined;
  readonly #onStatement: LoraGraphQLOptions["onStatement"];
  readonly #maskErrors: boolean;
  readonly #observer: Observer;
  readonly #guards: DocumentGuards | false;
  readonly #onError: LoraGraphQLOptions["onError"];
  /** Committed writes: `onWrite`, `changes()` and the engine feed. */
  readonly #changes: ChangeHub;
  /** Subscribers and the delivery of changes to them. */
  readonly #subscriptions: Subscriptions;
  /** Parsed and persisted documents. */
  readonly #documents: Documents;
  #degrees = new Map<string, number>();
  /** Bumped when statistics change: they shape cost estimates. */
  #statisticsVersion = 0;
  /**
   * Compiled reads by root field node (documents are cached, so a field
   * node repeats across requests), then by what each compile read.
   */
  readonly #compiled: CompileCache;
  #statistics: Statistics | undefined;
  #schema: GraphQLSchema | undefined;
  readonly #mutationTransaction: "field" | "operation";
  #warnedNotAtomic = false;
  readonly #timing: LoraGraphQLOptions["timing"];
  /** Per-request read-set collectors, by GraphQL context. */
  readonly #readSets = new WeakMap<object, ReadCollector>();
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
    this.#limits = new OperationLimits({
      operationTimeoutMs:
        options.operationTimeoutMs ??
        this.#timeoutMs * OPERATION_TIMEOUT_FACTOR,
      maxConcurrentStatements:
        options.maxConcurrentStatements ?? MAX_CONCURRENT_STATEMENTS,
      maxCost: options.maxCost ?? 50_000,
      budget: options.budget,
      onCost: options.onCost,
    });
    this.#maxBatch = options.maxBatch ?? 1000;
    this.#maxListArgument = options.maxListArgument ?? MAX_LIST_ARGUMENT;
    this.#maxFilterDepth = options.maxFilterDepth ?? MAX_FILTER_DEPTH;
    this.#maxListFilter = options.maxListFilter;
    this.#maxStringFilter = options.maxStringFilter;
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
    this.#changes = new ChangeHub({
      driver: options.driver,
      model: this.model,
      changeFeed: options.changeFeed,
      maxQueued: options.maxQueuedChanges ?? 1000,
      onError: this.#onError,
      onWrite: (listener) => this.onWrite(listener),
    });
    this.#subscriptions = new Subscriptions({
      model: this.model,
      driver: this.#driver,
      changes: this.#changes,
      maxSubscriptions: options.maxSubscriptions ?? 100,
      subscriptionScope:
        options.subscriptionScope ??
        ((context) =>
          context !== null && typeof context === "object"
            ? context
            : undefined),
      maxSubscriptionFilterDepth: options.maxSubscriptionFilterDepth ?? 1,
      subscriptionTimeoutMs:
        options.subscriptionTimeoutMs ?? Math.min(2000, this.#timeoutMs),
      schema: () => this.getSchema(),
      jwt: (context) => this.#jwt(context),
      context: (base, context) => this.#context(base, context),
      degrees: () => this.#degrees,
      statisticsVersion: () => this.#statisticsVersion,
      charge: (field, cost, context) =>
        this.#limits.charge(field, cost, context),
      run: (field, compiled, context, info, change, shareKey) =>
        this.#run(field, compiled, context, info, change, shareKey),
    });
    this.#observer = new Observer(options);
    this.#documents = new Documents({
      guards: this.#guards,
      persistedOnly: options.persistedOnly ?? false,
      bytesMax: cacheBytes,
      schema: () => this.getSchema(),
      rules: () => this.validationRules(),
    });
    this.#mutationTransaction = options.mutationTransaction ?? "field";
    this.#timing = options.timing;
  }

  /** The configured document guards as validation rules. */
  validationRules(): ValidationRule[] {
    return this.#guards === false ? [] : validationRules(this.#guards);
  }

  /**
   * The configured document guards as an Envelop / Yoga plugin. With
   * `mutationTransaction: "operation"` it also runs a mutation with
   * several root fields in one transaction, and it collapses identical
   * errors across list indices, as `execute()` does.
   */
  envelopPlugin(): ReturnType<typeof envelopPlugin> & {
    onExecute(payload: {
      args: ExecutionArgs;
      executeFn: (args: ExecutionArgs) => unknown;
      setExecuteFn: (fn: (args: ExecutionArgs) => unknown) => void;
    }): {
      onExecuteDone(payload: {
        result: unknown;
        setResult: (result: ExecutionResult) => void;
      }): void;
    };
  } {
    const guards = envelopPlugin(
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
    return {
      ...guards,
      onExecute: ({ args, executeFn, setExecuteFn }) => {
        const operation = getOperationAST(
          args.document,
          args.operationName ?? undefined,
        );
        if (this.#atomic(operation, args.contextValue)) {
          setExecuteFn(async (execArgs) => {
            const { tx, context } = await this.#openAtomic(
              execArgs.contextValue,
            );
            let result: ExecutionResult;
            try {
              result = (await executeFn({
                ...execArgs,
                contextValue: context,
              })) as ExecutionResult;
            } catch (err) {
              await tx.rollback();
              throw err;
            }
            return (await this.#closeAtomic(tx, result)).result;
          });
        }
        return {
          onExecuteDone({ result, setResult }) {
            // A streamed (incremental) result is left as it is.
            if (
              result === null ||
              typeof result !== "object" ||
              Symbol.asyncIterator in result
            ) {
              return;
            }
            const plain = result as ExecutionResult;
            if (plain.errors && plain.errors.length > 1) {
              setResult({ ...plain, errors: collapseErrors(plain.errors) });
            }
          },
        };
      },
    };
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
      subscribe: (node, args, context) =>
        this.#subscriptions.subscribe(node, args, context),
      resolveChangedNode: (node, event, info, context) =>
        event.operation === "DELETE"
          ? Promise.resolve(null)
          : span(info, context, () =>
              this.#subscriptions.resolveByKey(
                node,
                event.key,
                info,
                context,
                (event as { [CHANGE]?: WriteChange })[CHANGE],
              ),
            ),
      previousValue: (node, field, value, context) => {
        const ctx = this.#context(
          { schema: this.getSchema(), fragments: {}, variables: {} },
          context,
        );
        checkFieldAuthentication(ctx, node.name, field);
        return claimsMaskedValue(ctx, node, field, value);
      },
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
  assertSchema(options: AssertSchemaOptions = {}): Promise<SchemaAssertion> {
    return assertSchema(this.#schemaEnv(), options);
  }

  /** What the checks of the database's constraints and indexes need. */
  #schemaEnv(): SchemaEnv {
    return {
      driver: this.#driver,
      timeoutMs: this.#timeoutMs,
      requirements: () => this.requirements(),
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
   * Who may do what: for every type and guarded field, each operation as
   * each kind of caller (anonymous, authenticated, and each role the rules
   * test), with the verdict and the rules that decide it. Read off the
   * model; stable, so it can be snapshotted and reviewed as a diff.
   */
  accessMatrix(): AccessEntry[] {
    return accessMatrix(this.model, this.getSchema());
  }

  /**
   * Who may run an operation: per root field, the verdict for each kind
   * of caller, from the same rules as `accessMatrix()`, plus the most
   * restrictive verdict per caller. Takes a document (source or parsed)
   * or the id of a persisted operation; `operationName` picks one of
   * several operations.
   */
  operationAccess(
    document: string | DocumentNode,
    operationName?: string,
  ): OperationAccess {
    const doc =
      typeof document !== "string"
        ? document
        : (this.#documents.persisted(document) ?? parse(document));
    const operation = getOperationAST(doc, operationName);
    if (!operation) {
      throw new Error(
        operationName
          ? `operationAccess: no operation named ${operationName}`
          : "operationAccess: the document has no single operation; pass operationName",
      );
    }
    const fragments = new Map<string, FragmentDefinitionNode>();
    for (const def of doc.definitions) {
      if (def.kind === Kind.FRAGMENT_DEFINITION)
        fragments.set(def.name.value, def);
    }
    return operationAccess(this.model, this.getSchema(), operation, fragments);
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
      // An index defined differently from the model is as good as missing.
      missing: await this.assertSchema().then((a) => [
        ...a.missing,
        ...a.mismatched,
      ]),
      unused: await unusedIndexes(this.#schemaEnv()),
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
      const root = rootOf(this.model, sel.name.value);
      const search = searchOf(this.model, sel.name.value);
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
    this.#documents.persist(operations);
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
    this.#documents.load(manifest);
  }

  /**
   * Execute a query or mutation against the schema. Parsed and validated
   * documents are cached by source text; persisted ones by id. A
   * subscription runs with {@link LoraGraphQL.subscribe}; given one, this
   * returns a `WRONG_OPERATION_TYPE` error.
   */
  async execute(args: ExecuteArgs): Promise<LoraExecutionResult> {
    if (!args.readSet) return this.#execute(args);
    const context = args.context ?? {};
    if (typeof context !== "object" || context === null) {
      return this.#execute(args);
    }
    // Calls sharing a context share one collector: the union of their
    // reads over-approximates each, which is safe for invalidation.
    let collector = this.#readSets.get(context);
    if (collector) collector.active++;
    else {
      collector = {
        labels: new Set(),
        relationships: new Set(),
        opaque: false,
        active: 1,
      };
      this.#readSets.set(context, collector);
    }
    let result: ExecutionResult;
    try {
      result = await this.#execute({ ...args, context });
    } finally {
      if (--collector.active === 0) this.#readSets.delete(context);
    }
    const document = this.#documents.document(args);
    const operation = isDocument(document)
      ? getOperationAST(document, args.operationName)
      : undefined;
    const readSet: ReadSet = {
      labels: [...collector.labels].sort(),
      relationships: [...collector.relationships].sort(),
      // A mutation's reads follow its writes; nobody caches it.
      ...(collector.opaque || operation?.operation === "mutation"
        ? { opaque: true }
        : {}),
    };
    return Object.defineProperty({ ...result }, "readSet", {
      value: readSet,
      enumerable: false,
    }) as LoraExecutionResult;
  }

  async #execute(args: ExecuteArgs): Promise<ExecutionResult> {
    const started = performance.now();
    const document = this.#documents.document(args);
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
    let tx: LoraTransaction | undefined;
    if (
      this.#atomic(getOperationAST(document, args.operationName), contextValue)
    ) {
      ({ tx, context: contextValue } = await this.#openAtomic(contextValue));
    }
    const timed =
      typeof this.#timing === "function"
        ? this.#timing(contextValue)
        : this.#timing === true;
    if (timed && contextValue !== null && typeof contextValue === "object") {
      this.#timings.set(contextValue, new Map());
    }
    // This execution's own root value: cost is charged to it.
    const execution = {};
    this.#limits.track(execution);
    let result: ExecutionResult;
    try {
      result = (await graphqlExecute({
        schema: this.getSchema(),
        document,
        rootValue: execution,
        variableValues: args.variables,
        operationName: args.operationName,
        contextValue,
      })) as ExecutionResult;
    } catch (err) {
      await tx?.rollback();
      throw err;
    }
    // One error per path pattern, not one per row of every list.
    if (result.errors && result.errors.length > 1) {
      result = { ...result, errors: collapseErrors(result.errors) };
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
      const closed = await this.#closeAtomic(tx, result);
      // A failed commit answers with its error alone.
      if (!closed.committed && !result.errors?.length) return closed.result;
      result = closed.result;
    }
    // The operation's cost estimate, so clients can tune their queries.
    const cost = this.#limits.spent(execution);
    if (cost !== undefined) {
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
    const document = this.#documents.document(args);
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

  // -------------------------------------------------------------------------
  // Change tracking (S5)
  // -------------------------------------------------------------------------

  /** Called after every committed mutation with its exact write-set. */
  onWrite(listener: (change: WriteChange) => void): () => void {
    return this.#changes.onWrite(listener);
  }

  /**
   * Committed writes as an async iterator, e.g. for a subscription
   * resolver. Only writes made through this instance are seen. `return()`
   * or aborting `signal` (already aborted included) ends it at once.
   */
  changes(
    options: { signal?: AbortSignal; maxQueued?: number } = {},
  ): AsyncIterableIterator<WriteChange> {
    return this.#changes.changes(options);
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

  #args(
    ctx: CompileContext,
    parent: GraphQLObjectType,
    fieldNodes: readonly FieldNode[],
  ): Record<string, unknown> {
    const def = parent.getFields()[fieldNodes[0]!.name.value]!;
    return fieldArgs(ctx, def, fieldNodes[0]!);
  }

  async #run(
    field: string,
    compiled: CompiledRead,
    context: unknown,
    info?: GraphQLResolveInfo,
    change?: object,
    shareKey?: string,
  ): Promise<unknown> {
    this.#limits.charge(field, compiled.cost, context, info);
    const collector =
      typeof context === "object" && context !== null
        ? this.#readSets.get(context)
        : undefined;
    if (collector) {
      for (const l of compiled.reads.labels) collector.labels.add(l);
      for (const r of compiled.reads.relationships)
        collector.relationships.add(r);
      if (compiled.reads.opaque) collector.opaque = true;
    }
    for (const statement of compiled.statements) {
      this.#onStatement?.({ field, statement });
    }
    const signal = (context as LoraGraphQLContext | undefined)?.signal;
    const owned = (context as LoraGraphQLContext | undefined)?.transaction;
    const budget = this.#limits.operationBudget(context, info);
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
            : this.#subscriptions.shared(
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
      if (budget) this.#limits.leaveOperation(context as object, info!, budget);
      if (timing) {
        timingEntry(timing, rootKey(info) ?? field).databaseMs +=
          performance.now() - dbStart;
      }
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
      persistedId: info && this.#documents.persistedId(info.operation),
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
    return databaseError(
      {
        model: this.model,
        maskErrors: this.#maskErrors,
        onError: this.#onError,
      },
      field,
      err,
    );
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
    this.#warnNotAtomic(info, context);
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
    // A hand-written write has no known write-set: report it broadly,
    // when the transaction it ran in commits.
    const change: WriteChange = {
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
    };
    const owned = (context as LoraGraphQLContext | undefined)?.transaction;
    if (owned) owned.record(change);
    else this.#changes.emit(change);
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
    this.#warnNotAtomic(info, context);
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
      else this.#changes.emit(change);
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
      beforeDelete: (change, doomed, run) =>
        this.#subscriptions.probeDeletes(change, doomed, run),
    };
  }

  /**
   * Whether `operation` runs in one transaction: a mutation with several
   * root fields (a fragment at the root may hold several) under
   * `mutationTransaction: "operation"`, with no transaction in the context.
   */
  #atomic(
    operation: OperationDefinitionNode | null | undefined,
    context: unknown,
  ): boolean {
    return (
      this.#mutationTransaction === "operation" &&
      operation?.operation === "mutation" &&
      multiRoot(operation) &&
      !(context as LoraGraphQLContext | undefined)?.transaction &&
      this.#driver.begin !== undefined
    );
  }

  /** The operation's transaction, and a context carrying it. */
  async #openAtomic(
    context: unknown,
  ): Promise<{ tx: LoraTransaction; context: object }> {
    const tx = await this.begin();
    const base = (context ?? {}) as object;
    const opened = Object.assign(
      Object.create(Object.getPrototypeOf(base) as object) as object,
      base,
      { transaction: tx },
    );
    // The read-set collector follows the operation into its transaction.
    const collector = this.#readSets.get(base);
    if (collector) this.#readSets.set(opened, collector);
    return { tx, context: opened };
  }

  /**
   * Commit the operation's transaction when it reported no error; roll it
   * back otherwise, with `data: null`: nothing of it was written.
   */
  async #closeAtomic(
    tx: LoraTransaction,
    result: ExecutionResult,
  ): Promise<{ result: ExecutionResult; committed: boolean }> {
    if (result.errors?.length) {
      await tx.rollback();
      return { result: { ...result, data: null }, committed: false };
    }
    try {
      await tx.commit();
      return { result, committed: true };
    } catch (err) {
      await tx.rollback().catch(() => undefined);
      const error = this.#databaseError("commit", err);
      return {
        result: {
          data: null,
          errors: [
            error instanceof GraphQLError
              ? error
              : new GraphQLError(
                  error instanceof Error ? error.message : String(error),
                ),
          ],
        },
        committed: false,
      };
    }
  }

  /**
   * Warn once when a multi-root mutation runs outside a transaction while
   * `mutationTransaction: "operation"` asks for one: graphql-js on
   * `getSchema()` without `envelopPlugin()` commits each root field alone.
   */
  #warnNotAtomic(info: GraphQLResolveInfo, context: unknown): void {
    if (
      this.#warnedNotAtomic ||
      this.#mutationTransaction !== "operation" ||
      !multiRoot(info.operation) ||
      (context as LoraGraphQLContext | undefined)?.transaction
    ) {
      return;
    }
    this.#warnedNotAtomic = true;
    console.warn(
      'lora-graphql: mutationTransaction: "operation" is set, but a mutation with several root fields ran outside a transaction, so each root field commits on its own. Run operations through lora.execute(), add lora.envelopPlugin() to your Envelop/Yoga server, or put a lora.begin() transaction in the context.',
    );
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
    return new LoraTransaction(tx, (change) => this.#changes.emit(change));
  }

  /** Stop the engine change feed (with `changeFeed: true`). */
  close(): void {
    this.#changes.close();
  }
}
