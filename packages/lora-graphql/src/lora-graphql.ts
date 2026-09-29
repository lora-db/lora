import {
  execute as graphqlExecute,
  getVariableValues,
  GraphQLError,
  Kind,
  lexicographicSortSchema,
  parse,
  printSchema,
  validate,
  type DocumentNode,
  type ExecutionResult,
  type FieldNode,
  type FragmentDefinitionNode,
  type GraphQLObjectType,
  type GraphQLResolveInfo,
  type GraphQLSchema,
  type OperationDefinitionNode,
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
import { analyze, type Statistics } from "./analyze/statistics.js";
import { newContext, type CompileContext } from "./compile/context.js";
import {
  compileCypherRoot,
  compileRoot,
  type CompiledRead,
  type ReadSet,
  type RootKind,
} from "./compile/read.js";
import { fieldArgs, type SelectionContext } from "./compile/selection.js";
import type { LoraDriver, Statement } from "./driver.js";
import { affects, type WriteChange } from "./execute/changes.js";
import { executeCypherMutation } from "./execute/cypher-mutation.js";
import {
  executeMutation,
  mapWriteError,
  type MutationEnv,
} from "./execute/mutate.js";
import { requestError } from "./errors.js";
import { buildModel, type ModelOptions } from "./model/build.js";
import type {
  CypherField,
  GraphModel,
  ModelWarning,
  MutationOperation,
  NodeType,
} from "./model/types.js";
import { buildSchema } from "./schema/build.js";
import { fromGlobalId } from "./schema/global-id.js";
import { assertReadable } from "./schema/guard.js";
import { names } from "./schema/names.js";

export interface LoraGraphQLOptions extends ModelOptions {
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
   * The request's verified claims, from the GraphQL context. Default:
   * `context.jwt`. The library never verifies tokens: do that in the
   * server and put the claims in the context.
   */
  jwt?: (context: unknown) => Record<string, unknown> | undefined;
  /**
   * Most nodes one mutation may create, nested creates included. Default
   * 1000: larger imports belong in a Cypher load, not a GraphQL request.
   */
  maxBatch?: number;
  /** Called with every statement before it runs; for logging and tests. */
  onStatement?: (event: StatementEvent) => void;
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

export class LoraGraphQL {
  readonly model: GraphModel;
  readonly #driver: LoraDriver;
  readonly #timeoutMs: number;
  readonly #maxCost: number;
  readonly #maxBatch: number;
  readonly #jwt: (context: unknown) => Record<string, unknown> | undefined;
  readonly #onStatement: LoraGraphQLOptions["onStatement"];
  readonly #listeners = new Set<(change: WriteChange) => void>();
  readonly #documents = new Map<string, DocumentNode>();
  readonly #persisted = new Map<string, DocumentNode>();
  #degrees = new Map<string, number>();
  #statistics: Statistics | undefined;
  #schema: GraphQLSchema | undefined;

  constructor(options: LoraGraphQLOptions) {
    this.model = buildModel(options.typeDefs, options);
    this.#driver = options.driver;
    this.#timeoutMs = options.timeoutMs ?? 10_000;
    this.#maxCost = options.maxCost ?? 50_000;
    this.#maxBatch = options.maxBatch ?? 1000;
    this.#jwt =
      options.jwt ??
      ((context) => (context as LoraGraphQLContext | undefined)?.jwt);
    this.#onStatement = options.onStatement;
  }

  /** The executable schema, for any graphql-js server. */
  getSchema(): GraphQLSchema {
    this.#schema ??= buildSchema(this.model, {
      resolveRoot: (kind, node, info, context) =>
        this.#resolveRoot(kind, node, info, context),
      resolveNode: (id, info, context) => this.#resolveNode(id, info, context),
      resolveCypher: (field, info, context) =>
        this.#resolveCypher(field, info, context),
      resolveMutation: (op, node, info, context) =>
        this.#resolveMutation(op, node, info, context),
    });
    return this.#schema;
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
      plans: [],
      errors: [],
    };
    for (const [i, op] of (options.operations ?? []).entries()) {
      const operation = op.name ?? `operation ${i + 1}`;
      try {
        const fields = await this.explain(op.document, op.variables ?? {});
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
      let compiled: CompiledRead | undefined;
      if (cypher) compiled = compileCypherRoot(ctx, cypher, args, [sel]);
      else if (root) {
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
    options: { operationName?: string; context?: unknown } = {},
  ): Promise<Array<{ field: string; reports: PlanReport[] }>> {
    const out: Array<{ field: string; reports: PlanReport[] }> = [];
    for (const { field, compiled } of this.compile(
      document,
      variables,
      options,
    )) {
      out.push({ field, reports: await checkPlans(this.#driver, compiled) });
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
    const schema = this.getSchema();
    const problems: string[] = [];
    for (const [id, source] of Object.entries(operations)) {
      try {
        const doc = parse(source);
        const errors = validate(schema, doc);
        if (errors.length > 0) {
          problems.push(`${id}: ${errors.map((e) => e.message).join("; ")}`);
        } else {
          this.#persisted.set(id, doc);
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
      document = this.#documents.get(args.source);
      if (!document) {
        try {
          document = parse(args.source);
        } catch (err) {
          return { errors: [err as GraphQLError] };
        }
        const errors = validate(this.getSchema(), document);
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
    return graphqlExecute({
      schema: this.getSchema(),
      document,
      variableValues: args.variables,
      operationName: args.operationName,
      contextValue: args.context ?? {},
    });
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
    options: { signal?: AbortSignal } = {},
  ): AsyncIterableIterator<WriteChange> {
    const queue: WriteChange[] = [];
    let wake: (() => void) | undefined;
    let done = false;
    const stop = this.onWrite((change) => {
      queue.push(change);
      wake?.();
    });
    const finish = () => {
      done = true;
      stop();
      wake?.();
    };
    options.signal?.addEventListener("abort", finish, { once: true });
    const iterator: AsyncIterableIterator<WriteChange> = {
      [Symbol.asyncIterator]: () => iterator,
      next: async () => {
        while (queue.length === 0 && !done) {
          await new Promise<void>((resolve) => (wake = resolve));
          wake = undefined;
        }
        return queue.length > 0
          ? { value: queue.shift()!, done: false }
          : { value: undefined, done: true };
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
    }
    return undefined;
  }

  #context(base: SelectionContext, context: unknown): CompileContext {
    return newContext(base, this.model, {
      jwt: this.#jwt(context),
      degrees: this.#degrees,
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
  ): Promise<unknown> {
    if (compiled.cost > this.#maxCost) {
      throw requestError(
        "COST_EXCEEDED",
        `${field} would touch about ${Math.ceil(compiled.cost)} rows; the limit is ${this.#maxCost}. Ask for smaller pages or fewer nested lists.`,
        undefined,
        { cost: Math.ceil(compiled.cost), maxCost: this.#maxCost },
      );
    }
    for (const statement of compiled.statements) {
      this.#onStatement?.({ field, statement });
    }
    const signal = (context as LoraGraphQLContext | undefined)?.signal;
    let results;
    try {
      results = await this.#driver.run(compiled.statements, {
        mode: compiled.mode,
        timeoutMs: this.#timeoutMs,
        signal,
        // Queries and object @cypher fields are checked read-only when
        // the model is built; writes never reach this path.
        verified: compiled.mode === "read",
      });
    } catch (err) {
      throw this.#databaseError(err);
    }
    return assertReadable(compiled.shape(results));
  }

  #databaseError(err: unknown): unknown {
    if (err instanceof GraphQLError) return err;
    const mapped = mapWriteError(this.model, err);
    if (mapped !== err) return mapped;
    return requestError(
      "DATABASE_ERROR",
      err instanceof Error ? err.message : String(err),
      err,
    );
  }

  #resolveRoot(
    kind: RootKind,
    node: NodeType,
    info: GraphQLResolveInfo,
    context: unknown,
  ): Promise<unknown> {
    const ctx = this.#context(infoContext(info), context);
    const args = this.#args(ctx, info.parentType, info.fieldNodes);
    const compiled = compileRoot(ctx, kind, node, args, info.fieldNodes);
    return this.#run(info.fieldName, compiled, context);
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
    if (field.authentication && !ctx.jwt) {
      throw requestError(
        "UNAUTHENTICATED",
        `${field.owner}.${field.name} needs an authenticated request`,
      );
    }
    const args = this.#args(ctx, info.parentType, info.fieldNodes);
    if (field.owner !== "Mutation") {
      const compiled = compileCypherRoot(ctx, field, args, info.fieldNodes);
      return this.#run(info.fieldName, compiled, context);
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
      throw this.#databaseError(err);
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
    op: MutationOperation,
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
      this.#emit(change);
      return payload;
    } catch (err) {
      throw this.#databaseError(err);
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
      onStatement: (statement) =>
        this.#onStatement?.({ field: info.fieldName, statement }),
    };
  }

  #emit(change: WriteChange): void {
    for (const listener of this.#listeners) {
      try {
        listener(change);
      } catch {
        // A listener's failure must not fail a committed write.
      }
    }
  }
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
